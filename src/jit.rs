// Cranelift JIT backend for `lu run`.
//
// Records are scalarized: a Quat is four F64 SSA values, never memory — value
// semantics means aliasing is impossible, so nothing forces records into RAM.
// CFG reduction analysis emits vector accumulators when possible: the language
// defines `sum` as order-free, so reassociation is legal by construction.
use crate::ast::{FnDecl, Program};
use crate::backend::layout::{
    array_component_offsets, components as layout_components, field_offset, Component,
};
use crate::backend::optimization::{analyze_cfg, if_convert, inline_calls, licm, CfgAnalysis};
use crate::check::{resolve_type, Type as CType};
use crate::ir::{self, BinaryOp, Callee, Constant, InstKind, LoweredProgram, Terminator, UnaryOp};
use crate::runtime;
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{types, AbiParam, InstBuilder, MemFlags, Value};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module};
use std::collections::HashMap;

const RTOL: f64 = 9.094947017729282e-13; // 2^-40
const ATOL: f64 = 7.888609052210118e-31; // 2^-100

fn comps(p: &Program, t: &CType) -> Result<Vec<cranelift_codegen::ir::Type>, String> {
    Ok(layout_components(p, t)?
        .into_iter()
        .map(|component| match component {
            Component::F32 => types::F32,
            Component::F64 => types::F64,
            Component::I64 | Component::Ptr => types::I64,
        })
        .collect())
}

fn array_local_for_value(function: &ir::Function, value: ir::ValueId) -> Option<ir::LocalId> {
    function
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .find_map(|inst| {
            (inst.result == Some(value)).then(|| match inst.kind {
                InstKind::Load(local) => Some(local),
                _ => None,
            })?
        })
}

fn cfg_value_definition(
    function: &ir::Function,
    value: ir::ValueId,
) -> Option<(ir::BlockId, usize, &ir::Inst)> {
    function
        .blocks
        .iter()
        .enumerate()
        .find_map(|(block, contents)| {
            contents
                .instructions
                .iter()
                .enumerate()
                .find(|(_, inst)| inst.result == Some(value))
                .map(|(instruction, inst)| (block as ir::BlockId, instruction, inst))
        })
}

fn cfg_value_is_load(function: &ir::Function, value: ir::ValueId, local: ir::LocalId) -> bool {
    cfg_value_definition(function, value)
        .is_some_and(|(_, _, inst)| matches!(inst.kind, InstKind::Load(id) if id == local))
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
    pub fn run(ir: &'a LoweredProgram) -> Result<(), String> {
        let p = ir.source();
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
        opt_flags
            .set("opt_level", "speed")
            .map_err(|e| e.to_string())?;
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
            ("lu_arr_clone", runtime::lu_arr_clone as *const u8),
            ("lu_str_eq", runtime::lu_str_eq as *const u8),
            ("lu_oob", runtime::lu_oob as *const u8),
            ("lu_i64_div", runtime::lu_i64_div as *const u8),
            ("lu_i64_rem", runtime::lu_i64_rem as *const u8),
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
        let soa = std::env::var("LU_LAYOUT")
            .map(|v| v != "aos")
            .unwrap_or(true);
        let simd = std::env::var("LU_SIMD").map(|v| v != "off").unwrap_or(true);
        let ifconv = std::env::var("LU_IFCONV")
            .map(|v| v != "off")
            .unwrap_or(true);
        let do_licm = std::env::var("LU_LICM").map(|v| v != "off").unwrap_or(true);
        let inline_math = std::env::var("LU_MATH")
            .map(|v| v != "call")
            .unwrap_or(true);
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
        for (index, f) in p.fns.iter().enumerate() {
            let mut function = inline_calls(&ir.functions[index], &ir.functions, 3000);
            if jit.ifconv {
                if_convert(&mut function);
            }
            jit.compile_ir_fn(&function, f)?;
        }
        let mut main = inline_calls(
            ir.main.as_ref().ok_or("no `main` block in program")?,
            &ir.functions,
            3000,
        );
        if jit.ifconv {
            if_convert(&mut main);
        }
        let main_id = jit.compile_ir_main(&main)?;
        jit.module
            .finalize_definitions()
            .map_err(|e| e.to_string())?;
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
            ("lu_arr_new_raw", 1, &[types::I64, types::I64], false),
            ("lu_arr_clone", 1, &[types::I64], false),
            (
                "lu_str_eq",
                1,
                &[types::I64, types::I64, types::I64, types::I64],
                false,
            ),
            ("lu_oob", 0, &[types::I64, types::I64], false),
            ("lu_i64_div", 1, &[types::I64, types::I64], false),
            ("lu_i64_rem", 1, &[types::I64, types::I64], false),
            ("lu_sin", 2, &[types::F64], true),
            ("lu_cos", 2, &[types::F64], true),
            ("lu_acos", 2, &[types::F64], true),
            ("lu_atan2", 2, &[types::F64, types::F64], true),
            ("lu_pow", 2, &[types::F64, types::F64], true),
            ("lu_fmod", 2, &[types::F64, types::F64], true),
            ("lu_nargs", 1, &[], false),
            ("lu_arg", 1, &[types::I64], false),
            ("lu_read_file", 1, &[types::I64, types::I64], false),
            (
                "lu_write_file",
                0,
                &[types::I64, types::I64, types::I64, types::I64],
                false,
            ),
            ("lu_last_len", 1, &[], false),
            ("lu_chr", 1, &[types::I64], false),
            (
                "lu_concat",
                1,
                &[types::I64, types::I64, types::I64, types::I64],
                false,
            ),
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
            let params: Result<Vec<CType>, String> = f
                .params
                .iter()
                .map(|(_, t)| resolve_type(self.p, t))
                .collect();
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

    fn compile_ir_fn(&mut self, function: &ir::Function, decl: &FnDecl) -> Result<(), String> {
        let analysis = analyze_cfg(function);
        let info_id = self.fns[&decl.name].id;
        let params = self.fns[&decl.name].params.clone();
        let ret = self.fns[&decl.name].ret.clone();
        let mut ctx = self.module.make_context();
        let mut sig = self.module.make_signature();
        for ty in &params {
            for component in comps(self.p, ty)? {
                sig.params.push(AbiParam::new(component));
            }
        }
        for component in comps(self.p, &ret)? {
            sig.returns.push(AbiParam::new(component));
        }
        for &io in &decl.inouts {
            if io {
                sig.params.push(AbiParam::new(types::I64));
            }
        }
        ctx.func.signature = sig;
        let mut fbc = FunctionBuilderContext::new();
        {
            let mut b = FunctionBuilder::new(&mut ctx.func, &mut fbc);
            let blocks: Vec<_> = (0..function.blocks.len())
                .map(|_| b.create_block())
                .collect();
            b.append_block_params_for_function_params(blocks[0]);
            b.switch_to_block(blocks[0]);
            let incoming = b.block_params(blocks[0]).to_vec();
            let mut g = Gen {
                p: self.p,
                b,
                module: &mut self.module,
                fns: &self.fns,
                imports: &self.imports,
                env: vec![HashMap::new()],
                refs: HashMap::new(),
                soa: self.soa,
                simd: self.simd,
                inline_math: self.inline_math,
                inout_outs: Vec::new(),
                cfg: &analysis,
                cfg_trusted: HashMap::new(),
                location: (0, 0),
                skipped_cfg_blocks: std::collections::HashSet::new(),
            };
            g.declare_ir_locals(function)?;
            let mut cursor = 0;
            for &local in &function.params {
                let n = comps(g.p, &function.locals[local as usize].ty)?.len();
                let mut values = incoming[cursor..cursor + n].to_vec();
                for offset in array_component_offsets(
                    g.p,
                    &function.locals[local as usize].ty,
                )? {
                    values[offset] = g.call_import("lu_arr_clone", &[values[offset]])[0];
                }
                g.define_ir_local(local, &values)?;
                cursor += n;
            }
            for (&local, &io) in function.params.iter().zip(&function.inouts) {
                if io {
                    let ptr = incoming[cursor];
                    cursor += 1;
                    let (_, vars) = g.lookup(&Gen::ir_local(local)).unwrap();
                    g.inout_outs.push((ptr, vars));
                }
            }
            g.gen_ir_body(function, &blocks)?;
            g.b.seal_all_blocks();
            g.b.finalize();
        }
        ctx.optimize(self.opt_isa.as_ref(), &mut Default::default())
            .map_err(|e| e.to_string())?;
        if self.do_licm {
            licm(&mut ctx.func, &self.pure_imports);
        }
        self.module
            .define_function(info_id, &mut ctx)
            .map_err(|e| e.to_string())?;
        self.module.clear_context(&mut ctx);
        Ok(())
    }

    fn compile_ir_main(&mut self, function: &ir::Function) -> Result<FuncId, String> {
        let analysis = analyze_cfg(function);
        let sig = self.module.make_signature();
        let id = self
            .module
            .declare_function("__lu_main", Linkage::Local, &sig)
            .map_err(|e| e.to_string())?;
        let mut ctx = self.module.make_context();
        ctx.func.signature = self.module.make_signature();
        let mut fbc = FunctionBuilderContext::new();
        {
            let mut b = FunctionBuilder::new(&mut ctx.func, &mut fbc);
            let blocks: Vec<_> = (0..function.blocks.len())
                .map(|_| b.create_block())
                .collect();
            b.switch_to_block(blocks[0]);
            let mut g = Gen {
                p: self.p,
                b,
                module: &mut self.module,
                fns: &self.fns,
                imports: &self.imports,
                env: vec![HashMap::new()],
                refs: HashMap::new(),
                soa: self.soa,
                simd: self.simd,
                inline_math: self.inline_math,
                inout_outs: Vec::new(),
                cfg: &analysis,
                cfg_trusted: HashMap::new(),
                location: (0, 0),
                skipped_cfg_blocks: std::collections::HashSet::new(),
            };
            g.declare_ir_locals(function)?;
            g.gen_ir_body(function, &blocks)?;
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
        self.module
            .define_function(id, &mut ctx)
            .map_err(|e| e.to_string())?;
        self.module.clear_context(&mut ctx);
        Ok(id)
    }
}

struct Gen<'a, 'b> {
    p: &'a Program,
    b: FunctionBuilder<'b>,
    module: &'a mut JITModule,
    fns: &'a HashMap<String, FnInfo>,
    imports: &'a HashMap<&'static str, FuncId>,
    env: Vec<HashMap<String, (CType, Vec<Variable>)>>,
    refs: HashMap<String, cranelift_codegen::ir::FuncRef>,
    // SoA record-array layout (the default; LU_LAYOUT=aos flips it)
    soa: bool,
    // `sum` vectorization (the default; LU_SIMD=off flips it)
    simd: bool,
    inline_math: bool,
    // (out-pointer, component variables) of the current fn's inout params —
    // stored through the pointer at every outlined return
    inout_outs: Vec<(Value, Vec<Variable>)>,
    cfg: &'a CfgAnalysis,
    cfg_trusted: HashMap<(usize, ir::LocalId), Value>,
    location: (ir::BlockId, usize),
    skipped_cfg_blocks: std::collections::HashSet<ir::BlockId>,
}

impl<'a, 'b> Gen<'a, 'b> {
    /// Store the current fn's inout param values through their out-pointers
    /// (called right before every outlined return).
    fn emit_inout_stores(&mut self) {
        let outs = self.inout_outs.clone();
        for (ptr, vars) in outs {
            for (k, v) in vars.iter().enumerate() {
                let val = self.b.use_var(*v);
                self.b
                    .ins()
                    .store(MemFlags::trusted(), val, ptr, (k * 8) as i32);
            }
        }
    }

    fn ir_local(id: ir::LocalId) -> String {
        format!("$l{}", id)
    }
    fn declare_ir_locals(&mut self, function: &ir::Function) -> Result<(), String> {
        for (id, local) in function.locals.iter().enumerate() {
            let vars = comps(self.p, &local.ty)?
                .into_iter()
                .map(|ty| self.b.declare_var(ty))
                .collect();
            self.env[0].insert(Self::ir_local(id as u32), (local.ty.clone(), vars));
        }
        Ok(())
    }
    fn define_ir_local(&mut self, id: ir::LocalId, values: &[Value]) -> Result<(), String> {
        let (_, vars) = self.lookup(&Self::ir_local(id)).ok_or("invalid IR local")?;
        for (var, value) in vars.iter().zip(values) {
            self.b.def_var(*var, *value);
        }
        Ok(())
    }
    fn ir_value(
        values: &[Option<(CType, Vec<Value>)>],
        id: ir::ValueId,
    ) -> Result<(CType, Vec<Value>), String> {
        values
            .get(id as usize)
            .and_then(Clone::clone)
            .ok_or_else(|| format!("IR value %{} unavailable", id))
    }

    fn coerce(
        &mut self,
        want: &CType,
        got: &CType,
        mut values: Vec<Value>,
    ) -> Result<Vec<Value>, String> {
        if want == got {
            return Ok(values);
        }
        values = match (want, got) {
            (CType::F32, CType::I64) => {
                vec![self.b.ins().fcvt_from_sint(types::F32, values[0])]
            }
            (CType::F64, CType::I64) => {
                vec![self.b.ins().fcvt_from_sint(types::F64, values[0])]
            }
            (CType::F32, CType::F64) => vec![self.b.ins().fdemote(types::F32, values[0])],
            (CType::F64, CType::F32) => vec![self.b.ins().fpromote(types::F64, values[0])],
            _ => return Err(format!("cannot coerce IR value {:?} to {:?}", got, want)),
        };
        Ok(values)
    }

    fn gen_ir_body(
        &mut self,
        function: &ir::Function,
        blocks: &[cranelift_codegen::ir::Block],
    ) -> Result<(), String> {
        let mut values = vec![None; function.values.len()];
        for (index, block) in function.blocks.iter().enumerate() {
            self.location.0 = index as ir::BlockId;
            if index != 0 {
                self.b.switch_to_block(blocks[index]);
            }
            if self.skipped_cfg_blocks.contains(&(index as ir::BlockId)) {
                self.b
                    .ins()
                    .trap(cranelift_codegen::ir::TrapCode::unwrap_user(1));
                continue;
            }
            for (instruction, inst) in block.instructions.iter().enumerate() {
                self.location.1 = instruction;
                let result = self.gen_ir_inst(function, &values, &inst.kind, &inst.ty)?;
                if let Some(id) = inst.result {
                    values[id as usize] =
                        Some(result.ok_or("IR value instruction produced no value")?);
                }
            }
            let mut replaced_terminator = false;
            for loop_index in 0..self.cfg.loops.len() {
                if self.cfg.loops[loop_index].preheader == index as ir::BlockId {
                    self.hoist_cfg_checks(function, &values, loop_index)?;
                    if self.simd && self.emit_cfg_simd(function, &values, blocks, loop_index)? {
                        replaced_terminator = true;
                    }
                }
            }
            if replaced_terminator {
                continue;
            }
            match block.terminator {
                Terminator::Jump(target) => {
                    self.b.ins().jump(blocks[target as usize], &[]);
                }
                Terminator::Branch {
                    condition,
                    then_block,
                    else_block,
                } => {
                    let (_, value) = Self::ir_value(&values, condition)?;
                    self.b.ins().brif(
                        value[0],
                        blocks[then_block as usize],
                        &[],
                        blocks[else_block as usize],
                        &[],
                    );
                }
                Terminator::Return(id) => {
                    let (ty, vals) = Self::ir_value(&values, id)?;
                    let vals = self.coerce(&function.ret, &ty, vals)?;
                    self.emit_inout_stores();
                    self.b.ins().return_(&vals);
                }
                Terminator::Unreachable => {
                    self.b
                        .ins()
                        .trap(cranelift_codegen::ir::TrapCode::unwrap_user(1));
                }
            }
        }
        Ok(())
    }

    fn hoist_cfg_checks(
        &mut self,
        function: &ir::Function,
        values: &[Option<(CType, Vec<Value>)>],
        loop_index: usize,
    ) -> Result<(), String> {
        let loop_info = &self.cfg.loops[loop_index];
        let (_, lower) = Self::ir_value(values, loop_info.lower)?;
        let (_, upper) = Self::ir_value(values, loop_info.upper)?;
        for &array in &loop_info.arrays {
            let (ty, vars) = self
                .lookup(&Self::ir_local(array))
                .ok_or("invalid trusted array local")?;
            let CType::Arr(element) = ty else { continue };
            let base = self.b.use_var(vars[0]);
            let stored = self.b.ins().load(types::I64, MemFlags::trusted(), base, 0);
            let stride = comps(self.p, &element)?.len() as i64;
            let logical = if stride == 1 {
                stored
            } else {
                self.b.ins().sdiv_imm(stored, stride)
            };
            let zero = self.b.ins().iconst(types::I64, 0);
            let negative = self.b.ins().icmp(IntCC::SignedLessThan, lower[0], zero);
            let over = self
                .b
                .ins()
                .icmp(IntCC::SignedGreaterThan, upper[0], logical);
            let bad = self.b.ins().bor(negative, over);
            let oob = self.b.create_block();
            let ok = self.b.create_block();
            self.b.ins().brif(bad, oob, &[], ok, &[]);
            self.b.switch_to_block(oob);
            let r = self.callee("lu_oob");
            self.b.ins().call(r, &[upper[0], logical]);
            self.b.ins().jump(ok, &[]);
            self.b.switch_to_block(ok);
            self.cfg_trusted.insert((loop_index, array), logical);
        }
        let _ = function;
        Ok(())
    }

    fn emit_cfg_simd(
        &mut self,
        function: &ir::Function,
        values: &[Option<(CType, Vec<Value>)>],
        blocks: &[cranelift_codegen::ir::Block],
        loop_index: usize,
    ) -> Result<bool, String> {
        let loop_info = &self.cfg.loops[loop_index];
        let Some(reduction) = &loop_info.reduction else {
            return Ok(false);
        };
        if function.locals[reduction.accumulator as usize].ty != CType::F64
            || !self.cfg_vector_value(function, loop_index, reduction.value)
        {
            return Ok(false);
        }
        let (_, lower) = Self::ir_value(values, loop_info.lower)?;
        let (_, upper) = Self::ir_value(values, loop_info.upper)?;
        let index_var = self.b.declare_var(types::I64);
        self.b.def_var(index_var, lower[0]);
        let vector_accs: Vec<_> = (0..4).map(|_| self.b.declare_var(types::F64X2)).collect();
        let scalar_acc = self.b.declare_var(types::F64);
        let zero = self.b.ins().f64const(0.0);
        let vector_zero = self.b.ins().splat(types::F64X2, zero);
        for accumulator in &vector_accs {
            self.b.def_var(*accumulator, vector_zero);
        }
        self.b.def_var(scalar_acc, zero);

        let vector_head = self.b.create_block();
        let vector_body = self.b.create_block();
        let scalar_head = self.b.create_block();
        let scalar_body = self.b.create_block();
        let finish = self.b.create_block();
        self.b.ins().jump(vector_head, &[]);

        self.b.switch_to_block(vector_head);
        let index = self.b.use_var(index_var);
        let after_batch = self.b.ins().iadd_imm(index, 8);
        let fits = self
            .b
            .ins()
            .icmp(IntCC::SignedLessThanOrEqual, after_batch, upper[0]);
        self.b.ins().brif(fits, vector_body, &[], scalar_head, &[]);

        self.b.switch_to_block(vector_body);
        let batch = self.b.use_var(index_var);
        for (lane, accumulator) in vector_accs.iter().enumerate() {
            let at = self.b.ins().iadd_imm(batch, (lane * 2) as i64);
            let item = self.gen_cfg_vector_value(function, loop_index, reduction.value, at)?;
            let current = self.b.use_var(*accumulator);
            let next = self.b.ins().fadd(current, item);
            self.b.def_var(*accumulator, next);
        }
        let next_batch = self.b.ins().iadd_imm(batch, 8);
        self.b.def_var(index_var, next_batch);
        self.b.ins().jump(vector_head, &[]);

        self.b.switch_to_block(scalar_head);
        let index = self.b.use_var(index_var);
        let more = self.b.ins().icmp(IntCC::SignedLessThan, index, upper[0]);
        self.b.ins().brif(more, scalar_body, &[], finish, &[]);

        self.b.switch_to_block(scalar_body);
        let at = self.b.use_var(index_var);
        let item = self.gen_cfg_scalar_value(function, loop_index, reduction.value, at)?;
        let current = self.b.use_var(scalar_acc);
        let next = self.b.ins().fadd(current, item);
        self.b.def_var(scalar_acc, next);
        let next_index = self.b.ins().iadd_imm(at, 1);
        self.b.def_var(index_var, next_index);
        self.b.ins().jump(scalar_head, &[]);

        self.b.switch_to_block(finish);
        let a0 = self.b.use_var(vector_accs[0]);
        let a1 = self.b.use_var(vector_accs[1]);
        let a2 = self.b.use_var(vector_accs[2]);
        let a3 = self.b.use_var(vector_accs[3]);
        let pairs0 = self.b.ins().fadd(a0, a1);
        let pairs1 = self.b.ins().fadd(a2, a3);
        let vector_total = self.b.ins().fadd(pairs0, pairs1);
        let lane0 = self.b.ins().extractlane(vector_total, 0);
        let lane1 = self.b.ins().extractlane(vector_total, 1);
        let lanes = self.b.ins().fadd(lane0, lane1);
        let scalar = self.b.use_var(scalar_acc);
        let total = self.b.ins().fadd(lanes, scalar);
        self.define_ir_local(reduction.accumulator, &[total])?;
        self.b.ins().jump(blocks[loop_info.exit as usize], &[]);
        self.skipped_cfg_blocks
            .extend(loop_info.blocks.iter().copied());
        Ok(true)
    }

    fn cfg_vector_value(
        &self,
        function: &ir::Function,
        loop_index: usize,
        value: ir::ValueId,
    ) -> bool {
        let Some((block, instruction, inst)) = cfg_value_definition(function, value) else {
            return false;
        };
        match &inst.kind {
            InstKind::Constant(Constant::F64(_) | Constant::I64(_)) => true,
            InstKind::Load(local) => {
                *local != self.cfg.loops[loop_index].induction
                    && function.locals[*local as usize].ty == CType::F64
            }
            InstKind::Unary {
                op: UnaryOp::Neg,
                value,
            } => self.cfg_vector_value(function, loop_index, *value),
            InstKind::Binary { op, lhs, rhs }
                if matches!(
                    op,
                    BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div
                ) =>
            {
                self.cfg_vector_value(function, loop_index, *lhs)
                    && self.cfg_vector_value(function, loop_index, *rhs)
            }
            InstKind::Index { base, index } => {
                function.values[value as usize] == CType::F64
                    && self.cfg.trusted_accesses.get(&(block, instruction)) == Some(&loop_index)
                    && array_local_for_value(function, *base).is_some()
                    && cfg_value_is_load(function, *index, self.cfg.loops[loop_index].induction)
            }
            InstKind::Field { base, .. } if self.soa => {
                let Some((base_block, base_instruction, base_inst)) =
                    cfg_value_definition(function, *base)
                else {
                    return false;
                };
                let InstKind::Index { base: array, index } = base_inst.kind else {
                    return false;
                };
                function.values[value as usize] == CType::F64
                    && self
                        .cfg
                        .trusted_accesses
                        .get(&(base_block, base_instruction))
                        == Some(&loop_index)
                    && array_local_for_value(function, array).is_some()
                    && cfg_value_is_load(function, index, self.cfg.loops[loop_index].induction)
            }
            InstKind::Call {
                callee: Callee::Builtin(name),
                args,
                ..
            } if matches!(name.as_str(), "sqrt" | "abs" | "min" | "max") => args
                .iter()
                .all(|value| self.cfg_vector_value(function, loop_index, *value)),
            _ => false,
        }
    }

    fn gen_cfg_vector_value(
        &mut self,
        function: &ir::Function,
        loop_index: usize,
        value: ir::ValueId,
        index: Value,
    ) -> Result<Value, String> {
        let (_, _, inst) =
            cfg_value_definition(function, value).ok_or("missing vector value definition")?;
        Ok(match &inst.kind {
            InstKind::Constant(Constant::F64(value)) => {
                let scalar = self.b.ins().f64const(*value);
                self.b.ins().splat(types::F64X2, scalar)
            }
            InstKind::Constant(Constant::I64(value)) => {
                let scalar = self.b.ins().f64const(*value as f64);
                self.b.ins().splat(types::F64X2, scalar)
            }
            InstKind::Load(local) => {
                let (_, vars) = self
                    .lookup(&Self::ir_local(*local))
                    .ok_or("missing vector invariant")?;
                let scalar = self.b.use_var(vars[0]);
                self.b.ins().splat(types::F64X2, scalar)
            }
            InstKind::Unary { value, .. } => {
                let inner = self.gen_cfg_vector_value(function, loop_index, *value, index)?;
                self.b.ins().fneg(inner)
            }
            InstKind::Binary { op, lhs, rhs } => {
                let lhs = self.gen_cfg_vector_value(function, loop_index, *lhs, index)?;
                let rhs = self.gen_cfg_vector_value(function, loop_index, *rhs, index)?;
                match op {
                    BinaryOp::Add => self.b.ins().fadd(lhs, rhs),
                    BinaryOp::Sub => self.b.ins().fsub(lhs, rhs),
                    BinaryOp::Mul => self.b.ins().fmul(lhs, rhs),
                    BinaryOp::Div => self.b.ins().fdiv(lhs, rhs),
                    _ => return Err("unsupported vector binary".into()),
                }
            }
            InstKind::Index { base, .. } => {
                let local =
                    array_local_for_value(function, *base).ok_or("missing vector array local")?;
                let (_, vars) = self
                    .lookup(&Self::ir_local(local))
                    .ok_or("missing vector array")?;
                let base = self.b.use_var(vars[0]);
                let bytes = self.b.ins().imul_imm(index, 8);
                let data = self.b.ins().iadd_imm(base, 8);
                let address = self.b.ins().iadd(data, bytes);
                self.b
                    .ins()
                    .load(types::F64X2, MemFlags::trusted(), address, 0)
            }
            InstKind::Field {
                base,
                record,
                field,
            } => {
                let (_, _, indexed) =
                    cfg_value_definition(function, *base).ok_or("missing field base")?;
                let InstKind::Index { base: array, .. } = indexed.kind else {
                    return Err("vector field base is not an index".into());
                };
                let local =
                    array_local_for_value(function, array).ok_or("missing field array local")?;
                let (_, vars) = self
                    .lookup(&Self::ir_local(local))
                    .ok_or("missing field array")?;
                let base = self.b.use_var(vars[0]);
                let field_name = &self.p.types[*record].fields[*field].0;
                let (component, _) = field_offset(self.p, *record, field_name)?;
                let logical = self.cfg_trusted[&(loop_index, local)];
                let data = self.b.ins().iadd_imm(base, 8);
                let plane = self.b.ins().imul_imm(logical, (component * 8) as i64);
                let lane = self.b.ins().imul_imm(index, 8);
                let start = self.b.ins().iadd(data, plane);
                let address = self.b.ins().iadd(start, lane);
                self.b
                    .ins()
                    .load(types::F64X2, MemFlags::trusted(), address, 0)
            }
            InstKind::Call {
                callee: Callee::Builtin(name),
                args,
                ..
            } => {
                let args = args
                    .iter()
                    .map(|value| self.gen_cfg_vector_value(function, loop_index, *value, index))
                    .collect::<Result<Vec<_>, _>>()?;
                match name.as_str() {
                    "sqrt" => self.b.ins().sqrt(args[0]),
                    "abs" => self.b.ins().fabs(args[0]),
                    "min" => self.b.ins().fmin(args[0], args[1]),
                    "max" => self.b.ins().fmax(args[0], args[1]),
                    _ => return Err("unsupported vector builtin".into()),
                }
            }
            _ => return Err("unsupported vector value".into()),
        })
    }

    fn gen_cfg_scalar_value(
        &mut self,
        function: &ir::Function,
        loop_index: usize,
        value: ir::ValueId,
        index: Value,
    ) -> Result<Value, String> {
        let (_, _, inst) =
            cfg_value_definition(function, value).ok_or("missing scalar value definition")?;
        Ok(match &inst.kind {
            InstKind::Constant(Constant::F64(value)) => self.b.ins().f64const(*value),
            InstKind::Constant(Constant::I64(value)) => self.b.ins().f64const(*value as f64),
            InstKind::Load(local) if *local == self.cfg.loops[loop_index].induction => index,
            InstKind::Load(local) => {
                let (_, vars) = self
                    .lookup(&Self::ir_local(*local))
                    .ok_or("missing scalar invariant")?;
                self.b.use_var(vars[0])
            }
            InstKind::Unary { value, .. } => {
                let inner = self.gen_cfg_scalar_value(function, loop_index, *value, index)?;
                self.b.ins().fneg(inner)
            }
            InstKind::Binary { op, lhs, rhs } => {
                let lhs = self.gen_cfg_scalar_value(function, loop_index, *lhs, index)?;
                let rhs = self.gen_cfg_scalar_value(function, loop_index, *rhs, index)?;
                match op {
                    BinaryOp::Add => self.b.ins().fadd(lhs, rhs),
                    BinaryOp::Sub => self.b.ins().fsub(lhs, rhs),
                    BinaryOp::Mul => self.b.ins().fmul(lhs, rhs),
                    BinaryOp::Div => self.b.ins().fdiv(lhs, rhs),
                    _ => return Err("unsupported scalar binary".into()),
                }
            }
            InstKind::Index { base, .. } => {
                let local =
                    array_local_for_value(function, *base).ok_or("missing scalar array local")?;
                let (_, vars) = self
                    .lookup(&Self::ir_local(local))
                    .ok_or("missing scalar array")?;
                let base = self.b.use_var(vars[0]);
                let address = self.b.ins().imul_imm(index, 8);
                let data = self.b.ins().iadd_imm(base, 8);
                let address = self.b.ins().iadd(data, address);
                self.b
                    .ins()
                    .load(types::F64, MemFlags::trusted(), address, 0)
            }
            InstKind::Field {
                base,
                record,
                field,
            } => {
                let (_, _, indexed) =
                    cfg_value_definition(function, *base).ok_or("missing scalar field base")?;
                let InstKind::Index { base: array, .. } = indexed.kind else {
                    return Err("scalar field base is not an index".into());
                };
                let local =
                    array_local_for_value(function, array).ok_or("missing scalar field local")?;
                let (_, vars) = self
                    .lookup(&Self::ir_local(local))
                    .ok_or("missing scalar field array")?;
                let base = self.b.use_var(vars[0]);
                let field_name = &self.p.types[*record].fields[*field].0;
                let (component, _) = field_offset(self.p, *record, field_name)?;
                let logical = self.cfg_trusted[&(loop_index, local)];
                let data = self.b.ins().iadd_imm(base, 8);
                let plane = self.b.ins().imul_imm(logical, (component * 8) as i64);
                let lane = self.b.ins().imul_imm(index, 8);
                let start = self.b.ins().iadd(data, plane);
                let address = self.b.ins().iadd(start, lane);
                self.b
                    .ins()
                    .load(types::F64, MemFlags::trusted(), address, 0)
            }
            InstKind::Call {
                callee: Callee::Builtin(name),
                args,
                ..
            } => {
                let args = args
                    .iter()
                    .map(|value| self.gen_cfg_scalar_value(function, loop_index, *value, index))
                    .collect::<Result<Vec<_>, _>>()?;
                match name.as_str() {
                    "sqrt" => self.b.ins().sqrt(args[0]),
                    "abs" => self.b.ins().fabs(args[0]),
                    "min" => self.b.ins().fmin(args[0], args[1]),
                    "max" => self.b.ins().fmax(args[0], args[1]),
                    _ => return Err("unsupported scalar builtin".into()),
                }
            }
            _ => return Err("unsupported scalar reduction value".into()),
        })
    }

    fn gen_ir_inst(
        &mut self,
        function: &ir::Function,
        values: &[Option<(CType, Vec<Value>)>],
        kind: &InstKind,
        ty: &CType,
    ) -> Result<Option<(CType, Vec<Value>)>, String> {
        let value = |id| Self::ir_value(values, id);
        Ok(match kind {
            InstKind::Constant(c) => Some(match c {
                Constant::I64(v) => (CType::I64, vec![self.b.ins().iconst(types::I64, *v)]),
                Constant::F32(v) => (CType::F32, vec![self.b.ins().f32const(*v)]),
                Constant::F64(v) => (CType::F64, vec![self.b.ins().f64const(*v)]),
                Constant::Bool(v) => (
                    CType::Bool,
                    vec![self.b.ins().iconst(types::I64, *v as i64)],
                ),
                Constant::Bytes(bytes) => (
                    CType::Str,
                    vec![
                        self.b.ins().iconst(types::I64, bytes.as_ptr() as i64),
                        self.b.ins().iconst(types::I64, bytes.len() as i64),
                    ],
                ),
                Constant::Unit => (CType::Unit, vec![]),
            }),
            InstKind::Load(local) => {
                let (ty, vars) = self
                    .lookup(&Self::ir_local(*local))
                    .ok_or("invalid IR local")?;
                Some((ty, vars.iter().map(|v| self.b.use_var(*v)).collect()))
            }
            InstKind::Store { local, value: id } => {
                let (got, vals) = value(*id)?;
                let want = &function.locals[*local as usize].ty;
                let mut vals = self.coerce(want, &got, vals)?;
                for offset in array_component_offsets(self.p, want)? {
                    vals[offset] = self.call_import("lu_arr_clone", &[vals[offset]])[0];
                }
                self.define_ir_local(*local, &vals)?;
                None
            }
            InstKind::Unary { op, value: id } => {
                let (_, vals) = value(*id)?;
                Some((
                    ty.clone(),
                    vec![match op {
                        UnaryOp::Neg if *ty == CType::F64 => self.b.ins().fneg(vals[0]),
                        UnaryOp::Neg => self.b.ins().ineg(vals[0]),
                        UnaryOp::Not => self.b.ins().bxor_imm(vals[0], 1),
                    }],
                ))
            }
            InstKind::Binary { op, lhs, rhs } => {
                let (lhs_ty, lhs) = value(*lhs)?;
                let (rhs_ty, rhs) = value(*rhs)?;
                Some(self.gen_ir_binary(*op, lhs_ty, lhs, rhs_ty, rhs)?)
            }
            InstKind::Select {
                condition,
                then_value,
                else_value,
            } => {
                let (_, condition) = value(*condition)?;
                let (then_ty, then_values) = value(*then_value)?;
                let (_, else_values) = value(*else_value)?;
                Some((
                    then_ty,
                    then_values
                        .iter()
                        .zip(else_values)
                        .map(|(&yes, no)| self.b.ins().select(condition[0], yes, no))
                        .collect(),
                ))
            }
            InstKind::Call {
                callee,
                args,
                inout,
            } => {
                let args = args
                    .iter()
                    .map(|id| value(*id))
                    .collect::<Result<Vec<_>, _>>()?;
                Some(match callee {
                    Callee::Builtin(name) => {
                        let (types, vals): (Vec<_>, Vec<_>) = args.into_iter().unzip();
                        self.gen_call(name, types, vals)?
                    }
                    Callee::Function(id) => self.gen_ir_user_call(*id, args, inout)?,
                })
            }
            InstKind::Field {
                base,
                record,
                field,
            } => {
                let (_, vals) = value(*base)?;
                let name = &self.p.types[*record].fields[*field].0;
                let (off, field_ty) = field_offset(self.p, *record, name)?;
                let width = comps(self.p, &field_ty)?.len();
                Some((field_ty, vals[off..off + width].to_vec()))
            }
            InstKind::Index { base, index } => {
                let base_id = *base;
                let (base_ty, base) = value(*base)?;
                let (_, index) = value(*index)?;
                let trusted =
                    self.cfg
                        .trusted_accesses
                        .get(&self.location)
                        .and_then(|loop_index| {
                            let array = array_local_for_value(function, base_id)?;
                            self.cfg_trusted.get(&(*loop_index, array)).copied()
                        });
                Some(self.gen_ir_index(base_ty, base, index[0], trusted)?)
            }
            InstKind::Array(items) => {
                let items = items
                    .iter()
                    .map(|id| value(*id))
                    .collect::<Result<Vec<_>, _>>()?;
                Some(self.gen_ir_array(items, ty)?)
            }
            InstKind::Record { record, fields } => {
                let mut out = Vec::new();
                for (id, (_, type_name)) in fields.iter().zip(&self.p.types[*record].fields) {
                    let (got, vals) = value(*id)?;
                    let want = resolve_type(self.p, type_name)?;
                    let vals = self.coerce(&want, &got, vals)?;
                    out.extend(vals);
                }
                Some((ty.clone(), out))
            }
            InstKind::Enum { enumeration, tag } => Some((
                CType::Enum(*enumeration),
                vec![self.b.ins().iconst(types::I64, *tag)],
            )),
            InstKind::SetIndex {
                base,
                index,
                value: stored,
                ..
            } => {
                let base_id = *base;
                let (base_ty, base) = value(*base)?;
                let (_, index) = value(*index)?;
                let (_, stored) = value(*stored)?;
                let CType::Arr(elem) = base_ty else {
                    return Err("IR set-index on non-array".into());
                };
                let trusted =
                    self.cfg
                        .trusted_accesses
                        .get(&self.location)
                        .and_then(|loop_index| {
                            let array = array_local_for_value(function, base_id)?;
                            self.cfg_trusted.get(&(*loop_index, array)).copied()
                        });
                let addrs = self.elem_addrs(base[0], index[0], &elem, trusted)?;
                for (reg, addr) in stored.iter().zip(addrs) {
                    self.b.ins().store(MemFlags::trusted(), *reg, addr, 0);
                }
                None
            }
            InstKind::SetField {
                root,
                path,
                value: stored,
            } => {
                let (_, stored) = value(*stored)?;
                let (mut current, vars) = self
                    .lookup(&Self::ir_local(*root))
                    .ok_or("invalid field root")?;
                let mut offset = 0;
                for &field in path {
                    let CType::Rec(record) = current else {
                        return Err("field path on non-record".into());
                    };
                    let name = &self.p.types[record].fields[field].0;
                    let (add, next) = field_offset(self.p, record, name)?;
                    offset += add;
                    current = next;
                }
                for (var, value) in vars[offset..].iter().zip(stored) {
                    self.b.def_var(*var, value);
                }
                None
            }
        })
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

    fn elem_addrs(
        &mut self,
        base: Value,
        idx: Value,
        elem: &CType,
        trusted: Option<Value>,
    ) -> Result<Vec<Value>, String> {
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
                let bad = self
                    .b
                    .ins()
                    .icmp(IntCC::UnsignedGreaterThanOrEqual, idx, logical);
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
            Ok((0..stride)
                .map(|c| self.b.ins().iadd_imm(a0, c * 8))
                .collect())
        }
    }

    /// Emit `idx u< len` check, aborting via lu_oob on failure.
    fn check_idx(&mut self, idx: Value, len: Value) {
        let bad = self
            .b
            .ins()
            .icmp(IntCC::UnsignedGreaterThanOrEqual, idx, len);
        let oob = self.b.create_block();
        let ok = self.b.create_block();
        self.b.ins().brif(bad, oob, &[], ok, &[]);
        self.b.switch_to_block(oob);
        let r = self.callee("lu_oob");
        self.b.ins().call(r, &[idx, len]);
        self.b.ins().jump(ok, &[]);
        self.b.switch_to_block(ok);
    }

    fn f64_of(&mut self, t: &CType, v: Value) -> Value {
        match t {
            CType::I64 => self.b.ins().fcvt_from_sint(types::F64, v),
            CType::F32 => self.b.ins().fpromote(types::F64, v),
            _ => v,
        }
    }

    fn gen_ir_binary(
        &mut self,
        op: BinaryOp,
        lt: CType,
        lv: Vec<Value>,
        rt: CType,
        rv: Vec<Value>,
    ) -> Result<(CType, Vec<Value>), String> {
        use BinaryOp::*;
        if matches!(op, Add | Sub | Mul | Div | Rem) {
            if lt == CType::I64 && rt == CType::I64 {
                let v = match op {
                    Add => self.b.ins().iadd(lv[0], rv[0]),
                    Sub => self.b.ins().isub(lv[0], rv[0]),
                    Mul => self.b.ins().imul(lv[0], rv[0]),
                    Div => self.checked_int_div(lv[0], rv[0], false),
                    Rem => self.checked_int_div(lv[0], rv[0], true),
                    _ => unreachable!(),
                };
                return Ok((CType::I64, vec![v]));
            }
            let result_ty = if lt == CType::F64 || rt == CType::F64 {
                CType::F64
            } else {
                CType::F32
            };
            let a = self.coerce(&result_ty, &lt, lv)?[0];
            let b = self.coerce(&result_ty, &rt, rv)?[0];
            let v = match op {
                Add => self.b.ins().fadd(a, b),
                Sub => self.b.ins().fsub(a, b),
                Mul => self.b.ins().fmul(a, b),
                Div => self.b.ins().fdiv(a, b),
                Rem if result_ty == CType::F64 => self.call_import("lu_fmod", &[a, b])[0],
                Rem => {
                    let ap = self.b.ins().fpromote(types::F64, a);
                    let bp = self.b.ins().fpromote(types::F64, b);
                    let rem = self.call_import("lu_fmod", &[ap, bp])[0];
                    self.b.ins().fdemote(types::F32, rem)
                }
                _ => unreachable!(),
            };
            return Ok((result_ty, vec![v]));
        }
        if matches!(op, Eq | Ne) && lt == CType::Str && rt == CType::Str {
            let eq = self.call_import("lu_str_eq", &[lv[0], lv[1], rv[0], rv[1]])[0];
            return Ok((
                CType::Bool,
                vec![if op == Ne {
                    self.b.ins().bxor_imm(eq, 1)
                } else {
                    eq
                }],
            ));
        }
        if op == ApproxEq {
            let a = self.f64_of(&lt, lv[0]);
            let b = self.f64_of(&rt, rv[0]);
            let raw_diff = self.b.ins().fsub(a, b);
            let diff = self.b.ins().fabs(raw_diff);
            let abs_a = self.b.ins().fabs(a);
            let abs_b = self.b.ins().fabs(b);
            let scale = self.b.ins().fmax(abs_a, abs_b);
            let rtol = self.b.ins().f64const(RTOL);
            let atol = self.b.ins().f64const(ATOL);
            let scaled = self.b.ins().fmul(scale, rtol);
            let tol = self.b.ins().fadd(scaled, atol);
            let bit = self.b.ins().fcmp(FloatCC::LessThanOrEqual, diff, tol);
            return Ok((CType::Bool, vec![self.b.ins().uextend(types::I64, bit)]));
        }
        let both_int = matches!(lt, CType::I64 | CType::Bool | CType::Enum(_))
            && matches!(rt, CType::I64 | CType::Bool | CType::Enum(_));
        let bit = if both_int {
            self.b.ins().icmp(
                match op {
                    Eq => IntCC::Equal,
                    Ne => IntCC::NotEqual,
                    Lt => IntCC::SignedLessThan,
                    Le => IntCC::SignedLessThanOrEqual,
                    Gt => IntCC::SignedGreaterThan,
                    Ge => IntCC::SignedGreaterThanOrEqual,
                    _ => return Err("invalid comparison".into()),
                },
                lv[0],
                rv[0],
            )
        } else {
            let a = self.f64_of(&lt, lv[0]);
            let b = self.f64_of(&rt, rv[0]);
            self.b.ins().fcmp(
                match op {
                    Eq => FloatCC::Equal,
                    Ne => FloatCC::NotEqual,
                    Lt => FloatCC::LessThan,
                    Le => FloatCC::LessThanOrEqual,
                    Gt => FloatCC::GreaterThan,
                    Ge => FloatCC::GreaterThanOrEqual,
                    _ => return Err("invalid comparison".into()),
                },
                a,
                b,
            )
        };
        Ok((CType::Bool, vec![self.b.ins().uextend(types::I64, bit)]))
    }

    fn gen_ir_index(
        &mut self,
        base_ty: CType,
        base: Vec<Value>,
        index: Value,
        trusted: Option<Value>,
    ) -> Result<(CType, Vec<Value>), String> {
        if base_ty == CType::Str {
            self.check_idx(index, base[1]);
            let addr = self.b.ins().iadd(base[0], index);
            let byte = self
                .b
                .ins()
                .uload8(types::I64, MemFlags::trusted(), addr, 0);
            return Ok((CType::I64, vec![byte]));
        }
        let CType::Arr(elem) = base_ty else {
            return Err("IR index on non-array".into());
        };
        let addrs = self.elem_addrs(base[0], index, &elem, trusted)?;
        let mut out = Vec::new();
        for (component, addr) in comps(self.p, &elem)?.into_iter().zip(addrs) {
            out.push(self.b.ins().load(component, MemFlags::trusted(), addr, 0));
        }
        Ok((*elem, out))
    }

    fn gen_ir_array(
        &mut self,
        items: Vec<(CType, Vec<Value>)>,
        ty: &CType,
    ) -> Result<(CType, Vec<Value>), String> {
        let CType::Arr(elem) = ty else {
            return Err("IR array with non-array type".into());
        };
        let logical = self.b.ins().iconst(types::I64, items.len() as i64);
        let stride = self
            .b
            .ins()
            .iconst(types::I64, comps(self.p, elem)?.len() as i64);
        let base = self.call_import("lu_arr_new_raw", &[logical, stride])[0];
        for (i, (got, mut vals)) in items.into_iter().enumerate() {
            if **elem == CType::F64 && got == CType::I64 {
                vals = vec![self.b.ins().fcvt_from_sint(types::F64, vals[0])];
            }
            let index = self.b.ins().iconst(types::I64, i as i64);
            let addrs = self.elem_addrs(base, index, elem, Some(logical))?;
            for (value, addr) in vals.into_iter().zip(addrs) {
                self.b.ins().store(MemFlags::trusted(), value, addr, 0);
            }
        }
        Ok((ty.clone(), vec![base]))
    }

    fn gen_ir_user_call(
        &mut self,
        id: ir::FunctionId,
        args: Vec<(CType, Vec<Value>)>,
        inout: &[Option<ir::LocalId>],
    ) -> Result<(CType, Vec<Value>), String> {
        use cranelift_codegen::ir::{StackSlotData, StackSlotKind};
        let decl = &self.p.fns[id as usize];
        let info = &self.fns[&decl.name];
        let ret = info.ret.clone();
        let params = info.params.clone();
        let mut flat = Vec::new();
        for ((got, vals), want) in args.into_iter().zip(&params) {
            flat.extend(self.coerce(want, &got, vals)?);
        }
        let mut slots = Vec::new();
        for (i, (&io, ty)) in decl.inouts.iter().zip(&params).enumerate() {
            if io {
                let components = comps(self.p, ty)?;
                let slot = self.b.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    (components.len() * 8) as u32,
                    3,
                ));
                flat.push(self.b.ins().stack_addr(types::I64, slot, 0));
                slots.push((i, slot, components));
            }
        }
        let callee = self.callee(&decl.name);
        let call = self.b.ins().call(callee, &flat);
        let result = self.b.inst_results(call).to_vec();
        for (i, slot, components) in slots {
            let target = inout[i].ok_or("missing IR inout target")?;
            let loaded = components
                .iter()
                .enumerate()
                .map(|(k, &ty)| self.b.ins().stack_load(ty, slot, (k * 8) as i32))
                .collect::<Vec<_>>();
            self.define_ir_local(target, &loaded)?;
        }
        Ok((ret, result))
    }

    fn checked_int_div(&mut self, lhs: Value, rhs: Value, remainder: bool) -> Value {
        let name = if remainder {
            "lu_i64_rem"
        } else {
            "lu_i64_div"
        };
        self.call_import(name, &[lhs, rhs])[0]
    }

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
        let qn = if is_cos {
            self.b.ins().iadd_imm(q, 1)
        } else {
            q
        };
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
                        CType::F32 => {
                            let value = self.b.ins().fpromote(types::F64, vals[0]);
                            self.call_import("lu_print_f64", &[value]);
                        }
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
                let value = self.f64_of(&atys[0], avals[0][0]);
                self.call_import("lu_print_f64", &[value]);
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
                if atys[0] == CType::F32 {
                    Ok((CType::F32, vec![self.b.ins().fdemote(types::F32, v)]))
                } else {
                    Ok((CType::F64, vec![v]))
                }
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
                if atys.iter().all(|t| *t == CType::F32) {
                    Ok((CType::F32, vec![self.b.ins().fdemote(types::F32, v)]))
                } else {
                    Ok((CType::F64, vec![v]))
                }
            }
            "float" => {
                let v = self.f64_of(&atys[0], avals[0][0]);
                Ok((CType::F64, vec![v]))
            }
            "f32" => {
                let value = self.coerce(&CType::F32, &atys[0], avals[0].clone())?;
                Ok((CType::F32, value))
            }
            "int" => {
                let v = if matches!(atys[0], CType::F32 | CType::F64) {
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
                let n = self
                    .b
                    .ins()
                    .load(types::I64, MemFlags::trusted(), avals[0][0], 0);
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
                    t @ (CType::F32 | CType::Rec(_) | CType::Str) => {
                        let elem = t.clone();
                        let stride = comps(self.p, &elem)?.len() as i64;
                        let stride_val = self.b.ins().iconst(types::I64, stride);
                        let base = self.call_import("lu_arr_new_raw", &[n, stride_val])[0];
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
            _ => Err(format!("unknown builtin `{}`", name)),
        }
    }
}
