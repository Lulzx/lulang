use crate::ir::{self, BinaryOp, Callee, Constant, InstKind, LoweredProgram, Terminator, UnaryOp};
use std::io::Write as _;
use std::rc::Rc;

const RTOL: f64 = 9.094947017729282e-13; // 2^-40
const ATOL: f64 = 7.888609052210118e-31; // 2^-100

#[derive(Clone, Debug)]
pub enum Value {
    Int(i64),
    Float32(f32),
    Float(f64),
    Bool(bool),
    Str(Rc<Vec<u8>>),
    Arr(Rc<Vec<Value>>),
    Rec(usize, Rc<Vec<Value>>),
    Enum(usize, i64),
    Unit,
}

pub struct Interp<'a> {
    ir: &'a LoweredProgram,
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
            let slot = fields.get_mut(field).ok_or_else(|| format!("invalid field {}", field))?;
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

impl<'a> Interp<'a> {
    pub fn new(ir: &'a LoweredProgram) -> Self {
        Interp { ir }
    }

    pub fn run_main(&self) -> Result<(), String> {
        let main = self.ir.main.as_ref().ok_or("no `main` block in program")?;
        self.execute(main, Vec::new())?;
        Ok(())
    }

    pub fn run_properties(&self, runs: u32) -> Result<bool, String> {
        let mut all_ok = true;
        let mut rng: u64 = 0x9E3779B97F4A7C15;
        for prop in &self.ir.properties {
            let function_id = prop.function;
            let mut failed = None;
            for _ in 0..runs {
                let args: Result<Vec<Value>, String> =
                    prop.params.iter().map(|(_, t)| self.gen_value(t, &mut rng)).collect();
                let args = args?;
                let v = self.execute(&self.ir.functions[function_id as usize], args.clone())?.0;
                match v {
                    Value::Bool(true) => {}
                    Value::Bool(false) => {
                        failed = Some(args);
                        break;
                    }
                    v => return Err(format!("property `{}` returned non-bool {:?}", prop.name, v)),
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

    /// Greedy shrink: repeatedly try simpler variants of each argument, keeping
    /// any that still falsify the property. Returns the final args + step count.
    fn shrink(&self, function_id: ir::FunctionId, mut args: Vec<Value>) -> Result<(Vec<Value>, u32), String> {
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
                    if matches!(self.execute(&self.ir.functions[function_id as usize], trial.clone())?.0, Value::Bool(false)) {
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
                    let simpler_mag = c == 0.0 || c.abs() < f.abs() || (c == c.trunc() && *f != f.trunc());
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
                let fields: Result<Vec<Value>, String> =
                    self.ir.records[*ti].fields.iter().map(|(_, ft)| self.gen_value(ft, rng)).collect();
                Ok(Value::Rec(*ti, Rc::new(fields?)))
            }
            t => Err(format!("cannot generate values of type `{}`", self.type_name(t))),
        }
    }

    fn type_name(&self, ty: &crate::check::Type) -> String {
        use crate::check::Type::*;
        match ty { I64=>"i64".into(),F32=>"f32".into(),F64=>"f64".into(),Bool=>"bool".into(),Str=>"str".into(),Unit=>"()".into(),Arr(t)=>format!("[{}]",self.type_name(t)),Rec(i)=>self.ir.records[*i].name.clone(),Enum(i)=>self.ir.enums[*i].name.clone() }
    }

    fn execute(&self, function: &ir::Function, args: Vec<Value>) -> Result<(Value, Vec<Value>), String> {
        if args.len() != function.params.len() {
            return Err(format!("`{}` expects {} args, got {}", function.name, function.params.len(), args.len()));
        }
        let mut locals = vec![Value::Unit; function.locals.len()];
        for (&local, value) in function.params.iter().zip(args) {
            locals[local as usize] = coerce(value, &function.locals[local as usize].ty)?;
        }
        let mut values = vec![Value::Unit; function.values.len()];
        let mut block_id = function.entry;
        loop {
            let block = &function.blocks[block_id as usize];
            for inst in &block.instructions {
                let result = match &inst.kind {
                    InstKind::Constant(c) => Some(match c {
                        Constant::I64(v) => Value::Int(*v), Constant::F32(v) => Value::Float32(*v), Constant::F64(v) => Value::Float(*v),
                        Constant::Bool(v) => Value::Bool(*v), Constant::Bytes(v) => Value::Str(Rc::new(v.clone())), Constant::Unit => Value::Unit,
                    }),
                    InstKind::Load(local) => Some(locals[*local as usize].clone()),
                    InstKind::Store { local, value, .. } => {
                        locals[*local as usize] = coerce(values[*value as usize].clone(), &function.locals[*local as usize].ty)?;
                        None
                    }
                    InstKind::Unary { op, value } => Some(self.unary(*op, values[*value as usize].clone())?),
                    InstKind::Binary { op, lhs, rhs } => Some(self.binary(*op, &values[*lhs as usize], &values[*rhs as usize])?),
                    InstKind::Select { condition, then_value, else_value } => {
                        let Value::Bool(condition) = values[*condition as usize] else {
                            return Err("IR select condition is not bool".into());
                        };
                        Some(values[if condition { *then_value } else { *else_value } as usize].clone())
                    }
                    InstKind::Call { callee, args, inout } => {
                        let call_args = args.iter().map(|v| values[*v as usize].clone()).collect::<Vec<_>>();
                        let result = match callee {
                            Callee::Function(id) => {
                                let callee = &self.ir.functions[*id as usize];
                                let (result, callee_frame) = self.execute(callee, call_args)?;
                                for (i, target) in inout.iter().enumerate() {
                                    if let Some(target) = target {
                                        locals[*target as usize] = callee_frame[callee.params[i] as usize].clone();
                                    }
                                }
                                result
                            }
                            Callee::Extern(id) => {
                                let (result, copyouts) =
                                    self.call_extern(*id, call_args)?;
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
                    InstKind::Field { base, record, field } => match &values[*base as usize] {
                        Value::Rec(actual, fields) if actual == record => Some(fields.get(*field).cloned().ok_or("invalid field index")?),
                        value => return Err(format!("cannot access field on {:?}", value)),
                    },
                    InstKind::Index { base, index } => {
                        let index = as_i64(&values[*index as usize])?;
                        Some(match &values[*base as usize] {
                            Value::Arr(cells) => cells.get(index as usize).cloned().ok_or_else(|| format!("index {} out of bounds", index))?,
                            Value::Str(bytes) => Value::Int(*bytes.get(index as usize).ok_or_else(|| format!("index {} out of bounds (length {})", index, bytes.len()))? as i64),
                            value => return Err(format!("cannot index into {:?}", value)),
                        })
                    }
                    InstKind::Array(items) => Some(Value::Arr(Rc::new(items.iter().map(|v| values[*v as usize].clone()).collect()))),
                    InstKind::Record { record, fields } => Some(Value::Rec(*record, Rc::new(fields.iter().map(|v| values[*v as usize].clone()).collect()))),
                    InstKind::Enum { enumeration, tag } => Some(Value::Enum(*enumeration, *tag)),
                    InstKind::SetIndex { root, path, index, value, .. } => {
                        let index = as_i64(&values[*index as usize])?;
                        set_index(
                            &mut locals[*root as usize],
                            path,
                            index as usize,
                            values[*value as usize].clone(),
                        )?;
                        None
                    }
                    InstKind::SetField { root, path, value } => { set_field(&mut locals[*root as usize], path, values[*value as usize].clone())?; None }
                };
                if let (Some(id), Some(result)) = (inst.result, result) {
                    values[id as usize] = coerce(result, &inst.ty)?;
                }
            }
            match block.terminator {
                Terminator::Jump(next) => block_id = next,
                Terminator::Branch { condition, then_block, else_block } => block_id = if matches!(values[condition as usize], Value::Bool(true)) { then_block } else { else_block },
                Terminator::Return(value) => return Ok((coerce(values[value as usize].clone(), &function.ret)?, locals)),
                Terminator::Unreachable => return Err(format!("reached unterminated IR block in `{}`", function.name)),
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
                    let value = match op { Add=>a.wrapping_add(*b),Sub=>a.wrapping_sub(*b),Mul=>a.wrapping_mul(*b),Div|Rem => {
                        if *b==0 { return Err(if op==Div{"integer division by zero"}else{"integer modulo by zero"}.into()); }
                        if *a==i64::MIN && *b == -1 { return Err("integer division overflow".into()); }
                        if op==Div {a/b} else {a%b}
                    }, _=>unreachable!() }; Ok(Value::Int(value))
                }
                (Value::Float32(a), Value::Float32(b)) => Ok(Value::Float32(match op {Add=>a+b,Sub=>a-b,Mul=>a*b,Div=>a/b,Rem=>a%b,_=>unreachable!()})),
                _ => { let (a,b)=(as_f64(lhs)?,as_f64(rhs)?); Ok(Value::Float(match op {Add=>a+b,Sub=>a-b,Mul=>a*b,Div=>a/b,Rem=>a%b,_=>unreachable!()})) }
            },
            Eq | Ne => { let eq=match(lhs,rhs){(Value::Int(a),Value::Int(b))=>a==b,(Value::Bool(a),Value::Bool(b))=>a==b,(Value::Str(a),Value::Str(b))=>a==b,(Value::Enum(ae,a),Value::Enum(be,b))=>ae==be&&a==b,_=>as_f64(lhs)?==as_f64(rhs)?};Ok(Value::Bool(if op==Eq{eq}else{!eq})) }
            Lt|Le|Gt|Ge => {let(a,b)=(as_f64(lhs)?,as_f64(rhs)?);Ok(Value::Bool(match op{Lt=>a<b,Le=>a<=b,Gt=>a>b,Ge=>a>=b,_=>unreachable!()}))}
            ApproxEq => Ok(Value::Bool(approx_eq(as_f64(lhs)?,as_f64(rhs)?))),
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
                print!("{}", self.display(args.first().ok_or(format!("`{}` needs 1 arg", name))?));
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
                let s = crate::runtime::args().get(i as usize).cloned().unwrap_or_default();
                Ok(Value::Str(Rc::new(s.into_bytes())))
            }
            "chr" => {
                let c = as_i64(args.first().ok_or("`chr` needs 1 arg".to_string())?)?;
                Ok(Value::Str(Rc::new(vec![c as u8])))
            }
            "concat" => {
                match (&args[0], &args[1]) {
                    (Value::Str(a), Value::Str(b)) => {
                        let mut bytes = Vec::with_capacity(a.len() + b.len());
                        bytes.extend_from_slice(a);
                        bytes.extend_from_slice(b);
                        Ok(Value::Str(Rc::new(bytes)))
                    }
                    _ => Err("`concat` expects two strs".into()),
                }
            }
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
            "write_file" => {
                match (&args[0], &args[1]) {
                    (Value::Str(p), Value::Str(c)) => {
                        let path = String::from_utf8_lossy(p);
                        if let Err(e) = std::fs::write(path.as_ref(), c.as_slice()) {
                            eprintln!("error: cannot write {}: {}", path, e);
                            std::process::exit(1);
                        }
                        Ok(Value::Unit)
                    }
                    _ => Err("`write_file` expects (str, str)".into()),
                }
            }
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
                        return Err(format!("substr {}..{} out of bounds (length {})", lo, hi, s.len()));
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
            _ => {
                Err(format!("unknown builtin `{}`", name))
            }
        }
    }

    fn display(&self, v: &Value) -> String {
        match v {
            Value::Int(i) => i.to_string(),
            Value::Float32(f) => format!("{}", f),
            Value::Float(f) => format!("{}", f),
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
                (Type::F64, value) => {
                    floats[float_index] = as_f64(value)?;
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
                            cells
                                .iter()
                                .map(as_i64)
                                .collect::<Result<Vec<_>, _>>()?,
                        ),
                        Type::F64 => NativeArray::F64(
                            cells
                                .iter()
                                .map(as_f64)
                                .collect::<Result<Vec<_>, _>>()?,
                        ),
                        _ => return Err("unsupported FFI array element type".into()),
                    };
                    arrays.push((argument_index, native));
                    let array = &arrays.last().unwrap().1;
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
                _ => {
                    return Err(format!(
                        "cannot marshal {:?} as FFI type {:?}",
                        argument, ty
                    ))
                }
            }
        }
        let result = unsafe {
            match &declaration.ret {
                Type::F64 => Value::Float(crate::ffi::call_f64(pointer, ints, floats)),
                Type::Unit => {
                    crate::ffi::call_i64(pointer, ints, floats);
                    Value::Unit
                }
                Type::I64 => Value::Int(crate::ffi::call_i64(pointer, ints, floats)),
                Type::Bool => Value::Bool(crate::ffi::call_i64(pointer, ints, floats) != 0),
                Type::Enum(enumeration) => Value::Enum(
                    *enumeration,
                    crate::ffi::call_i64(pointer, ints, floats),
                ),
                ty => return Err(format!("cannot return FFI type {:?}", ty)),
            }
        };
        let mut copyouts = vec![None; args.len()];
        for (index, array) in arrays {
            let cells = match array {
                NativeArray::I64(values) => values.into_iter().map(Value::Int).collect(),
                NativeArray::F64(values) => values.into_iter().map(Value::Float).collect(),
            };
            copyouts[index] = Some(Value::Arr(Rc::new(cells)));
        }
        Ok((result, copyouts))
    }
}
