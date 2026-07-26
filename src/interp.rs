use crate::ir::{self, BinaryOp, Callee, Constant, InstKind, LoweredProgram, Terminator, UnaryOp};
use std::collections::HashMap;
use std::io::Write as _;
use std::rc::Rc;

const RTOL: f64 = 9.094947017729282e-13; // 2^-40
const ATOL: f64 = 7.888609052210118e-31; // 2^-100

#[derive(Clone, Debug)]
pub enum Value {
    Int(i64),
    Float32(f32),
    Float(f64),
    F32x4([f32; 4]),
    F64x2([f64; 2]),
    I64x2([i64; 2]),
    Bool(bool),
    Str(Rc<Vec<u8>>),
    Arr(Rc<Vec<Value>>),
    Rec(usize, Rc<Vec<Value>>),
    Enum(usize, i64),
    CPtr(usize),
    Unit,
}

pub struct Interp<'a> {
    ir: &'a LoweredProgram,
    /// Liveness per function, keyed by the address of the `ir::Function` that
    /// `execute` is handed. Computed once so recursive calls only pay a lookup.
    liveness: HashMap<usize, Rc<Liveness>>,
}

#[derive(Clone, Debug)]
pub struct PropertyStatus {
    pub name: String,
    pub passed: bool,
    pub runs: u32,
}

fn as_f64(v: &Value) -> Result<f64, String> {
    match v {
        Value::Float32(f) => Ok(*f as f64),
        Value::Float(f) => Ok(*f),
        Value::Int(i) => Ok(*i as f64),
        v => Err(format!("expected number, got {:?}", v)),
    }
}

fn coerce(value: Value, ty: &crate::check::Type) -> Result<Value, String> {
    use crate::check::Type;
    Ok(match (ty, value) {
        (Type::F32, Value::Int(v)) => Value::Float32(v as f32),
        (Type::F32, Value::Float(v)) => Value::Float32(v as f32),
        (Type::F32, Value::Float32(v)) => Value::Float32(v),
        (Type::F64, Value::Int(v)) => Value::Float(v as f64),
        (Type::F64, Value::Float32(v)) => Value::Float(v as f64),
        (_, value) => value,
    })
}

fn as_i64(v: &Value) -> Result<i64, String> {
    match v {
        Value::Int(i) => Ok(*i),
        v => Err(format!("expected integer, got {:?}", v)),
    }
}

fn set_field(slot: &mut Value, path: &[usize], v: Value) -> Result<(), String> {
    let Some(&field) = path.first() else {
        *slot = v;
        return Ok(());
    };
    match slot {
        Value::Rec(_, fields) => {
            let fields = Rc::make_mut(fields);
            let slot = fields
                .get_mut(field)
                .ok_or_else(|| format!("invalid field {}", field))?;
            set_field(slot, &path[1..], v)
        }
        v => Err(format!("cannot assign field {} on {:?}", field, v)),
    }
}

fn set_index(slot: &mut Value, path: &[usize], index: usize, value: Value) -> Result<(), String> {
    if let Some((&field, rest)) = path.split_first() {
        let Value::Rec(_, fields) = slot else {
            return Err(format!("cannot traverse array field on {:?}", slot));
        };
        let fields = Rc::make_mut(fields);
        let field = fields
            .get_mut(field)
            .ok_or_else(|| "invalid array field path".to_string())?;
        return set_index(field, rest, index, value);
    }
    let Value::Arr(cells) = slot else {
        return Err("cannot assign through non-array".into());
    };
    let cells = Rc::make_mut(cells);
    *cells
        .get_mut(index)
        .ok_or_else(|| format!("index {} out of bounds", index))? = value;
    Ok(())
}

fn approx_eq(a: f64, b: f64) -> bool {
    (a - b).abs() <= ATOL + RTOL * a.abs().max(b.abs())
}

/// Per-instruction last-use sets, so `execute` can release SSA slots the moment
/// they die.
///
/// This is what keeps element assignment O(1). `a[i] = x` lowers to a `Load` of
/// the array followed by `SetIndex`; the `Load` result is an extra `Rc` alias in
/// the value table, so `Rc::make_mut` in `set_index` would see a shared array and
/// deep-copy all of it on *every* store — quadratic in the array length. Dropping
/// the alias before the update leaves the owning local as the sole reference and
/// the write happens in place.
struct Liveness {
    /// `dying[block][instruction]`: values whose last read is that instruction.
    dying: Vec<Vec<Vec<ir::ValueId>>>,
}

impl Liveness {
    fn analyze(function: &ir::Function) -> Liveness {
        let blocks = &function.blocks;
        let count = function.values.len();

        // Upward-exposed uses (read before any definition in the block) and the
        // values each block defines. Restricting `gen` to upward-exposed uses is
        // what lets a value defined and consumed inside a loop body die there
        // instead of being kept live around the back edge.
        let mut gen = vec![vec![false; count]; blocks.len()];
        let mut defs = vec![vec![false; count]; blocks.len()];
        for (b, block) in blocks.iter().enumerate() {
            for inst in &block.instructions {
                for value in ir::operands(&inst.kind) {
                    if !defs[b][value as usize] {
                        gen[b][value as usize] = true;
                    }
                }
                if let Some(result) = inst.result {
                    defs[b][result as usize] = true;
                }
            }
            for value in terminator_operands(&block.terminator) {
                if !defs[b][value as usize] {
                    gen[b][value as usize] = true;
                }
            }
        }

        // Backward dataflow to a fixpoint:
        //   live_out[b] = ∪ live_in[successors]
        //   live_in[b]  = gen[b] ∪ (live_out[b] \ defs[b])
        let mut live_in = vec![vec![false; count]; blocks.len()];
        let mut live_out = vec![vec![false; count]; blocks.len()];
        let mut changed = true;
        while changed {
            changed = false;
            for b in (0..blocks.len()).rev() {
                let mut out = vec![false; count];
                for successor in successors(&blocks[b].terminator) {
                    for v in 0..count {
                        out[v] |= live_in[successor as usize][v];
                    }
                }
                let mut inn = vec![false; count];
                for v in 0..count {
                    inn[v] = gen[b][v] || (out[v] && !defs[b][v]);
                }
                if out != live_out[b] || inn != live_in[b] {
                    live_out[b] = out;
                    live_in[b] = inn;
                    changed = true;
                }
            }
        }

        // A value dies at its last read inside the block, unless it escapes the
        // block (live-out) or is read by the terminator.
        let mut dying = Vec::with_capacity(blocks.len());
        for (b, block) in blocks.iter().enumerate() {
            let mut per_inst = vec![Vec::new(); block.instructions.len()];
            let mut later_use = vec![false; count];
            for value in terminator_operands(&block.terminator) {
                later_use[value as usize] = true;
            }
            for (i, inst) in block.instructions.iter().enumerate().rev() {
                for value in ir::operands(&inst.kind) {
                    let slot = value as usize;
                    if !later_use[slot] && !live_out[b][slot] {
                        per_inst[i].push(value);
                    }
                    later_use[slot] = true;
                }
            }
            dying.push(per_inst);
        }
        Liveness { dying }
    }

    fn dying(&self, block: usize, instruction: usize) -> &[ir::ValueId] {
        &self.dying[block][instruction]
    }
}

fn successors(terminator: &Terminator) -> Vec<ir::BlockId> {
    match terminator {
        Terminator::Jump(target) => vec![*target],
        Terminator::Branch {
            then_block,
            else_block,
            ..
        } => vec![*then_block, *else_block],
        Terminator::Return(_) | Terminator::Unreachable => Vec::new(),
    }
}

fn terminator_operands(terminator: &Terminator) -> Vec<ir::ValueId> {
    match terminator {
        Terminator::Branch { condition, .. } => vec![*condition],
        Terminator::Return(value) => vec![*value],
        Terminator::Jump(_) | Terminator::Unreachable => Vec::new(),
    }
}

fn release(values: &mut [Value], dying: &[ir::ValueId]) {
    for &value in dying {
        values[value as usize] = Value::Unit;
    }
}

impl<'a> Interp<'a> {
    pub fn new(ir: &'a LoweredProgram) -> Self {
        let mut liveness = HashMap::new();
        for function in ir.functions.iter().chain(ir.main.iter()) {
            liveness.insert(
                function as *const ir::Function as usize,
                Rc::new(Liveness::analyze(function)),
            );
        }
        Interp { ir, liveness }
    }

    pub fn run_main(&self) -> Result<(), String> {
        let main = self.ir.main.as_ref().ok_or("no `main` block in program")?;
        self.execute(main, Vec::new())?;
        Ok(())
    }

    pub fn run_properties(&self, runs: u32) -> Result<bool, String> {
        self.run_properties_filtered(runs, None)
    }

    pub fn run_property(&self, runs: u32, name: &str) -> Result<bool, String> {
        if !self
            .ir
            .properties
            .iter()
            .any(|property| property.name == name)
        {
            return Err(format!("unknown property `{name}`"));
        }
        self.run_properties_filtered(runs, Some(name))
    }

    fn run_properties_filtered(&self, runs: u32, only: Option<&str>) -> Result<bool, String> {
        let mut all_ok = true;
        let mut rng: u64 = 0x9E3779B97F4A7C15;
        for prop in &self.ir.properties {
            if only.is_some_and(|name| name != prop.name) {
                continue;
            }
            let function_id = prop.function;
            let mut failed = None;
            for _ in 0..runs {
                let args: Result<Vec<Value>, String> = prop
                    .params
                    .iter()
                    .map(|(_, t)| self.gen_value(t, &mut rng))
                    .collect();
                let args = args?;
                let v = self
                    .execute(&self.ir.functions[function_id as usize], args.clone())?
                    .0;
                match v {
                    Value::Bool(true) => {}
                    Value::Bool(false) => {
                        failed = Some(args);
                        break;
                    }
                    v => {
                        return Err(format!(
                            "property `{}` returned non-bool {:?}",
                            prop.name, v
                        ))
                    }
                }
            }
            match failed {
                None => println!("property {} ... ok ({} runs)", prop.name, runs),
                Some(args) => {
                    all_ok = false;
                    let (args, steps) = self.shrink(function_id, args)?;
                    println!(
                        "property {} ... FAIL (counterexample shrunk {} steps)",
                        prop.name, steps
                    );
                    for ((name, ty), v) in prop.params.iter().zip(args.iter()) {
                        println!("  {}: {} = {}", name, self.type_name(ty), self.display(v));
                    }
                }
            }
        }
        Ok(all_ok)
    }

    pub fn property_statuses(&self, runs: u32) -> Result<Vec<PropertyStatus>, String> {
        let mut statuses = Vec::new();
        let mut rng: u64 = 0x9E3779B97F4A7C15;
        for property in &self.ir.properties {
            let mut passed = true;
            let mut completed_runs = 0;
            for _ in 0..runs {
                let args = property
                    .params
                    .iter()
                    .map(|(_, ty)| self.gen_value(ty, &mut rng))
                    .collect::<Result<Vec<_>, _>>()?;
                let value = self
                    .execute(&self.ir.functions[property.function as usize], args)?
                    .0;
                match value {
                    Value::Bool(true) => completed_runs += 1,
                    Value::Bool(false) => {
                        passed = false;
                        completed_runs += 1;
                        break;
                    }
                    value => {
                        return Err(format!(
                            "property `{}` returned non-bool {:?}",
                            property.name, value
                        ))
                    }
                }
            }
            statuses.push(PropertyStatus {
                name: property.name.clone(),
                passed,
                runs: completed_runs,
            });
        }
        Ok(statuses)
    }

    /// Greedy shrink: repeatedly try simpler variants of each argument, keeping
    /// any that still falsify the property. Returns the final args + step count.
    fn shrink(
        &self,
        function_id: ir::FunctionId,
        mut args: Vec<Value>,
    ) -> Result<(Vec<Value>, u32), String> {
        let mut steps = 0u32;
        let mut budget = 500u32; // max property evaluations while shrinking
        'outer: loop {
            for i in 0..args.len() {
                for cand in Self::simpler(&args[i]) {
                    if budget == 0 {
                        break 'outer;
                    }
                    budget -= 1;
                    let mut trial = args.clone();
                    trial[i] = cand;
                    if matches!(
                        self.execute(&self.ir.functions[function_id as usize], trial.clone())?
                            .0,
                        Value::Bool(false)
                    ) {
                        args = trial;
                        steps += 1;
                        continue 'outer;
                    }
                }
            }
            break;
        }
        Ok((args, steps))
    }

    /// Candidate simplifications of a value, most aggressive first.
    fn simpler(v: &Value) -> Vec<Value> {
        match v {
            Value::Float32(f) => {
                let mut out = Vec::new();
                for c in [0.0f32, 1.0, -1.0, f.trunc(), f / 2.0] {
                    if c != *f && c.is_finite() && (c == 0.0 || c.abs() < f.abs()) {
                        out.push(Value::Float32(c));
                    }
                }
                out
            }
            Value::Float(f) => {
                let mut out = Vec::new();
                for c in [0.0, 1.0, -1.0, f.trunc(), f / 2.0] {
                    let simpler_mag =
                        c == 0.0 || c.abs() < f.abs() || (c == c.trunc() && *f != f.trunc());
                    if c != *f && c.is_finite() && simpler_mag {
                        out.push(Value::Float(c));
                    }
                }
                out
            }
            Value::Int(i) => {
                let mut out = Vec::new();
                for c in [0, i / 2] {
                    if c != *i {
                        out.push(Value::Int(c));
                    }
                }
                out
            }
            Value::Bool(true) => vec![Value::Bool(false)],
            Value::Enum(ei, tag) if *tag != 0 => vec![Value::Enum(*ei, 0)],
            Value::Rec(ti, fields) => {
                let mut out = Vec::new();
                for (i, f) in fields.iter().enumerate() {
                    for cand in Self::simpler(f) {
                        let mut fs = fields.as_ref().clone();
                        fs[i] = cand;
                        out.push(Value::Rec(*ti, std::rc::Rc::new(fs)));
                    }
                }
                out
            }
            _ => Vec::new(),
        }
    }

    fn gen_value(&self, ty: &crate::check::Type, rng: &mut u64) -> Result<Value, String> {
        fn next(rng: &mut u64) -> u64 {
            *rng ^= *rng << 13;
            *rng ^= *rng >> 7;
            *rng ^= *rng << 17;
            *rng
        }
        match ty {
            crate::check::Type::F32 => {
                let u = (next(rng) >> 40) as f32 / 16777216.0;
                Ok(Value::Float32(u * 2.0 - 1.0))
            }
            crate::check::Type::F64 => {
                let u = (next(rng) >> 11) as f64 / 9007199254740992.0;
                Ok(Value::Float(u * 2.0 - 1.0))
            }
            crate::check::Type::I64 => Ok(Value::Int((next(rng) % 32) as i64)),
            crate::check::Type::Bool => Ok(Value::Bool(next(rng) & 1 == 1)),
            crate::check::Type::Enum(ei) => {
                let n = self.ir.enums[*ei].variants.len() as u64;
                Ok(Value::Enum(*ei, (next(rng) % n) as i64))
            }
            crate::check::Type::Rec(ti) => {
                let fields: Result<Vec<Value>, String> = self.ir.records[*ti]
                    .fields
                    .iter()
                    .map(|(_, ft)| self.gen_value(ft, rng))
                    .collect();
                Ok(Value::Rec(*ti, Rc::new(fields?)))
            }
            t => Err(format!(
                "cannot generate values of type `{}`",
                self.type_name(t)
            )),
        }
    }

    fn type_name(&self, ty: &crate::check::Type) -> String {
        use crate::check::Type::*;
        match ty {
            I64 => "i64".into(),
            F32 => "f32".into(),
            F64 => "f64".into(),
            F32x4 => "f32x4".into(),
            F64x2 => "f64x2".into(),
            I64x2 => "i64x2".into(),
            Bool => "bool".into(),
            Str => "str".into(),
            Unit => "()".into(),
            Arr(t) => format!("[{}]", self.type_name(t)),
            CSlice(t) => format!("c_slice[{}]", self.type_name(t)),
            CMutSlice(t) => format!("c_mut_slice[{}]", self.type_name(t)),
            CPtr(t) => format!("c_ptr[{}]", self.type_name(t)),
            CFn(params, ret) => format!(
                "c_fn[({})->{}]",
                params
                    .iter()
                    .map(|ty| self.type_name(ty))
                    .collect::<Vec<_>>()
                    .join(","),
                self.type_name(ret)
            ),
            Rec(i) => self.ir.records[*i].name.clone(),
            Enum(i) => self.ir.enums[*i].name.clone(),
        }
    }

    fn execute(
        &self,
        function: &ir::Function,
        args: Vec<Value>,
    ) -> Result<(Value, Vec<Value>), String> {
        if args.len() != function.params.len() {
            return Err(format!(
                "`{}` expects {} args, got {}",
                function.name,
                function.params.len(),
                args.len()
            ));
        }
        let mut locals = vec![Value::Unit; function.locals.len()];
        for (&local, value) in function.params.iter().zip(args) {
            locals[local as usize] = coerce(value, &function.locals[local as usize].ty)?;
        }
        let mut values = vec![Value::Unit; function.values.len()];
        let liveness = self
            .liveness
            .get(&(function as *const ir::Function as usize))
            .cloned()
            .unwrap_or_else(|| Rc::new(Liveness::analyze(function)));
        let mut block_id = function.entry;
        loop {
            let block = &function.blocks[block_id as usize];
            for (inst_id, inst) in block.instructions.iter().enumerate() {
                let dying = liveness.dying(block_id as usize, inst_id);
                let result = match &inst.kind {
                    InstKind::Constant(c) => Some(match c {
                        Constant::I64(v) => Value::Int(*v),
                        Constant::F32(v) => Value::Float32(*v),
                        Constant::F64(v) => Value::Float(*v),
                        Constant::Bool(v) => Value::Bool(*v),
                        Constant::Bytes(v) => Value::Str(Rc::new(v.clone())),
                        Constant::Unit => Value::Unit,
                    }),
                    InstKind::Load(local) => Some(locals[*local as usize].clone()),
                    InstKind::Store { local, value, .. } => {
                        // Release before the write: if this store is the stored
                        // value's last use, the local becomes its sole owner.
                        let stored = values[*value as usize].clone();
                        release(&mut values, dying);
                        locals[*local as usize] =
                            coerce(stored, &function.locals[*local as usize].ty)?;
                        None
                    }
                    InstKind::Unary { op, value } => {
                        Some(self.unary(*op, values[*value as usize].clone())?)
                    }
                    InstKind::Binary { op, lhs, rhs } => {
                        Some(self.binary(*op, &values[*lhs as usize], &values[*rhs as usize])?)
                    }
                    InstKind::Select {
                        condition,
                        then_value,
                        else_value,
                    } => {
                        let Value::Bool(condition) = values[*condition as usize] else {
                            return Err("IR select condition is not bool".into());
                        };
                        Some(
                            values[if condition { *then_value } else { *else_value } as usize]
                                .clone(),
                        )
                    }
                    InstKind::Call {
                        callee,
                        args,
                        inout,
                    } => {
                        let call_args = args
                            .iter()
                            .map(|v| values[*v as usize].clone())
                            .collect::<Vec<_>>();
                        // Drop dying arguments before the call so a callee that
                        // mutates a temporary array owns it outright.
                        release(&mut values, dying);
                        let result = match callee {
                            Callee::Function(id) => {
                                let callee = &self.ir.functions[*id as usize];
                                let (result, callee_frame) = self.execute(callee, call_args)?;
                                for (i, target) in inout.iter().enumerate() {
                                    if let Some(target) = target {
                                        locals[*target as usize] =
                                            callee_frame[callee.params[i] as usize].clone();
                                    }
                                }
                                result
                            }
                            Callee::Extern(id) => {
                                let (result, copyouts) = self.call_extern(*id, call_args)?;
                                for (target, copyout) in inout.iter().zip(copyouts) {
                                    if let (Some(target), Some(copyout)) = (target, copyout) {
                                        locals[*target as usize] = copyout;
                                    }
                                }
                                result
                            }
                            Callee::Builtin(name) => self.call(name, call_args)?,
                        };
                        Some(result)
                    }
                    InstKind::Field {
                        base,
                        record,
                        field,
                    } => match &values[*base as usize] {
                        Value::Rec(actual, fields) if actual == record => {
                            Some(fields.get(*field).cloned().ok_or("invalid field index")?)
                        }
                        value => return Err(format!("cannot access field on {:?}", value)),
                    },
                    InstKind::Index { base, index } => {
                        let index = as_i64(&values[*index as usize])?;
                        Some(match &values[*base as usize] {
                            Value::Arr(cells) => cells
                                .get(index as usize)
                                .cloned()
                                .ok_or_else(|| format!("index {} out of bounds", index))?,
                            Value::Str(bytes) => {
                                Value::Int(*bytes.get(index as usize).ok_or_else(|| {
                                    format!(
                                        "index {} out of bounds (length {})",
                                        index,
                                        bytes.len()
                                    )
                                })? as i64)
                            }
                            value => return Err(format!("cannot index into {:?}", value)),
                        })
                    }
                    InstKind::Array(items) => Some(Value::Arr(Rc::new(
                        items.iter().map(|v| values[*v as usize].clone()).collect(),
                    ))),
                    InstKind::Record { record, fields } => Some(Value::Rec(
                        *record,
                        Rc::new(fields.iter().map(|v| values[*v as usize].clone()).collect()),
                    )),
                    InstKind::Enum { enumeration, tag } => Some(Value::Enum(*enumeration, *tag)),
                    InstKind::SetIndex {
                        root,
                        path,
                        index,
                        value,
                        ..
                    } => {
                        let index = as_i64(&values[*index as usize])?;
                        let element = values[*value as usize].clone();
                        // The `base` operand is only an alias of the array the
                        // root local already owns; dropping it here is what lets
                        // `set_index` write in place instead of copying.
                        release(&mut values, dying);
                        set_index(&mut locals[*root as usize], path, index as usize, element)?;
                        None
                    }
                    InstKind::SetField { root, path, value } => {
                        let field = values[*value as usize].clone();
                        release(&mut values, dying);
                        set_field(&mut locals[*root as usize], path, field)?;
                        None
                    }
                };
                release(&mut values, dying);
                if let (Some(id), Some(result)) = (inst.result, result) {
                    values[id as usize] = coerce(result, &inst.ty)?;
                }
            }
            match block.terminator {
                Terminator::Jump(next) => block_id = next,
                Terminator::Branch {
                    condition,
                    then_block,
                    else_block,
                } => {
                    block_id = if matches!(values[condition as usize], Value::Bool(true)) {
                        then_block
                    } else {
                        else_block
                    }
                }
                Terminator::Return(value) => {
                    return Ok((
                        coerce(values[value as usize].clone(), &function.ret)?,
                        locals,
                    ))
                }
                Terminator::Unreachable => {
                    return Err(format!(
                        "reached unterminated IR block in `{}`",
                        function.name
                    ))
                }
            }
        }
    }

    fn unary(&self, op: UnaryOp, value: Value) -> Result<Value, String> {
        match (op, value) {
            (UnaryOp::Neg, Value::Int(v)) => Ok(Value::Int(v.wrapping_neg())),
            (UnaryOp::Neg, Value::Float32(v)) => Ok(Value::Float32(-v)),
            (UnaryOp::Neg, Value::Float(v)) => Ok(Value::Float(-v)),
            (UnaryOp::Not, Value::Bool(v)) => Ok(Value::Bool(!v)),
            (op, value) => Err(format!("cannot apply {:?} to {:?}", op, value)),
        }
    }

    fn binary(&self, op: BinaryOp, lhs: &Value, rhs: &Value) -> Result<Value, String> {
        use BinaryOp::*;
        match op {
            Add | Sub | Mul | Div | Rem => match (lhs, rhs) {
                (Value::Int(a), Value::Int(b)) => {
                    let value = match op {
                        Add => a.wrapping_add(*b),
                        Sub => a.wrapping_sub(*b),
                        Mul => a.wrapping_mul(*b),
                        Div | Rem => {
                            if *b == 0 {
                                return Err(if op == Div {
                                    "integer division by zero"
                                } else {
                                    "integer modulo by zero"
                                }
                                .into());
                            }
                            if *a == i64::MIN && *b == -1 {
                                return Err("integer division overflow".into());
                            }
                            if op == Div {
                                a / b
                            } else {
                                a % b
                            }
                        }
                        _ => unreachable!(),
                    };
                    Ok(Value::Int(value))
                }
                (Value::Float32(a), Value::Float32(b)) => Ok(Value::Float32(match op {
                    Add => a + b,
                    Sub => a - b,
                    Mul => a * b,
                    Div => a / b,
                    Rem => a % b,
                    _ => unreachable!(),
                })),
                _ => {
                    let (a, b) = (as_f64(lhs)?, as_f64(rhs)?);
                    Ok(Value::Float(match op {
                        Add => a + b,
                        Sub => a - b,
                        Mul => a * b,
                        Div => a / b,
                        Rem => a % b,
                        _ => unreachable!(),
                    }))
                }
            },
            Eq | Ne => {
                let eq = match (lhs, rhs) {
                    (Value::Int(a), Value::Int(b)) => a == b,
                    (Value::Bool(a), Value::Bool(b)) => a == b,
                    (Value::Str(a), Value::Str(b)) => a == b,
                    (Value::Enum(ae, a), Value::Enum(be, b)) => ae == be && a == b,
                    (Value::CPtr(a), Value::CPtr(b)) => a == b,
                    _ => as_f64(lhs)? == as_f64(rhs)?,
                };
                Ok(Value::Bool(if op == Eq { eq } else { !eq }))
            }
            Lt | Le | Gt | Ge => {
                let (a, b) = (as_f64(lhs)?, as_f64(rhs)?);
                Ok(Value::Bool(match op {
                    Lt => a < b,
                    Le => a <= b,
                    Gt => a > b,
                    Ge => a >= b,
                    _ => unreachable!(),
                }))
            }
            ApproxEq => Ok(Value::Bool(approx_eq(as_f64(lhs)?, as_f64(rhs)?))),
        }
    }

    fn call(&self, name: &str, args: Vec<Value>) -> Result<Value, String> {
        match name {
            "print" => {
                let mut out = std::io::stdout().lock();
                for (i, v) in args.iter().enumerate() {
                    if i > 0 {
                        out.write_all(b" ").map_err(|e| e.to_string())?;
                    }
                    match v {
                        Value::Str(s) => out.write_all(s).map_err(|e| e.to_string())?,
                        _ => out
                            .write_all(self.display(v).as_bytes())
                            .map_err(|e| e.to_string())?,
                    }
                }
                out.write_all(b"\n").map_err(|e| e.to_string())?;
                Ok(Value::Unit)
            }
            "puts" => {
                let Some(Value::Str(s)) = args.first() else {
                    return Err("`puts` expects a str".into());
                };
                std::io::stdout().write_all(s).map_err(|e| e.to_string())?;
                Ok(Value::Unit)
            }
            "puti" | "putf" | "putb" => {
                print!(
                    "{}",
                    self.display(args.first().ok_or(format!("`{}` needs 1 arg", name))?)
                );
                Ok(Value::Unit)
            }
            "putsp" => {
                print!(" ");
                Ok(Value::Unit)
            }
            "putnl" => {
                println!();
                Ok(Value::Unit)
            }
            "nargs" => Ok(Value::Int(crate::runtime::args().len() as i64)),
            "arg" => {
                let i = as_i64(args.first().ok_or("`arg` needs 1 arg".to_string())?)?;
                let s = crate::runtime::args()
                    .get(i as usize)
                    .cloned()
                    .unwrap_or_default();
                Ok(Value::Str(Rc::new(s.into_bytes())))
            }
            "chr" => {
                let c = as_i64(args.first().ok_or("`chr` needs 1 arg".to_string())?)?;
                Ok(Value::Str(Rc::new(vec![c as u8])))
            }
            "concat" => match (&args[0], &args[1]) {
                (Value::Str(a), Value::Str(b)) => {
                    let mut bytes = Vec::with_capacity(a.len() + b.len());
                    bytes.extend_from_slice(a);
                    bytes.extend_from_slice(b);
                    Ok(Value::Str(Rc::new(bytes)))
                }
                _ => Err("`concat` expects two strs".into()),
            },
            "read_file" => {
                let p = match args.first() {
                    Some(Value::Str(s)) => String::from_utf8_lossy(s).into_owned(),
                    _ => return Err("`read_file` expects a str".into()),
                };
                match std::fs::read(&p) {
                    Ok(s) => Ok(Value::Str(Rc::new(s))),
                    Err(e) => {
                        eprintln!("error: cannot read {}: {}", p, e);
                        std::process::exit(1);
                    }
                }
            }
            "write_file" => match (&args[0], &args[1]) {
                (Value::Str(p), Value::Str(c)) => {
                    let path = String::from_utf8_lossy(p);
                    if let Err(e) = std::fs::write(path.as_ref(), c.as_slice()) {
                        eprintln!("error: cannot write {}: {}", path, e);
                        std::process::exit(1);
                    }
                    Ok(Value::Unit)
                }
                _ => Err("`write_file` expects (str, str)".into()),
            },
            "sqrt" | "sin" | "cos" | "acos" | "abs" | "floor" => {
                let x = as_f64(args.first().ok_or(format!("`{}` needs 1 arg", name))?)?;
                Ok(Value::Float(match name {
                    "sqrt" => x.sqrt(),
                    "sin" => x.sin(),
                    "cos" => x.cos(),
                    "acos" => x.acos(),
                    "abs" => x.abs(),
                    _ => x.floor(),
                }))
            }
            "min" | "max" | "pow" | "atan2" => {
                if args.len() != 2 {
                    return Err(format!("`{}` needs 2 args", name));
                }
                let (a, b) = (as_f64(&args[0])?, as_f64(&args[1])?);
                Ok(Value::Float(match name {
                    "min" => a.min(b),
                    "max" => a.max(b),
                    "pow" => a.powf(b),
                    _ => a.atan2(b),
                }))
            }
            "float" => Ok(Value::Float(as_f64(&args[0])?)),
            "f32" => Ok(Value::Float32(as_f64(&args[0])? as f32)),
            "int" => match &args[0] {
                Value::Enum(_, tag) => Ok(Value::Int(*tag)),
                v => Ok(Value::Int(as_f64(v)? as i64)),
            },
            "len" => match &args[0] {
                Value::Arr(cells) => Ok(Value::Int(cells.len() as i64)),
                Value::Str(s) => Ok(Value::Int(s.len() as i64)),
                v => Err(format!("`len` expects array or str, got {:?}", v)),
            },
            "substr" => match (&args[0], as_i64(&args[1])?, as_i64(&args[2])?) {
                (Value::Str(s), lo, hi) => {
                    if lo < 0 || hi < lo || hi as usize > s.len() {
                        return Err(format!(
                            "substr {}..{} out of bounds (length {})",
                            lo,
                            hi,
                            s.len()
                        ));
                    }
                    Ok(Value::Str(Rc::new(s[lo as usize..hi as usize].to_vec())))
                }
                _ => Err("`substr` expects (str, i64, i64)".into()),
            },
            "arr" => {
                let n = as_i64(&args[0])?;
                if n < 0 {
                    return Err(format!("invalid array length {}", n));
                }
                let init = args.get(1).cloned().unwrap_or(Value::Float(0.0));
                let n = usize::try_from(n).map_err(|_| "array allocation size overflow")?;
                let mut cells = Vec::new();
                cells
                    .try_reserve_exact(n)
                    .map_err(|_| "array allocation failed".to_string())?;
                cells.resize(n, init);
                Ok(Value::Arr(Rc::new(cells)))
            }
            "f32x4" => Ok(Value::F32x4([
                as_f64(&args[0])? as f32,
                as_f64(&args[1])? as f32,
                as_f64(&args[2])? as f32,
                as_f64(&args[3])? as f32,
            ])),
            "f64x2" => Ok(Value::F64x2([as_f64(&args[0])?, as_f64(&args[1])?])),
            "i64x2" => Ok(Value::I64x2([as_i64(&args[0])?, as_i64(&args[1])?])),
            "f32x4_splat" => Ok(Value::F32x4([as_f64(&args[0])? as f32; 4])),
            "f64x2_splat" => Ok(Value::F64x2([as_f64(&args[0])?; 2])),
            "i64x2_splat" => Ok(Value::I64x2([as_i64(&args[0])?; 2])),
            "f32x4_add" | "f32x4_sub" | "f32x4_mul" | "f32x4_div" => {
                let (Value::F32x4(a), Value::F32x4(b)) = (&args[0], &args[1]) else {
                    return Err(format!("`{name}` expects f32x4 values"));
                };
                Ok(Value::F32x4(std::array::from_fn(|i| match name {
                    "f32x4_add" => a[i] + b[i],
                    "f32x4_sub" => a[i] - b[i],
                    "f32x4_mul" => a[i] * b[i],
                    _ => a[i] / b[i],
                })))
            }
            "f64x2_add" | "f64x2_sub" | "f64x2_mul" | "f64x2_div" => {
                let (Value::F64x2(a), Value::F64x2(b)) = (&args[0], &args[1]) else {
                    return Err(format!("`{name}` expects f64x2 values"));
                };
                Ok(Value::F64x2(std::array::from_fn(|i| match name {
                    "f64x2_add" => a[i] + b[i],
                    "f64x2_sub" => a[i] - b[i],
                    "f64x2_mul" => a[i] * b[i],
                    _ => a[i] / b[i],
                })))
            }
            "i64x2_add" | "i64x2_sub" | "i64x2_mul" | "i64x2_div" => {
                let (Value::I64x2(a), Value::I64x2(b)) = (&args[0], &args[1]) else {
                    return Err(format!("`{name}` expects i64x2 values"));
                };
                let mut out = [0; 2];
                for i in 0..2 {
                    out[i] = match name {
                        "i64x2_add" => a[i].wrapping_add(b[i]),
                        "i64x2_sub" => a[i].wrapping_sub(b[i]),
                        "i64x2_mul" => a[i].wrapping_mul(b[i]),
                        _ if b[i] == 0 => return Err("integer division by zero".into()),
                        _ if a[i] == i64::MIN && b[i] == -1 => {
                            return Err("integer division overflow".into())
                        }
                        _ => a[i] / b[i],
                    };
                }
                Ok(Value::I64x2(out))
            }
            "f32x4_sum" => match &args[0] {
                Value::F32x4(v) => Ok(Value::Float32(v.iter().copied().sum())),
                _ => Err("`f32x4_sum` expects f32x4".into()),
            },
            "f64x2_sum" => match &args[0] {
                Value::F64x2(v) => Ok(Value::Float(v.iter().copied().sum())),
                _ => Err("`f64x2_sum` expects f64x2".into()),
            },
            "i64x2_sum" => match &args[0] {
                Value::I64x2(v) => Ok(Value::Int(v[0].wrapping_add(v[1]))),
                _ => Err("`i64x2_sum` expects i64x2".into()),
            },
            "f32x4_extract" => match &args[0] {
                Value::F32x4(v) => {
                    let i = as_i64(&args[1])?;
                    v.get(i as usize)
                        .copied()
                        .map(Value::Float32)
                        .ok_or_else(|| format!("index {i} out of bounds for f32x4"))
                }
                _ => Err("`f32x4_extract` expects f32x4".into()),
            },
            "f64x2_extract" => match &args[0] {
                Value::F64x2(v) => {
                    let i = as_i64(&args[1])?;
                    v.get(i as usize)
                        .copied()
                        .map(Value::Float)
                        .ok_or_else(|| format!("index {i} out of bounds for f64x2"))
                }
                _ => Err("`f64x2_extract` expects f64x2".into()),
            },
            "i64x2_extract" => match &args[0] {
                Value::I64x2(v) => {
                    let i = as_i64(&args[1])?;
                    v.get(i as usize)
                        .copied()
                        .map(Value::Int)
                        .ok_or_else(|| format!("index {i} out of bounds for i64x2"))
                }
                _ => Err("`i64x2_extract` expects i64x2".into()),
            },
            _ => Err(format!("unknown builtin `{}`", name)),
        }
    }

    fn display(&self, v: &Value) -> String {
        match v {
            Value::Int(i) => i.to_string(),
            Value::Float32(f) => format!("{}", f),
            Value::Float(f) => format!("{}", f),
            Value::F32x4(v) => format!(
                "f32x4({})",
                v.iter()
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Value::F64x2(v) => format!("f64x2({}, {})", v[0], v[1]),
            Value::I64x2(v) => format!("i64x2({}, {})", v[0], v[1]),
            Value::Bool(b) => b.to_string(),
            Value::Str(s) => String::from_utf8_lossy(s).into_owned(),
            Value::Unit => "()".into(),
            Value::Arr(cells) => {
                let parts: Vec<String> = cells.iter().map(|v| self.display(v)).collect();
                format!("[{}]", parts.join(", "))
            }
            Value::Rec(ti, fields) => {
                let decl = &self.ir.records[*ti];
                let parts: Vec<String> = decl
                    .fields
                    .iter()
                    .zip(fields.iter())
                    .map(|((n, _), v)| format!("{}: {}", n, self.display(v)))
                    .collect();
                format!("{} {{ {} }}", decl.name, parts.join(", "))
            }
            Value::Enum(ei, tag) => {
                let decl = &self.ir.enums[*ei];
                format!("{}.{}", decl.name, decl.variants[*tag as usize])
            }
            Value::CPtr(pointer) => format!("c_ptr(0x{pointer:x})"),
        }
    }

    fn call_extern(
        &self,
        id: ir::ExternId,
        args: Vec<Value>,
    ) -> Result<(Value, Vec<Option<Value>>), String> {
        use crate::check::Type;
        enum NativeArray {
            I64(Vec<i64>),
            F64(Vec<f64>),
        }
        fn marshal_record(
            ir: &ir::LoweredProgram,
            record_index: usize,
            fields: &[Value],
            ints: &mut [i64; 6],
            int_index: &mut usize,
            floats: &mut [f64; 8],
            float_index: &mut usize,
        ) -> Result<(), String> {
            use crate::check::Type;
            for ((_, ty), value) in ir.records[record_index].fields.iter().zip(fields) {
                match (ty, value) {
                    (Type::I64, Value::Int(value)) => {
                        ints[*int_index] = *value;
                        *int_index += 1;
                    }
                    (Type::Bool, Value::Bool(value)) => {
                        ints[*int_index] = i64::from(*value);
                        *int_index += 1;
                    }
                    (Type::CPtr(_), Value::CPtr(pointer)) => {
                        ints[*int_index] = *pointer as i64;
                        *int_index += 1;
                    }
                    (Type::F64, value) => {
                        floats[*float_index] = as_f64(value)?;
                        *float_index += 1;
                    }
                    (Type::Rec(nested), Value::Rec(actual, nested_fields)) if nested == actual => {
                        marshal_record(
                            ir,
                            *nested,
                            nested_fields,
                            ints,
                            int_index,
                            floats,
                            float_index,
                        )?;
                    }
                    _ => return Err("cannot marshal @c_layout record field".into()),
                }
            }
            Ok(())
        }
        let declaration = &self.ir.externs[id as usize];
        let pointer = crate::ffi::resolve(declaration.lib.as_deref(), &declaration.name)?;
        let mut ints = [0i64; 6];
        let mut floats = [0f64; 8];
        let mut int_index = 0;
        let mut float_index = 0;
        let mut arrays = Vec::new();
        for (argument_index, (argument, (_, ty))) in
            args.iter().zip(&declaration.params).enumerate()
        {
            match (ty, argument) {
                (Type::I64, Value::Int(value)) => {
                    ints[int_index] = *value;
                    int_index += 1;
                }
                (Type::Bool, Value::Bool(value)) => {
                    ints[int_index] = i64::from(*value);
                    int_index += 1;
                }
                (Type::Enum(expected), Value::Enum(actual, tag)) if expected == actual => {
                    ints[int_index] = *tag;
                    int_index += 1;
                }
                (Type::CPtr(_), Value::CPtr(pointer)) => {
                    ints[int_index] = *pointer as i64;
                    int_index += 1;
                }
                (Type::CFn(_, _), Value::CPtr(pointer)) => {
                    ints[int_index] = *pointer as i64;
                    int_index += 1;
                }
                (Type::F64, value) => {
                    floats[float_index] = as_f64(value)?;
                    float_index += 1;
                }
                (Type::F32, value) => {
                    floats[float_index] = f64::from_bits((as_f64(value)? as f32).to_bits() as u64);
                    float_index += 1;
                }
                (Type::Str, Value::Str(bytes)) => {
                    ints[int_index] = bytes.as_ptr() as i64;
                    ints[int_index + 1] = bytes.len() as i64;
                    int_index += 2;
                }
                (Type::Arr(element), Value::Arr(cells)) => {
                    let native = match element.as_ref() {
                        Type::I64 => NativeArray::I64(
                            cells.iter().map(as_i64).collect::<Result<Vec<_>, _>>()?,
                        ),
                        Type::F64 => NativeArray::F64(
                            cells.iter().map(as_f64).collect::<Result<Vec<_>, _>>()?,
                        ),
                        _ => return Err("unsupported FFI array element type".into()),
                    };
                    arrays.push((argument_index, true, native));
                    let array = &arrays.last().unwrap().2;
                    match array {
                        NativeArray::I64(values) => {
                            ints[int_index] = values.as_ptr() as i64;
                            ints[int_index + 1] = values.len() as i64;
                        }
                        NativeArray::F64(values) => {
                            ints[int_index] = values.as_ptr() as i64;
                            ints[int_index + 1] = values.len() as i64;
                        }
                    }
                    int_index += 2;
                }
                (Type::CSlice(element), Value::Arr(cells)) => {
                    let native = match element.as_ref() {
                        Type::I64 => NativeArray::I64(
                            cells.iter().map(as_i64).collect::<Result<Vec<_>, _>>()?,
                        ),
                        Type::F64 => NativeArray::F64(
                            cells.iter().map(as_f64).collect::<Result<Vec<_>, _>>()?,
                        ),
                        _ => return Err("unsupported FFI c_slice element type".into()),
                    };
                    arrays.push((argument_index, false, native));
                    let array = &arrays.last().unwrap().2;
                    match array {
                        NativeArray::I64(values) => {
                            ints[int_index] = values.as_ptr() as i64;
                            ints[int_index + 1] = values.len() as i64;
                        }
                        NativeArray::F64(values) => {
                            ints[int_index] = values.as_ptr() as i64;
                            ints[int_index + 1] = values.len() as i64;
                        }
                    }
                    int_index += 2;
                }
                (Type::CMutSlice(element), Value::Arr(cells)) => {
                    let native = match element.as_ref() {
                        Type::I64 => NativeArray::I64(
                            cells.iter().map(as_i64).collect::<Result<Vec<_>, _>>()?,
                        ),
                        Type::F64 => NativeArray::F64(
                            cells.iter().map(as_f64).collect::<Result<Vec<_>, _>>()?,
                        ),
                        _ => return Err("unsupported FFI c_mut_slice element type".into()),
                    };
                    arrays.push((argument_index, true, native));
                    let array = &arrays.last().unwrap().2;
                    match array {
                        NativeArray::I64(values) => {
                            ints[int_index] = values.as_ptr() as i64;
                            ints[int_index + 1] = values.len() as i64;
                        }
                        NativeArray::F64(values) => {
                            ints[int_index] = values.as_ptr() as i64;
                            ints[int_index + 1] = values.len() as i64;
                        }
                    }
                    int_index += 2;
                }
                (Type::Rec(expected), Value::Rec(actual, fields)) if expected == actual => {
                    marshal_record(
                        self.ir,
                        *expected,
                        fields,
                        &mut ints,
                        &mut int_index,
                        &mut floats,
                        &mut float_index,
                    )?;
                }
                _ => {
                    return Err(format!(
                        "cannot marshal {:?} as FFI type {:?}",
                        argument, ty
                    ))
                }
            }
        }
        let mut returned_length = 0i64;
        if declaration.ret == Type::Str {
            ints[int_index] = (&mut returned_length as *mut i64) as i64;
        }
        let result = unsafe {
            match &declaration.ret {
                Type::F32 => Value::Float32(crate::ffi::call_f32(pointer, ints, floats)),
                Type::F64 => Value::Float(crate::ffi::call_f64(pointer, ints, floats)),
                Type::Unit => {
                    crate::ffi::call_i64(pointer, ints, floats);
                    Value::Unit
                }
                Type::I64 => Value::Int(crate::ffi::call_i64(pointer, ints, floats)),
                Type::Bool => Value::Bool(crate::ffi::call_i64(pointer, ints, floats) != 0),
                Type::Enum(enumeration) => {
                    Value::Enum(*enumeration, crate::ffi::call_i64(pointer, ints, floats))
                }
                Type::CPtr(_) => Value::CPtr(crate::ffi::call_i64(pointer, ints, floats) as usize),
                Type::CFn(_, _) => {
                    Value::CPtr(crate::ffi::call_i64(pointer, ints, floats) as usize)
                }
                Type::Str => {
                    let returned = crate::ffi::call_i64(pointer, ints, floats) as *const u8;
                    if returned_length < 0 || (returned.is_null() && returned_length != 0) {
                        return Err("invalid returned FFI string".into());
                    }
                    let bytes = if returned_length == 0 {
                        Vec::new()
                    } else {
                        std::slice::from_raw_parts(returned, returned_length as usize).to_vec()
                    };
                    Value::Str(Rc::new(bytes))
                }
                Type::Rec(record) => {
                    let definition = &self.ir.records[*record];
                    let float_class = matches!(definition.fields[0].1, Type::F64);
                    let integer_values;
                    let float_values;
                    if float_class {
                        float_values = if definition.fields.len() == 1 {
                            [crate::ffi::call_f64(pointer, ints, floats), 0.0]
                        } else {
                            let pair = crate::ffi::call_f64_pair(pointer, ints, floats);
                            [pair.first, pair.second]
                        };
                        integer_values = [0, 0];
                    } else {
                        integer_values = if definition.fields.len() == 1 {
                            [crate::ffi::call_i64(pointer, ints, floats), 0]
                        } else {
                            let pair = crate::ffi::call_i64_pair(pointer, ints, floats);
                            [pair.first, pair.second]
                        };
                        float_values = [0.0, 0.0];
                    }
                    let fields = definition
                        .fields
                        .iter()
                        .enumerate()
                        .map(|(index, (_, ty))| match ty {
                            Type::I64 => Ok(Value::Int(integer_values[index])),
                            Type::Bool => Ok(Value::Bool(integer_values[index] != 0)),
                            Type::CPtr(_) => Ok(Value::CPtr(integer_values[index] as usize)),
                            Type::F64 => Ok(Value::Float(float_values[index])),
                            _ => Err("cannot unmarshal @c_layout record return".into()),
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    Value::Rec(*record, Rc::new(fields))
                }
                ty => return Err(format!("cannot return FFI type {:?}", ty)),
            }
        };
        let mut copyouts = vec![None; args.len()];
        for (index, copyout, array) in arrays {
            if !copyout {
                continue;
            }
            let cells = match array {
                NativeArray::I64(values) => values.into_iter().map(Value::Int).collect(),
                NativeArray::F64(values) => values.into_iter().map(Value::Float).collect(),
            };
            copyouts[index] = Some(Value::Arr(Rc::new(cells)));
        }
        Ok((result, copyouts))
    }
}
