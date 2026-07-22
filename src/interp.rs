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
                    println!("property {} ... FAIL", prop.name);
                    for ((name, ty), v) in prop.params.iter().zip(args.iter()) {
                        println!("  {}: {} = {}", name, ty, self.display(v));
                    }
                }
            }
        }
        Ok(all_ok)
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
            t => {
                let ti = *self.types.get(t).ok_or(format!("cannot generate values of type `{}`", t))?;
                let fields: Result<Vec<Value>, String> =
                    self.p.types[ti].fields.iter().map(|(_, ft)| self.gen_value(ft, rng)).collect();
                Ok(Value::Rec(ti, Rc::new(fields?)))
            }
        }
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
                    Expr::Field(_, _) => return Err("field assignment is not supported in v0.1".into()),
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
                self.call(name, vs?)
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
            "int" => Ok(Value::Int(as_f64(&args[0])? as i64)),
            "len" => match &args[0] {
                Value::Arr(cells) => Ok(Value::Int(cells.borrow().len() as i64)),
                v => Err(format!("`len` expects array, got {:?}", v)),
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
        }
    }
}
