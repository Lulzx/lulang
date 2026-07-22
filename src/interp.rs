use crate::ast::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

const RTOL: f64 = 9.094947017729282e-13; // 2^-40
const ATOL: f64 = 7.888609052210118e-31; // 2^-100

#[derive(Clone, Debug)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(Rc<String>),
    Arr(Rc<RefCell<Vec<Value>>>),
    Rec(usize, Rc<Vec<Value>>),
    Enum(usize, i64),
    Unit,
}

enum Flow {
    Normal(Value),
    Return(Value),
}

pub struct Interp<'a> {
    p: &'a Program,
    fns: HashMap<String, usize>,
    types: HashMap<String, usize>,
}

type Env = Vec<HashMap<String, Value>>;

fn lookup<'e>(env: &'e mut Env, name: &str) -> Option<&'e mut Value> {
    for scope in env.iter_mut().rev() {
        if scope.contains_key(name) {
            return scope.get_mut(name);
        }
    }
    None
}

fn as_f64(v: &Value) -> Result<f64, String> {
    match v {
        Value::Float(f) => Ok(*f),
        Value::Int(i) => Ok(*i as f64),
        v => Err(format!("expected number, got {:?}", v)),
    }
}

fn as_i64(v: &Value) -> Result<i64, String> {
    match v {
        Value::Int(i) => Ok(*i),
        v => Err(format!("expected integer, got {:?}", v)),
    }
}

fn set_field(p: &Program, slot: &mut Value, path: &[String], v: Value) -> Result<(), String> {
    let Some(f) = path.first() else {
        *slot = v;
        return Ok(());
    };
    match slot {
        Value::Rec(ti, fields) => {
            let idx = p.types[*ti]
                .fields
                .iter()
                .position(|(n, _)| n == f)
                .ok_or(format!("type `{}` has no field `{}`", p.types[*ti].name, f))?;
            let fields = Rc::make_mut(fields);
            set_field(p, &mut fields[idx], &path[1..], v)
        }
        v => Err(format!("cannot assign field `{}` on {:?}", f, v)),
    }
}

fn approx_eq(a: f64, b: f64) -> bool {
    (a - b).abs() <= ATOL + RTOL * a.abs().max(b.abs())
}

impl<'a> Interp<'a> {
    pub fn new(p: &'a Program) -> Self {
        let mut fns = HashMap::new();
        for (i, f) in p.fns.iter().enumerate() {
            fns.insert(f.name.clone(), i);
        }
        let mut types = HashMap::new();
        for (i, t) in p.types.iter().enumerate() {
            types.insert(t.name.clone(), i);
        }
        Interp { p, fns, types }
    }

    pub fn run_main(&self) -> Result<(), String> {
        let body = self.p.main.as_ref().ok_or("no `main` block in program")?;
        let mut env: Env = vec![HashMap::new()];
        self.exec_block(body, &mut env)?;
        Ok(())
    }

    pub fn run_properties(&self, runs: u32) -> Result<bool, String> {
        let mut all_ok = true;
        let mut rng: u64 = 0x9E3779B97F4A7C15;
        for prop in &self.p.props {
            let mut failed = None;
            for _ in 0..runs {
                let args: Result<Vec<Value>, String> =
                    prop.params.iter().map(|(_, t)| self.gen_value(t, &mut rng)).collect();
                let args = args?;
                let v = self.call_decl(prop, args.clone())?;
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
                    let (args, steps) = self.shrink(prop, args)?;
                    println!(
                        "property {} ... FAIL (counterexample shrunk {} steps)",
                        prop.name, steps
                    );
                    for ((name, ty), v) in prop.params.iter().zip(args.iter()) {
                        println!("  {}: {} = {}", name, ty, self.display(v));
                    }
                }
            }
        }
        Ok(all_ok)
    }

    /// Greedy shrink: repeatedly try simpler variants of each argument, keeping
    /// any that still falsify the property. Returns the final args + step count.
    fn shrink(&self, prop: &FnDecl, mut args: Vec<Value>) -> Result<(Vec<Value>, u32), String> {
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
                    if matches!(self.call_decl(prop, trial.clone())?, Value::Bool(false)) {
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

    fn gen_value(&self, ty: &str, rng: &mut u64) -> Result<Value, String> {
        fn next(rng: &mut u64) -> u64 {
            *rng ^= *rng << 13;
            *rng ^= *rng >> 7;
            *rng ^= *rng << 17;
            *rng
        }
        match ty {
            "f64" | "f32" => {
                let u = (next(rng) >> 11) as f64 / 9007199254740992.0;
                Ok(Value::Float(u * 2.0 - 1.0))
            }
            "i64" => Ok(Value::Int((next(rng) % 32) as i64)),
            "bool" => Ok(Value::Bool(next(rng) & 1 == 1)),
            t if self.p.enums.iter().any(|e| e.name == t) => {
                let ei = self.p.enums.iter().position(|e| e.name == t).unwrap();
                let n = self.p.enums[ei].variants.len() as u64;
                Ok(Value::Enum(ei, (next(rng) % n) as i64))
            }
            t => {
                let ti = *self.types.get(t).ok_or(format!("cannot generate values of type `{}`", t))?;
                let fields: Result<Vec<Value>, String> =
                    self.p.types[ti].fields.iter().map(|(_, ft)| self.gen_value(ft, rng)).collect();
                Ok(Value::Rec(ti, Rc::new(fields?)))
            }
        }
    }

    /// Call with copy-in/copy-out `inout` params: after the body runs, the
    /// final parameter values are written back to the caller's variables.
    fn call_inout(
        &self,
        f: &FnDecl,
        args: Vec<Value>,
        arg_es: &[ExprId],
        env: &mut Env,
    ) -> Result<Value, String> {
        let mut scope = HashMap::new();
        for ((name, _), v) in f.params.iter().zip(args) {
            scope.insert(name.clone(), v);
        }
        let mut cenv: Env = vec![scope];
        let ret = match self.exec_block(&f.body, &mut cenv)? {
            Flow::Return(v) | Flow::Normal(v) => v,
        };
        for (i, (pname, _)) in f.params.iter().enumerate() {
            if f.inouts[i] {
                let out = cenv[0][pname].clone();
                let Expr::Ident(n) = self.p.expr(arg_es[i]) else {
                    return Err("inout arg must be a variable".into());
                };
                *lookup(env, n).ok_or(format!("unknown variable `{}`", n))? = out;
            }
        }
        Ok(ret)
    }

    fn call_decl(&self, f: &FnDecl, args: Vec<Value>) -> Result<Value, String> {
        if args.len() != f.params.len() {
            return Err(format!("`{}` expects {} args, got {}", f.name, f.params.len(), args.len()));
        }
        let mut scope = HashMap::new();
        for ((name, _), v) in f.params.iter().zip(args) {
            scope.insert(name.clone(), v);
        }
        let mut env: Env = vec![scope];
        match self.exec_block(&f.body, &mut env)? {
            Flow::Return(v) | Flow::Normal(v) => Ok(v),
        }
    }

    fn exec_block(&self, stmts: &[StmtId], env: &mut Env) -> Result<Flow, String> {
        env.push(HashMap::new());
        let mut last = Value::Unit;
        for &sid in stmts {
            match self.exec_stmt(sid, env)? {
                Flow::Return(v) => {
                    env.pop();
                    return Ok(Flow::Return(v));
                }
                Flow::Normal(v) => last = v,
            }
        }
        env.pop();
        Ok(Flow::Normal(last))
    }

    fn exec_stmt(&self, sid: StmtId, env: &mut Env) -> Result<Flow, String> {
        match self.p.stmt(sid) {
            Stmt::Let(n, e) | Stmt::Var(n, e) => {
                let v = self.eval(*e, env)?;
                env.last_mut().unwrap().insert(n.clone(), v);
                Ok(Flow::Normal(Value::Unit))
            }
            Stmt::Assign(target, e) => {
                let v = self.eval(*e, env)?;
                match self.p.expr(*target) {
                    Expr::Ident(n) => {
                        let slot = lookup(env, n).ok_or(format!("unknown variable `{}`", n))?;
                        *slot = v;
                    }
                    Expr::Index(a, i) => {
                        let idx = as_i64(&self.eval(*i, env)?)?;
                        let arr = self.eval(*a, env)?;
                        match arr {
                            Value::Arr(cells) => {
                                let mut cells = cells.borrow_mut();
                                let slot = cells
                                    .get_mut(idx as usize)
                                    .ok_or(format!("index {} out of bounds", idx))?;
                                *slot = v;
                            }
                            v => return Err(format!("cannot index into {:?}", v)),
                        }
                    }
                    Expr::Field(_, _) => {
                        // x.f = v (possibly nested): copy-on-write into the
                        // variable's record value — pure value semantics
                        let mut path = Vec::new();
                        let mut cur = *target;
                        let root = loop {
                            match self.p.expr(cur) {
                                Expr::Field(b, f) => {
                                    path.push(f.clone());
                                    cur = *b;
                                }
                                Expr::Ident(n) => break n.clone(),
                                _ => return Err("field assignment root must be a variable".into()),
                            }
                        };
                        path.reverse();
                        let slot =
                            lookup(env, &root).ok_or(format!("unknown variable `{}`", root))?;
                        set_field(self.p, slot, &path, v)?;
                    }
                    _ => return Err("invalid assignment target".into()),
                }
                Ok(Flow::Normal(Value::Unit))
            }
            Stmt::If(c, then, els) => {
                let cv = self.eval(*c, env)?;
                let b = matches!(cv, Value::Bool(true));
                if b {
                    self.exec_block(then, env)
                } else if !els.is_empty() {
                    self.exec_block(els, env)
                } else {
                    Ok(Flow::Normal(Value::Unit))
                }
            }
            Stmt::While(c, body) => {
                loop {
                    if !matches!(self.eval(*c, env)?, Value::Bool(true)) {
                        break;
                    }
                    if let Flow::Return(rv) = self.exec_block(body, env)? {
                        return Ok(Flow::Return(rv));
                    }
                }
                Ok(Flow::Normal(Value::Unit))
            }
            Stmt::For(v, lo, hi, body) => {
                let lo = as_i64(&self.eval(*lo, env)?)?;
                let hi = as_i64(&self.eval(*hi, env)?)?;
                for i in lo..hi {
                    env.push(HashMap::new());
                    env.last_mut().unwrap().insert(v.clone(), Value::Int(i));
                    let fl = self.exec_block(body, env)?;
                    env.pop();
                    if let Flow::Return(rv) = fl {
                        return Ok(Flow::Return(rv));
                    }
                }
                Ok(Flow::Normal(Value::Unit))
            }
            Stmt::Return(e) => {
                let v = match e {
                    Some(e) => self.eval(*e, env)?,
                    None => Value::Unit,
                };
                Ok(Flow::Return(v))
            }
            Stmt::Expr(e) => Ok(Flow::Normal(self.eval(*e, env)?)),
        }
    }

    fn eval(&self, eid: ExprId, env: &mut Env) -> Result<Value, String> {
        match self.p.expr(eid) {
            Expr::Int(v) => Ok(Value::Int(*v)),
            Expr::Float(v) => Ok(Value::Float(*v)),
            Expr::Str(s) => Ok(Value::Str(Rc::new(s.clone()))),
            Expr::Bool(b) => Ok(Value::Bool(*b)),
            Expr::Ident(n) => lookup(env, n).cloned().ok_or(format!("unknown variable `{}`", n)),
            Expr::Un(op, e) => {
                let v = self.eval(*e, env)?;
                match (op.as_str(), v) {
                    ("-", Value::Int(i)) => Ok(Value::Int(-i)),
                    ("-", Value::Float(f)) => Ok(Value::Float(-f)),
                    ("not", Value::Bool(b)) => Ok(Value::Bool(!b)),
                    (op, v) => Err(format!("cannot apply `{}` to {:?}", op, v)),
                }
            }
            Expr::Bin(op, l, r) => self.eval_bin(op, *l, *r, env),
            Expr::Circum(open, e) => {
                let fname = &self.p.circum_ops[open].1;
                let v = self.eval(*e, env)?;
                let fi = *self.fns.get(fname).ok_or(format!("unknown operator fn `{}`", fname))?;
                self.call_decl(&self.p.fns[fi], vec![v])
            }
            Expr::Field(e, f) => {
                let v = self.eval(*e, env)?;
                match v {
                    Value::Rec(ti, fields) => {
                        let idx = self.p.types[ti]
                            .fields
                            .iter()
                            .position(|(n, _)| n == f)
                            .ok_or(format!("type `{}` has no field `{}`", self.p.types[ti].name, f))?;
                        Ok(fields[idx].clone())
                    }
                    v => Err(format!("cannot access field `{}` on {:?}", f, v)),
                }
            }
            Expr::Index(a, i) => {
                let idx = as_i64(&self.eval(*i, env)?)?;
                match self.eval(*a, env)? {
                    Value::Arr(cells) => cells
                        .borrow()
                        .get(idx as usize)
                        .cloned()
                        .ok_or(format!("index {} out of bounds", idx)),
                    Value::Str(s) => s
                        .as_bytes()
                        .get(idx as usize)
                        .map(|&b| Value::Int(b as i64))
                        .ok_or(format!("index {} out of bounds (length {})", idx, s.len())),
                    v => Err(format!("cannot index into {:?}", v)),
                }
            }
            Expr::Array(items) => {
                let vs: Result<Vec<Value>, String> = items.iter().map(|&e| self.eval(e, env)).collect();
                Ok(Value::Arr(Rc::new(RefCell::new(vs?))))
            }
            Expr::Record(name, inits) => {
                let ti = *self.types.get(name).ok_or(format!("unknown type `{}`", name))?;
                let decl = &self.p.types[ti];
                if inits.len() != decl.fields.len() {
                    return Err(format!(
                        "`{}` has {} fields, literal provides {}",
                        name,
                        decl.fields.len(),
                        inits.len()
                    ));
                }
                let mut fields = vec![Value::Unit; decl.fields.len()];
                for (pos, (fname, e)) in inits.iter().enumerate() {
                    let idx = match fname {
                        Some(f) => decl
                            .fields
                            .iter()
                            .position(|(n, _)| n == f)
                            .ok_or(format!("type `{}` has no field `{}`", name, f))?,
                        None => pos,
                    };
                    fields[idx] = self.eval(*e, env)?;
                }
                Ok(Value::Rec(ti, Rc::new(fields)))
            }
            Expr::EnumVal(en, vn) => {
                let (ei, tag) = self
                    .p
                    .enum_tag(en, vn)
                    .ok_or(format!("unknown enum value `{}.{}`", en, vn))?;
                Ok(Value::Enum(ei, tag))
            }
            Expr::Sum { var, lo, hi, body } => {
                let lo = as_i64(&self.eval(*lo, env)?)?;
                let hi = as_i64(&self.eval(*hi, env)?)?;
                let mut acc = 0.0f64;
                let mut int_acc: Option<i64> = Some(0);
                env.push(HashMap::new());
                for i in lo..hi {
                    env.last_mut().unwrap().insert(var.clone(), Value::Int(i));
                    match self.eval(*body, env)? {
                        Value::Int(v) => {
                            acc += v as f64;
                            int_acc = int_acc.map(|a| a + v);
                        }
                        Value::Float(v) => {
                            acc += v;
                            int_acc = None;
                        }
                        v => {
                            env.pop();
                            return Err(format!("sum body must be numeric, got {:?}", v));
                        }
                    }
                }
                env.pop();
                Ok(match int_acc {
                    Some(i) => Value::Int(i),
                    None => Value::Float(acc),
                })
            }
            Expr::Call(name, args) => {
                let vs: Result<Vec<Value>, String> = args.iter().map(|&e| self.eval(e, env)).collect();
                let vs = vs?;
                if let Some(&fi) = self.fns.get(name.as_str()) {
                    if self.p.fns[fi].has_inout() {
                        return self.call_inout(&self.p.fns[fi], vs, args, env);
                    }
                }
                self.call(name, vs)
            }
        }
    }

    fn eval_bin(&self, op: &str, l: ExprId, r: ExprId, env: &mut Env) -> Result<Value, String> {
        if op == "and" || op == "or" {
            let lv = matches!(self.eval(l, env)?, Value::Bool(true));
            return Ok(Value::Bool(match op {
                "and" => lv && matches!(self.eval(r, env)?, Value::Bool(true)),
                _ => lv || matches!(self.eval(r, env)?, Value::Bool(true)),
            }));
        }
        let lv = self.eval(l, env)?;
        let rv = self.eval(r, env)?;
        if let Some(fname) = self.p.infix_ops.get(op) {
            let fi = *self.fns.get(fname).ok_or(format!("unknown operator fn `{}`", fname))?;
            return self.call_decl(&self.p.fns[fi], vec![lv, rv]);
        }
        match op {
            "+" | "-" | "*" | "/" | "%" => match (&lv, &rv) {
                (Value::Int(a), Value::Int(b)) => Ok(Value::Int(match op {
                    "+" => a + b,
                    "-" => a - b,
                    "*" => a * b,
                    "/" => {
                        if *b == 0 {
                            return Err("integer division by zero".into());
                        }
                        a / b
                    }
                    _ => {
                        if *b == 0 {
                            return Err("integer modulo by zero".into());
                        }
                        a % b
                    }
                })),
                _ => {
                    let (a, b) = (as_f64(&lv)?, as_f64(&rv)?);
                    Ok(Value::Float(match op {
                        "+" => a + b,
                        "-" => a - b,
                        "*" => a * b,
                        "/" => a / b,
                        _ => a % b,
                    }))
                }
            },
            "==" | "!=" => {
                let eq = match (&lv, &rv) {
                    (Value::Int(a), Value::Int(b)) => a == b,
                    (Value::Bool(a), Value::Bool(b)) => a == b,
                    (Value::Str(a), Value::Str(b)) => a == b,
                    (Value::Enum(_, a), Value::Enum(_, b)) => a == b,
                    _ => as_f64(&lv)? == as_f64(&rv)?,
                };
                Ok(Value::Bool(if op == "==" { eq } else { !eq }))
            }
            "<" | "<=" | ">" | ">=" => {
                let (a, b) = (as_f64(&lv)?, as_f64(&rv)?);
                Ok(Value::Bool(match op {
                    "<" => a < b,
                    "<=" => a <= b,
                    ">" => a > b,
                    _ => a >= b,
                }))
            }
            "~=" | "\u{2248}" => Ok(Value::Bool(approx_eq(as_f64(&lv)?, as_f64(&rv)?))),
            op => Err(format!("unknown operator `{}`", op)),
        }
    }

    fn call(&self, name: &str, args: Vec<Value>) -> Result<Value, String> {
        match name {
            "print" => {
                let parts: Vec<String> = args.iter().map(|v| self.display(v)).collect();
                println!("{}", parts.join(" "));
                Ok(Value::Unit)
            }
            "puti" | "putf" | "putb" | "puts" => {
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
                Ok(Value::Str(Rc::new(s)))
            }
            "chr" => {
                let c = as_i64(args.first().ok_or("`chr` needs 1 arg".to_string())?)?;
                Ok(Value::Str(Rc::new(String::from_utf8_lossy(&[c as u8]).into_owned())))
            }
            "concat" => {
                match (&args[0], &args[1]) {
                    (Value::Str(a), Value::Str(b)) => Ok(Value::Str(Rc::new(format!("{}{}", a, b)))),
                    _ => Err("`concat` expects two strs".into()),
                }
            }
            "read_file" => {
                let p = match args.first() {
                    Some(Value::Str(s)) => s.to_string(),
                    _ => return Err("`read_file` expects a str".into()),
                };
                match std::fs::read_to_string(&p) {
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
                        if let Err(e) = std::fs::write(p.as_str(), c.as_bytes()) {
                            eprintln!("error: cannot write {}: {}", p, e);
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
            "int" => match &args[0] {
                Value::Enum(_, tag) => Ok(Value::Int(*tag)),
                v => Ok(Value::Int(as_f64(v)? as i64)),
            },
            "len" => match &args[0] {
                Value::Arr(cells) => Ok(Value::Int(cells.borrow().len() as i64)),
                Value::Str(s) => Ok(Value::Int(s.len() as i64)),
                v => Err(format!("`len` expects array or str, got {:?}", v)),
            },
            "substr" => match (&args[0], as_i64(&args[1])?, as_i64(&args[2])?) {
                (Value::Str(s), lo, hi) => {
                    if lo < 0 || hi < lo || hi as usize > s.len() {
                        return Err(format!("substr {}..{} out of bounds (length {})", lo, hi, s.len()));
                    }
                    let bytes = &s.as_bytes()[lo as usize..hi as usize];
                    Ok(Value::Str(Rc::new(String::from_utf8_lossy(bytes).into_owned())))
                }
                _ => Err("`substr` expects (str, i64, i64)".into()),
            },
            "arr" => {
                let n = as_i64(&args[0])?;
                let init = args.get(1).cloned().unwrap_or(Value::Float(0.0));
                Ok(Value::Arr(Rc::new(RefCell::new(vec![init; n as usize]))))
            }
            _ => {
                let fi = *self.fns.get(name).ok_or(format!("unknown function `{}`", name))?;
                self.call_decl(&self.p.fns[fi], args)
            }
        }
    }

    fn display(&self, v: &Value) -> String {
        match v {
            Value::Int(i) => i.to_string(),
            Value::Float(f) => format!("{}", f),
            Value::Bool(b) => b.to_string(),
            Value::Str(s) => s.to_string(),
            Value::Unit => "()".into(),
            Value::Arr(cells) => {
                let parts: Vec<String> = cells.borrow().iter().map(|v| self.display(v)).collect();
                format!("[{}]", parts.join(", "))
            }
            Value::Rec(ti, fields) => {
                let decl = &self.p.types[*ti];
                let parts: Vec<String> = decl
                    .fields
                    .iter()
                    .zip(fields.iter())
                    .map(|((n, _), v)| format!("{}: {}", n, self.display(v)))
                    .collect();
                format!("{} {{ {} }}", decl.name, parts.join(", "))
            }
            Value::Enum(ei, tag) => {
                let decl = &self.p.enums[*ei];
                format!("{}.{}", decl.name, decl.variants[*tag as usize])
            }
        }
    }
}
