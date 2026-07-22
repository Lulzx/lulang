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
        CType::I64 | CType::Bool | CType::Enum(_) => vec![types::I64],
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
    opt_isa: cranelift_codegen::isa::OwnedTargetIsa,
    soa: bool,
    simd: bool,
    ifconv: bool,
    do_licm: bool,
    inline_math: bool,
    fns: HashMap<String, FnInfo>,
    imports: HashMap<&'static str, FuncId>,
    pure_imports: std::collections::HashSet<u32>,
}

impl<'a> Jit<'a> {
    pub fn run(p: &'a Program) -> Result<(), String> {
        use cranelift_codegen::settings::Configurable as _;
        // The module ISA stays at opt_level=none: we run the egraph optimizer
        // manually per-function and then our own LICM pass on its output —
        // letting define_function re-run the egraph would re-elaborate
        // instruction placement and sink hoisted code back into loops.
        let isa = cranelift_native::builder()
            .map_err(|e| e.to_string())?
            .finish(cranelift_codegen::settings::Flags::new(
                cranelift_codegen::settings::builder(),
            ))
            .map_err(|e| e.to_string())?;
        let mut opt_flags = cranelift_codegen::settings::builder();
        opt_flags.set("opt_level", "speed").map_err(|e| e.to_string())?;
        let opt_isa = cranelift_native::builder()
            .map_err(|e| e.to_string())?
            .finish(cranelift_codegen::settings::Flags::new(opt_flags))
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
            ("lu_arr_new_raw", runtime::lu_arr_new_raw as *const u8),
            ("lu_str_eq", runtime::lu_str_eq as *const u8),
            ("lu_oob", runtime::lu_oob as *const u8),
            ("lu_sin", runtime::lu_sin as *const u8),
            ("lu_cos", runtime::lu_cos as *const u8),
            ("lu_acos", runtime::lu_acos as *const u8),
            ("lu_atan2", runtime::lu_atan2 as *const u8),
            ("lu_pow", runtime::lu_pow as *const u8),
            ("lu_fmod", runtime::lu_fmod as *const u8),
            ("lu_nargs", runtime::lu_nargs as *const u8),
            ("lu_arg", runtime::lu_arg as *const u8),
            ("lu_read_file", runtime::lu_read_file as *const u8),
            ("lu_write_file", runtime::lu_write_file as *const u8),
            ("lu_last_len", runtime::lu_last_len as *const u8),
            ("lu_chr", runtime::lu_chr as *const u8),
            ("lu_concat", runtime::lu_concat as *const u8),
        ];
        for (n, ptr) in syms {
            jb.symbol(*n, *ptr);
        }
        let module = JITModule::new(jb);
        let soa = std::env::var("LU_LAYOUT").map(|v| v != "aos").unwrap_or(true);
        let simd = std::env::var("LU_SIMD").map(|v| v != "off").unwrap_or(true);
        let ifconv = std::env::var("LU_IFCONV").map(|v| v != "off").unwrap_or(true);
        let do_licm = std::env::var("LU_LICM").map(|v| v != "off").unwrap_or(true);
        let inline_math = std::env::var("LU_MATH").map(|v| v != "call").unwrap_or(true);
        let mut jit = Jit {
            p,
            module,
            opt_isa,
            soa,
            simd,
            ifconv,
            do_licm,
            inline_math,
            fns: HashMap::new(),
            imports: HashMap::new(),
            pure_imports: std::collections::HashSet::new(),
        };
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
            ("lu_arr_new_raw", 1, &[types::I64], false),
            ("lu_str_eq", 1, &[types::I64, types::I64, types::I64, types::I64], false),
            ("lu_oob", 0, &[types::I64, types::I64], false),
            ("lu_sin", 2, &[types::F64], true),
            ("lu_cos", 2, &[types::F64], true),
            ("lu_acos", 2, &[types::F64], true),
            ("lu_atan2", 2, &[types::F64, types::F64], true),
            ("lu_pow", 2, &[types::F64, types::F64], true),
            ("lu_fmod", 2, &[types::F64, types::F64], true),
            ("lu_nargs", 1, &[], false),
            ("lu_arg", 1, &[types::I64], false),
            ("lu_read_file", 1, &[types::I64, types::I64], false),
            ("lu_write_file", 0, &[types::I64, types::I64, types::I64, types::I64], false),
            ("lu_last_len", 1, &[], false),
            ("lu_chr", 1, &[types::I64], false),
            ("lu_concat", 1, &[types::I64, types::I64, types::I64, types::I64], false),
        ];
        for (name, kind, params, pure) in specs {
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
            if *pure {
                self.pure_imports.insert(id.as_u32());
            }
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
            // inout params are copy-in/copy-out: the outlined-call ABI passes a
            // hidden out-pointer per inout param (final values may not fit in
            // return registers) — the callee stores the copy-out through it
            for &io in f.inouts.iter() {
                if io {
                    sig.params.push(AbiParam::new(types::I64));
                }
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
        for &io in f.inouts.iter() {
            if io {
                sig.params.push(AbiParam::new(types::I64));
            }
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
                inline_spent: 0,
                trusted_idx: Vec::new(),
                soa: self.soa,
                simd: self.simd,
                ifconv: self.ifconv,
                inline_math: self.inline_math,
                inout_outs: Vec::new(),
            };
            let mut cursor = 0;
            for ((name, _), t) in f.params.iter().zip(params.iter()) {
                let n = comps(g.p, t)?.len();
                let vals = entry_params[cursor..cursor + n].to_vec();
                cursor += n;
                g.bind(name, t.clone(), &vals)?;
            }
            for ((name, _), &io) in f.params.iter().zip(f.inouts.iter()) {
                if io {
                    let ptr = entry_params[cursor];
                    cursor += 1;
                    let (_, vars) = g.lookup(name).unwrap();
                    g.inout_outs.push((ptr, vars));
                }
            }
            let (terminated, last) = g.gen_block(&f.body)?;
            if !terminated {
                if ret == CType::Unit {
                    g.emit_inout_stores();
                    g.b.ins().return_(&[]);
                } else {
                    match last {
                        Some((t, vals)) if t == ret => {
                            g.emit_inout_stores();
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
        ctx.optimize(self.opt_isa.as_ref(), &mut Default::default())
            .map_err(|e| e.to_string())?;
        if self.do_licm {
            licm(&mut ctx.func, &self.pure_imports);
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
                inline_spent: 0,
                trusted_idx: Vec::new(),
                soa: self.soa,
                simd: self.simd,
                ifconv: self.ifconv,
                inline_math: self.inline_math,
                inout_outs: Vec::new(),
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
        ctx.optimize(self.opt_isa.as_ref(), &mut Default::default())
            .map_err(|e| e.to_string())?;
        if self.do_licm {
            licm(&mut ctx.func, &self.pure_imports);
        }
        if std::env::var("LU_DUMP").is_ok() {
            eprintln!("{}", ctx.func.display());
        }
        self.module.define_function(id, &mut ctx).map_err(|e| e.to_string())?;
        self.module.clear_context(&mut ctx);
        Ok(id)
    }
}

/// M5 middle-end pass: loop-invariant code motion at the CLIF level.
///
/// Cranelift's egraph optimizer GVNs and folds pure code but (as measured on
/// the slerp kernel) leaves invariant chains fed by rematerialized constants
/// inside loops. We own the pass instead: for every loop with a dedicated
/// preheader at a shallower loop depth, move pure non-trapping instructions
/// whose operands are all defined outside the loop to the preheader, to a
/// fixpoint. Speculation is safe by construction — hoisted instructions
/// cannot trap, load, store, or call, with one exception: calls to the
/// runtime's pure math imports (`pure_imports`, sin/cos/acos/atan2/pow/fmod —
/// total over f64, non-trapping, no observable state), which under
/// `LU_MATH=call` are what keep slerp's invariant `acos(d)`/`sin(th)` chain
/// pinned inside the loop.
fn licm(func: &mut cranelift_codegen::ir::Function, pure_imports: &std::collections::HashSet<u32>) {
    use cranelift_codegen::dominator_tree::DominatorTree;
    use cranelift_codegen::flowgraph::ControlFlowGraph;
    use cranelift_codegen::ir::Block;
    use cranelift_codegen::loop_analysis::LoopAnalysis;

    // A call is hoistable iff its target is one of the whitelisted pure
    // math imports (module namespace 0, FuncId index in `pure_imports`).
    let pure_call = |func: &cranelift_codegen::ir::Function,
                     inst: cranelift_codegen::ir::Inst| {
        use cranelift_codegen::ir::{ExternalName, InstructionData};
        let InstructionData::Call { func_ref, .. } = func.dfg.insts[inst] else {
            return false;
        };
        match func.dfg.ext_funcs[func_ref].name {
            ExternalName::User(r) => func
                .params
                .user_named_funcs()
                .get(r)
                .is_some_and(|n| n.namespace == 0 && pure_imports.contains(&n.index)),
            _ => false,
        }
    };

    let cfg = ControlFlowGraph::with_function(func);
    let domtree = DominatorTree::with_function(func, &cfg);
    let mut la = LoopAnalysis::new();
    la.compute(func, &cfg, &domtree);

    let loops: Vec<_> = la.loops().collect();
    for lp in loops {
        let header = la.loop_header(lp);
        // dedicated preheader: the unique predecessor outside the loop,
        // strictly shallower (never hoist into a sibling loop's body)
        let outside: Vec<Block> = cfg
            .pred_iter(header)
            .map(|p| p.block)
            .filter(|b| !la.is_in_loop(*b, lp))
            .collect();
        let [pre] = outside[..] else { continue };
        if la.loop_level(pre).level() >= la.loop_level(header).level() {
            continue;
        }
        let Some(term) = func.layout.last_inst(pre) else { continue };

        let in_loop_blocks: Vec<Block> = func
            .layout
            .blocks()
            .filter(|b| la.is_in_loop(*b, lp))
            .collect();
        loop {
            let mut changed = false;
            for &block in &in_loop_blocks {
                let mut next = func.layout.first_inst(block);
                while let Some(inst) = next {
                    next = func.layout.next_inst(inst);
                    let op = func.dfg.insts[inst].opcode();
                    if (op.is_branch()
                        || op.is_call()
                        || op.is_return()
                        || op.is_terminator()
                        || op.can_load()
                        || op.can_store()
                        || op.can_trap()
                        || op.other_side_effects())
                        && !(op.is_call() && pure_call(func, inst))
                    {
                        continue;
                    }
                    let args: Vec<_> = func.dfg.inst_args(inst).to_vec();
                    let invariant = args.into_iter().all(|v| {
                        let v = func.dfg.resolve_aliases(v);
                        let def_block = match func.dfg.value_def(v) {
                            cranelift_codegen::ir::ValueDef::Result(i, _) => {
                                func.layout.inst_block(i)
                            }
                            cranelift_codegen::ir::ValueDef::Param(b, _) => Some(b),
                            _ => None,
                        };
                        match def_block {
                            Some(b) => !la.is_in_loop(b, lp),
                            None => false,
                        }
                    });
                    if invariant {
                        func.layout.remove_inst(inst);
                        func.layout.insert_inst(inst, term);
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
    }
}

/// Statement count of a body including nested blocks — the unit of the
/// per-function inlining budget.
fn body_size(p: &Program, stmts: &[StmtId]) -> usize {
    let mut n = 0;
    for &sid in stmts {
        n += 1;
        match p.stmt(sid) {
            Stmt::If(_, t, e) => n += body_size(p, t) + body_size(p, e),
            Stmt::For(_, _, _, b) | Stmt::While(_, b) => n += body_size(p, b),
            _ => {}
        }
    }
    n
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
    // statements of inlined bodies emitted into the current function; the
    // budget stops exponential blowup on large mutually-calling functions
    inline_spent: usize,
    // (array ident, loop var, logical length) triples whose whole index range
    // was checked at loop entry — accesses through them skip the per-element
    // bounds check and reuse the hoisted length for SoA plane addressing.
    trusted_idx: Vec<(String, String, Value)>,
    // SoA record-array layout (the default; LU_LAYOUT=aos flips it)
    soa: bool,
    // `sum` vectorization (the default; LU_SIMD=off flips it)
    simd: bool,
    ifconv: bool,
    inline_math: bool,
    // (out-pointer, component variables) of the current fn's inout params —
    // stored through the pointer at every outlined return
    inout_outs: Vec<(Value, Vec<Variable>)>,
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
            Expr::Call(name, args) => {
                // a call with inout params may rebind a candidate array
                if p.find_fn(name).is_some_and(|f| f.has_inout()) {
                    *ok = false;
                }
                args.iter().for_each(|&a| walk_e(p, a, var, arrays, ok));
            }
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
                Stmt::While(c, body) => {
                    walk_e(p, *c, var, arrays, ok);
                    walk_s(p, body, var, arrays, ok);
                }
                Stmt::Return(Some(e)) | Stmt::Expr(e) => walk_e(p, *e, var, arrays, ok),
                Stmt::Return(None) => {}
            }
        }
    }

fn collect_assigned(p: &Program, stmts: &[StmtId], out: &mut Vec<String>) {
    for &sid in stmts {
        match p.stmt(sid) {
            Stmt::Assign(target, _) => {
                if let Expr::Ident(n) = p.expr(*target) {
                    if !out.contains(n) {
                        out.push(n.clone());
                    }
                }
            }
            Stmt::If(_, a, b) => {
                collect_assigned(p, a, out);
                collect_assigned(p, b, out);
            }
            _ => {}
        }
    }
}

impl<'a, 'b> Gen<'a, 'b> {
    /// Store the current fn's inout param values through their out-pointers
    /// (called right before every outlined return).
    fn emit_inout_stores(&mut self) {
        let outs = self.inout_outs.clone();
        for (ptr, vars) in outs {
            for (k, v) in vars.iter().enumerate() {
                let val = self.b.use_var(*v);
                self.b.ins().store(MemFlags::trusted(), val, ptr, (k * 8) as i32);
            }
        }
    }

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
                        let addrs = self.elem_addrs(avals[0], ivals[0], &elem, trusted)?;
                        for (v, a) in vals.iter().zip(addrs.iter()) {
                            self.b.ins().store(MemFlags::trusted(), *v, *a, 0);
                        }
                    }
                    Expr::Field(_, _) => {
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
                        let (mut t, vars) = self
                            .lookup(&root)
                            .ok_or(format!("unknown variable `{}`", root))?;
                        let mut off = 0;
                        for f in &path {
                            match t {
                                CType::Rec(ti) => {
                                    let (o, ft) = field_offset(self.p, ti, f)?;
                                    off += o;
                                    t = ft;
                                }
                                t => return Err(format!("cannot assign field on {:?}", t)),
                            }
                        }
                        for (k, v) in vals.iter().enumerate() {
                            self.b.def_var(vars[off + k], *v);
                        }
                    }
                    _ => return Err("invalid assignment target".into()),
                }
                Ok(StmtOut::Value(None))
            }
            Stmt::If(c, then_s, else_s) => {
                if self.ifconv {
                    if let Some(assigned) = self.ifconv_candidate(then_s, else_s) {
                        return self.gen_if_select(*c, then_s, else_s, &assigned);
                    }
                }
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
            Stmt::While(c, body) => {
                let header = self.b.create_block();
                let body_b = self.b.create_block();
                let exit = self.b.create_block();
                self.b.ins().jump(header, &[]);
                self.b.switch_to_block(header);
                let (_, cv) = self.gen_expr(*c)?;
                self.b.ins().brif(cv[0], body_b, &[], exit, &[]);
                self.b.switch_to_block(body_b);
                let (term, _) = self.gen_block(body)?;
                if !term {
                    self.b.ins().jump(header, &[]);
                }
                self.b.switch_to_block(exit);
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
                    self.emit_inout_stores();
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

    /// Per-component addresses of element `idx`. Scalar arrays and AoS
    /// records: components contiguous at base+8+(idx*stride+c)*8. SoA records
    /// (the default — compiler-owned layout): component c lives in its own
    /// plane of `n` slots at base+8+(c*n+idx)*8. `trusted` carries the
    /// loop-hoisted logical length when the bounds check was already done.
    fn elem_addrs(&mut self, base: Value, idx: Value, elem: &CType, trusted: Option<Value>) -> Result<Vec<Value>, String> {
        let stride = comps(self.p, elem)?.len() as i64;
        let logical = match trusted {
            Some(n) => n,
            None => {
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
                logical
            }
        };
        let base8 = self.b.ins().iadd_imm(base, 8);
        if stride > 1 && self.soa {
            let off = self.b.ins().imul_imm(idx, 8);
            let lane0 = self.b.ins().iadd(base8, off);
            let mut out = Vec::new();
            for c in 0..stride {
                let plane = self.b.ins().imul_imm(logical, 8 * c);
                out.push(self.b.ins().iadd(lane0, plane));
            }
            Ok(out)
        } else {
            let off = self.b.ins().imul_imm(idx, stride * 8);
            let a0 = self.b.ins().iadd(base8, off);
            Ok((0..stride).map(|c| self.b.ins().iadd_imm(a0, c * 8)).collect())
        }
    }

    /// Emit `idx u< len` check, aborting via lu_oob on failure.
    fn check_idx(&mut self, idx: Value, len: Value) {
        let bad = self.b.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, idx, len);
        let oob = self.b.create_block();
        let ok = self.b.create_block();
        self.b.ins().brif(bad, oob, &[], ok, &[]);
        self.b.switch_to_block(oob);
        let r = self.callee("lu_oob");
        self.b.ins().call(r, &[idx, len]);
        self.b.ins().jump(ok, &[]);
        self.b.switch_to_block(ok);
    }

    fn is_trusted(&self, a: ExprId, i: ExprId) -> Option<Value> {
        if let (Expr::Ident(an), Expr::Ident(inm)) = (self.p.expr(a), self.p.expr(i)) {
            self.trusted_idx
                .iter()
                .find(|(x, y, _)| x == an && y == inm)
                .map(|(_, _, n)| *n)
        } else {
            None
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
            self.trusted_idx.push((name.clone(), var.to_string(), logical));
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
                self.gen_user_call(&fname, vals, None)
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
                if at == CType::Str {
                    self.check_idx(ivals[0], avals[1]);
                    let addr = self.b.ins().iadd(avals[0], ivals[0]);
                    let byte = self.b.ins().uload8(types::I64, MemFlags::trusted(), addr, 0);
                    return Ok((CType::I64, vec![byte]));
                }
                let elem = match at {
                    CType::Arr(e) => *e,
                    _ => return Err("cannot index non-array".into()),
                };
                let addrs = self.elem_addrs(avals[0], ivals[0], &elem, trusted)?;
                let mut out = Vec::new();
                for (c, a) in comps(self.p, &elem)?.into_iter().zip(addrs) {
                    out.push(self.b.ins().load(c, MemFlags::trusted(), a, 0));
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
            Expr::EnumVal(en, vn) => {
                let (ei, tag) = self
                    .p
                    .enum_tag(en, vn)
                    .ok_or(format!("unknown enum value `{}.{}`", en, vn))?;
                let c = self.b.ins().iconst(types::I64, tag);
                Ok((CType::Enum(ei), vec![c]))
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
                self.gen_call(name, atys, avals, args)
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
        // `and`/`or` are short-circuit by language semantics: the right side
        // must not evaluate (it may index past a bound the left side guards)
        if op == "and" || op == "or" {
            let (_, lv) = self.gen_expr(l)?;
            let res = self.b.declare_var(types::I64);
            self.b.def_var(res, lv[0]);
            let rhs_b = self.b.create_block();
            let merge = self.b.create_block();
            if op == "and" {
                self.b.ins().brif(lv[0], rhs_b, &[], merge, &[]);
            } else {
                self.b.ins().brif(lv[0], merge, &[], rhs_b, &[]);
            }
            self.b.switch_to_block(rhs_b);
            let (_, rv) = self.gen_expr(r)?;
            self.b.def_var(res, rv[0]);
            self.b.ins().jump(merge, &[]);
            self.b.switch_to_block(merge);
            let out = self.b.use_var(res);
            return Ok((CType::Bool, vec![out]));
        }
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
            return self.gen_user_call(&fname, args, None);
        }
        let (lt, lv) = self.gen_expr(l)?;
        let (rt, rv) = self.gen_expr(r)?;
        match op.as_str() {
            // `and`/`or` never reach here — handled short-circuit above
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
                        _ => self.call_import("lu_fmod", &[a, b])[0],
                    };
                    Ok((CType::F64, vec![v]))
                }
            }
            "==" | "!=" if lt == CType::Str && rt == CType::Str => {
                let eq =
                    self.call_import("lu_str_eq", &[lv[0], lv[1], rv[0], rv[1]])[0];
                let v = if op == "!=" { self.b.ins().bxor_imm(eq, 1) } else { eq };
                Ok((CType::Bool, vec![v]))
            }
            "==" | "!=" | "<" | "<=" | ">" | ">=" => {
                let both_int = matches!(lt, CType::I64 | CType::Bool | CType::Enum(_))
                    && matches!(rt, CType::I64 | CType::Bool | CType::Enum(_));
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
        // M5: vectorize the reduction when the body is pure f64 arithmetic
        // over check-free unit-stride loads — f64x2 lanes, two vector
        // accumulators (4 lanes total, same reassociation contract).
        if self.simd && self.vec_ok(body, &var) {
            let out = self.gen_sum_simd(&var, lov[0], hiv[0], body);
            self.trusted_idx.truncate(self.trusted_idx.len() - pushed);
            return out;
        }
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

    // ---- if-conversion (M5) ----
    //
    // A CFG diamond turns every variable assigned inside it into a merge
    // block-param, which hides loop-invariance from the egraph optimizer (in
    // slerp, `d = -d` inside an `if` is what keeps `acos(d)` stuck in the
    // loop). When both arms are speculation-safe — pure and non-trapping, so
    // executing the untaken arm is unobservable — emit both arms straight-line
    // and merge each assigned variable with a select. Downstream values stay
    // pure SSA and Cranelift's LICM/GVN see through them.

    /// Type of `e` if it is safe to speculate (pure, cannot trap or perform
    /// I/O), else None. Rejects: array indexing (bounds check can abort),
    /// integer `/`/`%` (trap on zero), `arr`/`print`, `sum`, loops.
    fn spec_expr(&self, e: ExprId, locals: &mut Vec<HashMap<String, CType>>, depth: usize) -> Option<CType> {
        match self.p.expr(e) {
            Expr::Int(_) => Some(CType::I64),
            Expr::Float(_) => Some(CType::F64),
            Expr::Bool(_) => Some(CType::Bool),
            Expr::Str(_) => Some(CType::Str),
            Expr::Ident(n) => locals
                .iter()
                .rev()
                .find_map(|s| s.get(n).cloned())
                .or_else(|| self.lookup(n).map(|(t, _)| t)),
            Expr::Un(_, x) => self.spec_expr(*x, locals, depth),
            Expr::EnumVal(en, vn) => self.p.enum_tag(en, vn).map(|(ei, _)| CType::Enum(ei)),
            Expr::Bin(op, l, r) => {
                let a = self.spec_expr(*l, locals, depth)?;
                let b = self.spec_expr(*r, locals, depth)?;
                if let Some(f) = self.p.infix_ops.get(op) {
                    return self.spec_fn(f, depth);
                }
                match op.as_str() {
                    "+" | "-" | "*" => {
                        Some(if a == CType::F64 || b == CType::F64 { CType::F64 } else { CType::I64 })
                    }
                    "/" | "%" => {
                        if a == CType::F64 || b == CType::F64 {
                            Some(CType::F64)
                        } else {
                            None // integer division traps on zero
                        }
                    }
                    "and" | "or" | "==" | "!=" | "<" | "<=" | ">" | ">=" | "~=" | "\u{2248}" => {
                        Some(CType::Bool)
                    }
                    _ => None,
                }
            }
            Expr::Circum(open, x) => {
                self.spec_expr(*x, locals, depth)?;
                let fname = self.p.circum_ops[open].1.clone();
                self.spec_fn(&fname, depth)
            }
            Expr::Field(x, fname) => match self.spec_expr(*x, locals, depth)? {
                CType::Rec(ti) => field_offset(self.p, ti, fname).ok().map(|(_, t)| t),
                _ => None,
            },
            Expr::Record(name, inits) => {
                for (_, e) in inits {
                    self.spec_expr(*e, locals, depth)?;
                }
                let ti = self.p.types.iter().position(|t| t.name == *name)?;
                Some(CType::Rec(ti))
            }
            Expr::Call(name, args) => {
                for &a in args {
                    self.spec_expr(a, locals, depth)?;
                }
                match name.as_str() {
                    "sqrt" | "abs" | "floor" | "sin" | "cos" | "acos" | "min" | "max" | "pow"
                    | "atan2" | "float" => Some(CType::F64),
                    "int" | "len" | "nargs" => Some(CType::I64),
                    "arg" | "read_file" | "chr" | "concat" => Some(CType::Str),
                    _ => self.spec_fn(name, depth),
                }
            }
            _ => None,
        }
    }

    fn spec_fn(&self, fname: &str, depth: usize) -> Option<CType> {
        if depth == 0 {
            return None;
        }
        let decl = self.p.fns.iter().find(|f| f.name == fname)?;
        if decl.has_inout() {
            return None; // mutates caller variables — not speculatable
        }
        let info = self.fns.get(fname)?;
        let mut scope = HashMap::new();
        for ((pname, _), t) in decl.params.iter().zip(info.params.iter()) {
            scope.insert(pname.clone(), t.clone());
        }
        let mut locals = vec![scope];
        if self.spec_stmts(&decl.body, &mut locals, depth - 1, true) {
            Some(info.ret.clone())
        } else {
            None
        }
    }

    fn spec_stmts(
        &self,
        stmts: &[StmtId],
        locals: &mut Vec<HashMap<String, CType>>,
        depth: usize,
        allow_return: bool,
    ) -> bool {
        locals.push(HashMap::new());
        let ok = stmts.iter().all(|&sid| match self.p.stmt(sid) {
            Stmt::Let(n, e) | Stmt::Var(n, e) => match self.spec_expr(*e, locals, depth) {
                Some(t) => {
                    locals.last_mut().unwrap().insert(n.clone(), t);
                    true
                }
                None => false,
            },
            Stmt::Assign(target, e) => {
                matches!(self.p.expr(*target), Expr::Ident(_))
                    && self.spec_expr(*e, locals, depth).is_some()
            }
            Stmt::If(c, a, b) => {
                self.spec_expr(*c, locals, depth).is_some()
                    && self.spec_stmts(a, locals, depth, allow_return)
                    && self.spec_stmts(b, locals, depth, allow_return)
            }
            Stmt::Return(Some(e)) => allow_return && self.spec_expr(*e, locals, depth).is_some(),
            Stmt::Return(None) => allow_return,
            Stmt::Expr(e) => self.spec_expr(*e, locals, depth).is_some(),
            Stmt::For(..) | Stmt::While(..) => false,
        });
        locals.pop();
        ok
    }

    /// If both arms are speculation-safe, return the outer variables they
    /// assign (the ones to merge with selects).
    fn ifconv_candidate(&self, then_s: &[StmtId], else_s: &[StmtId]) -> Option<Vec<String>> {
        let mut locals = Vec::new();
        if !self.spec_stmts(then_s, &mut locals, 8, false)
            || !self.spec_stmts(else_s, &mut locals, 8, false)
        {
            return None;
        }
        let mut assigned = Vec::new();
        collect_assigned(self.p, then_s, &mut assigned);
        collect_assigned(self.p, else_s, &mut assigned);
        // only pre-existing variables need merging (arm-local lets die with scope)
        assigned.retain(|n| self.lookup(n).is_some());
        Some(assigned)
    }

    fn gen_if_select(
        &mut self,
        c: ExprId,
        then_s: &[StmtId],
        else_s: &[StmtId],
        assigned: &[String],
    ) -> Result<StmtOut, String> {
        let (_, cv) = self.gen_expr(c)?;
        let pre: Vec<(Vec<Variable>, Vec<Value>)> = assigned
            .iter()
            .map(|n| {
                let (_, vars) = self.lookup(n).unwrap();
                let vals = vars.iter().map(|&v| self.b.use_var(v)).collect();
                (vars, vals)
            })
            .collect();
        self.gen_block(then_s)?; // spec-checked: cannot terminate
        let then_vals: Vec<Vec<Value>> = pre
            .iter()
            .map(|(vars, _)| vars.iter().map(|&v| self.b.use_var(v)).collect())
            .collect();
        for (vars, pv) in &pre {
            for (var, val) in vars.iter().zip(pv.iter()) {
                self.b.def_var(*var, *val);
            }
        }
        self.gen_block(else_s)?;
        for ((vars, _), tv) in pre.iter().zip(then_vals.iter()) {
            for (i, var) in vars.iter().enumerate() {
                let ev = self.b.use_var(*var);
                let sel = self.b.ins().select(cv[0], tv[i], ev);
                self.b.def_var(*var, sel);
            }
        }
        Ok(StmtOut::Value(None))
    }

    // ---- inline math kernels (M5) ----
    //
    // sin/cos/acos are emitted as branch-free pure IR: Cody-Waite range
    // reduction + musl minimax polynomials, selects instead of branches. A
    // libcall is a black box to Cranelift; an inline kernel is ordinary pure
    // arithmetic, so the egraph optimizer GVNs it and hoists loop-invariant
    // chains out of loops (the slerp `acos(d)`/`sin(th)` win). Accuracy is a
    // few ulp — far inside the approximate-FP contract (rtol 2^-40).

    /// Horner evaluation coefs[0] + z*(coefs[1] + z*(...)) via fma.
    fn poly(&mut self, z: Value, coefs: &[f64]) -> Value {
        let mut acc = self.b.ins().f64const(*coefs.last().unwrap());
        for &c in coefs.iter().rev().skip(1) {
            let cv = self.b.ins().f64const(c);
            acc = self.b.ins().fma(acc, z, cv);
        }
        acc
    }

    fn emit_trig(&mut self, x: Value, is_cos: bool) -> Value {
        const INVPIO2: f64 = 6.36619772367581382433e-01;
        const PIO2_1: f64 = 1.57079632673412561417e+00;
        const PIO2_1T: f64 = 6.07710050650619224932e-11;
        const S: [f64; 6] = [
            -1.66666666666666324348e-01,
            8.33333333332248946124e-03,
            -1.98412698298579493134e-04,
            2.75573137070700676789e-06,
            -2.50507602534068634195e-08,
            1.58969099521155010221e-10,
        ];
        const C: [f64; 6] = [
            4.16666666666666019037e-02,
            -1.38888888888741095749e-03,
            2.48015872894767294178e-05,
            -2.75573143513906633035e-07,
            2.08757232129817482790e-09,
            -1.13596475577881948265e-11,
        ];
        let inv = self.b.ins().f64const(INVPIO2);
        let scaled = self.b.ins().fmul(x, inv);
        let nf = self.b.ins().nearest(scaled);
        let p1 = self.b.ins().f64const(-PIO2_1);
        let r0 = self.b.ins().fma(nf, p1, x);
        let p1t = self.b.ins().f64const(-PIO2_1T);
        let r = self.b.ins().fma(nf, p1t, r0);
        let q = self.b.ins().fcvt_to_sint_sat(types::I64, nf);
        let z = self.b.ins().fmul(r, r);
        // sinp = r + r*z*S(z)
        let sp = self.poly(z, &S);
        let rz = self.b.ins().fmul(r, z);
        let sinp = self.b.ins().fma(rz, sp, r);
        // cosp = 1 - z/2 + z*z*C(z)
        let cp = self.poly(z, &C);
        let zz = self.b.ins().fmul(z, z);
        let mhalf = self.b.ins().f64const(-0.5);
        let one = self.b.ins().f64const(1.0);
        let base = self.b.ins().fma(z, mhalf, one);
        let cosp = self.b.ins().fma(zz, cp, base);
        // quadrant: sin picks q0→sinp 1→cosp 2→-sinp 3→-cosp;
        //           cos picks q0→cosp 1→-sinp 2→-cosp 3→sinp
        let bit0 = self.b.ins().band_imm(q, 1);
        let use_alt = self.b.ins().icmp_imm(IntCC::NotEqual, bit0, 0);
        let val = if is_cos {
            self.b.ins().select(use_alt, sinp, cosp)
        } else {
            self.b.ins().select(use_alt, cosp, sinp)
        };
        let qn = if is_cos { self.b.ins().iadd_imm(q, 1) } else { q };
        let bit1 = self.b.ins().band_imm(qn, 2);
        let negate = self.b.ins().icmp_imm(IntCC::NotEqual, bit1, 0);
        let nval = self.b.ins().fneg(val);
        self.b.ins().select(negate, nval, val)
    }

    fn emit_acos(&mut self, x: Value) -> Value {
        const PS: [f64; 6] = [
            1.66666666666666657415e-01,
            -3.25565818622400915405e-01,
            2.01212532134862925881e-01,
            -4.00555345006794114027e-02,
            7.91534994289814532176e-04,
            3.47933107596021167570e-05,
        ];
        const QS: [f64; 5] = [
            1.0,
            -2.40339491173441421878e+00,
            2.02094576023350569471e+00,
            -6.88283971605453293030e-01,
            7.70381505559019352791e-02,
        ];
        let a = self.b.ins().fabs(x);
        let half = self.b.ins().f64const(0.5);
        let small = self.b.ins().fcmp(FloatCC::LessThan, a, half);
        // |x| < 0.5:  acos = pi/2 - (x + x*R(x^2))
        // |x| >= 0.5: z=(1-|x|)/2, s=sqrt(z), t=2*(s + s*R(z));
        //             x>0 → t, x<0 → pi - t
        let xx = self.b.ins().fmul(x, x);
        let one = self.b.ins().f64const(1.0);
        let om = self.b.ins().fsub(one, a);
        let zbig = self.b.ins().fmul(om, half);
        let z = self.b.ins().select(small, xx, zbig);
        let pnum = self.poly(z, &PS);
        let num = self.b.ins().fmul(z, pnum);
        let den = self.poly(z, &QS);
        let r = self.b.ins().fdiv(num, den);
        let s = self.b.ins().sqrt(z);
        let xr = self.b.ins().fma(x, r, x);
        let pio2 = self.b.ins().f64const(std::f64::consts::FRAC_PI_2);
        let res_small = self.b.ins().fsub(pio2, xr);
        let sr = self.b.ins().fma(s, r, s);
        let two = self.b.ins().f64const(2.0);
        let big = self.b.ins().fmul(two, sr);
        let pi = self.b.ins().f64const(std::f64::consts::PI);
        let res_neg = self.b.ins().fsub(pi, big);
        let zero = self.b.ins().f64const(0.0);
        let isneg = self.b.ins().fcmp(FloatCC::LessThan, x, zero);
        let res_big = self.b.ins().select(isneg, res_neg, big);
        self.b.ins().select(small, res_small, res_big)
    }

    // ---- `sum` vectorization (M5) ----

    /// Can `e` be evaluated as an f64x2 vector over consecutive values of
    /// `var`? Requires every leaf to be a Float literal, an invariant f64
    /// scalar, or a trusted unit-stride `a[var]` load.
    fn vec_ok(&self, e: ExprId, var: &str) -> bool {
        match self.p.expr(e) {
            Expr::Float(_) => true,
            Expr::Ident(n) => {
                n != var && matches!(self.lookup(n), Some((CType::F64, _)))
            }
            Expr::Index(a, i) => match (self.p.expr(*a), self.p.expr(*i)) {
                (Expr::Ident(an), Expr::Ident(inm)) => {
                    inm == var
                        && self.trusted_idx.iter().any(|(x, y, _)| x == an && y == var)
                        && matches!(self.lookup(an), Some((CType::Arr(e), _)) if *e == CType::F64)
                }
                _ => false,
            },
            Expr::Bin(op, l, r) => {
                matches!(op.as_str(), "+" | "-" | "*" | "/")
                    && !self.p.infix_ops.contains_key(op)
                    && self.vec_ok(*l, var)
                    && self.vec_ok(*r, var)
            }
            Expr::Un(op, x) => op == "-" && self.vec_ok(*x, var),
            Expr::Call(name, args) => {
                matches!(name.as_str(), "sqrt" | "abs" | "min" | "max")
                    && args.iter().all(|&a| self.vec_ok(a, var))
            }
            // SoA makes every record field a unit-stride plane, so
            // qs[i].field is a plain vector load — layout and SIMD compose.
            Expr::Field(x, fname) => {
                if !self.soa {
                    return false;
                }
                let Expr::Index(a, i) = self.p.expr(*x) else { return false };
                let (Expr::Ident(an), Expr::Ident(inm)) = (self.p.expr(*a), self.p.expr(*i)) else {
                    return false;
                };
                inm == var
                    && self.trusted_idx.iter().any(|(x2, y, _)| x2 == an && y == var)
                    && match self.lookup(an) {
                        Some((CType::Arr(e), _)) => match *e {
                            CType::Rec(ti) => {
                                matches!(field_offset(self.p, ti, fname), Ok((_, CType::F64)))
                            }
                            _ => false,
                        },
                        _ => false,
                    }
            }
            _ => false,
        }
    }

    /// Evaluate `e` as an f64x2 holding lanes for indices `idx` and `idx+1`.
    fn gen_vec_expr(&mut self, e: ExprId, var: &str, idx: Value) -> Result<Value, String> {
        match self.p.expr(e) {
            Expr::Float(v) => {
                let c = self.b.ins().f64const(*v);
                Ok(self.b.ins().splat(types::F64X2, c))
            }
            Expr::Ident(n) => {
                let (_, vars) = self.lookup(n).ok_or(format!("unknown variable `{}`", n))?;
                let v = self.b.use_var(vars[0]);
                Ok(self.b.ins().splat(types::F64X2, v))
            }
            Expr::Index(a, _) => {
                let (_, avals) = self.gen_expr(*a)?;
                let sbytes = self.b.ins().iconst(types::I64, 8);
                let off = self.b.ins().imul(idx, sbytes);
                let base8 = self.b.ins().iadd_imm(avals[0], 8);
                let addr = self.b.ins().iadd(base8, off);
                Ok(self.b.ins().load(types::F64X2, MemFlags::trusted(), addr, 0))
            }
            Expr::Bin(op, l, r) => {
                let a = self.gen_vec_expr(*l, var, idx)?;
                let b = self.gen_vec_expr(*r, var, idx)?;
                Ok(match op.as_str() {
                    "+" => self.b.ins().fadd(a, b),
                    "-" => self.b.ins().fsub(a, b),
                    "*" => self.b.ins().fmul(a, b),
                    _ => self.b.ins().fdiv(a, b),
                })
            }
            Expr::Un(_, x) => {
                let v = self.gen_vec_expr(*x, var, idx)?;
                Ok(self.b.ins().fneg(v))
            }
            Expr::Call(name, args) => {
                let vs: Result<Vec<Value>, String> = args
                    .iter()
                    .map(|&a| self.gen_vec_expr(a, var, idx))
                    .collect();
                let vs = vs?;
                Ok(match name.as_str() {
                    "sqrt" => self.b.ins().sqrt(vs[0]),
                    "abs" => self.b.ins().fabs(vs[0]),
                    "min" => self.b.ins().fmin(vs[0], vs[1]),
                    _ => self.b.ins().fmax(vs[0], vs[1]),
                })
            }
            Expr::Field(x, fname) => {
                let Expr::Index(a, _) = self.p.expr(*x) else {
                    return Err("non-vectorizable field".into());
                };
                let an = match self.p.expr(*a) {
                    Expr::Ident(n) => n.clone(),
                    _ => return Err("non-vectorizable field base".into()),
                };
                let n = self
                    .trusted_idx
                    .iter()
                    .find(|(x2, y, _)| *x2 == an && y == var)
                    .map(|(_, _, n)| *n)
                    .ok_or("untrusted array in vector body")?;
                let (at, avals) = self.gen_expr(*a)?;
                let ti = match at {
                    CType::Arr(e) => match *e {
                        CType::Rec(ti) => ti,
                        _ => return Err("field of non-record".into()),
                    },
                    _ => return Err("index of non-array".into()),
                };
                let (c, _) = field_offset(self.p, ti, fname)?;
                let base8 = self.b.ins().iadd_imm(avals[0], 8);
                let plane = self.b.ins().imul_imm(n, 8 * c as i64);
                let lane = self.b.ins().imul_imm(idx, 8);
                let pb = self.b.ins().iadd(base8, plane);
                let addr = self.b.ins().iadd(pb, lane);
                Ok(self.b.ins().load(types::F64X2, MemFlags::trusted(), addr, 0))
            }
            _ => Err("non-vectorizable expression".into()),
        }
    }

    fn gen_sum_simd(
        &mut self,
        var: &str,
        lo: Value,
        hi: Value,
        body: ExprId,
    ) -> Result<(CType, Vec<Value>), String> {
        let ivar = self.b.declare_var(types::I64);
        self.b.def_var(ivar, lo);
        self.env.push(HashMap::new());
        self.env
            .last_mut()
            .unwrap()
            .insert(var.to_string(), (CType::I64, vec![ivar]));

        let accs: Vec<Variable> = (0..4).map(|_| self.b.declare_var(types::F64X2)).collect();
        let sacc = self.b.declare_var(types::F64);
        let zs = self.b.ins().f64const(0.0);
        let zv = self.b.ins().splat(types::F64X2, zs);
        for &a in &accs {
            self.b.def_var(a, zv);
        }
        self.b.def_var(sacc, zs);

        let head4 = self.b.create_block();
        let body4 = self.b.create_block();
        let head1 = self.b.create_block();
        let body1 = self.b.create_block();
        let exit = self.b.create_block();

        self.b.ins().jump(head4, &[]);
        self.b.switch_to_block(head4);
        let iv = self.b.use_var(ivar);
        let i7 = self.b.ins().iadd_imm(iv, 8);
        let fits = self.b.ins().icmp(IntCC::SignedLessThanOrEqual, i7, hi);
        self.b.ins().brif(fits, body4, &[], head1, &[]);

        self.b.switch_to_block(body4);
        let ib = self.b.use_var(ivar);
        for (k, &acc) in accs.iter().enumerate() {
            let ik = self.b.ins().iadd_imm(ib, 2 * k as i64);
            let v = self.gen_vec_expr(body, var, ik)?;
            let a = self.b.use_var(acc);
            let an = self.b.ins().fadd(a, v);
            self.b.def_var(acc, an);
        }
        let ivn = self.b.ins().iadd_imm(ib, 8);
        self.b.def_var(ivar, ivn);
        self.b.ins().jump(head4, &[]);

        self.b.switch_to_block(head1);
        let iv1 = self.b.use_var(ivar);
        let more = self.b.ins().icmp(IntCC::SignedLessThan, iv1, hi);
        self.b.ins().brif(more, body1, &[], exit, &[]);

        self.b.switch_to_block(body1);
        let (t, vals) = self.gen_expr(body)?;
        let f = self.f64_of(&t, vals[0]);
        let cur = self.b.use_var(sacc);
        let nxt = self.b.ins().fadd(cur, f);
        self.b.def_var(sacc, nxt);
        let ivt = self.b.use_var(ivar);
        let ivt2 = self.b.ins().iadd_imm(ivt, 1);
        self.b.def_var(ivar, ivt2);
        self.b.ins().jump(head1, &[]);

        self.b.switch_to_block(exit);
        let va0 = self.b.use_var(accs[0]);
        let va1 = self.b.use_var(accs[1]);
        let va2 = self.b.use_var(accs[2]);
        let va3 = self.b.use_var(accs[3]);
        let s01 = self.b.ins().fadd(va0, va1);
        let s23 = self.b.ins().fadd(va2, va3);
        let vs = self.b.ins().fadd(s01, s23);
        let l0 = self.b.ins().extractlane(vs, 0);
        let l1 = self.b.ins().extractlane(vs, 1);
        let lv = self.b.ins().fadd(l0, l1);
        let sv = self.b.use_var(sacc);
        let total = self.b.ins().fadd(lv, sv);
        self.env.pop();
        Ok((CType::F64, vec![total]))
    }

    fn gen_user_call(
        &mut self,
        fname: &str,
        args: Vec<Value>,
        arg_exprs: Option<&[ExprId]>,
    ) -> Result<(CType, Vec<Value>), String> {
        let ret = self.fns[fname].ret.clone();
        let decl = self
            .p
            .fns
            .iter()
            .find(|f| f.name == fname)
            .expect("declared fn must exist");
        // Inline every user function up to a depth cap (recursion falls back to a
        // real call). Args are SSA values bound to fresh variables — evaluated
        // exactly once, so this is always semantics-preserving. A per-function
        // size budget keeps large mutually-calling functions (e.g. a
        // tree-walking evaluator) from exploding exponentially.
        let recursive = self.inline_stack.iter().any(|n| n == fname);
        let size = body_size(self.p, &decl.body);
        if !recursive && self.inline_stack.len() < 8 && self.inline_spent + size <= 3000 {
            self.inline_spent += size;
            let params = self.fns[fname].params.clone();
            let saved_env = std::mem::replace(&mut self.env, vec![HashMap::new()]);
            let saved_trust = std::mem::take(&mut self.trusted_idx);
            let mut cursor = 0;
            let mut param_vars: Vec<Vec<Variable>> = Vec::new();
            for ((pname, _), t) in decl.params.iter().zip(params.iter()) {
                let n = comps(self.p, t)?.len();
                let vals = args[cursor..cursor + n].to_vec();
                cursor += n;
                self.bind(pname, t.clone(), &vals)?;
                param_vars.push(self.lookup(pname).unwrap().1);
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
            // inout write-back: the param variables hold the final values here
            let outs: Vec<(usize, Vec<Value>)> = decl
                .inouts
                .iter()
                .enumerate()
                .filter(|(_, &io)| io)
                .map(|(i, _)| {
                    let vals = param_vars[i].iter().map(|&v| self.b.use_var(v)).collect();
                    (i, vals)
                })
                .collect();
            self.env = saved_env;
            self.trusted_idx = saved_trust;
            for (i, vals) in outs {
                self.write_back_inout(arg_exprs, i, &vals)?;
            }
            let out: Vec<Value> = result.iter().map(|&v| self.b.use_var(v)).collect();
            return Ok((ret, out));
        }
        use cranelift_codegen::ir::{StackSlotData, StackSlotKind};
        let params = self.fns[fname].params.clone();
        let mut args = args;
        let mut slots = Vec::new();
        for (i, (&io, t)) in decl.inouts.iter().zip(params.iter()).enumerate() {
            if io {
                let cs = comps(self.p, t)?;
                let ss = self.b.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    (cs.len() * 8) as u32,
                    3,
                ));
                let addr = self.b.ins().stack_addr(types::I64, ss, 0);
                args.push(addr);
                slots.push((i, ss, cs));
            }
        }
        let r = self.callee(fname);
        let call = self.b.ins().call(r, &args);
        let out = self.b.inst_results(call).to_vec();
        for (i, ss, cs) in slots {
            let vals: Vec<Value> = cs
                .iter()
                .enumerate()
                .map(|(k, &c)| self.b.ins().stack_load(c, ss, (k * 8) as i32))
                .collect();
            self.write_back_inout(arg_exprs, i, &vals)?;
        }
        Ok((ret, out))
    }

    fn write_back_inout(
        &mut self,
        arg_exprs: Option<&[ExprId]>,
        i: usize,
        vals: &[Value],
    ) -> Result<(), String> {
        let arg_exprs = arg_exprs.ok_or("inout functions cannot be used as operators")?;
        let Expr::Ident(n) = self.p.expr(arg_exprs[i]) else {
            return Err("inout arg must be a variable".into());
        };
        let (_, vars) = self.lookup(n).ok_or(format!("unknown variable `{}`", n))?;
        for (var, val) in vars.iter().zip(vals.iter()) {
            self.b.def_var(*var, *val);
        }
        Ok(())
    }

    fn gen_call(
        &mut self,
        name: &str,
        atys: Vec<CType>,
        avals: Vec<Vec<Value>>,
        arg_exprs: &[ExprId],
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
            "puti" => {
                self.call_import("lu_print_i64", &[avals[0][0]]);
                Ok((CType::Unit, vec![]))
            }
            "putf" => {
                self.call_import("lu_print_f64", &[avals[0][0]]);
                Ok((CType::Unit, vec![]))
            }
            "putb" => {
                self.call_import("lu_print_bool", &[avals[0][0]]);
                Ok((CType::Unit, vec![]))
            }
            "puts" => {
                self.call_import("lu_print_str", &[avals[0][0], avals[0][1]]);
                Ok((CType::Unit, vec![]))
            }
            "putsp" => {
                self.call_import("lu_print_sep", &[]);
                Ok((CType::Unit, vec![]))
            }
            "putnl" => {
                self.call_import("lu_print_nl", &[]);
                Ok((CType::Unit, vec![]))
            }
            "nargs" => {
                let v = self.call_import("lu_nargs", &[])[0];
                Ok((CType::I64, vec![v]))
            }
            "arg" => {
                let p = self.call_import("lu_arg", &[avals[0][0]])[0];
                let l = self.call_import("lu_last_len", &[])[0];
                Ok((CType::Str, vec![p, l]))
            }
            "read_file" => {
                let p = self.call_import("lu_read_file", &[avals[0][0], avals[0][1]])[0];
                let l = self.call_import("lu_last_len", &[])[0];
                Ok((CType::Str, vec![p, l]))
            }
            "write_file" => {
                self.call_import(
                    "lu_write_file",
                    &[avals[0][0], avals[0][1], avals[1][0], avals[1][1]],
                );
                Ok((CType::Unit, vec![]))
            }
            "chr" => {
                let p = self.call_import("lu_chr", &[avals[0][0]])[0];
                let l = self.call_import("lu_last_len", &[])[0];
                Ok((CType::Str, vec![p, l]))
            }
            "concat" => {
                let p = self.call_import(
                    "lu_concat",
                    &[avals[0][0], avals[0][1], avals[1][0], avals[1][1]],
                )[0];
                let l = self.call_import("lu_last_len", &[])[0];
                Ok((CType::Str, vec![p, l]))
            }
            "sqrt" | "abs" | "floor" | "sin" | "cos" | "acos" => {
                let x = self.f64_of(&atys[0], avals[0][0]);
                let v = match name {
                    "sqrt" => self.b.ins().sqrt(x),
                    "abs" => self.b.ins().fabs(x),
                    "floor" => self.b.ins().floor(x),
                    "sin" if self.inline_math => self.emit_trig(x, false),
                    "cos" if self.inline_math => self.emit_trig(x, true),
                    "acos" if self.inline_math => self.emit_acos(x),
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
                    avals[0][0] // i64, bool, enum tag: already an integer
                };
                Ok((CType::I64, vec![v]))
            }
            "len" if atys[0] == CType::Str => Ok((CType::I64, vec![avals[0][1]])),
            "substr" => {
                let (p0, l0) = (avals[0][0], avals[0][1]);
                let (lo, hi) = (avals[1][0], avals[2][0]);
                // 0 <= lo <= hi <= len, else abort
                let zero = self.b.ins().iconst(types::I64, 0);
                let neg = self.b.ins().icmp(IntCC::SignedLessThan, lo, zero);
                let inv = self.b.ins().icmp(IntCC::SignedLessThan, hi, lo);
                let over = self.b.ins().icmp(IntCC::SignedGreaterThan, hi, l0);
                let b1 = self.b.ins().bor(neg, inv);
                let bad = self.b.ins().bor(b1, over);
                let oob = self.b.create_block();
                let ok = self.b.create_block();
                self.b.ins().brif(bad, oob, &[], ok, &[]);
                self.b.switch_to_block(oob);
                let r = self.callee("lu_oob");
                self.b.ins().call(r, &[hi, l0]);
                self.b.ins().jump(ok, &[]);
                self.b.switch_to_block(ok);
                let np = self.b.ins().iadd(p0, lo);
                let nl = self.b.ins().isub(hi, lo);
                Ok((CType::Str, vec![np, nl]))
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
                    t @ (CType::Bool | CType::Enum(_)) => {
                        let elem = t.clone();
                        let p = self.call_import("lu_arr_new_i64", &[n, avals[1][0]])[0];
                        Ok((CType::Arr(Box::new(elem)), vec![p]))
                    }
                    t @ (CType::Rec(_) | CType::Str) => {
                        let elem = t.clone();
                        let stride = comps(self.p, &elem)?.len() as i64;
                        let slots = self.b.ins().imul_imm(n, stride);
                        let base = self.call_import("lu_arr_new_raw", &[slots])[0];
                        // fill loop: SoA planes (or AoS under LU_LAYOUT=aos)
                        let ivar = self.b.declare_var(types::I64);
                        let zero = self.b.ins().iconst(types::I64, 0);
                        self.b.def_var(ivar, zero);
                        let header = self.b.create_block();
                        let body = self.b.create_block();
                        let exit = self.b.create_block();
                        self.b.ins().jump(header, &[]);
                        self.b.switch_to_block(header);
                        let iv = self.b.use_var(ivar);
                        let more = self.b.ins().icmp(IntCC::SignedLessThan, iv, n);
                        self.b.ins().brif(more, body, &[], exit, &[]);
                        self.b.switch_to_block(body);
                        let addrs = self.elem_addrs(base, iv, &elem, Some(n))?;
                        for (v, a) in avals[1].iter().zip(addrs.iter()) {
                            self.b.ins().store(MemFlags::trusted(), *v, *a, 0);
                        }
                        let ivn = self.b.ins().iadd_imm(iv, 1);
                        self.b.def_var(ivar, ivn);
                        self.b.ins().jump(header, &[]);
                        self.b.switch_to_block(exit);
                        Ok((CType::Arr(Box::new(elem)), vec![base]))
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
                self.gen_user_call(name, args, Some(arg_exprs))
            }
        }
    }
}

enum StmtOut {
    Value(Option<(CType, Vec<Value>)>),
    Terminated,
}
