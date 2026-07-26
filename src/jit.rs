// Cranelift backend. Drives `lu run` (in-memory JIT) and `lu build --fast`
// (object emission through the same code generator).
//
// Records are scalarized: a Quat is four F64 SSA values, never memory — value
// semantics means aliasing is impossible, so nothing forces records into RAM.
// CFG reduction analysis emits vector accumulators when possible: the language
// defines `sum` as order-free, so reassociation is legal by construction.
use crate::ast::{FnDecl, Program};
use crate::backend::layout::{
    array_component_offsets, components as layout_components, field_offset, Component,
};
use crate::backend::optimization::{
    analyze_cfg, array_local_for_value, if_convert, inline_calls, licm, simd_reduction_plan,
    simd_store_plan, CfgAnalysis, SimdExpr, SimdScalar,
};
use crate::backend::simd::{lane_count, SIMD128};
use crate::check::{resolve_type, Type as CType};
use crate::ir::{self, BinaryOp, Callee, Constant, InstKind, LoweredProgram, Terminator, UnaryOp};
use crate::runtime;
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{
    types, AbiParam, InstBuilder, InstructionData, MemFlags, Opcode, Value, ValueDef,
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module};
use std::collections::HashMap;

/// Default per-function inline budget, in IR instructions.
///
/// Measured on the corpus (experiment 5 in `experiments/RESULTS.md`): inlining
/// pays for exactly one thing, the operator-chain tax in `bench_slerp`, and it
/// is paid off by ~128 instructions. Past that, more inlining costs compile
/// time and buys nothing — the previous 3000 made `lu run selfhost/interp.lu`
/// 2.1× slower for runtime performance within noise of this budget.
/// `LU_INLINE` overrides it.
const INLINE_BUDGET: usize = 256;

const RTOL: f64 = 9.094947017729282e-13; // 2^-40
const ATOL: f64 = 7.888609052210118e-31; // 2^-100

fn comps(p: &Program, t: &CType) -> Result<Vec<cranelift_codegen::ir::Type>, String> {
    Ok(layout_components(p, t)?
        .into_iter()
        .map(|component| match component {
            Component::F32 => types::F32,
            Component::F64 => types::F64,
            Component::I64 | Component::Ptr => types::I64,
            Component::F32x4 => types::F32X4,
            Component::F64x2 => types::F64X2,
            Component::I64x2 => types::I64X2,
        })
        .collect())
}

struct FnInfo {
    id: FuncId,
    params: Vec<CType>,
    ret: CType,
}

/// Where `Constant::Bytes` payloads live.
///
/// The JIT keeps its own boxed copies alive and bakes their addresses into the
/// generated code — optimized functions are temporary IR clones, so pointers
/// into them would dangle. An object file cannot bake in host addresses at all,
/// so it emits real data symbols and lets the linker resolve them.
enum Strings {
    Baked(Vec<Box<[u8]>>),
    Data {
        /// Interned by content: the same literal in two functions is one symbol.
        ids: HashMap<Vec<u8>, cranelift_module::DataId>,
    },
}

pub struct Jit<'a, M: Module> {
    p: &'a Program,
    module: M,
    opt_isa: cranelift_codegen::isa::OwnedTargetIsa,
    soa: bool,
    simd: bool,
    simd_bits: u16,
    ifconv: bool,
    do_licm: bool,
    inline_math: bool,
    inline_budget: usize,
    fns: HashMap<String, FnInfo>,
    externs: Vec<FnInfo>,
    imports: HashMap<&'static str, FuncId>,
    pure_imports: std::collections::HashSet<u32>,
    strings: Strings,
}

/// The pass switches shared by both Cranelift drivers, read once from the
/// environment so `lu run` and `lu build --fast` generate identical code.
struct Passes {
    soa: bool,
    simd: bool,
    ifconv: bool,
    do_licm: bool,
    inline_math: bool,
    /// Instructions the inliner may paste into one function before it stops.
    /// Every inlined instruction is one Cranelift compiles and one clang never
    /// sees, so this trades build latency against runtime directly.
    inline_budget: usize,
}

impl Passes {
    fn from_env() -> Passes {
        Passes {
            soa: std::env::var("LU_LAYOUT")
                .map(|value| value != "aos")
                .unwrap_or(true),
            simd: std::env::var("LU_SIMD")
                .map(|value| value != "off")
                .unwrap_or(true),
            ifconv: std::env::var("LU_IFCONV")
                .map(|value| value != "off")
                .unwrap_or(true),
            do_licm: std::env::var("LU_LICM")
                .map(|value| value != "off")
                .unwrap_or(true),
            inline_math: std::env::var("LU_MATH")
                .map(|value| value != "call")
                .unwrap_or(true),
            inline_budget: std::env::var("LU_INLINE")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(INLINE_BUDGET),
        }
    }
}

/// An ISA at `opt_level=none` for the module, plus one at `opt_level=speed` for
/// the per-function egraph pass we run ourselves.
///
/// The module ISA stays at `none` on purpose: we run the egraph optimizer
/// manually per function and then our own LICM over its output — letting
/// `define_function` re-run the egraph would re-elaborate instruction placement
/// and sink hoisted code back into loops.
fn isa_pair(
    pic: bool,
) -> Result<
    (
        cranelift_codegen::isa::OwnedTargetIsa,
        cranelift_codegen::isa::OwnedTargetIsa,
    ),
    String,
> {
    use cranelift_codegen::settings::Configurable as _;
    // Object output is linked into a position-independent executable, so it
    // must reference imports through the GOT rather than baking text
    // relocations the linker refuses.
    let mut flags = cranelift_codegen::settings::builder();
    let mut opt_flags = cranelift_codegen::settings::builder();
    if pic {
        flags.set("is_pic", "true").map_err(|e| e.to_string())?;
        opt_flags.set("is_pic", "true").map_err(|e| e.to_string())?;
    }
    let isa = cranelift_native::builder()
        .map_err(|e| e.to_string())?
        .finish(cranelift_codegen::settings::Flags::new(flags))
        .map_err(|e| e.to_string())?;
    opt_flags
        .set("opt_level", "speed")
        .map_err(|e| e.to_string())?;
    let opt_isa = cranelift_native::builder()
        .map_err(|e| e.to_string())?
        .finish(cranelift_codegen::settings::Flags::new(opt_flags))
        .map_err(|e| e.to_string())?;
    Ok((isa, opt_isa))
}

impl<'a> Jit<'a, JITModule> {
    pub fn run(ir: &'a LoweredProgram) -> Result<(), String> {
        let p = ir.source();
        let (isa, opt_isa) = isa_pair(false)?;
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
            ("lu_arr_share", runtime::lu_arr_share as *const u8),
            ("lu_arr_cow", runtime::lu_arr_cow as *const u8),
            ("lu_str_eq", runtime::lu_str_eq as *const u8),
            ("lu_str_copy", runtime::lu_str_copy as *const u8),
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
        for e in &p.externs {
            let pointer = crate::ffi::resolve(e.lib.as_deref(), &e.name)?;
            jb.symbol(&e.name, pointer as *const u8);
        }
        let module = JITModule::new(jb);
        let mut jit = Jit::new(p, module, opt_isa, Strings::Baked(Vec::new()));
        let main_id = jit.compile_program(ir)?;
        jit.module
            .finalize_definitions()
            .map_err(|e| e.to_string())?;
        let ptr = jit.module.get_finalized_function(main_id);
        let entry: extern "C" fn() = unsafe { std::mem::transmute(ptr) };
        entry();
        Ok(())
    }
}

impl<'a> Jit<'a, cranelift_object::ObjectModule> {
    /// Compile a program straight to a relocatable object, skipping LLVM.
    ///
    /// Code quality is Cranelift's rather than `clang -O3`'s, so this backs
    /// `lu build --fast` (the dev loop) while `lu build` keeps the LLVM tier and
    /// its measured numbers. Externs are left as undefined symbols for the
    /// linker instead of being resolved in-process the way the JIT does.
    pub fn emit_object(ir: &'a LoweredProgram, path: &std::path::Path) -> Result<(), String> {
        let p = ir.source();
        let (isa, opt_isa) = isa_pair(true)?;
        let builder = cranelift_object::ObjectBuilder::new(
            isa,
            "lu",
            cranelift_module::default_libcall_names(),
        )
        .map_err(|error| error.to_string())?;
        let mut jit = Jit::new(
            p,
            cranelift_object::ObjectModule::new(builder),
            opt_isa,
            Strings::Data {
                ids: HashMap::new(),
            },
        );
        let main_id = jit.compile_program(ir)?;
        jit.emit_entry(main_id)?;
        let mut product = jit.module.finish();
        stamp_platform(&mut product.object);
        let object = product.emit().map_err(|error| error.to_string())?;
        std::fs::write(path, object).map_err(|error| error.to_string())
    }

    /// `int lu_entry(void)` — the symbol `src/lu_runtime.c`'s `main` calls. The
    /// generated main returns nothing, so the wrapper calls it and reports
    /// success; a failing program traps or exits from inside the runtime.
    fn emit_entry(&mut self, main_id: FuncId) -> Result<(), String> {
        let mut sig = self.module.make_signature();
        sig.returns.push(AbiParam::new(types::I32));
        let entry = self
            .module
            .declare_function("lu_entry", Linkage::Export, &sig)
            .map_err(|error| error.to_string())?;
        let mut ctx = self.module.make_context();
        ctx.func.signature = sig;
        let mut fbc = FunctionBuilderContext::new();
        {
            let mut b = FunctionBuilder::new(&mut ctx.func, &mut fbc);
            let block = b.create_block();
            b.switch_to_block(block);
            let callee = self.module.declare_func_in_func(main_id, b.func);
            b.ins().call(callee, &[]);
            let zero = b.ins().iconst(types::I32, 0);
            b.ins().return_(&[zero]);
            b.seal_all_blocks();
            b.finalize();
        }
        self.module
            .define_function(entry, &mut ctx)
            .map_err(|error| error.to_string())?;
        self.module.clear_context(&mut ctx);
        Ok(())
    }
}

/// Record the target platform in the object.
///
/// Cranelift emits no `LC_BUILD_VERSION`, and `ld` warns on every link that it
/// is "assuming: macOS". The values only have to be plausible — the linker reads
/// them to pick deployment defaults, and the runtime object beside us carries
/// clang's real ones.
#[cfg(target_os = "macos")]
fn stamp_platform(object: &mut cranelift_object::object::write::Object<'_>) {
    use cranelift_object::object::write::MachOBuildVersion;
    let mut version = MachOBuildVersion::default();
    version.platform = cranelift_object::object::macho::PLATFORM_MACOS;
    version.minos = 11 << 16; // 11.0.0, encoded xxxx.yy.zz in nibbles
    version.sdk = 11 << 16;
    object.set_macho_build_version(version);
}

#[cfg(not(target_os = "macos"))]
fn stamp_platform(_object: &mut cranelift_object::object::write::Object<'_>) {}

/// `lu build --fast`: Cranelift object emission plus a link, no LLVM.
///
/// Returns the path of the executable. The generated code is the JIT's, so a
/// binary built this way runs like `lu run` without the startup compile — it is
/// the dev-loop build, not the one the benchmark numbers come from.
pub fn build_fast(
    ir: &LoweredProgram,
    source_path: &str,
    output_name: Option<&str>,
) -> Result<String, String> {
    let stem = std::path::Path::new(source_path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("out");
    let output = output_name.map(String::from).unwrap_or_else(|| stem.into());
    let object = std::env::temp_dir().join(format!("lu_{}_{}.o", stem, std::process::id()));
    Jit::emit_object(ir, &object)?;
    let runtime = crate::backend::link::runtime_object(false)?;
    let libraries = crate::backend::link::library_arguments(
        ir.externs
            .iter()
            .filter_map(|declaration| declaration.lib.as_deref()),
    );
    let result =
        crate::backend::link::link_executable(&output, &[object.clone(), runtime], &libraries);
    let _ = std::fs::remove_file(&object);
    result?;
    Ok(output)
}

impl<'a, M: Module> Jit<'a, M> {
    fn new(
        p: &'a Program,
        module: M,
        opt_isa: cranelift_codegen::isa::OwnedTargetIsa,
        strings: Strings,
    ) -> Jit<'a, M> {
        let passes = Passes::from_env();
        Jit {
            p,
            module,
            opt_isa,
            soa: passes.soa,
            simd: passes.simd,
            // Cranelift's native backends currently reject fixed vectors wider
            // than 128 bits; LLVM AOT widens independently when the host can.
            simd_bits: SIMD128,
            ifconv: passes.ifconv,
            do_licm: passes.do_licm,
            inline_math: passes.inline_math,
            inline_budget: passes.inline_budget,
            fns: HashMap::new(),
            externs: Vec::new(),
            imports: HashMap::new(),
            pure_imports: std::collections::HashSet::new(),
            strings,
        }
    }

    /// Declare everything, then compile every function and `main`. Returns the
    /// id of the compiled `main`.
    fn compile_program(&mut self, ir: &'a LoweredProgram) -> Result<FuncId, String> {
        self.declare_imports()?;
        self.declare_externs()?;
        self.declare_fns()?;
        let p = self.p;
        for (index, f) in p.fns.iter().enumerate() {
            let mut function =
                inline_calls(&ir.functions[index], &ir.functions, self.inline_budget);
            if self.ifconv {
                if_convert(&mut function);
            }
            self.compile_ir_fn(&function, f)?;
        }
        let mut main = inline_calls(
            ir.main.as_ref().ok_or("no `main` block in program")?,
            &ir.functions,
            self.inline_budget,
        );
        if self.ifconv {
            if_convert(&mut main);
        }
        self.compile_ir_main(&main)
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
            (
                "lu_arr_new_raw",
                1,
                &[types::I64, types::I64, types::I64, types::I64],
                false,
            ),
            ("lu_arr_share", 1, &[types::I64], false),
            ("lu_arr_cow", 1, &[types::I64], false),
            (
                "lu_str_eq",
                1,
                &[types::I64, types::I64, types::I64, types::I64],
                false,
            ),
            ("lu_str_copy", 1, &[types::I64, types::I64], false),
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

    fn declare_externs(&mut self) -> Result<(), String> {
        for e in &self.p.externs {
            let params = e
                .params
                .iter()
                .map(|(_, ty)| resolve_type(self.p, ty))
                .collect::<Result<Vec<_>, _>>()?;
            let ret = resolve_type(self.p, &e.ret)?;
            let mut sig = self.module.make_signature();
            for ty in &params {
                match ty {
                    CType::Arr(_) => {
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                    }
                    _ => {
                        for component in comps(self.p, ty)? {
                            sig.params.push(AbiParam::new(component));
                        }
                    }
                }
            }
            if ret == CType::Str {
                sig.params.push(AbiParam::new(types::I64));
                sig.returns.push(AbiParam::new(types::I64));
            } else {
                for component in comps(self.p, &ret)? {
                    sig.returns.push(AbiParam::new(component));
                }
            }
            let id = self
                .module
                .declare_function(&e.name, Linkage::Import, &sig)
                .map_err(|error| error.to_string())?;
            self.externs.push(FnInfo { id, params, ret });
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
                externs: &self.externs,
                imports: &self.imports,
                env: vec![HashMap::new()],
                refs: HashMap::new(),
                soa: self.soa,
                simd: self.simd,
                simd_bits: self.simd_bits,
                inline_math: self.inline_math,
                inout_outs: Vec::new(),
                cfg: &analysis,
                cfg_trusted: HashMap::new(),
                location: (0, 0),
                skipped_cfg_blocks: std::collections::HashSet::new(),
                strings: &mut self.strings,
            };
            g.declare_ir_locals(function)?;
            let mut cursor = 0;
            for &local in &function.params {
                let n = comps(g.p, &function.locals[local as usize].ty)?.len();
                let values = incoming[cursor..cursor + n].to_vec();
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
                externs: &self.externs,
                imports: &self.imports,
                env: vec![HashMap::new()],
                refs: HashMap::new(),
                soa: self.soa,
                simd: self.simd,
                simd_bits: self.simd_bits,
                inline_math: self.inline_math,
                inout_outs: Vec::new(),
                cfg: &analysis,
                cfg_trusted: HashMap::new(),
                location: (0, 0),
                skipped_cfg_blocks: std::collections::HashSet::new(),
                strings: &mut self.strings,
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
    module: &'a mut dyn Module,
    fns: &'a HashMap<String, FnInfo>,
    externs: &'a [FnInfo],
    imports: &'a HashMap<&'static str, FuncId>,
    env: Vec<HashMap<String, (CType, Vec<Variable>)>>,
    refs: HashMap<String, cranelift_codegen::ir::FuncRef>,
    // SoA record-array layout (the default; LU_LAYOUT=aos flips it)
    soa: bool,
    // `sum` vectorization (the default; LU_SIMD=off flips it)
    simd: bool,
    simd_bits: u16,
    inline_math: bool,
    // (out-pointer, component variables) of the current fn's inout params —
    // stored through the pointer at every outlined return
    inout_outs: Vec<(Value, Vec<Variable>)>,
    cfg: &'a CfgAnalysis,
    cfg_trusted: HashMap<(usize, ir::LocalId), Value>,
    location: (ir::BlockId, usize),
    skipped_cfg_blocks: std::collections::HashSet<ir::BlockId>,
    strings: &'a mut Strings,
}

impl<'a, 'b> Gen<'a, 'b> {
    fn simd_lanes(&self, scalar: SimdScalar) -> u16 {
        lane_count(self.simd_bits, scalar)
    }

    fn simd_vector_type(&self, scalar: SimdScalar) -> cranelift_codegen::ir::Type {
        let lane = match scalar {
            SimdScalar::F32 => types::F32,
            SimdScalar::F64 => types::F64,
            SimdScalar::I64 => types::I64,
        };
        lane.by(self.simd_lanes(scalar) as u32)
            .expect("supported fixed SIMD width")
    }

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
            (CType::CSlice(want), CType::Arr(got)) | (CType::CMutSlice(want), CType::Arr(got))
                if want == got =>
            {
                let length = self
                    .b
                    .ins()
                    .load(types::I64, MemFlags::trusted(), values[0], 0);
                vec![self.b.ins().iadd_imm(values[0], 16), length]
            }
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
            let CType::Arr(_) = ty else { continue };
            let base = self.b.use_var(vars[0]);
            let logical = self.b.ins().load(types::I64, MemFlags::trusted(), base, 0);
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
        let Some(plan) = simd_reduction_plan(function, self.cfg, loop_index, self.soa) else {
            return self.emit_cfg_simd_store(function, values, blocks, loop_index);
        };
        let (_, lower) = Self::ir_value(values, loop_info.lower)?;
        let (_, upper) = Self::ir_value(values, loop_info.upper)?;
        let index_var = self.b.declare_var(types::I64);
        self.b.def_var(index_var, lower[0]);
        let (scalar_type, vector_type) = match plan.scalar {
            SimdScalar::F32 => (types::F32, self.simd_vector_type(plan.scalar)),
            SimdScalar::F64 => (types::F64, self.simd_vector_type(plan.scalar)),
            SimdScalar::I64 => (types::I64, self.simd_vector_type(plan.scalar)),
        };
        let lanes = self.simd_lanes(plan.scalar) as i64;
        let batch_size = lanes * 4;
        let vector_accs: Vec<_> = (0..4).map(|_| self.b.declare_var(vector_type)).collect();
        let scalar_acc = self.b.declare_var(scalar_type);
        let zero = match plan.scalar {
            SimdScalar::F32 => self.b.ins().f32const(0.0),
            SimdScalar::F64 => self.b.ins().f64const(0.0),
            SimdScalar::I64 => self.b.ins().iconst(types::I64, 0),
        };
        let vector_zero = self.b.ins().splat(vector_type, zero);
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
        let after_batch = self.b.ins().iadd_imm(index, batch_size);
        let fits = self
            .b
            .ins()
            .icmp(IntCC::SignedLessThanOrEqual, after_batch, upper[0]);
        self.b.ins().brif(fits, vector_body, &[], scalar_head, &[]);

        self.b.switch_to_block(vector_body);
        let batch = self.b.use_var(index_var);
        for (lane, accumulator) in vector_accs.iter().enumerate() {
            let at = self.b.ins().iadd_imm(batch, (lane as i64) * lanes);
            let item = self.gen_simd_vector_expr(loop_index, plan.scalar, &plan.value, at)?;
            let current = self.b.use_var(*accumulator);
            let next = match plan.scalar {
                SimdScalar::F32 => self.b.ins().fadd(current, item),
                SimdScalar::F64 => self.b.ins().fadd(current, item),
                SimdScalar::I64 => self.b.ins().iadd(current, item),
            };
            self.b.def_var(*accumulator, next);
        }
        let next_batch = self.b.ins().iadd_imm(batch, batch_size);
        self.b.def_var(index_var, next_batch);
        self.b.ins().jump(vector_head, &[]);

        self.b.switch_to_block(scalar_head);
        let index = self.b.use_var(index_var);
        let more = self.b.ins().icmp(IntCC::SignedLessThan, index, upper[0]);
        self.b.ins().brif(more, scalar_body, &[], finish, &[]);

        self.b.switch_to_block(scalar_body);
        let at = self.b.use_var(index_var);
        let item = self.gen_simd_scalar_expr(loop_index, plan.scalar, &plan.value, at)?;
        let current = self.b.use_var(scalar_acc);
        let next = match plan.scalar {
            SimdScalar::F32 => self.b.ins().fadd(current, item),
            SimdScalar::F64 => self.b.ins().fadd(current, item),
            SimdScalar::I64 => self.b.ins().iadd(current, item),
        };
        self.b.def_var(scalar_acc, next);
        let next_index = self.b.ins().iadd_imm(at, 1);
        self.b.def_var(index_var, next_index);
        self.b.ins().jump(scalar_head, &[]);

        self.b.switch_to_block(finish);
        let a0 = self.b.use_var(vector_accs[0]);
        let a1 = self.b.use_var(vector_accs[1]);
        let a2 = self.b.use_var(vector_accs[2]);
        let a3 = self.b.use_var(vector_accs[3]);
        let pairs0 = match plan.scalar {
            SimdScalar::F32 => self.b.ins().fadd(a0, a1),
            SimdScalar::F64 => self.b.ins().fadd(a0, a1),
            SimdScalar::I64 => self.b.ins().iadd(a0, a1),
        };
        let pairs1 = match plan.scalar {
            SimdScalar::F32 => self.b.ins().fadd(a2, a3),
            SimdScalar::F64 => self.b.ins().fadd(a2, a3),
            SimdScalar::I64 => self.b.ins().iadd(a2, a3),
        };
        let vector_total = match plan.scalar {
            SimdScalar::F32 => self.b.ins().fadd(pairs0, pairs1),
            SimdScalar::F64 => self.b.ins().fadd(pairs0, pairs1),
            SimdScalar::I64 => self.b.ins().iadd(pairs0, pairs1),
        };
        let mut lane_total = self.b.ins().extractlane(vector_total, 0);
        for lane in 1..lanes {
            let value = self.b.ins().extractlane(vector_total, lane as u8);
            lane_total = match plan.scalar {
                SimdScalar::F32 | SimdScalar::F64 => self.b.ins().fadd(lane_total, value),
                SimdScalar::I64 => self.b.ins().iadd(lane_total, value),
            };
        }
        let scalar = self.b.use_var(scalar_acc);
        let total = match plan.scalar {
            SimdScalar::F32 | SimdScalar::F64 => self.b.ins().fadd(lane_total, scalar),
            SimdScalar::I64 => self.b.ins().iadd(lane_total, scalar),
        };
        self.define_ir_local(plan.accumulator, &[total])?;
        self.b.ins().jump(blocks[loop_info.exit as usize], &[]);
        self.skipped_cfg_blocks
            .extend(loop_info.blocks.iter().copied());
        Ok(true)
    }

    fn emit_cfg_simd_store(
        &mut self,
        function: &ir::Function,
        values: &[Option<(CType, Vec<Value>)>],
        blocks: &[cranelift_codegen::ir::Block],
        loop_index: usize,
    ) -> Result<bool, String> {
        let Some(plan) = simd_store_plan(function, self.cfg, loop_index, self.soa) else {
            return Ok(false);
        };
        let loop_info = &self.cfg.loops[loop_index];
        let (_, lower) = Self::ir_value(values, loop_info.lower)?;
        let (_, upper) = Self::ir_value(values, loop_info.upper)?;
        let (_, destination_vars) = self
            .lookup(&Self::ir_local(plan.destination))
            .ok_or("missing SIMD store destination")?;
        let base = self.b.use_var(destination_vars[0]);
        let unique = self.call_import("lu_arr_cow", &[base])[0];
        self.b.def_var(destination_vars[0], unique);

        let lanes = self.simd_lanes(plan.scalar) as i64;
        let width = if plan.scalar == SimdScalar::F32 { 4 } else { 8 };
        let index_var = self.b.declare_var(types::I64);
        self.b.def_var(index_var, lower[0]);
        let vector_head = self.b.create_block();
        let vector_body = self.b.create_block();
        let scalar_head = self.b.create_block();
        let scalar_body = self.b.create_block();
        let finish = self.b.create_block();
        self.b.ins().jump(vector_head, &[]);

        self.b.switch_to_block(vector_head);
        let index = self.b.use_var(index_var);
        let after_vector = self.b.ins().iadd_imm(index, lanes);
        let fits = self
            .b
            .ins()
            .icmp(IntCC::SignedLessThanOrEqual, after_vector, upper[0]);
        self.b.ins().brif(fits, vector_body, &[], scalar_head, &[]);

        self.b.switch_to_block(vector_body);
        let index = self.b.use_var(index_var);
        let value = self.gen_simd_vector_expr(loop_index, plan.scalar, &plan.value, index)?;
        let offset = self.b.ins().imul_imm(index, width);
        let data = self.b.ins().iadd_imm(unique, 16);
        let address = self.b.ins().iadd(data, offset);
        self.b.ins().store(MemFlags::trusted(), value, address, 0);
        let next = self.b.ins().iadd_imm(index, lanes);
        self.b.def_var(index_var, next);
        self.b.ins().jump(vector_head, &[]);

        self.b.switch_to_block(scalar_head);
        let index = self.b.use_var(index_var);
        let more = self.b.ins().icmp(IntCC::SignedLessThan, index, upper[0]);
        self.b.ins().brif(more, scalar_body, &[], finish, &[]);

        self.b.switch_to_block(scalar_body);
        let index = self.b.use_var(index_var);
        let value = self.gen_simd_scalar_expr(loop_index, plan.scalar, &plan.value, index)?;
        let offset = self.b.ins().imul_imm(index, width);
        let data = self.b.ins().iadd_imm(unique, 16);
        let address = self.b.ins().iadd(data, offset);
        self.b.ins().store(MemFlags::trusted(), value, address, 0);
        let next = self.b.ins().iadd_imm(index, 1);
        self.b.def_var(index_var, next);
        self.b.ins().jump(scalar_head, &[]);

        self.b.switch_to_block(finish);
        self.b.ins().jump(blocks[loop_info.exit as usize], &[]);
        self.skipped_cfg_blocks
            .extend(loop_info.blocks.iter().copied());
        Ok(true)
    }

    fn gen_simd_vector_expr(
        &mut self,
        loop_index: usize,
        scalar: SimdScalar,
        expr: &SimdExpr,
        index: Value,
    ) -> Result<Value, String> {
        Ok(match expr {
            SimdExpr::F32(value) => {
                let lane = self.b.ins().f32const(*value);
                let vector_type = self.simd_vector_type(scalar);
                self.b.ins().splat(vector_type, lane)
            }
            SimdExpr::F64(value) => {
                let lane = self.b.ins().f64const(*value);
                let vector_type = self.simd_vector_type(scalar);
                self.b.ins().splat(vector_type, lane)
            }
            SimdExpr::I64(value) => {
                let value = match scalar {
                    SimdScalar::F32 => self.b.ins().f32const(*value as f32),
                    SimdScalar::F64 => self.b.ins().f64const(*value as f64),
                    SimdScalar::I64 => self.b.ins().iconst(types::I64, *value),
                };
                let vector_type = self.simd_vector_type(scalar);
                self.b.ins().splat(vector_type, value)
            }
            SimdExpr::Invariant(local) => {
                let (_, vars) = self
                    .lookup(&Self::ir_local(*local))
                    .ok_or("missing vector invariant")?;
                let invariant = self.b.use_var(vars[0]);
                let vector_type = self.simd_vector_type(scalar);
                self.b.ins().splat(vector_type, invariant)
            }
            SimdExpr::Neg(value) => {
                let inner = self.gen_simd_vector_expr(loop_index, scalar, value, index)?;
                match scalar {
                    SimdScalar::F32 => self.b.ins().fneg(inner),
                    SimdScalar::F64 => self.b.ins().fneg(inner),
                    SimdScalar::I64 => self.b.ins().ineg(inner),
                }
            }
            SimdExpr::Binary { op, lhs, rhs } => {
                let lhs = self.gen_simd_vector_expr(loop_index, scalar, lhs, index)?;
                let rhs = self.gen_simd_vector_expr(loop_index, scalar, rhs, index)?;
                match (scalar, op) {
                    (SimdScalar::F32, BinaryOp::Add) => self.b.ins().fadd(lhs, rhs),
                    (SimdScalar::F32, BinaryOp::Sub) => self.b.ins().fsub(lhs, rhs),
                    (SimdScalar::F32, BinaryOp::Mul) => self.b.ins().fmul(lhs, rhs),
                    (SimdScalar::F32, BinaryOp::Div) => self.b.ins().fdiv(lhs, rhs),
                    (SimdScalar::F64, BinaryOp::Add) => self.b.ins().fadd(lhs, rhs),
                    (SimdScalar::F64, BinaryOp::Sub) => self.b.ins().fsub(lhs, rhs),
                    (SimdScalar::F64, BinaryOp::Mul) => self.b.ins().fmul(lhs, rhs),
                    (SimdScalar::F64, BinaryOp::Div) => self.b.ins().fdiv(lhs, rhs),
                    (SimdScalar::I64, BinaryOp::Add) => self.b.ins().iadd(lhs, rhs),
                    (SimdScalar::I64, BinaryOp::Sub) => self.b.ins().isub(lhs, rhs),
                    (SimdScalar::I64, BinaryOp::Mul) => self.b.ins().imul(lhs, rhs),
                    _ => return Err("unsupported vector binary".into()),
                }
            }
            SimdExpr::Array { local } => {
                let (_, vars) = self
                    .lookup(&Self::ir_local(*local))
                    .ok_or("missing vector array")?;
                let base = self.b.use_var(vars[0]);
                let width = if scalar == SimdScalar::F32 { 4 } else { 8 };
                let bytes = self.b.ins().imul_imm(index, width);
                let data = self.b.ins().iadd_imm(base, 16);
                let address = self.b.ins().iadd(data, bytes);
                let vector_type = self.simd_vector_type(scalar);
                self.b
                    .ins()
                    .load(vector_type, MemFlags::trusted(), address, 0)
            }
            SimdExpr::Field {
                local,
                record,
                field,
            } => {
                let (_, vars) = self
                    .lookup(&Self::ir_local(*local))
                    .ok_or("missing field array")?;
                let base = self.b.use_var(vars[0]);
                let field_name = &self.p.types[*record].fields[*field].0;
                let (component, _) = field_offset(self.p, *record, field_name)?;
                let logical = self.cfg_trusted[&(loop_index, *local)];
                let address =
                    self.soa_component_address(base, index, logical, *record, component)?;
                let vector_type = self.simd_vector_type(scalar);
                self.b
                    .ins()
                    .load(vector_type, MemFlags::trusted(), address, 0)
            }
            SimdExpr::Builtin { name, args } => {
                let args = args
                    .iter()
                    .map(|value| self.gen_simd_vector_expr(loop_index, scalar, value, index))
                    .collect::<Result<Vec<_>, _>>()?;
                match name.as_str() {
                    "sqrt" => self.b.ins().sqrt(args[0]),
                    "abs" => self.b.ins().fabs(args[0]),
                    "min" => self.b.ins().fmin(args[0], args[1]),
                    "max" => self.b.ins().fmax(args[0], args[1]),
                    _ => return Err("unsupported vector builtin".into()),
                }
            }
        })
    }

    fn gen_simd_scalar_expr(
        &mut self,
        loop_index: usize,
        scalar: SimdScalar,
        expr: &SimdExpr,
        index: Value,
    ) -> Result<Value, String> {
        Ok(match expr {
            SimdExpr::F32(value) => self.b.ins().f32const(*value),
            SimdExpr::F64(value) => self.b.ins().f64const(*value),
            SimdExpr::I64(value) => match scalar {
                SimdScalar::F32 => self.b.ins().f32const(*value as f32),
                SimdScalar::F64 => self.b.ins().f64const(*value as f64),
                SimdScalar::I64 => self.b.ins().iconst(types::I64, *value),
            },
            SimdExpr::Invariant(local) => {
                let (_, vars) = self
                    .lookup(&Self::ir_local(*local))
                    .ok_or("missing scalar invariant")?;
                self.b.use_var(vars[0])
            }
            SimdExpr::Neg(value) => {
                let inner = self.gen_simd_scalar_expr(loop_index, scalar, value, index)?;
                match scalar {
                    SimdScalar::F32 => self.b.ins().fneg(inner),
                    SimdScalar::F64 => self.b.ins().fneg(inner),
                    SimdScalar::I64 => self.b.ins().ineg(inner),
                }
            }
            SimdExpr::Binary { op, lhs, rhs } => {
                let lhs = self.gen_simd_scalar_expr(loop_index, scalar, lhs, index)?;
                let rhs = self.gen_simd_scalar_expr(loop_index, scalar, rhs, index)?;
                match (scalar, op) {
                    (SimdScalar::F32, BinaryOp::Add) => self.b.ins().fadd(lhs, rhs),
                    (SimdScalar::F32, BinaryOp::Sub) => self.b.ins().fsub(lhs, rhs),
                    (SimdScalar::F32, BinaryOp::Mul) => self.b.ins().fmul(lhs, rhs),
                    (SimdScalar::F32, BinaryOp::Div) => self.b.ins().fdiv(lhs, rhs),
                    (SimdScalar::F64, BinaryOp::Add) => self.b.ins().fadd(lhs, rhs),
                    (SimdScalar::F64, BinaryOp::Sub) => self.b.ins().fsub(lhs, rhs),
                    (SimdScalar::F64, BinaryOp::Mul) => self.b.ins().fmul(lhs, rhs),
                    (SimdScalar::F64, BinaryOp::Div) => self.b.ins().fdiv(lhs, rhs),
                    (SimdScalar::I64, BinaryOp::Add) => self.b.ins().iadd(lhs, rhs),
                    (SimdScalar::I64, BinaryOp::Sub) => self.b.ins().isub(lhs, rhs),
                    (SimdScalar::I64, BinaryOp::Mul) => self.b.ins().imul(lhs, rhs),
                    _ => return Err("unsupported scalar binary".into()),
                }
            }
            SimdExpr::Array { local } => {
                let (_, vars) = self
                    .lookup(&Self::ir_local(*local))
                    .ok_or("missing scalar array")?;
                let base = self.b.use_var(vars[0]);
                let width = if scalar == SimdScalar::F32 { 4 } else { 8 };
                let address = self.b.ins().imul_imm(index, width);
                let data = self.b.ins().iadd_imm(base, 16);
                let address = self.b.ins().iadd(data, address);
                self.b.ins().load(
                    match scalar {
                        SimdScalar::F32 => types::F32,
                        SimdScalar::F64 => types::F64,
                        SimdScalar::I64 => types::I64,
                    },
                    MemFlags::trusted(),
                    address,
                    0,
                )
            }
            SimdExpr::Field {
                local,
                record,
                field,
            } => {
                let (_, vars) = self
                    .lookup(&Self::ir_local(*local))
                    .ok_or("missing scalar field array")?;
                let base = self.b.use_var(vars[0]);
                let field_name = &self.p.types[*record].fields[*field].0;
                let (component, _) = field_offset(self.p, *record, field_name)?;
                let logical = self.cfg_trusted[&(loop_index, *local)];
                let address =
                    self.soa_component_address(base, index, logical, *record, component)?;
                self.b.ins().load(
                    match scalar {
                        SimdScalar::F32 => types::F32,
                        SimdScalar::F64 => types::F64,
                        SimdScalar::I64 => types::I64,
                    },
                    MemFlags::trusted(),
                    address,
                    0,
                )
            }
            SimdExpr::Builtin { name, args } => {
                let args = args
                    .iter()
                    .map(|value| self.gen_simd_scalar_expr(loop_index, scalar, value, index))
                    .collect::<Result<Vec<_>, _>>()?;
                match name.as_str() {
                    "sqrt" => self.b.ins().sqrt(args[0]),
                    "abs" => self.b.ins().fabs(args[0]),
                    "min" => self.b.ins().fmin(args[0], args[1]),
                    "max" => self.b.ins().fmax(args[0], args[1]),
                    _ => return Err("unsupported scalar builtin".into()),
                }
            }
        })
    }

    /// Address of a string literal's bytes.
    ///
    /// In the JIT that is a constant: the pool owns a copy that outlives
    /// execution, so its address can be baked in. In an object file the bytes
    /// become a data symbol and the address is a relocation the linker fills.
    fn string_pointer(&mut self, bytes: &[u8]) -> Result<Value, String> {
        match self.strings {
            Strings::Baked(pool) => {
                let owned = bytes.to_vec().into_boxed_slice();
                let pointer = owned.as_ptr();
                pool.push(owned);
                Ok(self.b.ins().iconst(types::I64, pointer as i64))
            }
            Strings::Data { ids } => {
                let next = ids.len();
                let id = match ids.get(bytes) {
                    Some(id) => *id,
                    None => {
                        let id = self
                            .module
                            .declare_data(&format!(".Lstr.{}", next), Linkage::Local, false, false)
                            .map_err(|error| error.to_string())?;
                        let mut description = cranelift_module::DataDescription::new();
                        description.define(bytes.to_vec().into_boxed_slice());
                        self.module
                            .define_data(id, &description)
                            .map_err(|error| error.to_string())?;
                        ids.insert(bytes.to_vec(), id);
                        id
                    }
                };
                let global = self.module.declare_data_in_func(id, self.b.func);
                Ok(self.b.ins().global_value(types::I64, global))
            }
        }
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
                Constant::Bytes(bytes) => {
                    let len = bytes.len();
                    let pointer = self.string_pointer(bytes)?;
                    (
                        CType::Str,
                        vec![pointer, self.b.ins().iconst(types::I64, len as i64)],
                    )
                }
                Constant::Unit => (CType::Unit, vec![]),
            }),
            InstKind::Load(local) => {
                let (ty, vars) = self
                    .lookup(&Self::ir_local(*local))
                    .ok_or("invalid IR local")?;
                Some((ty, vars.iter().map(|v| self.b.use_var(*v)).collect()))
            }
            InstKind::Store {
                local,
                value: id,
                retain_arrays,
            } => {
                let (got, vals) = value(*id)?;
                let want = &function.locals[*local as usize].ty;
                let mut vals = self.coerce(want, &got, vals)?;
                if *retain_arrays {
                    for offset in array_component_offsets(self.p, want)? {
                        vals[offset] = self.call_import("lu_arr_share", &[vals[offset]])[0];
                    }
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
                    Callee::Extern(id) => self.gen_ir_extern_call(*id, args, inout)?,
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
                root,
                path,
                base,
                index,
                value: stored,
            } => {
                let base_id = *base;
                let (base_ty, base) = value(*base)?;
                let (_, index) = value(*index)?;
                let (stored_ty, stored) = value(*stored)?;
                if let CType::CMutSlice(elem) = &base_ty {
                    self.check_idx(index[0], base[1]);
                    let components = comps(self.p, elem)?;
                    if components.len() != 1 {
                        return Err("c_mut_slice elements must have one ABI component".into());
                    }
                    let stored = self.coerce(elem, &stored_ty, stored)?;
                    let offset = self
                        .b
                        .ins()
                        .imul_imm(index[0], components[0].bytes() as i64);
                    let address = self.b.ins().iadd(base[0], offset);
                    self.b
                        .ins()
                        .store(MemFlags::trusted(), stored[0], address, 0);
                    return Ok(None);
                }
                let CType::Arr(elem) = base_ty else {
                    return Err("IR set-index on non-mutable array view".into());
                };
                let trusted =
                    self.cfg
                        .trusted_accesses
                        .get(&self.location)
                        .and_then(|loop_index| {
                            let array = array_local_for_value(function, base_id)?;
                            self.cfg_trusted.get(&(*loop_index, array)).copied()
                        });
                let unique = self.call_import("lu_arr_cow", &[base[0]])[0];
                let (mut current, vars) = self
                    .lookup(&Self::ir_local(*root))
                    .ok_or("invalid indexed root")?;
                let mut offset = 0;
                for &field in path {
                    let CType::Rec(record) = current else {
                        return Err("indexed path crosses non-record".into());
                    };
                    let name = &self.p.types[record].fields[field].0;
                    let (add, next) = field_offset(self.p, record, name)?;
                    offset += add;
                    current = next;
                }
                if !matches!(current, CType::Arr(_)) {
                    return Err("indexed path does not end at an array".into());
                }
                self.b.def_var(vars[offset], unique);
                let addrs = self.elem_addrs(unique, index[0], &elem, trusted)?;
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
        let components = layout_components(self.p, elem)?;
        let logical = match trusted {
            Some(n) => n,
            None => {
                let logical = self.b.ins().load(types::I64, MemFlags::trusted(), base, 0);
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
        let data = self.b.ins().iadd_imm(base, 16);
        if components.len() == 1 {
            let off = self.b.ins().imul_imm(idx, components[0].bytes() as i64);
            return Ok(vec![self.b.ins().iadd(data, off)]);
        }
        if self.soa {
            let mut plane = self.b.ins().iconst(types::I64, 0);
            let mut out = Vec::new();
            for component in components {
                let lane = self.b.ins().imul_imm(idx, component.bytes() as i64);
                let start = self.b.ins().iadd(data, plane);
                out.push(self.b.ins().iadd(start, lane));
                let raw = self.b.ins().imul_imm(logical, component.bytes() as i64);
                let padded = self.b.ins().iadd_imm(raw, 7);
                let span = self.b.ins().band_imm(padded, -8);
                plane = self.b.ins().iadd(plane, span);
            }
            Ok(out)
        } else {
            let off = self.b.ins().imul_imm(idx, (components.len() * 8) as i64);
            let first = self.b.ins().iadd(data, off);
            Ok((0..components.len())
                .map(|component| self.b.ins().iadd_imm(first, (component * 8) as i64))
                .collect())
        }
    }

    fn alloc_array_raw(&mut self, logical: Value, elem: &CType) -> Result<Value, String> {
        let components = layout_components(self.p, elem)?;
        let f32_components = components
            .iter()
            .filter(|component| **component == Component::F32)
            .count() as i64;
        let wide_components = components.len() as i64 - f32_components;
        let f32_components = self.b.ins().iconst(types::I64, f32_components);
        let wide_components = self.b.ins().iconst(types::I64, wide_components);
        let soa = self.b.ins().iconst(types::I64, self.soa as i64);
        Ok(self.call_import(
            "lu_arr_new_raw",
            &[logical, f32_components, wide_components, soa],
        )[0])
    }

    fn soa_component_address(
        &mut self,
        base: Value,
        index: Value,
        logical: Value,
        record: usize,
        component_index: usize,
    ) -> Result<Value, String> {
        let components = layout_components(self.p, &CType::Rec(record))?;
        let data = self.b.ins().iadd_imm(base, 16);
        let mut plane = self.b.ins().iconst(types::I64, 0);
        for component in components.iter().take(component_index) {
            let raw = self.b.ins().imul_imm(logical, component.bytes() as i64);
            let padded = self.b.ins().iadd_imm(raw, 7);
            let span = self.b.ins().band_imm(padded, -8);
            plane = self.b.ins().iadd(plane, span);
        }
        let component = components
            .get(component_index)
            .ok_or("invalid SIMD record component")?;
        let lane = self.b.ins().imul_imm(index, component.bytes() as i64);
        let start = self.b.ins().iadd(data, plane);
        Ok(self.b.ins().iadd(start, lane))
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
        let both_int = matches!(
            lt,
            CType::I64 | CType::Bool | CType::Enum(_) | CType::CPtr(_)
        ) && matches!(
            rt,
            CType::I64 | CType::Bool | CType::Enum(_) | CType::CPtr(_)
        );
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
        if let CType::CSlice(elem) | CType::CMutSlice(elem) = base_ty {
            self.check_idx(index, base[1]);
            let components = comps(self.p, &elem)?;
            if components.len() != 1 {
                return Err("borrowed C slice elements must have one ABI component".into());
            }
            let offset = self.b.ins().imul_imm(index, components[0].bytes() as i64);
            let address = self.b.ins().iadd(base[0], offset);
            return Ok((
                *elem,
                vec![self
                    .b
                    .ins()
                    .load(components[0], MemFlags::trusted(), address, 0)],
            ));
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
        let base = self.alloc_array_raw(logical, elem)?;
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
        for (index, ((got, mut vals), want)) in args.into_iter().zip(&params).enumerate() {
            if matches!(want, CType::CMutSlice(_)) && matches!(got, CType::Arr(_)) {
                let unique = self.call_import("lu_arr_cow", &[vals[0]])[0];
                let target = inout[index].ok_or("missing c_mut_slice borrow target")?;
                self.define_ir_local(target, &[unique])?;
                vals[0] = unique;
            }
            let mut values = self.coerce(want, &got, vals)?;
            if decl.exported && matches!(want, CType::Arr(_)) {
                values[0] = self.call_import("lu_arr_share", &[values[0]])[0];
            }
            flat.extend(values);
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

    fn gen_ir_extern_call(
        &mut self,
        id: ir::ExternId,
        args: Vec<(CType, Vec<Value>)>,
        inout: &[Option<ir::LocalId>],
    ) -> Result<(CType, Vec<Value>), String> {
        let info = &self.externs[id as usize];
        let params = info.params.clone();
        let ret = info.ret.clone();
        let mut flat = Vec::new();
        for (index, ((got, mut values), want)) in args.into_iter().zip(&params).enumerate() {
            if matches!(want, CType::CMutSlice(_)) && matches!(got, CType::Arr(_)) {
                let unique = self.call_import("lu_arr_cow", &[values[0]])[0];
                let target = inout[index].ok_or("missing c_mut_slice borrow target")?;
                self.define_ir_local(target, &[unique])?;
                values[0] = unique;
            }
            let values = self.coerce(want, &got, values)?;
            match want {
                CType::Arr(_) => {
                    let handle = values[0];
                    let length = self
                        .b
                        .ins()
                        .load(types::I64, MemFlags::trusted(), handle, 0);
                    flat.push(self.b.ins().iadd_imm(handle, 16));
                    flat.push(length);
                }
                _ => flat.extend(values),
            }
        }
        if ret == CType::Str {
            use cranelift_codegen::ir::{StackSlotData, StackSlotKind};
            let length_slot = self.b.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                8,
                3,
            ));
            let length_pointer = self.b.ins().stack_addr(types::I64, length_slot, 0);
            flat.push(length_pointer);
            let callee = self.module.declare_func_in_func(info.id, self.b.func);
            let call = self.b.ins().call(callee, &flat);
            let pointer = self.b.inst_results(call)[0];
            let length = self.b.ins().stack_load(types::I64, length_slot, 0);
            let copy = self.call_import("lu_str_copy", &[pointer, length])[0];
            return Ok((ret, vec![copy, length]));
        }
        let callee = self.module.declare_func_in_func(info.id, self.b.func);
        let call = self.b.ins().call(callee, &flat);
        Ok((ret, self.b.inst_results(call).to_vec()))
    }

    /// The i64 constant `v` was defined by, or `None` if it is not a literal.
    fn const_i64(&self, v: Value) -> Option<i64> {
        match self.b.func.dfg.value_def(v) {
            ValueDef::Result(inst, _) => match self.b.func.dfg.insts[inst] {
                InstructionData::UnaryImm {
                    opcode: Opcode::Iconst,
                    imm,
                } => Some(imm.bits()),
                _ => None,
            },
            _ => None,
        }
    }

    fn checked_int_div(&mut self, lhs: Value, rhs: Value, remainder: bool) -> Value {
        // See the matching comment in `emit_checked_int_div` (src/llvm.rs): a
        // literal divisor that is neither 0 nor -1 can trip neither of the
        // SPEC 3.1 traps, so the runtime helper is pure overhead there.
        if self.const_i64(rhs).is_some_and(|d| d != 0 && d != -1) {
            return if remainder {
                self.b.ins().srem(lhs, rhs)
            } else {
                self.b.ins().sdiv(lhs, rhs)
            };
        }
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
            "f32x4" | "f64x2" | "i64x2" => {
                let (ty, lane_ty) = match name {
                    "f32x4" => (CType::F32x4, CType::F32),
                    "f64x2" => (CType::F64x2, CType::F64),
                    _ => (CType::I64x2, CType::I64),
                };
                let vector_type = comps(self.p, &ty)?[0];
                let mut vector = {
                    let lane = self.coerce(&lane_ty, &atys[0], avals[0].clone())?[0];
                    self.b.ins().splat(vector_type, lane)
                };
                for lane in 1..avals.len() {
                    let value = self.coerce(&lane_ty, &atys[lane], avals[lane].clone())?[0];
                    vector = self.b.ins().insertlane(vector, value, lane as u8);
                }
                Ok((ty, vec![vector]))
            }
            "f32x4_splat" | "f64x2_splat" | "i64x2_splat" => {
                let (ty, lane_ty) = match name {
                    "f32x4_splat" => (CType::F32x4, CType::F32),
                    "f64x2_splat" => (CType::F64x2, CType::F64),
                    _ => (CType::I64x2, CType::I64),
                };
                let lane = self.coerce(&lane_ty, &atys[0], avals[0].clone())?[0];
                let vector_type = comps(self.p, &ty)?[0];
                Ok((ty, vec![self.b.ins().splat(vector_type, lane)]))
            }
            "f32x4_add" | "f32x4_sub" | "f32x4_mul" | "f32x4_div" | "f64x2_add" | "f64x2_sub"
            | "f64x2_mul" | "f64x2_div" | "i64x2_add" | "i64x2_sub" | "i64x2_mul" => {
                let result = match name.rsplit('_').next().unwrap() {
                    "add" if name.starts_with('i') => self.b.ins().iadd(avals[0][0], avals[1][0]),
                    "sub" if name.starts_with('i') => self.b.ins().isub(avals[0][0], avals[1][0]),
                    "mul" if name.starts_with('i') => self.b.ins().imul(avals[0][0], avals[1][0]),
                    "add" => self.b.ins().fadd(avals[0][0], avals[1][0]),
                    "sub" => self.b.ins().fsub(avals[0][0], avals[1][0]),
                    "mul" => self.b.ins().fmul(avals[0][0], avals[1][0]),
                    _ => self.b.ins().fdiv(avals[0][0], avals[1][0]),
                };
                Ok((atys[0].clone(), vec![result]))
            }
            "i64x2_div" => {
                let mut lanes = Vec::new();
                for lane in 0..2 {
                    let lhs = self.b.ins().extractlane(avals[0][0], lane);
                    let rhs = self.b.ins().extractlane(avals[1][0], lane);
                    lanes.push(self.call_import("lu_i64_div", &[lhs, rhs])[0]);
                }
                let mut vector = self.b.ins().splat(types::I64X2, lanes[0]);
                vector = self.b.ins().insertlane(vector, lanes[1], 1);
                Ok((CType::I64x2, vec![vector]))
            }
            "f32x4_sum" | "f64x2_sum" | "i64x2_sum" => {
                let lanes = if name.starts_with("f32") { 4 } else { 2 };
                let mut total = self.b.ins().extractlane(avals[0][0], 0);
                for lane in 1..lanes {
                    let value = self.b.ins().extractlane(avals[0][0], lane as u8);
                    total = if name.starts_with('i') {
                        self.b.ins().iadd(total, value)
                    } else {
                        self.b.ins().fadd(total, value)
                    };
                }
                let ty = match name {
                    "f32x4_sum" => CType::F32,
                    "f64x2_sum" => CType::F64,
                    _ => CType::I64,
                };
                Ok((ty, vec![total]))
            }
            "f32x4_extract" | "f64x2_extract" | "i64x2_extract" => {
                use cranelift_codegen::ir::{StackSlotData, StackSlotKind};
                let (lanes, lane_type, ty, align_log2) = match name {
                    "f32x4_extract" => (4, types::F32, CType::F32, 2),
                    "f64x2_extract" => (2, types::F64, CType::F64, 3),
                    _ => (2, types::I64, CType::I64, 3),
                };
                let length = self.b.ins().iconst(types::I64, lanes);
                self.check_idx(avals[1][0], length);
                let slot = self.b.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    16,
                    4,
                ));
                self.b.ins().stack_store(avals[0][0], slot, 0);
                let base = self.b.ins().stack_addr(types::I64, slot, 0);
                let offset = self.b.ins().imul_imm(avals[1][0], 1_i64 << align_log2);
                let address = self.b.ins().iadd(base, offset);
                let value = self
                    .b
                    .ins()
                    .load(lane_type, MemFlags::trusted(), address, 0);
                Ok((ty, vec![value]))
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
                match &atys[0] {
                    CType::Arr(_) => {}
                    CType::CSlice(_) | CType::CMutSlice(_) => {
                        return Ok((CType::I64, vec![avals[0][1]]));
                    }
                    _ => return Err("`len` expects array".into()),
                }
                let n = self
                    .b
                    .ins()
                    .load(types::I64, MemFlags::trusted(), avals[0][0], 0);
                Ok((CType::I64, vec![n]))
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
                        let base = self.alloc_array_raw(n, &elem)?;
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
