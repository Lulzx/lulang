// Cranelift JIT backend for `lu run`.
//
// Records are scalarized: a Quat is four F64 SSA values, never memory — value
// semantics means aliasing is impossible, so nothing forces records into RAM.
// `sum` is emitted with 4 independent accumulators: the language defines
// reductions as order-free, so the reassociation is legal by construction.
use crate::ast::*;
use crate::check::{resolve_type, Type as CType};
use crate::runtime;
use cranelift_codegen::ir::{types, AbiParam, InstBuilder, MemFlags, Value};
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module};
use std::collections::HashMap;

const RTOL: f64 = 9.094947017729282e-13; // 2^-40
const ATOL: f64 = 7.888609052210118e-31; // 2^-100

fn comps(p: &Program, t: &CType) -> Result<Vec<cranelift_codegen::ir::Type>, String> {
    Ok(match t {
        CType::F64 => vec![types::F64],
        CType::I64 | CType::Bool => vec![types::I64],
        CType::Str => vec![types::I64, types::I64], // ptr, len
        CType::Arr(_) => vec![types::I64],          // ptr to header+data
        CType::Unit => vec![],
        CType::Rec(ti) => {
            let mut out = Vec::new();
            for (_, ft) in &p.types[*ti].fields {
                out.extend(comps(p, &resolve_type(p, ft)?)?);
            }
            out
        }
    })
}

pub fn field_offset(p: &Program, ti: usize, field: &str) -> Result<(usize, CType), String> {
    let mut off = 0;
    for (n, ft) in &p.types[ti].fields {
        let t = resolve_type(p, ft)?;
        let w = comps(p, &t)?.len();
        if n == field {
            return Ok((off, t));
        }
        off += w;
    }
    Err(format!("type `{}` has no field `{}`", p.types[ti].name, field))
}

struct FnInfo {
    id: FuncId,
    params: Vec<CType>,
    ret: CType,
}

pub struct Jit<'a> {
    p: &'a Program,
    module: JITModule,
    fns: HashMap<String, FnInfo>,
    imports: HashMap<&'static str, FuncId>,
}

impl<'a> Jit<'a> {
    pub fn run(p: &'a Program) -> Result<(), String> {
        use cranelift_codegen::settings::Configurable as _;
        let mut flags = cranelift_codegen::settings::builder();
        flags.set("opt_level", "speed").map_err(|e| e.to_string())?;
        let isa = cranelift_native::builder()
            .map_err(|e| e.to_string())?
            .finish(cranelift_codegen::settings::Flags::new(flags))
            .map_err(|e| e.to_string())?;
        let mut jb = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
        let syms: &[(&str, *const u8)] = &[
            ("lu_print_f64", runtime::lu_print_f64 as *const u8),
            ("lu_print_i64", runtime::lu_print_i64 as *const u8),
            ("lu_print_bool", runtime::lu_print_bool as *const u8),
            ("lu_print_str", runtime::lu_print_str as *const u8),
            ("lu_print_sep", runtime::lu_print_sep as *const u8),
            ("lu_print_nl", runtime::lu_print_nl as *const u8),
            ("lu_arr_new_f64", runtime::lu_arr_new_f64 as *const u8),
            ("lu_arr_new_i64", runtime::lu_arr_new_i64 as *const u8),
            ("lu_oob", runtime::lu_oob as *const u8),
            ("lu_sin", runtime::lu_sin as *const u8),
            ("lu_cos", runtime::lu_cos as *const u8),
            ("lu_acos", runtime::lu_acos as *const u8),
            ("lu_atan2", runtime::lu_atan2 as *const u8),
            ("lu_pow", runtime::lu_pow as *const u8),
        ];
        for (n, ptr) in syms {
            jb.symbol(*n, *ptr);
        }
        let module = JITModule::new(jb);
        let mut jit = Jit { p, module, fns: HashMap::new(), imports: HashMap::new() };
        jit.declare_imports()?;
        jit.declare_fns()?;
        for f in &p.fns {
            jit.compile_fn(f)?;
        }
        let main_id = jit.compile_main()?;
        jit.module.finalize_definitions().map_err(|e| e.to_string())?;
        let ptr = jit.module.get_finalized_function(main_id);
        let entry: extern "C" fn() = unsafe { std::mem::transmute(ptr) };
        entry();
        Ok(())
    }

    fn declare_imports(&mut self) -> Result<(), String> {
        let specs: &[(&'static str, usize, &[cranelift_codegen::ir::Type], bool)] = &[
            ("lu_print_f64", 0, &[types::F64], false),
            ("lu_print_i64", 0, &[types::I64], false),
            ("lu_print_bool", 0, &[types::I64], false),
            ("lu_print_str", 0, &[types::I64, types::I64], false),
            ("lu_print_sep", 0, &[], false),
            ("lu_print_nl", 0, &[], false),
            ("lu_arr_new_f64", 1, &[types::I64, types::F64], false),
            ("lu_arr_new_i64", 1, &[types::I64, types::I64], false),
            ("lu_oob", 0, &[types::I64, types::I64], false),
            ("lu_sin", 2, &[types::F64], true),
            ("lu_cos", 2, &[types::F64], true),
            ("lu_acos", 2, &[types::F64], true),
            ("lu_atan2", 2, &[types::F64, types::F64], true),
            ("lu_pow", 2, &[types::F64, types::F64], true),
        ];
        for (name, kind, params, _) in specs {
            let mut sig = self.module.make_signature();
            for &t in params.iter() {
                sig.params.push(AbiParam::new(t));
            }
            match kind {
                1 => sig.returns.push(AbiParam::new(types::I64)),
                2 => sig.returns.push(AbiParam::new(types::F64)),
                _ => {}
            }
            let id = self
                .module
                .declare_function(name, Linkage::Import, &sig)
                .map_err(|e| e.to_string())?;
            self.imports.insert(name, id);
        }
        Ok(())
    }

    fn declare_fns(&mut self) -> Result<(), String> {
        for f in &self.p.fns {
            let params: Result<Vec<CType>, String> =
                f.params.iter().map(|(_, t)| resolve_type(self.p, t)).collect();
            let params = params?;
            let ret = resolve_type(self.p, &f.ret)?;
            let mut sig = self.module.make_signature();
            for t in &params {
                for c in comps(self.p, t)? {
                    sig.params.push(AbiParam::new(c));
                }
            }
            for c in comps(self.p, &ret)? {
                sig.returns.push(AbiParam::new(c));
            }
            let id = self
                .module
                .declare_function(&f.name, Linkage::Local, &sig)
                .map_err(|e| e.to_string())?;
            self.fns.insert(f.name.clone(), FnInfo { id, params, ret });
        }
        Ok(())
    }

    fn compile_fn(&mut self, f: &FnDecl) -> Result<(), String> {
        let info_id = self.fns[&f.name].id;
        let params = self.fns[&f.name].params.clone();
        let ret = self.fns[&f.name].ret.clone();
        let mut ctx = self.module.make_context();
        let mut sig = self.module.make_signature();
        for t in &params {
            for c in comps(self.p, t)? {
                sig.params.push(AbiParam::new(c));
            }
        }
        for c in comps(self.p, &ret)? {
            sig.returns.push(AbiParam::new(c));
        }
        ctx.func.signature = sig;
        let mut fbc = FunctionBuilderContext::new();
        {
            let mut b = FunctionBuilder::new(&mut ctx.func, &mut fbc);
            let entry = b.create_block();
            b.append_block_params_for_function_params(entry);
            b.switch_to_block(entry);
            let entry_params: Vec<Value> = b.block_params(entry).to_vec();
            let mut g = Gen {
                p: self.p,
                b,
                module: &mut self.module,
                fns: &self.fns,
                imports: &self.imports,
                env: vec![HashMap::new()],
                refs: HashMap::new(),
                inline_frames: Vec::new(),
                inline_stack: Vec::new(),
                trusted_idx: Vec::new(),
            };
            let mut cursor = 0;
            for ((name, _), t) in f.params.iter().zip(params.iter()) {
                let n = comps(g.p, t)?.len();
                let vals = entry_params[cursor..cursor + n].to_vec();
                cursor += n;
                g.bind(name, t.clone(), &vals)?;
            }
            let (terminated, last) = g.gen_block(&f.body)?;
            if !terminated {
                if ret == CType::Unit {
                    g.b.ins().return_(&[]);
                } else {
                    match last {
                        Some((t, vals)) if t == ret => {
                            g.b.ins().return_(&vals);
                        }
                        _ => {
                            return Err(format!(
                                "function `{}` may end without returning a value",
                                f.name
                            ))
                        }
                    }
                }
            }
            g.b.seal_all_blocks();
            g.b.finalize();
        }
        self.module.define_function(info_id, &mut ctx).map_err(|e| e.to_string())?;
        self.module.clear_context(&mut ctx);
        Ok(())
    }

    fn compile_main(&mut self) -> Result<FuncId, String> {
        let body = self.p.main.as_ref().ok_or("no `main` block in program")?;
        let sig = self.module.make_signature();
        let id = self
            .module
            .declare_function("__lu_main", Linkage::Local, &sig)
            .map_err(|e| e.to_string())?;
        let mut ctx = self.module.make_context();
        ctx.func.signature = self.module.make_signature();
        let mut fbc = FunctionBuilderContext::new();
        {
            let b = FunctionBuilder::new(&mut ctx.func, &mut fbc);
            let mut g = Gen {
                p: self.p,
                b,
                module: &mut self.module,
                fns: &self.fns,
                imports: &self.imports,
                env: vec![HashMap::new()],
                refs: HashMap::new(),
                inline_frames: Vec::new(),
                inline_stack: Vec::new(),
                trusted_idx: Vec::new(),
            };
            let entry = g.b.create_block();
            g.b.switch_to_block(entry);
            let (terminated, _) = g.gen_block(body)?;
            if !terminated {
                g.b.ins().return_(&[]);
            }
            g.b.seal_all_blocks();
            g.b.finalize();
        }
        self.module.define_function(id, &mut ctx).map_err(|e| e.to_string())?;
        self.module.clear_context(&mut ctx);
        Ok(id)
    }
}

struct InlineFrame {
    result: Vec<Variable>,
    cont: cranelift_codegen::ir::Block,
}

struct Gen<'a, 'b> {
    p: &'a Program,
    b: FunctionBuilder<'b>,
    module: &'a mut JITModule,
    fns: &'a HashMap<String, FnInfo>,
    imports: &'a HashMap<&'static str, FuncId>,
    env: Vec<HashMap<String, (CType, Vec<Variable>)>>,
    refs: HashMap<String, cranelift_codegen::ir::FuncRef>,
    inline_frames: Vec<InlineFrame>,
    inline_stack: Vec<String>,
    // (array ident, loop var) pairs whose whole index range was checked at loop
    // entry — accesses through them skip the per-element bounds check.
    trusted_idx: Vec<(String, String)>,
}

/// Find arrays indexed as `a[i]` (i = the loop variable) in a loop body, so the
/// bounds check can be hoisted to loop entry. Returns None (trust nothing) if the
/// body shadows the loop variable or rebinds/reassigns any candidate array.
pub fn scan_trusted_expr(p: &Program, e: ExprId, var: &str, arrays: &mut Vec<String>, ok: &mut bool) {
    walk_e(p, e, var, arrays, ok)
}

pub fn scan_trusted(p: &Program, stmts: &[StmtId], var: &str) -> Option<Vec<String>> {
    let mut arrays = Vec::new();
    let mut ok = true;
    walk_s(p, stmts, var, &mut arrays, &mut ok);
    if ok && !arrays.is_empty() {
        Some(arrays)
    } else {
        None
    }
}

fn walk_e(p: &Program, e: ExprId, var: &str, arrays: &mut Vec<String>, ok: &mut bool) {
        match p.expr(e) {
            Expr::Index(a, i) => {
                if let (Expr::Ident(an), Expr::Ident(inm)) = (p.expr(*a), p.expr(*i)) {
                    if inm == var && !arrays.contains(an) {
                        arrays.push(an.clone());
                    }
                }
                walk_e(p, *a, var, arrays, ok);
                walk_e(p, *i, var, arrays, ok);
            }
            Expr::Bin(_, l, r) => {
                walk_e(p, *l, var, arrays, ok);
                walk_e(p, *r, var, arrays, ok);
            }
            Expr::Un(_, x) | Expr::Circum(_, x) | Expr::Field(x, _) => walk_e(p, *x, var, arrays, ok),
            Expr::Call(_, args) => args.iter().for_each(|&a| walk_e(p, a, var, arrays, ok)),
            Expr::Array(items) => items.iter().for_each(|&a| walk_e(p, a, var, arrays, ok)),
            Expr::Record(_, inits) => inits.iter().for_each(|(_, a)| walk_e(p, *a, var, arrays, ok)),
            Expr::Sum { var: v2, lo, hi, body } => {
                walk_e(p, *lo, var, arrays, ok);
                walk_e(p, *hi, var, arrays, ok);
                if v2 == var {
                    *ok = false; // shadowed
                } else {
                    walk_e(p, *body, var, arrays, ok);
                }
            }
            _ => {}
        }
    }
fn walk_s(p: &Program, stmts: &[StmtId], var: &str, arrays: &mut Vec<String>, ok: &mut bool) {
        for &sid in stmts {
            match p.stmt(sid) {
                Stmt::Let(n, e) | Stmt::Var(n, e) => {
                    if n == var {
                        *ok = false;
                    }
                    walk_e(p, *e, var, arrays, ok);
                }
                Stmt::Assign(t, e) => {
                    if let Expr::Ident(n) = p.expr(*t) {
                        // rebinding an array variable invalidates its hoisted check
                        if arrays.contains(n) {
                            *ok = false;
                        }
                    }
                    walk_e(p, *t, var, arrays, ok);
                    walk_e(p, *e, var, arrays, ok);
                }
                Stmt::If(c, a, b) => {
                    walk_e(p, *c, var, arrays, ok);
                    walk_s(p, a, var, arrays, ok);
                    walk_s(p, b, var, arrays, ok);
                }
                Stmt::For(v2, lo, hi, body) => {
                    walk_e(p, *lo, var, arrays, ok);
                    walk_e(p, *hi, var, arrays, ok);
                    if v2 == var {
                        *ok = false;
                    } else {
                        walk_s(p, body, var, arrays, ok);
                    }
                }
                Stmt::Return(Some(e)) | Stmt::Expr(e) => walk_e(p, *e, var, arrays, ok),
                Stmt::Return(None) => {}
            }
        }
    }

impl<'a, 'b> Gen<'a, 'b> {
    fn bind(&mut self, name: &str, t: CType, vals: &[Value]) -> Result<(), String> {
        let mut vars = Vec::new();
        for (i, c) in comps(self.p, &t)?.into_iter().enumerate() {
            let v = self.b.declare_var(c);
            self.b.def_var(v, vals[i]);
            vars.push(v);
        }
        self.env.last_mut().unwrap().insert(name.to_string(), (t, vars));
        Ok(())
    }

    fn lookup(&self, name: &str) -> Option<(CType, Vec<Variable>)> {
        self.env.iter().rev().find_map(|s| s.get(name).cloned())
    }

    fn callee(&mut self, name: &str) -> cranelift_codegen::ir::FuncRef {
        if let Some(&r) = self.refs.get(name) {
            return r;
        }
        let id = self
            .fns
            .get(name)
            .map(|f| f.id)
            .or_else(|| self.imports.get(name).copied())
            .expect("callee must be pre-declared");
        let r = self.module.declare_func_in_func(id, self.b.func);
        self.refs.insert(name.to_string(), r);
        r
    }

    fn call_import(&mut self, name: &'static str, args: &[Value]) -> Vec<Value> {
        let r = self.callee(name);
        let call = self.b.ins().call(r, args);
        self.b.inst_results(call).to_vec()
    }

    fn gen_block(&mut self, stmts: &[StmtId]) -> Result<(bool, Option<(CType, Vec<Value>)>), String> {
        self.env.push(HashMap::new());
        let mut last = None;
        for &sid in stmts {
            match self.gen_stmt(sid)? {
                StmtOut::Value(v) => last = v,
                StmtOut::Terminated => {
                    self.env.pop();
                    return Ok((true, None));
                }
            }
        }
        self.env.pop();
        Ok((false, last))
    }

    fn gen_stmt(&mut self, sid: StmtId) -> Result<StmtOut, String> {
        match self.p.stmt(sid) {
            Stmt::Let(n, e) | Stmt::Var(n, e) => {
                let (t, vals) = self.gen_expr(*e)?;
                self.bind(n, t, &vals)?;
                Ok(StmtOut::Value(None))
            }
            Stmt::Assign(target, e) => {
                let (_, vals) = self.gen_expr(*e)?;
                match self.p.expr(*target) {
                    Expr::Ident(n) => {
                        let (_, vars) = self.lookup(n).ok_or(format!("unknown variable `{}`", n))?;
                        for (var, val) in vars.iter().zip(vals.iter()) {
                            self.b.def_var(*var, *val);
                        }
                    }
                    Expr::Index(a, i) => {
                        let trusted = self.is_trusted(*a, *i);
                        let (at, avals) = self.gen_expr(*a)?;
                        let (_, ivals) = self.gen_expr(*i)?;
                        let elem = match at {
                            CType::Arr(e) => *e,
                            _ => return Err("cannot index non-array".into()),
                        };
                        let addr = self.index_addr(avals[0], ivals[0], &elem, trusted)?;
                        let mut off = 0i32;
                        for v in &vals {
                            self.b.ins().store(MemFlags::trusted(), *v, addr, off);
                            off += 8;
                        }
                    }
                    _ => return Err("invalid assignment target".into()),
                }
                Ok(StmtOut::Value(None))
            }
            Stmt::If(c, then_s, else_s) => {
                let (_, cv) = self.gen_expr(*c)?;
                let then_b = self.b.create_block();
                let else_b = self.b.create_block();
                let merge = self.b.create_block();
                self.b.ins().brif(cv[0], then_b, &[], else_b, &[]);
                self.b.switch_to_block(then_b);
                let (t_term, _) = self.gen_block(then_s)?;
                if !t_term {
                    self.b.ins().jump(merge, &[]);
                }
                self.b.switch_to_block(else_b);
                let (e_term, _) = self.gen_block(else_s)?;
                if !e_term {
                    self.b.ins().jump(merge, &[]);
                }
                if t_term && e_term {
                    // merge is unreachable but must still be a well-formed block
                    self.b.switch_to_block(merge);
                    self.b.ins().trap(cranelift_codegen::ir::TrapCode::unwrap_user(1));
                    return Ok(StmtOut::Terminated);
                }
                self.b.switch_to_block(merge);
                Ok(StmtOut::Value(None))
            }
            Stmt::For(v, lo, hi, body) => {
                let (_, lov) = self.gen_expr(*lo)?;
                let (_, hiv) = self.gen_expr(*hi)?;
                let pushed = match scan_trusted(self.p, body, v) {
                    Some(arrays) => self.hoist_checks(&arrays, v, lov[0], hiv[0]),
                    None => 0,
                };
                let ivar = self.b.declare_var(types::I64);
                self.b.def_var(ivar, lov[0]);
                let header = self.b.create_block();
                let body_b = self.b.create_block();
                let exit = self.b.create_block();
                self.b.ins().jump(header, &[]);
                self.b.switch_to_block(header);
                let iv = self.b.use_var(ivar);
                let cond = self.b.ins().icmp(IntCC::SignedLessThan, iv, hiv[0]);
                self.b.ins().brif(cond, body_b, &[], exit, &[]);
                self.b.switch_to_block(body_b);
                self.env.push(HashMap::new());
                self.env
                    .last_mut()
                    .unwrap()
                    .insert(v.clone(), (CType::I64, vec![ivar]));
                let (term, _) = self.gen_block(body)?;
                self.env.pop();
                if !term {
                    let iv2 = self.b.use_var(ivar);
                    let one = self.b.ins().iconst(types::I64, 1);
                    let next = self.b.ins().iadd(iv2, one);
                    self.b.def_var(ivar, next);
                    self.b.ins().jump(header, &[]);
                }
                self.b.switch_to_block(exit);
                self.trusted_idx.truncate(self.trusted_idx.len() - pushed);
                Ok(StmtOut::Value(None))
            }
            Stmt::Return(e) => {
                let vals = match e {
                    Some(e) => self.gen_expr(*e)?.1,
                    None => Vec::new(),
                };
                if let Some(frame) = self.inline_frames.last() {
                    let result = frame.result.clone();
                    let cont = frame.cont;
                    for (var, val) in result.iter().zip(vals.iter()) {
                        self.b.def_var(*var, *val);
                    }
                    self.b.ins().jump(cont, &[]);
                } else {
                    self.b.ins().return_(&vals);
                }
                Ok(StmtOut::Terminated)
            }
            Stmt::Expr(e) => {
                let out = self.gen_expr(*e)?;
                Ok(StmtOut::Value(Some(out)))
            }
        }
    }

    fn index_addr(&mut self, base: Value, idx: Value, elem: &CType, checked: bool) -> Result<Value, String> {
        let stride = comps(self.p, elem)?.len() as i64;
        if !checked {
            let len = self.b.ins().load(types::I64, MemFlags::trusted(), base, 0);
            let logical = if stride == 1 {
                len
            } else {
                let s = self.b.ins().iconst(types::I64, stride);
                self.b.ins().sdiv(len, s)
            };
            let bad = self.b.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, idx, logical);
            let oob = self.b.create_block();
            let ok = self.b.create_block();
            self.b.ins().brif(bad, oob, &[], ok, &[]);
            self.b.switch_to_block(oob);
            let r = self.callee("lu_oob");
            self.b.ins().call(r, &[idx, logical]);
            self.b.ins().jump(ok, &[]); // lu_oob never returns; edge keeps CFG well-formed
            self.b.switch_to_block(ok);
        }
        let sbytes = self.b.ins().iconst(types::I64, stride * 8);
        let off = self.b.ins().imul(idx, sbytes);
        let base8 = self.b.ins().iadd_imm(base, 8);
        Ok(self.b.ins().iadd(base8, off))
    }

    fn is_trusted(&self, a: ExprId, i: ExprId) -> bool {
        if let (Expr::Ident(an), Expr::Ident(inm)) = (self.p.expr(a), self.p.expr(i)) {
            self.trusted_idx.iter().any(|(x, y)| x == an && y == inm)
        } else {
            false
        }
    }

    /// Emit one whole-range check per array at loop entry, then mark (array, var)
    /// trusted so body accesses skip per-element checks. Returns how many pairs
    /// were pushed (caller truncates trusted_idx back after the loop).
    fn hoist_checks(&mut self, arrays: &[String], var: &str, lo: Value, hi: Value) -> usize {
        let mut pushed = 0;
        for name in arrays {
            let Some((CType::Arr(elem), vars)) = self.lookup(name) else { continue };
            let stride = match comps(self.p, &elem) {
                Ok(c) => c.len() as i64,
                Err(_) => continue,
            };
            let base = self.b.use_var(vars[0]);
            let len = self.b.ins().load(types::I64, MemFlags::trusted(), base, 0);
            let logical = if stride == 1 {
                len
            } else {
                let s = self.b.ins().iconst(types::I64, stride);
                self.b.ins().sdiv(len, s)
            };
            let zero = self.b.ins().iconst(types::I64, 0);
            let neg = self.b.ins().icmp(IntCC::SignedLessThan, lo, zero);
            let over = self.b.ins().icmp(IntCC::SignedGreaterThan, hi, logical);
            let bad = self.b.ins().bor(neg, over);
            let oob = self.b.create_block();
            let ok = self.b.create_block();
            self.b.ins().brif(bad, oob, &[], ok, &[]);
            self.b.switch_to_block(oob);
            let r = self.callee("lu_oob");
            self.b.ins().call(r, &[hi, logical]);
            self.b.ins().jump(ok, &[]);
            self.b.switch_to_block(ok);
            self.trusted_idx.push((name.clone(), var.to_string()));
            pushed += 1;
        }
        pushed
    }

    fn gen_expr(&mut self, eid: ExprId) -> Result<(CType, Vec<Value>), String> {
        match self.p.expr(eid) {
            Expr::Int(v) => {
                let c = self.b.ins().iconst(types::I64, *v);
                Ok((CType::I64, vec![c]))
            }
            Expr::Float(v) => {
                let c = self.b.ins().f64const(*v);
                Ok((CType::F64, vec![c]))
            }
            Expr::Bool(v) => {
                let c = self.b.ins().iconst(types::I64, *v as i64);
                Ok((CType::Bool, vec![c]))
            }
            Expr::Str(s) => {
                let ptr = self.b.ins().iconst(types::I64, s.as_ptr() as i64);
                let len = self.b.ins().iconst(types::I64, s.len() as i64);
                Ok((CType::Str, vec![ptr, len]))
            }
            Expr::Ident(n) => {
                let (t, vars) = self.lookup(n).ok_or(format!("unknown variable `{}`", n))?;
                let vals: Vec<Value> = vars.iter().map(|&v| self.b.use_var(v)).collect();
                Ok((t, vals))
            }
            Expr::Un(op, e) => {
                let (t, vals) = self.gen_expr(*e)?;
                match (op.as_str(), &t) {
                    ("-", CType::F64) => {
                        let v = self.b.ins().fneg(vals[0]);
                        Ok((CType::F64, vec![v]))
                    }
                    ("-", CType::I64) => {
                        let v = self.b.ins().ineg(vals[0]);
                        Ok((CType::I64, vec![v]))
                    }
                    ("not", CType::Bool) => {
                        let v = self.b.ins().bxor_imm(vals[0], 1);
                        Ok((CType::Bool, vec![v]))
                    }
                    _ => Err(format!("cannot apply `{}` here", op)),
                }
            }
            Expr::Bin(op, l, r) => self.gen_bin(op.clone(), *l, *r),
            Expr::Circum(open, e) => {
                let fname = self.p.circum_ops[open].1.clone();
                let (_, vals) = self.gen_expr(*e)?;
                self.gen_user_call(&fname, vals)
            }
            Expr::Field(e, fname) => {
                let (t, vals) = self.gen_expr(*e)?;
                match t {
                    CType::Rec(ti) => {
                        let (off, ft) = field_offset(self.p, ti, fname)?;
                        let w = comps(self.p, &ft)?.len();
                        Ok((ft, vals[off..off + w].to_vec()))
                    }
                    _ => Err(format!("cannot access field `{}`", fname)),
                }
            }
            Expr::Index(a, i) => {
                let trusted = self.is_trusted(*a, *i);
                let (at, avals) = self.gen_expr(*a)?;
                let (_, ivals) = self.gen_expr(*i)?;
                let elem = match at {
                    CType::Arr(e) => *e,
                    _ => return Err("cannot index non-array".into()),
                };
                let addr = self.index_addr(avals[0], ivals[0], &elem, trusted)?;
                let mut out = Vec::new();
                let mut off = 0i32;
                for c in comps(self.p, &elem)? {
                    out.push(self.b.ins().load(c, MemFlags::trusted(), addr, off));
                    off += 8;
                }
                Ok((elem, out))
            }
            Expr::Array(_) => Err("array literals are not supported by the JIT yet (use arr)".into()),
            Expr::Record(name, inits) => {
                let ti = self
                    .p
                    .types
                    .iter()
                    .position(|t| t.name == *name)
                    .ok_or(format!("unknown type `{}`", name))?;
                let decl = &self.p.types[ti];
                let mut slots: Vec<Option<Vec<Value>>> = vec![None; decl.fields.len()];
                for (pos, (fname, e)) in inits.iter().enumerate() {
                    let idx = match fname {
                        Some(f) => decl.fields.iter().position(|(n, _)| n == f).unwrap(),
                        None => pos,
                    };
                    let want = resolve_type(self.p, &decl.fields[idx].1)?;
                    let (got, mut vals) = self.gen_expr(*e)?;
                    if want == CType::F64 && got == CType::I64 {
                        vals = vec![self.b.ins().fcvt_from_sint(types::F64, vals[0])];
                    }
                    slots[idx] = Some(vals);
                }
                let mut out = Vec::new();
                for s in slots {
                    out.extend(s.expect("checker guarantees all fields initialized"));
                }
                Ok((CType::Rec(ti), out))
            }
            Expr::Sum { var, lo, hi, body } => self.gen_sum(var.clone(), *lo, *hi, *body),
            Expr::Call(name, args) => {
                let mut avals = Vec::new();
                let mut atys = Vec::new();
                for &a in args {
                    let (t, vs) = self.gen_expr(a)?;
                    atys.push(t);
                    avals.push(vs);
                }
                self.gen_call(name, atys, avals)
            }
        }
    }

    fn f64_of(&mut self, t: &CType, v: Value) -> Value {
        if *t == CType::I64 {
            self.b.ins().fcvt_from_sint(types::F64, v)
        } else {
            v
        }
    }

    fn gen_bin(&mut self, op: String, l: ExprId, r: ExprId) -> Result<(CType, Vec<Value>), String> {
        if let Some(fname) = self.p.infix_ops.get(&op).cloned() {
            let (lt, lv) = self.gen_expr(l)?;
            let (rt, rv) = self.gen_expr(r)?;
            let info = &self.fns[&fname];
            let mut args = Vec::new();
            for ((t, vals), want) in [(lt, lv), (rt, rv)].into_iter().zip(info.params.clone()) {
                if want == CType::F64 && t == CType::I64 {
                    args.push(self.b.ins().fcvt_from_sint(types::F64, vals[0]));
                } else {
                    args.extend(vals);
                }
            }
            return self.gen_user_call(&fname, args);
        }
        let (lt, lv) = self.gen_expr(l)?;
        let (rt, rv) = self.gen_expr(r)?;
        match op.as_str() {
            "and" => Ok((CType::Bool, vec![self.b.ins().band(lv[0], rv[0])])),
            "or" => Ok((CType::Bool, vec![self.b.ins().bor(lv[0], rv[0])])),
            "+" | "-" | "*" | "/" | "%" => {
                if lt == CType::I64 && rt == CType::I64 {
                    let v = match op.as_str() {
                        "+" => self.b.ins().iadd(lv[0], rv[0]),
                        "-" => self.b.ins().isub(lv[0], rv[0]),
                        "*" => self.b.ins().imul(lv[0], rv[0]),
                        "/" => self.b.ins().sdiv(lv[0], rv[0]),
                        _ => self.b.ins().srem(lv[0], rv[0]),
                    };
                    Ok((CType::I64, vec![v]))
                } else {
                    let a = self.f64_of(&lt, lv[0]);
                    let b = self.f64_of(&rt, rv[0]);
                    let v = match op.as_str() {
                        "+" => self.b.ins().fadd(a, b),
                        "-" => self.b.ins().fsub(a, b),
                        "*" => self.b.ins().fmul(a, b),
                        "/" => self.b.ins().fdiv(a, b),
                        _ => return Err("`%` on floats is not supported yet".into()),
                    };
                    Ok((CType::F64, vec![v]))
                }
            }
            "==" | "!=" | "<" | "<=" | ">" | ">=" => {
                let both_int = matches!(lt, CType::I64 | CType::Bool) && matches!(rt, CType::I64 | CType::Bool);
                let c8 = if both_int {
                    let cc = match op.as_str() {
                        "==" => IntCC::Equal,
                        "!=" => IntCC::NotEqual,
                        "<" => IntCC::SignedLessThan,
                        "<=" => IntCC::SignedLessThanOrEqual,
                        ">" => IntCC::SignedGreaterThan,
                        _ => IntCC::SignedGreaterThanOrEqual,
                    };
                    self.b.ins().icmp(cc, lv[0], rv[0])
                } else {
                    let a = self.f64_of(&lt, lv[0]);
                    let b = self.f64_of(&rt, rv[0]);
                    let cc = match op.as_str() {
                        "==" => FloatCC::Equal,
                        "!=" => FloatCC::NotEqual,
                        "<" => FloatCC::LessThan,
                        "<=" => FloatCC::LessThanOrEqual,
                        ">" => FloatCC::GreaterThan,
                        _ => FloatCC::GreaterThanOrEqual,
                    };
                    self.b.ins().fcmp(cc, a, b)
                };
                let v = self.b.ins().uextend(types::I64, c8);
                Ok((CType::Bool, vec![v]))
            }
            "~=" | "\u{2248}" => {
                let a = self.f64_of(&lt, lv[0]);
                let bv = self.f64_of(&rt, rv[0]);
                let diff = self.b.ins().fsub(a, bv);
                let adiff = self.b.ins().fabs(diff);
                let aa = self.b.ins().fabs(a);
                let ab = self.b.ins().fabs(bv);
                let mx = self.b.ins().fmax(aa, ab);
                let rtol = self.b.ins().f64const(RTOL);
                let atol = self.b.ins().f64const(ATOL);
                let scaled = self.b.ins().fmul(mx, rtol);
                let tol = self.b.ins().fadd(scaled, atol);
                let c8 = self.b.ins().fcmp(FloatCC::LessThanOrEqual, adiff, tol);
                let v = self.b.ins().uextend(types::I64, c8);
                Ok((CType::Bool, vec![v]))
            }
            op => Err(format!("unknown operator `{}`", op)),
        }
    }

    fn gen_sum(&mut self, var: String, lo: ExprId, hi: ExprId, body: ExprId) -> Result<(CType, Vec<Value>), String> {
        let (_, lov) = self.gen_expr(lo)?;
        let (_, hiv) = self.gen_expr(hi)?;
        let expr_stmt_scan = {
            let mut arrays = Vec::new();
            let mut ok = true;
            scan_trusted_expr(self.p, body, &var, &mut arrays, &mut ok);
            if ok && !arrays.is_empty() { Some(arrays) } else { None }
        };
        let pushed = match expr_stmt_scan {
            Some(arrays) => self.hoist_checks(&arrays, &var, lov[0], hiv[0]),
            None => 0,
        };
        let ivar = self.b.declare_var(types::I64);
        self.b.def_var(ivar, lov[0]);
        self.env.push(HashMap::new());
        self.env.last_mut().unwrap().insert(var, (CType::I64, vec![ivar]));

        // `sum` is order-free by language contract: emit 4 independent
        // accumulators (the reassociation C++ needs -ffast-math to allow).
        let accs: Vec<Variable> = (0..4).map(|_| self.b.declare_var(types::F64)).collect();
        let zero = self.b.ins().f64const(0.0);
        for &a in &accs {
            self.b.def_var(a, zero);
        }
        let is_int = self.b.declare_var(types::I64); // discovered from body type below

        let head4 = self.b.create_block();
        let body4 = self.b.create_block();
        let head1 = self.b.create_block();
        let body1 = self.b.create_block();
        let exit = self.b.create_block();

        self.b.ins().jump(head4, &[]);
        self.b.switch_to_block(head4);
        let iv = self.b.use_var(ivar);
        let i3 = self.b.ins().iadd_imm(iv, 4);
        let fits = self.b.ins().icmp(IntCC::SignedLessThanOrEqual, i3, hiv[0]);
        self.b.ins().brif(fits, body4, &[], head1, &[]);

        self.b.switch_to_block(body4);
        let mut body_ty = CType::F64;
        for k in 0..4 {
            let ivk = self.b.use_var(ivar);
            let ik = self.b.ins().iadd_imm(ivk, k);
            self.b.def_var(ivar, ik);
            let (t, vals) = self.gen_expr(body)?;
            body_ty = t.clone();
            let f = self.f64_of(&t, vals[0]);
            let cur = self.b.use_var(accs[k as usize]);
            let nxt = self.b.ins().fadd(cur, f);
            self.b.def_var(accs[k as usize], nxt);
            // restore i to base before next unroll step
            let back = self.b.ins().iadd_imm(ik, -k);
            self.b.def_var(ivar, back);
        }
        let ivb = self.b.use_var(ivar);
        let ivn = self.b.ins().iadd_imm(ivb, 4);
        self.b.def_var(ivar, ivn);
        self.b.ins().jump(head4, &[]);

        self.b.switch_to_block(head1);
        let iv1 = self.b.use_var(ivar);
        let more = self.b.ins().icmp(IntCC::SignedLessThan, iv1, hiv[0]);
        self.b.ins().brif(more, body1, &[], exit, &[]);

        self.b.switch_to_block(body1);
        let (t, vals) = self.gen_expr(body)?;
        let f = self.f64_of(&t, vals[0]);
        let cur = self.b.use_var(accs[0]);
        let nxt = self.b.ins().fadd(cur, f);
        self.b.def_var(accs[0], nxt);
        let ivt = self.b.use_var(ivar);
        let ivt2 = self.b.ins().iadd_imm(ivt, 1);
        self.b.def_var(ivar, ivt2);
        self.b.ins().jump(head1, &[]);

        self.b.switch_to_block(exit);
        let a0 = self.b.use_var(accs[0]);
        let a1 = self.b.use_var(accs[1]);
        let a2 = self.b.use_var(accs[2]);
        let a3 = self.b.use_var(accs[3]);
        let s01 = self.b.ins().fadd(a0, a1);
        let s23 = self.b.ins().fadd(a2, a3);
        let total = self.b.ins().fadd(s01, s23);
        self.trusted_idx.truncate(self.trusted_idx.len() - pushed);
        self.env.pop();
        let _ = is_int;
        if body_ty == CType::I64 {
            let vi = self.b.ins().fcvt_to_sint(types::I64, total);
            Ok((CType::I64, vec![vi]))
        } else {
            Ok((CType::F64, vec![total]))
        }
    }

    fn gen_user_call(&mut self, fname: &str, args: Vec<Value>) -> Result<(CType, Vec<Value>), String> {
        let ret = self.fns[fname].ret.clone();
        // Inline every user function up to a depth cap (recursion falls back to a
        // real call). Args are SSA values bound to fresh variables — evaluated
        // exactly once, so this is always semantics-preserving.
        let recursive = self.inline_stack.iter().any(|n| n == fname);
        if !recursive && self.inline_stack.len() < 8 {
            let decl = self
                .p
                .fns
                .iter()
                .find(|f| f.name == fname)
                .expect("declared fn must exist");
            let params = self.fns[fname].params.clone();
            let saved_env = std::mem::replace(&mut self.env, vec![HashMap::new()]);
            let saved_trust = std::mem::take(&mut self.trusted_idx);
            let mut cursor = 0;
            for ((pname, _), t) in decl.params.iter().zip(params.iter()) {
                let n = comps(self.p, t)?.len();
                let vals = args[cursor..cursor + n].to_vec();
                cursor += n;
                self.bind(pname, t.clone(), &vals)?;
            }
            let result: Vec<Variable> = comps(self.p, &ret)?
                .into_iter()
                .map(|c| self.b.declare_var(c))
                .collect();
            let cont = self.b.create_block();
            self.inline_frames.push(InlineFrame { result: result.clone(), cont });
            self.inline_stack.push(fname.to_string());
            let (terminated, last) = self.gen_block(&decl.body)?;
            self.inline_stack.pop();
            self.inline_frames.pop();
            if !terminated {
                match last {
                    Some((t, vals)) if t == ret => {
                        for (var, val) in result.iter().zip(vals.iter()) {
                            self.b.def_var(*var, *val);
                        }
                    }
                    _ if ret == CType::Unit => {}
                    _ => return Err(format!("function `{}` may end without a value", fname)),
                }
                self.b.ins().jump(cont, &[]);
            }
            self.b.switch_to_block(cont);
            self.env = saved_env;
            self.trusted_idx = saved_trust;
            let out: Vec<Value> = result.iter().map(|&v| self.b.use_var(v)).collect();
            return Ok((ret, out));
        }
        let r = self.callee(fname);
        let call = self.b.ins().call(r, &args);
        let out = self.b.inst_results(call).to_vec();
        Ok((ret, out))
    }

    fn gen_call(
        &mut self,
        name: &str,
        atys: Vec<CType>,
        avals: Vec<Vec<Value>>,
    ) -> Result<(CType, Vec<Value>), String> {
        match name {
            "print" => {
                for (i, (t, vals)) in atys.iter().zip(avals.iter()).enumerate() {
                    if i > 0 {
                        self.call_import("lu_print_sep", &[]);
                    }
                    match t {
                        CType::F64 => {
                            self.call_import("lu_print_f64", &[vals[0]]);
                        }
                        CType::I64 => {
                            self.call_import("lu_print_i64", &[vals[0]]);
                        }
                        CType::Bool => {
                            self.call_import("lu_print_bool", &[vals[0]]);
                        }
                        CType::Str => {
                            self.call_import("lu_print_str", &[vals[0], vals[1]]);
                        }
                        t => return Err(format!("cannot print {:?} in JIT mode yet", t)),
                    }
                }
                self.call_import("lu_print_nl", &[]);
                Ok((CType::Unit, vec![]))
            }
            "sqrt" | "abs" | "floor" | "sin" | "cos" | "acos" => {
                let x = self.f64_of(&atys[0], avals[0][0]);
                let v = match name {
                    "sqrt" => self.b.ins().sqrt(x),
                    "abs" => self.b.ins().fabs(x),
                    "floor" => self.b.ins().floor(x),
                    "sin" => self.call_import("lu_sin", &[x])[0],
                    "cos" => self.call_import("lu_cos", &[x])[0],
                    _ => self.call_import("lu_acos", &[x])[0],
                };
                Ok((CType::F64, vec![v]))
            }
            "min" | "max" | "pow" | "atan2" => {
                let a = self.f64_of(&atys[0], avals[0][0]);
                let b = self.f64_of(&atys[1], avals[1][0]);
                let v = match name {
                    "min" => self.b.ins().fmin(a, b),
                    "max" => self.b.ins().fmax(a, b),
                    "pow" => self.call_import("lu_pow", &[a, b])[0],
                    _ => self.call_import("lu_atan2", &[a, b])[0],
                };
                Ok((CType::F64, vec![v]))
            }
            "float" => {
                let v = self.f64_of(&atys[0], avals[0][0]);
                Ok((CType::F64, vec![v]))
            }
            "int" => {
                let v = if atys[0] == CType::F64 {
                    self.b.ins().fcvt_to_sint(types::I64, avals[0][0])
                } else {
                    avals[0][0]
                };
                Ok((CType::I64, vec![v]))
            }
            "len" => {
                let elem = match &atys[0] {
                    CType::Arr(e) => e.as_ref().clone(),
                    _ => return Err("`len` expects array".into()),
                };
                let stride = comps(self.p, &elem)?.len() as i64;
                let n = self.b.ins().load(types::I64, MemFlags::trusted(), avals[0][0], 0);
                let v = if stride == 1 {
                    n
                } else {
                    let s = self.b.ins().iconst(types::I64, stride);
                    self.b.ins().sdiv(n, s)
                };
                Ok((CType::I64, vec![v]))
            }
            "arr" => {
                let n = avals[0][0];
                match &atys[1] {
                    CType::F64 => {
                        let p = self.call_import("lu_arr_new_f64", &[n, avals[1][0]])[0];
                        Ok((CType::Arr(Box::new(CType::F64)), vec![p]))
                    }
                    CType::I64 => {
                        let p = self.call_import("lu_arr_new_i64", &[n, avals[1][0]])[0];
                        Ok((CType::Arr(Box::new(CType::I64)), vec![p]))
                    }
                    t => Err(format!("arr of {:?} is not supported by the JIT yet", t)),
                }
            }
            _ => {
                let info = self
                    .fns
                    .get(name)
                    .ok_or(format!("unknown function `{}`", name))?;
                let params = info.params.clone();
                let mut args = Vec::new();
                for ((t, vals), want) in atys.into_iter().zip(avals).zip(params) {
                    if want == CType::F64 && t == CType::I64 {
                        args.push(self.b.ins().fcvt_from_sint(types::F64, vals[0]));
                    } else {
                        args.extend(vals);
                    }
                }
                self.gen_user_call(name, args)
            }
        }
    }
}

enum StmtOut {
    Value(Option<(CType, Vec<Value>)>),
    Terminated,
}
