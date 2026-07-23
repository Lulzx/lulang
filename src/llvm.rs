// LLVM AOT backend for `lu build`: emit textual LLVM IR, hand it to clang.
//
// Language semantics become IR facts:
//  - every FP op carries the `fast` flag (approximate-FP-by-contract) — LLVM may
//    reassociate, contract, and vectorize reductions;
//  - math functions are declared `memory(none)`, so LICM hoists loop-invariant
//    calls (the win the JIT tier can't get from Cranelift);
//  - CFG loop analysis hoists range bounds checks, leaving check-free hot loops
//    for the vectorizer.
use crate::ast::{FnDecl, Program};
use crate::backend::abi::return_components;
use crate::backend::layout::{
    array_component_offsets, components as layout_components, field_offset, Component,
};
use crate::backend::optimization::{analyze_cfg, simd_reduction_plan, CfgAnalysis, SimdExpr};
use crate::check::{resolve_type, Type as CType};
use crate::ir::{self, BinaryOp, Callee, Constant, InstKind, LoweredProgram, Terminator, UnaryOp};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

const RTOL: f64 = 9.094947017729282e-13; // 2^-40
const ATOL: f64 = 7.888609052210118e-31; // 2^-100

fn lty(p: &Program, t: &CType) -> Result<Vec<&'static str>, String> {
    Ok(layout_components(p, t)?
        .into_iter()
        .map(llvm_component)
        .collect())
}

/// ABI return components of a function: declared return + inout param comps.
fn abi_ret_comps<'x>(p: &Program, f: &FnDecl) -> Result<Vec<&'x str>, String> {
    Ok(return_components(p, f)?
        .into_iter()
        .map(llvm_component)
        .collect())
}

fn llvm_component(component: Component) -> &'static str {
    match component {
        Component::I64 => "i64",
        Component::F32 => "float",
        Component::F64 => "double",
        Component::Ptr => "ptr",
    }
}

fn comps_ty(c: &[&str]) -> String {
    match c.len() {
        0 => "void".into(),
        1 => c[0].into(),
        _ => format!("{{ {} }}", c.join(", ")),
    }
}

fn internal_symbol(decl: &FnDecl) -> String {
    if decl.exported {
        format!("__lu_internal_{}", decl.name)
    } else {
        decl.name.clone()
    }
}

fn emit_export_wrapper(
    program: &Program,
    function: &ir::Function,
    decl: &FnDecl,
) -> Result<String, String> {
    let mut params = Vec::new();
    let mut internal_args = Vec::new();
    let mut arrays = Vec::new();
    let mut records = Vec::new();
    let mut argument = 0usize;
    for &local_id in &function.params {
        let local = &function.locals[local_id as usize];
        match &local.ty {
            CType::Arr(element) => {
                let element_components = lty(program, element)?;
                if element_components.len() != 1 {
                    return Err(format!(
                        "export `{}` array elements must have one ABI component",
                        decl.name
                    ));
                }
                params.push(format!("ptr %c{}", argument));
                let data = format!("%c{}", argument);
                argument += 1;
                params.push(format!("i64 %c{}", argument));
                let len = format!("%c{}", argument);
                argument += 1;
                let handle = format!("%wa{}_handle", arrays.len());
                internal_args.push(format!("ptr {}", handle));
                arrays.push((data, len, handle, element_components[0]));
            }
            CType::Rec(index) if program.types[*index].c_layout => {
                let record_components = lty(program, &local.ty)?;
                let aggregate = comps_ty(&record_components);
                let source = format!("%c{}", argument);
                params.push(format!("{} {}", aggregate, source));
                let record_number = records.len();
                for (component_index, component) in record_components.iter().enumerate() {
                    internal_args.push(format!(
                        "{} %wr{}_{}",
                        component, record_number, component_index
                    ));
                }
                records.push((source, aggregate, record_components));
                argument += 1;
            }
            ty => {
                for component in lty(program, ty)? {
                    params.push(format!("{} %c{}", component, argument));
                    internal_args.push(format!("{} %c{}", component, argument));
                    argument += 1;
                }
            }
        }
    }

    let ret_components = lty(program, &function.ret)?;
    let internal_ret_type = comps_ty(&ret_components);
    let string_length_out = if function.ret == CType::Str {
        let name = format!("%c{}", argument);
        params.push(format!("ptr {}", name));
        Some(name)
    } else {
        None
    };
    let wrapper_ret_type = if string_length_out.is_some() {
        "ptr".into()
    } else {
        internal_ret_type.clone()
    };
    let mut out = format!(
        "define dso_local {} @\"{}\"({}) {{\nentry:\n",
        wrapper_ret_type,
        decl.name,
        params.join(", ")
    );
    for (record_index, (source, aggregate, components)) in records.iter().enumerate() {
        for component_index in 0..components.len() {
            let _ = writeln!(
                out,
                "  %wr{record_index}_{component_index} = extractvalue {aggregate} {source}, {component_index}"
            );
        }
    }
    for (index, (source, len, handle, component)) in arrays.iter().enumerate() {
        let _ = writeln!(
            out,
            "  {handle} = call ptr @lu_arr_new_raw(i64 {len}, i64 1)\n\
             \x20 %wa{index}_data = getelementptr i8, ptr {handle}, i64 8\n\
             \x20 %wa{index}_in_idx = alloca i64\n\
             \x20 store i64 0, ptr %wa{index}_in_idx\n\
             \x20 br label %wa{index}_in_cond\n\
             wa{index}_in_cond:\n\
             \x20 %wa{index}_in_i = load i64, ptr %wa{index}_in_idx\n\
             \x20 %wa{index}_in_more = icmp slt i64 %wa{index}_in_i, {len}\n\
             \x20 br i1 %wa{index}_in_more, label %wa{index}_in_body, label %wa{index}_in_done\n\
             wa{index}_in_body:\n\
             \x20 %wa{index}_src = getelementptr {component}, ptr {source}, i64 %wa{index}_in_i\n\
             \x20 %wa{index}_value = load {component}, ptr %wa{index}_src\n\
             \x20 %wa{index}_dst = getelementptr {component}, ptr %wa{index}_data, i64 %wa{index}_in_i\n\
             \x20 store {component} %wa{index}_value, ptr %wa{index}_dst\n\
             \x20 %wa{index}_in_next = add i64 %wa{index}_in_i, 1\n\
             \x20 store i64 %wa{index}_in_next, ptr %wa{index}_in_idx\n\
             \x20 br label %wa{index}_in_cond\n\
             wa{index}_in_done:"
        );
    }
    if ret_components.is_empty() {
        let _ = writeln!(
            out,
            "  call void @\"{}\"({})",
            internal_symbol(decl),
            internal_args.join(", ")
        );
    } else {
        let _ = writeln!(
            out,
            "  %wrapper_result = call {} @\"{}\"({})",
            internal_ret_type,
            internal_symbol(decl),
            internal_args.join(", ")
        );
    }
    for (index, (destination, len, handle, component)) in arrays.iter().enumerate() {
        let _ = writeln!(
            out,
            "  %wa{index}_out_data = getelementptr i8, ptr {handle}, i64 8\n\
             \x20 %wa{index}_out_idx = alloca i64\n\
             \x20 store i64 0, ptr %wa{index}_out_idx\n\
             \x20 br label %wa{index}_out_cond\n\
             wa{index}_out_cond:\n\
             \x20 %wa{index}_out_i = load i64, ptr %wa{index}_out_idx\n\
             \x20 %wa{index}_out_more = icmp slt i64 %wa{index}_out_i, {len}\n\
             \x20 br i1 %wa{index}_out_more, label %wa{index}_out_body, label %wa{index}_out_done\n\
             wa{index}_out_body:\n\
             \x20 %wa{index}_out_src = getelementptr {component}, ptr %wa{index}_out_data, i64 %wa{index}_out_i\n\
             \x20 %wa{index}_out_value = load {component}, ptr %wa{index}_out_src\n\
             \x20 %wa{index}_out_dst = getelementptr {component}, ptr {destination}, i64 %wa{index}_out_i\n\
             \x20 store {component} %wa{index}_out_value, ptr %wa{index}_out_dst\n\
             \x20 %wa{index}_out_next = add i64 %wa{index}_out_i, 1\n\
             \x20 store i64 %wa{index}_out_next, ptr %wa{index}_out_idx\n\
             \x20 br label %wa{index}_out_cond\n\
             wa{index}_out_done:"
        );
    }
    if let Some(length_out) = string_length_out {
        let _ = writeln!(
            out,
            "  %wrapper_str_ptr = extractvalue {{ ptr, i64 }} %wrapper_result, 0\n\
             \x20 %wrapper_str_len = extractvalue {{ ptr, i64 }} %wrapper_result, 1\n\
             \x20 store i64 %wrapper_str_len, ptr {length_out}\n\
             \x20 ret ptr %wrapper_str_ptr\n}}\n"
        );
    } else if let CType::Arr(element) = &function.ret {
        let wrapper = match element.as_ref() {
            CType::I64 => "lu_owned_i64_wrap",
            CType::F64 => "lu_owned_f64_wrap",
            _ => return Err("owned export results require i64 or f64 arrays".into()),
        };
        let _ = writeln!(
            out,
            "  %wrapper_owned_result = call ptr @{wrapper}(ptr %wrapper_result)\n\
             \x20 ret ptr %wrapper_owned_result\n}}\n"
        );
    } else if ret_components.is_empty() {
        out.push_str("  ret void\n}\n\n");
    } else {
        let _ = writeln!(out, "  ret {} %wrapper_result\n}}\n", wrapper_ret_type);
    }
    Ok(out)
}

fn llvm_array_local_for_value(function: &ir::Function, value: ir::ValueId) -> Option<ir::LocalId> {
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

#[derive(Clone)]
struct EV {
    ty: CType,
    regs: Vec<String>, // literal constants or %regs, one per component
}

struct Emit<'a> {
    p: &'a Program,
    out: String,
    tmp: u32,
    lbl: u32,
    // name -> (type, alloca ptrs per component)
    env: Vec<HashMap<String, (CType, Vec<String>)>>,
    strings: Vec<String>,
    soa: bool,
    // (param name, type) of the current fn's inout params — their final
    // values are appended to every return (copy-out travels via the ABI)
    inout_params: Vec<(String, CType)>,
    ret: CType,
    terminated: bool,
    in_main: bool,
    cfg: CfgAnalysis,
    cfg_trusted: HashMap<(usize, ir::LocalId), String>,
    skipped_cfg_blocks: HashSet<ir::BlockId>,
    simd: bool,
    location: (ir::BlockId, usize),
    externs: &'a [ir::ExternDef],
}

pub fn build(
    ir: &LoweredProgram,
    src_path: &str,
    out_path: Option<&str>,
) -> Result<String, String> {
    build_output(ir, src_path, out_path, false, false, None, false)
}

pub fn emit_llvm(
    ir: &LoweredProgram,
    src_path: &str,
    out_path: Option<&str>,
) -> Result<String, String> {
    build_output(ir, src_path, out_path, false, false, None, true)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WasmTarget {
    Wasi,
    Web,
}

fn wasm_web_loader() -> &'static str {
    r#"// Generated by lu for a wasm32-web module.
// The module uses WASI's byte-oriented stdout ABI but needs no WASI runtime.
export async function instantiateLulang(source, onWrite = (text) => console.log(text)) {
  let instance;
  let pending = "";
  const decoder = new TextDecoder();
  const memory = () => instance.exports.memory;
  const view = () => new DataView(memory().buffer);
  const flush = (force = false) => {
    let newline;
    while ((newline = pending.indexOf("\n")) >= 0) {
      onWrite(pending.slice(0, newline));
      pending = pending.slice(newline + 1);
    }
    if (force && pending.length) {
      onWrite(pending);
      pending = "";
    }
  };
  const wasi = {
    fd_write(fd, iovs, count, written) {
      if (fd !== 1 && fd !== 2) return 8;
      const data = view();
      let total = 0;
      for (let i = 0; i < count; i++) {
        const pointer = data.getUint32(iovs + i * 8, true);
        const length = data.getUint32(iovs + i * 8 + 4, true);
        pending += decoder.decode(new Uint8Array(memory().buffer, pointer, length), { stream: true });
        total += length;
      }
      data.setUint32(written, total, true);
      flush(false);
      return 0;
    },
    fd_close() { return 0; },
    fd_fdstat_get(_fd, stat) {
      new Uint8Array(memory().buffer, stat, 24).fill(0);
      view().setUint8(stat, 2);
      return 0;
    },
    fd_seek() { return 70; },
    args_sizes_get(argc, size) {
      const data = view();
      data.setUint32(argc, 0, true);
      data.setUint32(size, 0, true);
      return 0;
    },
    args_get() { return 0; },
    environ_sizes_get(count, size) {
      const data = view();
      data.setUint32(count, 0, true);
      data.setUint32(size, 0, true);
      return 0;
    },
    environ_get() { return 0; },
    clock_time_get(_clock, _precision, result) {
      const nanos = BigInt(Date.now()) * 1000000n;
      view().setBigUint64(result, nanos, true);
      return 0;
    },
    random_get(pointer, length) {
      crypto.getRandomValues(new Uint8Array(memory().buffer, pointer, length));
      return 0;
    },
    proc_exit(code) { throw new Error(`lulang wasm exited with ${code}`); }
  };
  const imports = { wasi_snapshot_preview1: wasi };
  const result = typeof source === "string" || source instanceof URL
    ? await WebAssembly.instantiateStreaming(fetch(source), imports)
    : await WebAssembly.instantiate(source, imports);
  instance = result.instance;
  if (instance.exports._initialize) instance.exports._initialize();
  return {
    instance,
    run() {
      const code = instance.exports.lu_web_run();
      flush(true);
      return code;
    }
  };
}
"#
}

pub fn build_wasm(
    ir: &LoweredProgram,
    src_path: &str,
    out_path: Option<&str>,
    target: WasmTarget,
) -> Result<Vec<String>, String> {
    if !ir.externs.is_empty() {
        return Err(
            "wasm32 builds cannot use native `extern` declarations; provide a wasm host import layer"
                .into(),
        );
    }
    let artifact = build_output(ir, src_path, out_path, false, false, Some(target), false)?;
    let mut outputs = vec![artifact.clone()];
    if target == WasmTarget::Web {
        let loader = std::path::Path::new(&artifact).with_extension("js");
        std::fs::write(&loader, wasm_web_loader()).map_err(|error| error.to_string())?;
        outputs.push(loader.to_string_lossy().into_owned());
    }
    Ok(outputs)
}

pub fn build_library(
    ir: &LoweredProgram,
    src_path: &str,
    out_name: Option<&str>,
    shared: bool,
) -> Result<Vec<String>, String> {
    if !ir.functions.iter().any(|function| function.exported) {
        return Err("library has no `export fn` declarations".into());
    }
    let stem = std::path::Path::new(src_path)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("out");
    let requested = std::path::Path::new(out_name.unwrap_or(stem));
    let parent = requested
        .parent()
        .unwrap_or_else(|| std::path::Path::new(""));
    let name = requested
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("invalid library output name")?;
    let base = parent.join(name);
    let base_string = base.to_string_lossy().into_owned();
    let artifact = build_output(ir, src_path, Some(&base_string), true, shared, None, false)?;
    let header_path = parent.join(format!("{}.h", name));
    let manifest_path = parent.join(format!("{}.json", name));
    std::fs::write(&header_path, crate::cheader::emit_header(ir, name)?)
        .map_err(|error| error.to_string())?;
    std::fs::write(&manifest_path, crate::cheader::emit_manifest(ir, name))
        .map_err(|error| error.to_string())?;
    Ok(vec![
        artifact,
        header_path.to_string_lossy().into_owned(),
        manifest_path.to_string_lossy().into_owned(),
    ])
}

fn build_output(
    ir: &LoweredProgram,
    src_path: &str,
    out_path: Option<&str>,
    library: bool,
    shared: bool,
    wasm: Option<WasmTarget>,
    emit_only: bool,
) -> Result<String, String> {
    let p = ir.source();
    let mut e = Emit {
        p,
        out: String::new(),
        tmp: 0,
        lbl: 0,
        env: vec![HashMap::new()],
        strings: Vec::new(),
        soa: std::env::var("LU_LAYOUT")
            .map(|v| v != "aos")
            .unwrap_or(true),
        inout_params: Vec::new(),
        ret: CType::Unit,
        terminated: false,
        in_main: false,
        cfg: CfgAnalysis::default(),
        cfg_trusted: HashMap::new(),
        skipped_cfg_blocks: HashSet::new(),
        simd: std::env::var("LU_SIMD")
            .map(|value| value != "off" && value != "0")
            .unwrap_or(true),
        location: (0, 0),
        externs: &ir.externs,
    };
    let mut module = String::new();
    module.push_str("; generated by lu\n");
    // Ask clang for the triple it actually stamps on compiled modules (the
    // driver maps e.g. darwin25 -> macosx26 per deployment target, so
    // -print-target-triple alone is not what IR modules carry).
    let triple = if wasm.is_some() {
        "wasm32-unknown-wasi".to_string()
    } else {
        let probe = std::process::Command::new("clang")
            .args(["-x", "c", "-", "-S", "-emit-llvm", "-o", "-"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .and_then(|mut c| {
                use std::io::Write as _;
                c.stdin.take().unwrap().write_all(b"int lu_probe;\n")?;
                c.wait_with_output()
            })
            .map_err(|e| format!("failed to probe clang target triple: {}", e))?;
        String::from_utf8_lossy(&probe.stdout)
            .lines()
            .find_map(|l| {
                l.strip_prefix("target triple = \"")
                    .map(|r| r.trim_end_matches('"').to_string())
            })
            .ok_or("could not determine target triple from clang")?
    };
    let _ = writeln!(module, "target triple = \"{}\"", triple);
    module.push_str(
        "declare double @llvm.sqrt.f64(double)\ndeclare <2 x double> @llvm.sqrt.v2f64(<2 x double>)\n\
         declare double @llvm.sin.f64(double)\n\
         declare double @llvm.cos.f64(double)\ndeclare double @llvm.fabs.f64(double)\n\
         declare <2 x double> @llvm.fabs.v2f64(<2 x double>)\n\
         declare double @llvm.floor.f64(double)\ndeclare double @llvm.minnum.f64(double, double)\n\
         declare <2 x double> @llvm.minnum.v2f64(<2 x double>, <2 x double>)\n\
         declare double @llvm.maxnum.f64(double, double)\n\
         declare <2 x double> @llvm.maxnum.v2f64(<2 x double>, <2 x double>)\n\
         declare double @llvm.pow.f64(double, double)\n\
         declare double @acos(double) #0\ndeclare double @atan2(double, double) #0\n\
         declare void @lu_print_f64(double)\ndeclare void @lu_print_i64(i64)\n\
         declare void @lu_print_bool(i64)\ndeclare void @lu_print_str(ptr, i64)\n\
         declare void @lu_print_sep()\ndeclare void @lu_print_nl()\n\
         declare ptr @lu_arr_new_raw(i64, i64)\n\
         declare ptr @lu_arr_clone(ptr)\n\
         declare ptr @lu_owned_i64_wrap(ptr)\n\
         declare ptr @lu_owned_f64_wrap(ptr)\n\
         declare i64 @lu_str_eq(ptr, i64, ptr, i64) #0\n\
         declare ptr @lu_str_copy(ptr, i64)\n\
         declare ptr @lu_arr_new_f64(i64, double)\ndeclare ptr @lu_arr_new_i64(i64, i64)\n\
         declare void @lu_oob(i64, i64) #1\n\
         declare i64 @lu_i64_div(i64, i64)\ndeclare i64 @lu_i64_rem(i64, i64)\n\
         declare i64 @lu_nargs()\ndeclare ptr @lu_arg(i64)\n\
         declare ptr @lu_read_file(ptr, i64)\ndeclare i64 @lu_last_len()\n\
         declare void @lu_write_file(ptr, i64, ptr, i64)\n\
         declare ptr @lu_chr(i64)\ndeclare ptr @lu_concat(ptr, i64, ptr, i64)\n\
         attributes #0 = { nounwind willreturn memory(none) }\n\
         attributes #1 = { noreturn }\n\n",
    );
    for declaration in &ir.externs {
        let ret = if declaration.ret == CType::Str {
            "ptr".into()
        } else {
            comps_ty(&lty(p, &declaration.ret)?)
        };
        let mut params = Vec::new();
        for (_, ty) in &declaration.params {
            if matches!(ty, CType::Arr(_)) {
                params.push("ptr".to_string());
                params.push("i64".to_string());
            } else if matches!(ty, CType::Rec(index) if p.types[*index].c_layout) {
                params.push(comps_ty(&lty(p, ty)?));
            } else {
                params.extend(lty(p, ty)?.into_iter().map(String::from));
            }
        }
        if declaration.ret == CType::Str {
            params.push("ptr".into());
        }
        let _ = writeln!(
            module,
            "declare {} @\"{}\"({})",
            ret,
            declaration.name,
            params.join(", ")
        );
        if let Some(library) = &declaration.lib {
            let _ = writeln!(module, "; link: {}", library);
        }
    }
    if !ir.externs.is_empty() {
        module.push('\n');
    }
    for (index, f) in p.fns.iter().enumerate() {
        module.push_str(&e.emit_ir_fn(&ir.functions[index], f)?);
        if f.exported {
            module.push_str(&emit_export_wrapper(p, &ir.functions[index], f)?);
        }
    }
    if let Some(main) = &ir.main {
        if !library {
            module.push_str(&e.emit_ir_main(main)?);
        }
    } else if !library {
        return Err("no `main` block in program".into());
    }
    for (i, s) in e.strings.iter().enumerate() {
        let bytes: String = s.bytes().map(|b| format!("\\{:02X}", b)).collect();
        let _ = writeln!(
            module,
            "@.str.{} = private unnamed_addr constant [{} x i8] c\"{}\"",
            i,
            s.len(),
            bytes
        );
    }

    // write .ll and compile with clang
    let stem = std::path::Path::new(src_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("out");
    let out_bin = if emit_only {
        out_path
            .map(String::from)
            .unwrap_or_else(|| format!("{stem}.ll"))
    } else if wasm.is_some() {
        out_path
            .map(String::from)
            .unwrap_or_else(|| format!("{stem}.wasm"))
    } else if library {
        let requested = std::path::Path::new(out_path.unwrap_or(stem));
        let parent = requested
            .parent()
            .unwrap_or_else(|| std::path::Path::new(""));
        let name = requested
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or("invalid library output name")?;
        let extension = if shared {
            if cfg!(target_os = "macos") {
                "dylib"
            } else {
                "so"
            }
        } else {
            "a"
        };
        parent
            .join(format!("lib{}.{}", name, extension))
            .to_string_lossy()
            .into_owned()
    } else {
        out_path
            .map(String::from)
            .unwrap_or_else(|| stem.to_string())
    };
    if emit_only {
        std::fs::write(&out_bin, &module).map_err(|error| error.to_string())?;
        return Ok(out_bin);
    }
    let pid = std::process::id();
    let ll_path = std::env::temp_dir().join(format!("lu_{}_{}.ll", stem, pid));
    std::fs::write(&ll_path, &module).map_err(|e| e.to_string())?;
    if let Some(wasm_target) = wasm {
        let runtime_c = std::env::temp_dir().join(format!("lu_runtime_wasm_{}.c", pid));
        std::fs::write(&runtime_c, include_str!("lu_runtime.c")).map_err(|e| e.to_string())?;
        let mut zig = std::process::Command::new("zig");
        zig.args(["cc", "-target", "wasm32-wasi", "-O3", "-msimd128"]);
        if wasm_target == WasmTarget::Web {
            zig.args([
                "-DLU_WEB",
                "-mexec-model=reactor",
                "-Wl,--export=lu_web_run",
                "-Wl,--export-memory",
            ]);
        }
        let status = zig
            .arg(&ll_path)
            .arg(&runtime_c)
            .arg("-o")
            .arg(&out_bin)
            .status()
            .map_err(|error| {
                format!("failed to invoke Zig for wasm32; install `zig` or set it on PATH: {error}")
            })?;
        let _ = std::fs::remove_file(&ll_path);
        let _ = std::fs::remove_file(&runtime_c);
        if !status.success() {
            return Err("Zig failed compiling the wasm32 module".into());
        }
        return Ok(out_bin);
    }
    // compile the runtime once per source revision, then just link the object
    let rt_src = include_str!("lu_runtime.c");
    let mut rt_hash: u64 = rt_src.bytes().fold(1469598103934665603u64, |h, b| {
        (h ^ b as u64).wrapping_mul(1099511628211)
    });
    if library {
        rt_hash ^= 0x4c55_5f4c_4942;
    }
    let runtime_o = std::env::temp_dir().join(format!("lu_runtime_{:016x}.o", rt_hash));
    if !runtime_o.exists() {
        let runtime_c = std::env::temp_dir().join(format!("lu_runtime_{}.c", pid));
        let runtime_tmp_o =
            std::env::temp_dir().join(format!("lu_runtime_{:016x}_{}.o", rt_hash, pid));
        std::fs::write(&runtime_c, rt_src).map_err(|e| e.to_string())?;
        let mut runtime_clang = std::process::Command::new("clang");
        runtime_clang.args(["-O3", "-mcpu=native", "-c"]);
        if library {
            runtime_clang.args(["-DLU_LIB", "-fPIC"]);
        }
        let st = runtime_clang
            .arg("-o")
            .arg(&runtime_tmp_o)
            .arg(&runtime_c)
            .status()
            .map_err(|e| format!("failed to invoke clang: {}", e))?;
        if !st.success() {
            return Err("clang failed compiling the runtime".into());
        }
        if let Err(e) = std::fs::rename(&runtime_tmp_o, &runtime_o) {
            if !runtime_o.exists() {
                return Err(format!("failed to install runtime object: {}", e));
            }
            let _ = std::fs::remove_file(&runtime_tmp_o);
        }
        let _ = std::fs::remove_file(&runtime_c);
    }
    if library {
        let module_o = std::env::temp_dir().join(format!("lu_{}_{}.o", stem, pid));
        let status = std::process::Command::new("clang")
            .args(["-O3", "-mcpu=native", "-fPIC", "-c", "-o"])
            .arg(&module_o)
            .arg(&ll_path)
            .status()
            .map_err(|e| format!("failed to invoke clang: {}", e))?;
        if !status.success() {
            return Err(format!("clang failed on {}", ll_path.display()));
        }
        let status = if shared {
            let mut clang = std::process::Command::new("clang");
            if cfg!(target_os = "macos") {
                clang.arg("-dynamiclib");
            } else {
                clang.arg("-shared");
            }
            clang.arg("-o").arg(&out_bin).arg(&module_o).arg(&runtime_o);
            let mut linked = std::collections::HashSet::new();
            for lib in ir
                .externs
                .iter()
                .filter_map(|declaration| declaration.lib.as_deref())
            {
                if !linked.insert(lib) {
                    continue;
                }
                if lib.contains('/') || lib.ends_with(".so") || lib.ends_with(".dylib") {
                    clang.arg(lib);
                } else {
                    clang.arg(format!("-l{}", lib));
                }
            }
            if let Ok(flags) = std::env::var("LU_LINK_FLAGS") {
                clang.args(flags.split_whitespace());
            }
            clang.status()
        } else {
            std::process::Command::new("ar")
                .arg("rcs")
                .arg(&out_bin)
                .arg(&module_o)
                .arg(&runtime_o)
                .status()
        }
        .map_err(|e| format!("failed to build library: {}", e))?;
        let _ = std::fs::remove_file(&module_o);
        let _ = std::fs::remove_file(&ll_path);
        if !status.success() {
            return Err("library linker failed".into());
        }
        return Ok(out_bin);
    }

    let mut clang = std::process::Command::new("clang");
    clang
        .args(["-O3", "-mcpu=native", "-o", &out_bin])
        .arg(&ll_path)
        .arg(&runtime_o);
    let mut linked = std::collections::HashSet::new();
    for lib in ir
        .externs
        .iter()
        .filter_map(|declaration| declaration.lib.as_deref())
    {
        if !linked.insert(lib) {
            continue;
        }
        if lib.contains('/') || lib.ends_with(".so") || lib.ends_with(".dylib") {
            clang.arg(lib);
        } else {
            clang.arg(format!("-l{}", lib));
        }
    }
    if let Ok(flags) = std::env::var("LU_LINK_FLAGS") {
        clang.args(flags.split_whitespace());
    }
    let status = clang
        .status()
        .map_err(|e| format!("failed to invoke clang: {}", e))?;
    if !status.success() {
        return Err(format!("clang failed on {}", ll_path.display()));
    }
    Ok(out_bin)
}

impl<'a> Emit<'a> {
    fn t(&mut self) -> String {
        self.tmp += 1;
        format!("%t{}", self.tmp)
    }
    fn l(&mut self) -> String {
        self.lbl += 1;
        format!("L{}", self.lbl)
    }
    fn line(&mut self, s: String) {
        self.out.push_str("  ");
        self.out.push_str(&s);
        self.out.push('\n');
    }
    fn label(&mut self, l: &str) {
        self.out.push_str(l);
        self.out.push_str(":\n");
    }

    fn ir_local(id: ir::LocalId) -> String {
        format!("$l{}", id)
    }

    fn declare_uninit(&mut self, name: &str, ty: &CType) -> Result<(), String> {
        let mut ptrs = Vec::new();
        for component in lty(self.p, ty)? {
            let ptr = self.t();
            self.line(format!("{} = alloca {}", ptr, component));
            ptrs.push(ptr);
        }
        self.env
            .last_mut()
            .unwrap()
            .insert(name.to_string(), (ty.clone(), ptrs));
        Ok(())
    }

    fn emit_ir_fn(&mut self, function: &ir::Function, decl: &FnDecl) -> Result<String, String> {
        self.out.clear();
        self.tmp = 0;
        self.lbl = 0;
        self.env = vec![HashMap::new()];
        self.terminated = false;
        self.ret = function.ret.clone();
        self.in_main = false;
        let mut params = Vec::new();
        let mut incoming = Vec::new();
        let mut cursor = 0;
        for &local in &function.params {
            let ty = &function.locals[local as usize].ty;
            let mut regs = Vec::new();
            for component in lty(self.p, ty)? {
                params.push(format!("{} %p{}", component, cursor));
                regs.push(format!("%p{}", cursor));
                cursor += 1;
            }
            incoming.push((local, regs));
        }
        self.inout_params = function
            .params
            .iter()
            .zip(&function.inouts)
            .filter(|(_, io)| **io)
            .map(|(&local, _)| {
                (
                    Self::ir_local(local),
                    function.locals[local as usize].ty.clone(),
                )
            })
            .collect();
        let header = format!(
            "define internal {} @\"{}\"({}) {{\n",
            comps_ty(&abi_ret_comps(self.p, decl)?),
            internal_symbol(decl),
            params.join(", ")
        );
        self.label("entry");
        for (id, local) in function.locals.iter().enumerate() {
            self.declare_uninit(&Self::ir_local(id as u32), &local.ty)?;
        }
        for (local, regs) in incoming {
            let value = EV {
                ty: function.locals[local as usize].ty.clone(),
                regs,
            };
            // Parameters borrow their incoming value. Ordinary parameters are
            // immutable, and `inout` is exclusive; persistent stores already
            // clone owning array components. This matches the JIT and avoids
            // copying read-only arrays on every call.
            self.store_var(&Self::ir_local(local), &value)?;
        }
        self.emit_ir_body(function)?;
        Ok(format!("{}{}}}\n\n", header, self.out))
    }

    fn emit_ir_main(&mut self, function: &ir::Function) -> Result<String, String> {
        self.out.clear();
        self.tmp = 0;
        self.lbl = 0;
        self.env = vec![HashMap::new()];
        self.terminated = false;
        self.ret = CType::Unit;
        self.in_main = true;
        self.inout_params.clear();
        self.label("entry");
        for (id, local) in function.locals.iter().enumerate() {
            self.declare_uninit(&Self::ir_local(id as u32), &local.ty)?;
        }
        self.emit_ir_body(function)?;
        Ok(format!("define i32 @lu_entry() {{\n{}}}\n\n", self.out))
    }

    fn emit_ir_body(&mut self, function: &ir::Function) -> Result<(), String> {
        self.cfg = analyze_cfg(function);
        self.cfg_trusted.clear();
        self.skipped_cfg_blocks.clear();
        let mut values: Vec<Option<EV>> = vec![None; function.values.len()];
        for (block_index, block) in function.blocks.iter().enumerate() {
            self.location.0 = block_index as ir::BlockId;
            if block_index != 0 {
                self.label(&format!("B{}", block_index));
            }
            if self
                .skipped_cfg_blocks
                .contains(&(block_index as ir::BlockId))
            {
                self.line("unreachable".into());
                continue;
            }
            self.terminated = false;
            for (instruction, inst) in block.instructions.iter().enumerate() {
                self.location.1 = instruction;
                let value = self.emit_ir_inst(function, &values, &inst.kind, &inst.ty)?;
                if let Some(id) = inst.result {
                    values[id as usize] = Some(value.ok_or("value instruction produced no value")?);
                }
            }
            let mut emitted_simd = false;
            for loop_index in 0..self.cfg.loops.len() {
                if self.cfg.loops[loop_index].preheader == block_index as ir::BlockId {
                    self.hoist_cfg_checks(function, &values, loop_index)?;
                    if self.simd && self.emit_cfg_simd(function, &values, loop_index)? {
                        emitted_simd = true;
                        break;
                    }
                }
            }
            if emitted_simd {
                continue;
            }
            match block.terminator {
                Terminator::Jump(target) => self.line(format!("br label %B{}", target)),
                Terminator::Branch {
                    condition,
                    then_block,
                    else_block,
                } => {
                    let condition = Self::ir_value(&values, condition)?;
                    let bit = self.t();
                    self.line(format!("{} = icmp ne i64 {}, 0", bit, condition.regs[0]));
                    self.line(format!(
                        "br i1 {}, label %B{}, label %B{}",
                        bit, then_block, else_block
                    ));
                }
                Terminator::Return(value) => {
                    let value =
                        self.coerce_ev(Self::ir_value(&values, value)?.clone(), &function.ret)?;
                    self.emit_ret(&value)?;
                }
                Terminator::Unreachable => self.line("unreachable".into()),
            }
        }
        Ok(())
    }

    fn emit_cfg_simd(
        &mut self,
        function: &ir::Function,
        values: &[Option<EV>],
        loop_index: usize,
    ) -> Result<bool, String> {
        let Some(plan) = simd_reduction_plan(function, &self.cfg, loop_index, self.soa) else {
            return Ok(false);
        };
        let loop_info = self.cfg.loops[loop_index].clone();
        let lower = Self::ir_value(values, loop_info.lower)?.regs[0].clone();
        let upper = Self::ir_value(values, loop_info.upper)?.regs[0].clone();

        let index_ptr = self.t();
        self.line(format!("{} = alloca i64", index_ptr));
        self.line(format!("store i64 {}, ptr {}", lower, index_ptr));
        let mut vector_ptrs = Vec::new();
        for _ in 0..4 {
            let ptr = self.t();
            self.line(format!("{} = alloca <2 x double>", ptr));
            self.line(format!("store <2 x double> zeroinitializer, ptr {}", ptr));
            vector_ptrs.push(ptr);
        }
        let scalar_ptr = self.t();
        self.line(format!("{} = alloca double", scalar_ptr));
        self.line(format!("store double 0.0, ptr {}", scalar_ptr));

        let vector_head = self.l();
        let vector_body = self.l();
        let scalar_head = self.l();
        let scalar_body = self.l();
        let finish = self.l();
        self.line(format!("br label %{}", vector_head));

        self.label(&vector_head);
        let index = self.t();
        self.line(format!("{} = load i64, ptr {}", index, index_ptr));
        let after_batch = self.t();
        self.line(format!("{} = add i64 {}, 8", after_batch, index));
        let fits = self.t();
        self.line(format!(
            "{} = icmp sle i64 {}, {}",
            fits, after_batch, upper
        ));
        self.line(format!(
            "br i1 {}, label %{}, label %{}",
            fits, vector_body, scalar_head
        ));

        self.label(&vector_body);
        for (lane, accumulator) in vector_ptrs.iter().enumerate() {
            let at = if lane == 0 {
                index.clone()
            } else {
                let at = self.t();
                self.line(format!("{} = add i64 {}, {}", at, index, lane * 2));
                at
            };
            let item = self.emit_simd_vector_expr(loop_index, &plan.value, &at)?;
            let current = self.t();
            self.line(format!(
                "{} = load <2 x double>, ptr {}",
                current, accumulator
            ));
            let next = self.t();
            self.line(format!(
                "{} = fadd fast <2 x double> {}, {}",
                next, current, item
            ));
            self.line(format!("store <2 x double> {}, ptr {}", next, accumulator));
        }
        self.line(format!("store i64 {}, ptr {}", after_batch, index_ptr));
        self.line(format!("br label %{}", vector_head));

        self.label(&scalar_head);
        let scalar_index = self.t();
        self.line(format!("{} = load i64, ptr {}", scalar_index, index_ptr));
        let more = self.t();
        self.line(format!(
            "{} = icmp slt i64 {}, {}",
            more, scalar_index, upper
        ));
        self.line(format!(
            "br i1 {}, label %{}, label %{}",
            more, scalar_body, finish
        ));

        self.label(&scalar_body);
        let item = self.emit_simd_scalar_expr(loop_index, &plan.value, &scalar_index)?;
        let current = self.t();
        self.line(format!("{} = load double, ptr {}", current, scalar_ptr));
        let next = self.t();
        self.line(format!("{} = fadd fast double {}, {}", next, current, item));
        self.line(format!("store double {}, ptr {}", next, scalar_ptr));
        let next_index = self.t();
        self.line(format!("{} = add i64 {}, 1", next_index, scalar_index));
        self.line(format!("store i64 {}, ptr {}", next_index, index_ptr));
        self.line(format!("br label %{}", scalar_head));

        self.label(&finish);
        let mut vectors = Vec::new();
        for accumulator in &vector_ptrs {
            let value = self.t();
            self.line(format!(
                "{} = load <2 x double>, ptr {}",
                value, accumulator
            ));
            vectors.push(value);
        }
        let pair0 = self.t();
        self.line(format!(
            "{} = fadd fast <2 x double> {}, {}",
            pair0, vectors[0], vectors[1]
        ));
        let pair1 = self.t();
        self.line(format!(
            "{} = fadd fast <2 x double> {}, {}",
            pair1, vectors[2], vectors[3]
        ));
        let vector_total = self.t();
        self.line(format!(
            "{} = fadd fast <2 x double> {}, {}",
            vector_total, pair0, pair1
        ));
        let lane0 = self.t();
        self.line(format!(
            "{} = extractelement <2 x double> {}, i64 0",
            lane0, vector_total
        ));
        let lane1 = self.t();
        self.line(format!(
            "{} = extractelement <2 x double> {}, i64 1",
            lane1, vector_total
        ));
        let lanes = self.t();
        self.line(format!("{} = fadd fast double {}, {}", lanes, lane0, lane1));
        let scalar = self.t();
        self.line(format!("{} = load double, ptr {}", scalar, scalar_ptr));
        let total = self.t();
        self.line(format!(
            "{} = fadd fast double {}, {}",
            total, lanes, scalar
        ));
        self.store_var(
            &Self::ir_local(plan.accumulator),
            &EV {
                ty: CType::F64,
                regs: vec![total],
            },
        )?;
        self.line(format!("br label %B{}", loop_info.exit));
        self.skipped_cfg_blocks
            .extend(loop_info.blocks.iter().copied());
        Ok(true)
    }

    fn emit_simd_vector_expr(
        &mut self,
        loop_index: usize,
        expr: &SimdExpr,
        index: &str,
    ) -> Result<String, String> {
        Ok(match expr {
            SimdExpr::F64(value) => {
                let bits = format!("0x{:016X}", value.to_bits());
                format!("<double {bits}, double {bits}>")
            }
            SimdExpr::I64(value) => {
                let bits = format!("0x{:016X}", (*value as f64).to_bits());
                format!("<double {bits}, double {bits}>")
            }
            SimdExpr::Invariant(local) => {
                let scalar = self.load_var(&Self::ir_local(*local))?.regs[0].clone();
                let inserted = self.t();
                self.line(format!(
                    "{} = insertelement <2 x double> poison, double {}, i64 0",
                    inserted, scalar
                ));
                let splat = self.t();
                self.line(format!(
                    "{} = shufflevector <2 x double> {}, <2 x double> poison, <2 x i32> zeroinitializer",
                    splat, inserted
                ));
                splat
            }
            SimdExpr::Neg(value) => {
                let value = self.emit_simd_vector_expr(loop_index, value, index)?;
                let out = self.t();
                self.line(format!("{} = fneg fast <2 x double> {}", out, value));
                out
            }
            SimdExpr::Binary { op, lhs, rhs } => {
                let lhs = self.emit_simd_vector_expr(loop_index, lhs, index)?;
                let rhs = self.emit_simd_vector_expr(loop_index, rhs, index)?;
                let instruction = match op {
                    BinaryOp::Add => "fadd",
                    BinaryOp::Sub => "fsub",
                    BinaryOp::Mul => "fmul",
                    BinaryOp::Div => "fdiv",
                    _ => return Err("unsupported SIMD binary operation".into()),
                };
                let out = self.t();
                self.line(format!(
                    "{} = {} fast <2 x double> {}, {}",
                    out, instruction, lhs, rhs
                ));
                out
            }
            SimdExpr::Array { local } => {
                let base = self.load_var(&Self::ir_local(*local))?.regs[0].clone();
                self.emit_simd_vector_load(&base, index, None)?
            }
            SimdExpr::Field {
                local,
                record,
                field,
            } => {
                let base = self.load_var(&Self::ir_local(*local))?.regs[0].clone();
                let field_name = &self.p.types[*record].fields[*field].0;
                let (component, _) = field_offset(self.p, *record, field_name)?;
                let logical = self
                    .cfg_trusted
                    .get(&(loop_index, *local))
                    .cloned()
                    .ok_or("missing trusted SIMD field length")?;
                self.emit_simd_vector_load(&base, index, Some((component, logical)))?
            }
            SimdExpr::Builtin { name, args } => {
                let args = args
                    .iter()
                    .map(|arg| self.emit_simd_vector_expr(loop_index, arg, index))
                    .collect::<Result<Vec<_>, _>>()?;
                let out = self.t();
                match name.as_str() {
                    "sqrt" => self.line(format!(
                        "{} = call fast <2 x double> @llvm.sqrt.v2f64(<2 x double> {})",
                        out, args[0]
                    )),
                    "abs" => self.line(format!(
                        "{} = call fast <2 x double> @llvm.fabs.v2f64(<2 x double> {})",
                        out, args[0]
                    )),
                    "min" => self.line(format!(
                        "{} = call fast <2 x double> @llvm.minnum.v2f64(<2 x double> {}, <2 x double> {})",
                        out, args[0], args[1]
                    )),
                    "max" => self.line(format!(
                        "{} = call fast <2 x double> @llvm.maxnum.v2f64(<2 x double> {}, <2 x double> {})",
                        out, args[0], args[1]
                    )),
                    _ => return Err("unsupported SIMD builtin".into()),
                }
                out
            }
        })
    }

    fn emit_simd_vector_load(
        &mut self,
        base: &str,
        index: &str,
        plane: Option<(usize, String)>,
    ) -> Result<String, String> {
        let data = self.t();
        self.line(format!("{} = getelementptr i8, ptr {}, i64 8", data, base));
        let at = if let Some((component, logical)) = plane {
            let offset = self.t();
            self.line(format!("{} = mul i64 {}, {}", offset, logical, component));
            let at = self.t();
            self.line(format!("{} = add i64 {}, {}", at, offset, index));
            at
        } else {
            index.to_string()
        };
        let address = self.t();
        self.line(format!(
            "{} = getelementptr double, ptr {}, i64 {}",
            address, data, at
        ));
        let value = self.t();
        self.line(format!(
            "{} = load <2 x double>, ptr {}, align 8",
            value, address
        ));
        Ok(value)
    }

    fn emit_simd_scalar_expr(
        &mut self,
        loop_index: usize,
        expr: &SimdExpr,
        index: &str,
    ) -> Result<String, String> {
        Ok(match expr {
            SimdExpr::F64(value) => format!("0x{:016X}", value.to_bits()),
            SimdExpr::I64(value) => format!("0x{:016X}", (*value as f64).to_bits()),
            SimdExpr::Invariant(local) => self.load_var(&Self::ir_local(*local))?.regs[0].clone(),
            SimdExpr::Neg(value) => {
                let value = self.emit_simd_scalar_expr(loop_index, value, index)?;
                let out = self.t();
                self.line(format!("{} = fneg fast double {}", out, value));
                out
            }
            SimdExpr::Binary { op, lhs, rhs } => {
                let lhs = self.emit_simd_scalar_expr(loop_index, lhs, index)?;
                let rhs = self.emit_simd_scalar_expr(loop_index, rhs, index)?;
                let instruction = match op {
                    BinaryOp::Add => "fadd",
                    BinaryOp::Sub => "fsub",
                    BinaryOp::Mul => "fmul",
                    BinaryOp::Div => "fdiv",
                    _ => return Err("unsupported scalar SIMD binary operation".into()),
                };
                let out = self.t();
                self.line(format!(
                    "{} = {} fast double {}, {}",
                    out, instruction, lhs, rhs
                ));
                out
            }
            SimdExpr::Array { local } => {
                let base = self.load_var(&Self::ir_local(*local))?.regs[0].clone();
                self.emit_simd_scalar_load(&base, index, None)?
            }
            SimdExpr::Field {
                local,
                record,
                field,
            } => {
                let base = self.load_var(&Self::ir_local(*local))?.regs[0].clone();
                let field_name = &self.p.types[*record].fields[*field].0;
                let (component, _) = field_offset(self.p, *record, field_name)?;
                let logical = self
                    .cfg_trusted
                    .get(&(loop_index, *local))
                    .cloned()
                    .ok_or("missing trusted scalar-tail field length")?;
                self.emit_simd_scalar_load(&base, index, Some((component, logical)))?
            }
            SimdExpr::Builtin { name, args } => {
                let args = args
                    .iter()
                    .map(|arg| self.emit_simd_scalar_expr(loop_index, arg, index))
                    .collect::<Result<Vec<_>, _>>()?;
                let out = self.t();
                match name.as_str() {
                    "sqrt" => self.line(format!(
                        "{} = call fast double @llvm.sqrt.f64(double {})",
                        out, args[0]
                    )),
                    "abs" => self.line(format!(
                        "{} = call fast double @llvm.fabs.f64(double {})",
                        out, args[0]
                    )),
                    "min" => self.line(format!(
                        "{} = call fast double @llvm.minnum.f64(double {}, double {})",
                        out, args[0], args[1]
                    )),
                    "max" => self.line(format!(
                        "{} = call fast double @llvm.maxnum.f64(double {}, double {})",
                        out, args[0], args[1]
                    )),
                    _ => return Err("unsupported scalar-tail builtin".into()),
                }
                out
            }
        })
    }

    fn emit_simd_scalar_load(
        &mut self,
        base: &str,
        index: &str,
        plane: Option<(usize, String)>,
    ) -> Result<String, String> {
        let data = self.t();
        self.line(format!("{} = getelementptr i8, ptr {}, i64 8", data, base));
        let at = if let Some((component, logical)) = plane {
            let offset = self.t();
            self.line(format!("{} = mul i64 {}, {}", offset, logical, component));
            let at = self.t();
            self.line(format!("{} = add i64 {}, {}", at, offset, index));
            at
        } else {
            index.to_string()
        };
        let address = self.t();
        self.line(format!(
            "{} = getelementptr double, ptr {}, i64 {}",
            address, data, at
        ));
        let value = self.t();
        self.line(format!("{} = load double, ptr {}, align 8", value, address));
        Ok(value)
    }

    fn ir_value(values: &[Option<EV>], id: ir::ValueId) -> Result<&EV, String> {
        values
            .get(id as usize)
            .and_then(Option::as_ref)
            .ok_or_else(|| format!("IR value %{} is unavailable", id))
    }

    fn hoist_cfg_checks(
        &mut self,
        function: &ir::Function,
        values: &[Option<EV>],
        loop_index: usize,
    ) -> Result<(), String> {
        let loop_info = &self.cfg.loops[loop_index];
        let lower = Self::ir_value(values, loop_info.lower)?.regs[0].clone();
        let upper = Self::ir_value(values, loop_info.upper)?.regs[0].clone();
        let arrays = loop_info.arrays.clone();
        for array in arrays {
            let ty = function.locals[array as usize].ty.clone();
            let CType::Arr(element) = ty else { continue };
            let base = self.load_var(&Self::ir_local(array))?.regs[0].clone();
            let stored = self.t();
            self.line(format!("{} = load i64, ptr {}", stored, base));
            let stride = lty(self.p, &element)?.len();
            let logical = if stride == 1 {
                stored
            } else {
                let logical = self.t();
                self.line(format!("{} = sdiv i64 {}, {}", logical, stored, stride));
                logical
            };
            let negative = self.t();
            self.line(format!("{} = icmp slt i64 {}, 0", negative, lower));
            let over = self.t();
            self.line(format!("{} = icmp sgt i64 {}, {}", over, upper, logical));
            let bad = self.t();
            self.line(format!("{} = or i1 {}, {}", bad, negative, over));
            let fail = self.l();
            let ok = self.l();
            self.line(format!("br i1 {}, label %{}, label %{}", bad, fail, ok));
            self.label(&fail);
            self.line(format!("call void @lu_oob(i64 {}, i64 {})", upper, logical));
            self.line("unreachable".into());
            self.label(&ok);
            self.cfg_trusted.insert((loop_index, array), logical);
        }
        Ok(())
    }

    fn emit_ir_inst(
        &mut self,
        function: &ir::Function,
        values: &[Option<EV>],
        kind: &InstKind,
        ty: &CType,
    ) -> Result<Option<EV>, String> {
        let value = |id| Self::ir_value(values, id).cloned();
        Ok(match kind {
            InstKind::Constant(c) => Some(match c {
                Constant::I64(v) => EV {
                    ty: CType::I64,
                    regs: vec![v.to_string()],
                },
                Constant::F32(v) => EV {
                    ty: CType::F32,
                    regs: vec![format!("0x{:016X}", (*v as f64).to_bits())],
                },
                Constant::F64(v) => EV {
                    ty: CType::F64,
                    regs: vec![format!("0x{:016X}", v.to_bits())],
                },
                Constant::Bool(v) => EV {
                    ty: CType::Bool,
                    regs: vec![(*v as i64).to_string()],
                },
                Constant::Bytes(bytes) => {
                    let text = String::from_utf8(bytes.clone())
                        .map_err(|_| "source string is not UTF-8")?;
                    let id = self
                        .strings
                        .iter()
                        .position(|s| s == &text)
                        .unwrap_or_else(|| {
                            self.strings.push(text);
                            self.strings.len() - 1
                        });
                    EV {
                        ty: CType::Str,
                        regs: vec![format!("@.str.{}", id), bytes.len().to_string()],
                    }
                }
                Constant::Unit => EV {
                    ty: CType::Unit,
                    regs: vec![],
                },
            }),
            InstKind::Load(local) => Some(self.load_var(&Self::ir_local(*local))?),
            InstKind::Store {
                local,
                value: id,
                retain_arrays,
            } => {
                let v = value(*id)?;
                let want = &function.locals[*local as usize].ty;
                let mut v = self.coerce_ev(v, want)?;
                if *retain_arrays {
                    for offset in array_component_offsets(self.p, want)? {
                        let copy = self.t();
                        self.line(format!(
                            "{} = call ptr @lu_arr_clone(ptr {})",
                            copy, v.regs[offset]
                        ));
                        v.regs[offset] = copy;
                    }
                }
                self.store_var(&Self::ir_local(*local), &v)?;
                None
            }
            InstKind::Unary { op, value: id } => {
                let v = value(*id)?;
                let out = self.t();
                match (op, &v.ty) {
                    (UnaryOp::Neg, CType::F32) => {
                        self.line(format!("{} = fneg fast float {}", out, v.regs[0]))
                    }
                    (UnaryOp::Neg, CType::F64) => {
                        self.line(format!("{} = fneg fast double {}", out, v.regs[0]))
                    }
                    (UnaryOp::Neg, CType::I64) => {
                        self.line(format!("{} = sub i64 0, {}", out, v.regs[0]))
                    }
                    (UnaryOp::Not, CType::Bool) => {
                        self.line(format!("{} = xor i64 {}, 1", out, v.regs[0]))
                    }
                    _ => return Err("invalid IR unary operation".into()),
                };
                Some(EV {
                    ty: ty.clone(),
                    regs: vec![out],
                })
            }
            InstKind::Binary { op, lhs, rhs } => {
                Some(self.emit_ir_binary(*op, value(*lhs)?, value(*rhs)?)?)
            }
            InstKind::Select {
                condition,
                then_value,
                else_value,
            } => {
                let condition = value(*condition)?;
                let then_value = value(*then_value)?;
                let else_value = value(*else_value)?;
                let mut regs = Vec::new();
                for ((component, yes), no) in lty(self.p, &then_value.ty)?
                    .iter()
                    .zip(&then_value.regs)
                    .zip(&else_value.regs)
                {
                    let bit = self.t();
                    self.line(format!("{} = trunc i64 {} to i1", bit, condition.regs[0]));
                    let out = self.t();
                    self.line(format!(
                        "{} = select i1 {}, {} {}, {} {}",
                        out, bit, component, yes, component, no
                    ));
                    regs.push(out);
                }
                Some(EV {
                    ty: then_value.ty,
                    regs,
                })
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
                    Callee::Builtin(name) => self.emit_call(name, args)?,
                    Callee::Function(id) => self.emit_ir_user_call(*id, args, inout)?,
                    Callee::Extern(id) => self.emit_ir_extern_call(*id, args)?,
                })
            }
            InstKind::Field {
                base,
                record,
                field,
            } => {
                let base = value(*base)?;
                let name = &self.p.types[*record].fields[*field].0;
                let (off, ft) = field_offset(self.p, *record, name)?;
                let width = lty(self.p, &ft)?.len();
                Some(EV {
                    ty: ft,
                    regs: base.regs[off..off + width].to_vec(),
                })
            }
            InstKind::Index { base, index } => {
                let trusted =
                    self.cfg
                        .trusted_accesses
                        .get(&self.location)
                        .and_then(|loop_index| {
                            let array = llvm_array_local_for_value(function, *base)?;
                            self.cfg_trusted.get(&(*loop_index, array)).cloned()
                        });
                Some(self.emit_ir_index(value(*base)?, value(*index)?, trusted)?)
            }
            InstKind::Array(items) => {
                let items = items
                    .iter()
                    .map(|id| value(*id))
                    .collect::<Result<Vec<_>, _>>()?;
                Some(self.emit_ir_array(items, ty)?)
            }
            InstKind::Record { record, fields } => {
                let mut regs = Vec::new();
                for (id, (_, tstr)) in fields.iter().zip(&self.p.types[*record].fields) {
                    let v = value(*id)?;
                    let want = resolve_type(self.p, tstr)?;
                    let v = self.coerce_ev(v, &want)?;
                    regs.extend(v.regs);
                }
                Some(EV {
                    ty: ty.clone(),
                    regs,
                })
            }
            InstKind::Enum { enumeration, tag } => Some(EV {
                ty: CType::Enum(*enumeration),
                regs: vec![tag.to_string()],
            }),
            InstKind::SetIndex {
                base,
                index,
                value: stored,
                ..
            } => {
                let base_id = *base;
                let base = value(*base)?;
                let index = value(*index)?;
                let mut stored = value(*stored)?;
                if let CType::CMutSlice(elem) = &base.ty {
                    stored = self.coerce_ev(stored, elem)?;
                    let bad = self.t();
                    self.line(format!(
                        "{} = icmp uge i64 {}, {}",
                        bad, index.regs[0], base.regs[1]
                    ));
                    let fail = self.l();
                    let ok = self.l();
                    self.line(format!("br i1 {}, label %{}, label %{}", bad, fail, ok));
                    self.label(&fail);
                    self.line(format!(
                        "call void @lu_oob(i64 {}, i64 {})",
                        index.regs[0], base.regs[1]
                    ));
                    self.line("unreachable".into());
                    self.label(&ok);
                    let components = lty(self.p, elem)?;
                    if components.len() != 1 {
                        return Err("c_mut_slice elements must have one ABI component".into());
                    }
                    let address = self.t();
                    self.line(format!(
                        "{} = getelementptr {}, ptr {}, i64 {}",
                        address, components[0], base.regs[0], index.regs[0]
                    ));
                    self.line(format!(
                        "store {} {}, ptr {}",
                        components[0], stored.regs[0], address
                    ));
                    return Ok(None);
                }
                let CType::Arr(elem) = &base.ty else {
                    return Err("IR set-index on non-mutable array view".into());
                };
                let trusted =
                    self.cfg
                        .trusted_accesses
                        .get(&self.location)
                        .and_then(|loop_index| {
                            let array = llvm_array_local_for_value(function, base_id)?;
                            self.cfg_trusted.get(&(*loop_index, array)).cloned()
                        });
                let addrs = self.elem_addrs(&base.regs[0], &index.regs[0], elem, trusted)?;
                for ((component, reg), addr) in
                    lty(self.p, elem)?.iter().zip(&stored.regs).zip(addrs)
                {
                    self.line(format!("store {} {}, ptr {}", component, reg, addr));
                }
                None
            }
            InstKind::SetField {
                root,
                path,
                value: stored,
            } => {
                let stored = value(*stored)?;
                let (mut current, ptrs) = self
                    .lookup(&Self::ir_local(*root))
                    .ok_or("invalid IR field root")?;
                let mut offset = 0;
                for &field in path {
                    let CType::Rec(record) = current else {
                        return Err("IR field path on non-record".into());
                    };
                    let name = &self.p.types[record].fields[field].0;
                    let (o, next) = field_offset(self.p, record, name)?;
                    offset += o;
                    current = next;
                }
                for ((component, ptr), reg) in lty(self.p, &current)?
                    .iter()
                    .zip(&ptrs[offset..])
                    .zip(&stored.regs)
                {
                    self.line(format!("store {} {}, ptr {}", component, reg, ptr));
                }
                None
            }
        })
    }

    fn emit_ir_binary(&mut self, op: BinaryOp, lhs: EV, rhs: EV) -> Result<EV, String> {
        use BinaryOp::*;
        if matches!(op, Add | Sub | Mul | Div | Rem) {
            if lhs.ty == CType::I64 && rhs.ty == CType::I64 {
                if matches!(op, Div | Rem) {
                    return self.emit_checked_int_div(&lhs.regs[0], &rhs.regs[0], op == Rem);
                }
                let out = self.t();
                let opcode = match op {
                    Add => "add",
                    Sub => "sub",
                    Mul => "mul",
                    _ => unreachable!(),
                };
                self.line(format!(
                    "{} = {} i64 {}, {}",
                    out, opcode, lhs.regs[0], rhs.regs[0]
                ));
                return Ok(EV {
                    ty: CType::I64,
                    regs: vec![out],
                });
            }
            let result_ty = if lhs.ty == CType::F64 || rhs.ty == CType::F64 {
                CType::F64
            } else {
                CType::F32
            };
            let lhs = self.coerce_ev(lhs, &result_ty)?;
            let rhs = self.coerce_ev(rhs, &result_ty)?;
            let out = self.t();
            let opcode = match op {
                Add => "fadd",
                Sub => "fsub",
                Mul => "fmul",
                Div => "fdiv",
                Rem => "frem",
                _ => unreachable!(),
            };
            let llvm_ty = if result_ty == CType::F32 {
                "float"
            } else {
                "double"
            };
            self.line(format!(
                "{} = {} fast {} {}, {}",
                out, opcode, llvm_ty, lhs.regs[0], rhs.regs[0]
            ));
            return Ok(EV {
                ty: result_ty,
                regs: vec![out],
            });
        }
        if matches!(op, Eq | Ne) && lhs.ty == CType::Str && rhs.ty == CType::Str {
            let eq = self.t();
            self.line(format!(
                "{} = call i64 @lu_str_eq(ptr {}, i64 {}, ptr {}, i64 {})",
                eq, lhs.regs[0], lhs.regs[1], rhs.regs[0], rhs.regs[1]
            ));
            if op == Ne {
                let out = self.t();
                self.line(format!("{} = xor i64 {}, 1", out, eq));
                return Ok(EV {
                    ty: CType::Bool,
                    regs: vec![out],
                });
            }
            return Ok(EV {
                ty: CType::Bool,
                regs: vec![eq],
            });
        }
        if matches!(op, Eq | Ne)
            && !matches!(lhs.ty, CType::F32 | CType::F64)
            && !matches!(rhs.ty, CType::F32 | CType::F64)
        {
            let bit = self.t();
            let compare_type = if matches!(lhs.ty, CType::CPtr(_)) {
                "ptr"
            } else {
                "i64"
            };
            self.line(format!(
                "{} = icmp {} {} {}, {}",
                bit,
                if op == Eq { "eq" } else { "ne" },
                compare_type,
                lhs.regs[0],
                rhs.regs[0]
            ));
            let out = self.t();
            self.line(format!("{} = zext i1 {} to i64", out, bit));
            return Ok(EV {
                ty: CType::Bool,
                regs: vec![out],
            });
        }
        let a = self.to_f64(&lhs)?;
        let b = self.to_f64(&rhs)?;
        if op == ApproxEq {
            let d = self.t();
            self.line(format!("{} = fsub fast double {}, {}", d, a, b));
            let ad = self.t();
            self.line(format!("{} = call double @llvm.fabs.f64(double {})", ad, d));
            let aa = self.t();
            self.line(format!("{} = call double @llvm.fabs.f64(double {})", aa, a));
            let ab = self.t();
            self.line(format!("{} = call double @llvm.fabs.f64(double {})", ab, b));
            let scale = self.t();
            self.line(format!(
                "{} = call double @llvm.maxnum.f64(double {}, double {})",
                scale, aa, ab
            ));
            let rel = self.t();
            self.line(format!(
                "{} = fmul fast double {}, 0x{:016X}",
                rel,
                scale,
                RTOL.to_bits()
            ));
            let tol = self.t();
            self.line(format!(
                "{} = fadd fast double {}, 0x{:016X}",
                tol,
                rel,
                ATOL.to_bits()
            ));
            let bit = self.t();
            self.line(format!("{} = fcmp fast ole double {}, {}", bit, ad, tol));
            let out = self.t();
            self.line(format!("{} = zext i1 {} to i64", out, bit));
            return Ok(EV {
                ty: CType::Bool,
                regs: vec![out],
            });
        }
        let pred = match op {
            Eq => "oeq",
            Ne => "one",
            Lt => "olt",
            Le => "ole",
            Gt => "ogt",
            Ge => "oge",
            _ => return Err("invalid IR comparison".into()),
        };
        let bit = self.t();
        self.line(format!("{} = fcmp fast {} double {}, {}", bit, pred, a, b));
        let out = self.t();
        self.line(format!("{} = zext i1 {} to i64", out, bit));
        Ok(EV {
            ty: CType::Bool,
            regs: vec![out],
        })
    }

    fn emit_ir_index(
        &mut self,
        base: EV,
        index: EV,
        trusted: Option<String>,
    ) -> Result<EV, String> {
        if base.ty == CType::Str {
            let bad = self.t();
            self.line(format!(
                "{} = icmp uge i64 {}, {}",
                bad, index.regs[0], base.regs[1]
            ));
            let fail = self.l();
            let ok = self.l();
            self.line(format!("br i1 {}, label %{}, label %{}", bad, fail, ok));
            self.label(&fail);
            self.line(format!(
                "call void @lu_oob(i64 {}, i64 {})",
                index.regs[0], base.regs[1]
            ));
            self.line("unreachable".into());
            self.label(&ok);
            let ptr = self.t();
            self.line(format!(
                "{} = getelementptr i8, ptr {}, i64 {}",
                ptr, base.regs[0], index.regs[0]
            ));
            let byte = self.t();
            self.line(format!("{} = load i8, ptr {}", byte, ptr));
            let out = self.t();
            self.line(format!("{} = zext i8 {} to i64", out, byte));
            return Ok(EV {
                ty: CType::I64,
                regs: vec![out],
            });
        }
        if let CType::CSlice(elem) | CType::CMutSlice(elem) = base.ty {
            let bad = self.t();
            self.line(format!(
                "{} = icmp uge i64 {}, {}",
                bad, index.regs[0], base.regs[1]
            ));
            let fail = self.l();
            let ok = self.l();
            self.line(format!("br i1 {}, label %{}, label %{}", bad, fail, ok));
            self.label(&fail);
            self.line(format!(
                "call void @lu_oob(i64 {}, i64 {})",
                index.regs[0], base.regs[1]
            ));
            self.line("unreachable".into());
            self.label(&ok);
            let components = lty(self.p, &elem)?;
            if components.len() != 1 {
                return Err("borrowed C slice elements must have one ABI component".into());
            }
            let address = self.t();
            self.line(format!(
                "{} = getelementptr {}, ptr {}, i64 {}",
                address, components[0], base.regs[0], index.regs[0]
            ));
            let result = self.t();
            self.line(format!(
                "{} = load {}, ptr {}",
                result, components[0], address
            ));
            return Ok(EV {
                ty: *elem,
                regs: vec![result],
            });
        }
        let CType::Arr(elem) = base.ty else {
            return Err("IR index on non-array".into());
        };
        let addrs = self.elem_addrs(&base.regs[0], &index.regs[0], &elem, trusted)?;
        let mut regs = Vec::new();
        for (component, addr) in lty(self.p, &elem)?.iter().zip(addrs) {
            let out = self.t();
            self.line(format!("{} = load {}, ptr {}", out, component, addr));
            regs.push(out);
        }
        Ok(EV { ty: *elem, regs })
    }

    fn emit_ir_array(&mut self, items: Vec<EV>, ty: &CType) -> Result<EV, String> {
        let CType::Arr(elem) = ty else {
            return Err("IR array has non-array type".into());
        };
        let components = lty(self.p, elem)?;
        let logical = items.len() as i64;
        let base = self.t();
        self.line(format!(
            "{} = call ptr @lu_arr_new_raw(i64 {}, i64 {})",
            base,
            logical,
            components.len()
        ));
        for (i, mut value) in items.into_iter().enumerate() {
            value = self.coerce_ev(value, elem)?;
            let addrs = self.elem_addrs(&base, &i.to_string(), elem, Some(logical.to_string()))?;
            for ((component, reg), addr) in components.iter().zip(&value.regs).zip(addrs) {
                self.line(format!("store {} {}, ptr {}", component, reg, addr));
            }
        }
        Ok(EV {
            ty: ty.clone(),
            regs: vec![base],
        })
    }

    fn emit_ir_user_call(
        &mut self,
        id: ir::FunctionId,
        args: Vec<EV>,
        inout: &[Option<ir::LocalId>],
    ) -> Result<EV, String> {
        let decl = &self.p.fns[id as usize];
        let ret = resolve_type(self.p, &decl.ret)?;
        let mut parts = Vec::new();
        for ((_, tstr), mut value) in decl.params.iter().zip(args) {
            let want = resolve_type(self.p, tstr)?;
            value = self.coerce_ev(value, &want)?;
            if decl.exported && matches!(&want, CType::Arr(_)) {
                let copy = self.t();
                self.line(format!(
                    "{} = call ptr @lu_arr_clone(ptr {})",
                    copy, value.regs[0]
                ));
                value.regs[0] = copy;
            }
            for (component, reg) in lty(self.p, &want)?.iter().zip(&value.regs) {
                parts.push(format!("{} {}", component, reg));
            }
        }
        let abi = abi_ret_comps(self.p, decl)?;
        let rt = comps_ty(&abi);
        if abi.is_empty() {
            self.line(format!(
                "call void @\"{}\"({})",
                internal_symbol(decl),
                parts.join(", ")
            ));
            return Ok(EV {
                ty: CType::Unit,
                regs: vec![],
            });
        }
        let call = self.t();
        self.line(format!(
            "{} = call {} @\"{}\"({})",
            call,
            rt,
            internal_symbol(decl),
            parts.join(", ")
        ));
        let mut all = Vec::new();
        if abi.len() == 1 {
            all.push(call);
        } else {
            for i in 0..abi.len() {
                let out = self.t();
                self.line(format!("{} = extractvalue {} {}, {}", out, rt, call, i));
                all.push(out);
            }
        }
        let width = lty(self.p, &ret)?.len();
        let regs = all[..width].to_vec();
        let mut cursor = width;
        for (i, ((_, tstr), io)) in decl.params.iter().zip(&decl.inouts).enumerate() {
            if *io {
                let ty = resolve_type(self.p, tstr)?;
                let w = lty(self.p, &ty)?.len();
                let target = inout[i].ok_or("missing IR inout target")?;
                self.store_var(
                    &Self::ir_local(target),
                    &EV {
                        ty,
                        regs: all[cursor..cursor + w].to_vec(),
                    },
                )?;
                cursor += w;
            }
        }
        Ok(EV { ty: ret, regs })
    }

    fn emit_ir_extern_call(&mut self, id: ir::ExternId, args: Vec<EV>) -> Result<EV, String> {
        let declaration = &self.externs[id as usize];
        let mut parts = Vec::new();
        for ((_, want), value) in declaration.params.iter().zip(args) {
            let value = self.coerce_ev(value, want)?;
            match want {
                CType::Arr(element) => {
                    let data = self.t();
                    self.line(format!(
                        "{} = getelementptr i8, ptr {}, i64 8",
                        data, value.regs[0]
                    ));
                    let slots = self.t();
                    self.line(format!("{} = load i64, ptr {}", slots, value.regs[0]));
                    let stride = lty(self.p, element)?.len();
                    let length = if stride == 1 {
                        slots
                    } else {
                        let length = self.t();
                        self.line(format!("{} = sdiv i64 {}, {}", length, slots, stride));
                        length
                    };
                    parts.push(format!("ptr {}", data));
                    parts.push(format!("i64 {}", length));
                }
                CType::Rec(index) if self.p.types[*index].c_layout => {
                    let components = lty(self.p, want)?;
                    let aggregate = comps_ty(&components);
                    let mut packed = "poison".to_string();
                    for (component_index, (component, register)) in
                        components.iter().zip(&value.regs).enumerate()
                    {
                        let next = self.t();
                        self.line(format!(
                            "{} = insertvalue {} {}, {} {}, {}",
                            next, aggregate, packed, component, register, component_index
                        ));
                        packed = next;
                    }
                    parts.push(format!("{} {}", aggregate, packed));
                }
                _ => {
                    for (component, register) in lty(self.p, want)?.iter().zip(&value.regs) {
                        parts.push(format!("{} {}", component, register));
                    }
                }
            }
        }
        if declaration.ret == CType::Str {
            let length_pointer = self.t();
            self.line(format!("{} = alloca i64", length_pointer));
            parts.push(format!("ptr {}", length_pointer));
            let returned = self.t();
            self.line(format!(
                "{} = call ptr @\"{}\"({})",
                returned,
                declaration.name,
                parts.join(", ")
            ));
            let length = self.t();
            self.line(format!("{} = load i64, ptr {}", length, length_pointer));
            let copied = self.t();
            self.line(format!(
                "{} = call ptr @lu_str_copy(ptr {}, i64 {})",
                copied, returned, length
            ));
            return Ok(EV {
                ty: CType::Str,
                regs: vec![copied, length],
            });
        }
        let components = lty(self.p, &declaration.ret)?;
        if components.is_empty() {
            self.line(format!(
                "call void @\"{}\"({})",
                declaration.name,
                parts.join(", ")
            ));
            return Ok(EV {
                ty: CType::Unit,
                regs: Vec::new(),
            });
        }
        let result = self.t();
        self.line(format!(
            "{} = call {} @\"{}\"({})",
            result,
            comps_ty(&components),
            declaration.name,
            parts.join(", ")
        ));
        let registers = if components.len() <= 1 {
            vec![result]
        } else {
            let aggregate = comps_ty(&components);
            let mut registers = Vec::new();
            for (index, component) in components.iter().enumerate() {
                let register = self.t();
                self.line(format!(
                    "{} = extractvalue {} {}, {}",
                    register, aggregate, result, index
                ));
                let _ = component;
                registers.push(register);
            }
            registers
        };
        Ok(EV {
            ty: declaration.ret.clone(),
            regs: registers,
        })
    }

    fn emit_ret(&mut self, v: &EV) -> Result<(), String> {
        let mut comps = lty(self.p, &v.ty)?;
        let mut regs = v.regs.clone();
        for (pname, t) in self.inout_params.clone() {
            let out = self.load_var(&pname)?;
            comps.extend(lty(self.p, &t)?);
            regs.extend(out.regs);
        }
        match comps.len() {
            0 if self.in_main => self.line("ret i32 0".into()),
            0 => self.line("ret void".into()),
            1 => self.line(format!("ret {} {}", comps[0], regs[0])),
            _ => {
                let sty = comps_ty(&comps);
                let mut cur = "undef".to_string();
                for (i, (c, r)) in comps.iter().zip(regs.iter()).enumerate() {
                    let t = self.t();
                    self.line(format!(
                        "{} = insertvalue {} {}, {} {}, {}",
                        t, sty, cur, c, r, i
                    ));
                    cur = t;
                }
                self.line(format!("ret {} {}", sty, cur));
            }
        }
        self.terminated = true;
        Ok(())
    }

    fn lookup(&self, name: &str) -> Option<(CType, Vec<String>)> {
        self.env.iter().rev().find_map(|s| s.get(name).cloned())
    }

    fn load_var(&mut self, name: &str) -> Result<EV, String> {
        let (t, ptrs) = self
            .lookup(name)
            .ok_or(format!("unknown variable `{}`", name))?;
        let comps = lty(self.p, &t)?;
        let mut regs = Vec::new();
        for (c, ptr) in comps.iter().zip(ptrs.iter()) {
            let r = self.t();
            self.line(format!("{} = load {}, ptr {}", r, c, ptr));
            regs.push(r);
        }
        Ok(EV { ty: t, regs })
    }

    fn store_var(&mut self, name: &str, v: &EV) -> Result<(), String> {
        let (t, ptrs) = self
            .lookup(name)
            .ok_or(format!("unknown variable `{}`", name))?;
        let comps = lty(self.p, &t)?;
        for ((c, ptr), r) in comps.iter().zip(ptrs.iter()).zip(v.regs.iter()) {
            self.line(format!("store {} {}, ptr {}", c, r, ptr));
        }
        Ok(())
    }

    fn to_f64(&mut self, v: &EV) -> Result<String, String> {
        match v.ty {
            CType::I64 => {
                let t = self.t();
                self.line(format!("{} = sitofp i64 {} to double", t, v.regs[0]));
                Ok(t)
            }
            CType::F32 => {
                let t = self.t();
                self.line(format!("{} = fpext float {} to double", t, v.regs[0]));
                Ok(t)
            }
            _ => Ok(v.regs[0].clone()),
        }
    }

    fn coerce_ev(&mut self, value: EV, want: &CType) -> Result<EV, String> {
        if &value.ty == want {
            return Ok(value);
        }
        let reg = match (want, &value.ty) {
            (CType::F32, CType::I64) => {
                let out = self.t();
                self.line(format!("{} = sitofp i64 {} to float", out, value.regs[0]));
                out
            }
            (CType::F64, CType::I64) => self.to_f64(&value)?,
            (CType::F32, CType::F64) => {
                let out = self.t();
                self.line(format!(
                    "{} = fptrunc double {} to float",
                    out, value.regs[0]
                ));
                out
            }
            (CType::F64, CType::F32) => self.to_f64(&value)?,
            (CType::CSlice(want_element), CType::Arr(got_element))
            | (CType::CMutSlice(want_element), CType::Arr(got_element))
                if want_element == got_element =>
            {
                let slots = self.t();
                self.line(format!("{} = load i64, ptr {}", slots, value.regs[0]));
                let stride = lty(self.p, want_element)?.len();
                let length = if stride == 1 {
                    slots
                } else {
                    let length = self.t();
                    self.line(format!("{} = sdiv i64 {}, {}", length, slots, stride));
                    length
                };
                let data = self.t();
                self.line(format!(
                    "{} = getelementptr i8, ptr {}, i64 8",
                    data, value.regs[0]
                ));
                return Ok(EV {
                    ty: want.clone(),
                    regs: vec![data, length],
                });
            }
            _ => {
                return Err(format!(
                    "cannot coerce LLVM value {:?} to {:?}",
                    value.ty, want
                ))
            }
        };
        Ok(EV {
            ty: want.clone(),
            regs: vec![reg],
        })
    }

    fn elem_addrs(
        &mut self,
        base: &str,
        idx: &str,
        elem: &CType,
        trusted: Option<String>,
    ) -> Result<Vec<String>, String> {
        let stride = lty(self.p, elem)?.len() as i64;
        let logical = match trusted {
            Some(n) => n,
            None => {
                let lenr = self.t();
                self.line(format!("{} = load i64, ptr {}", lenr, base));
                let logical = if stride == 1 {
                    lenr.clone()
                } else {
                    let d = self.t();
                    self.line(format!("{} = sdiv i64 {}, {}", d, lenr, stride));
                    d
                };
                let bad = self.t();
                self.line(format!("{} = icmp uge i64 {}, {}", bad, idx, logical));
                let lb = self.l();
                let lg = self.l();
                self.line(format!("br i1 {}, label %{}, label %{}", bad, lb, lg));
                self.label(&lb);
                self.line(format!("call void @lu_oob(i64 {}, i64 {})", idx, logical));
                self.line("unreachable".into());
                self.label(&lg);
                logical
            }
        };
        let mut out = Vec::new();
        if stride > 1 && self.soa {
            let lane = self.t();
            self.line(format!("{} = mul i64 {}, 8", lane, idx));
            let lane8 = self.t();
            self.line(format!("{} = add i64 {}, 8", lane8, lane));
            for c in 0..stride {
                let plane = self.t();
                self.line(format!("{} = mul i64 {}, {}", plane, logical, 8 * c));
                let off = self.t();
                self.line(format!("{} = add i64 {}, {}", off, lane8, plane));
                let addr = self.t();
                self.line(format!(
                    "{} = getelementptr i8, ptr {}, i64 {}",
                    addr, base, off
                ));
                out.push(addr);
            }
        } else {
            let off = self.t();
            self.line(format!("{} = mul i64 {}, {}", off, idx, stride * 8));
            for c in 0..stride {
                let offc = self.t();
                self.line(format!("{} = add i64 {}, {}", offc, off, 8 + 8 * c));
                let addr = self.t();
                self.line(format!(
                    "{} = getelementptr i8, ptr {}, i64 {}",
                    addr, base, offc
                ));
                out.push(addr);
            }
        }
        Ok(out)
    }

    fn emit_checked_int_div(
        &mut self,
        lhs: &str,
        rhs: &str,
        remainder: bool,
    ) -> Result<EV, String> {
        let out = self.t();
        let callee = if remainder {
            "lu_i64_rem"
        } else {
            "lu_i64_div"
        };
        self.line(format!(
            "{} = call i64 @{}(i64 {}, i64 {})",
            out, callee, lhs, rhs
        ));
        Ok(EV {
            ty: CType::I64,
            regs: vec![out],
        })
    }

    fn emit_call(&mut self, name: &str, args: Vec<EV>) -> Result<EV, String> {
        match name {
            "print" => {
                for (i, v) in args.iter().enumerate() {
                    if i > 0 {
                        self.line("call void @lu_print_sep()".into());
                    }
                    match &v.ty {
                        CType::F32 => {
                            let value = self.to_f64(v)?;
                            self.line(format!("call void @lu_print_f64(double {})", value))
                        }
                        CType::F64 => {
                            self.line(format!("call void @lu_print_f64(double {})", v.regs[0]))
                        }
                        CType::I64 => {
                            self.line(format!("call void @lu_print_i64(i64 {})", v.regs[0]))
                        }
                        CType::Bool => {
                            self.line(format!("call void @lu_print_bool(i64 {})", v.regs[0]))
                        }
                        CType::Str => self.line(format!(
                            "call void @lu_print_str(ptr {}, i64 {})",
                            v.regs[0], v.regs[1]
                        )),
                        t => return Err(format!("cannot print {:?} in AOT yet", t)),
                    }
                }
                self.line("call void @lu_print_nl()".into());
                Ok(EV {
                    ty: CType::Unit,
                    regs: vec![],
                })
            }
            "puti" => {
                self.line(format!("call void @lu_print_i64(i64 {})", args[0].regs[0]));
                Ok(EV {
                    ty: CType::Unit,
                    regs: vec![],
                })
            }
            "putf" => {
                let value = self.to_f64(&args[0])?;
                self.line(format!("call void @lu_print_f64(double {})", value));
                Ok(EV {
                    ty: CType::Unit,
                    regs: vec![],
                })
            }
            "putb" => {
                self.line(format!("call void @lu_print_bool(i64 {})", args[0].regs[0]));
                Ok(EV {
                    ty: CType::Unit,
                    regs: vec![],
                })
            }
            "puts" => {
                self.line(format!(
                    "call void @lu_print_str(ptr {}, i64 {})",
                    args[0].regs[0], args[0].regs[1]
                ));
                Ok(EV {
                    ty: CType::Unit,
                    regs: vec![],
                })
            }
            "putsp" => {
                self.line("call void @lu_print_sep()".into());
                Ok(EV {
                    ty: CType::Unit,
                    regs: vec![],
                })
            }
            "putnl" => {
                self.line("call void @lu_print_nl()".into());
                Ok(EV {
                    ty: CType::Unit,
                    regs: vec![],
                })
            }
            "nargs" => {
                let t = self.t();
                self.line(format!("{} = call i64 @lu_nargs()", t));
                Ok(EV {
                    ty: CType::I64,
                    regs: vec![t],
                })
            }
            "arg" => {
                let p = self.t();
                self.line(format!("{} = call ptr @lu_arg(i64 {})", p, args[0].regs[0]));
                let l = self.t();
                self.line(format!("{} = call i64 @lu_last_len()", l));
                Ok(EV {
                    ty: CType::Str,
                    regs: vec![p, l],
                })
            }
            "read_file" => {
                let p = self.t();
                self.line(format!(
                    "{} = call ptr @lu_read_file(ptr {}, i64 {})",
                    p, args[0].regs[0], args[0].regs[1]
                ));
                let l = self.t();
                self.line(format!("{} = call i64 @lu_last_len()", l));
                Ok(EV {
                    ty: CType::Str,
                    regs: vec![p, l],
                })
            }
            "write_file" => {
                self.line(format!(
                    "call void @lu_write_file(ptr {}, i64 {}, ptr {}, i64 {})",
                    args[0].regs[0], args[0].regs[1], args[1].regs[0], args[1].regs[1]
                ));
                Ok(EV {
                    ty: CType::Unit,
                    regs: vec![],
                })
            }
            "chr" => {
                let p = self.t();
                self.line(format!("{} = call ptr @lu_chr(i64 {})", p, args[0].regs[0]));
                let l = self.t();
                self.line(format!("{} = call i64 @lu_last_len()", l));
                Ok(EV {
                    ty: CType::Str,
                    regs: vec![p, l],
                })
            }
            "concat" => {
                let p = self.t();
                self.line(format!(
                    "{} = call ptr @lu_concat(ptr {}, i64 {}, ptr {}, i64 {})",
                    p, args[0].regs[0], args[0].regs[1], args[1].regs[0], args[1].regs[1]
                ));
                let l = self.t();
                self.line(format!("{} = call i64 @lu_last_len()", l));
                Ok(EV {
                    ty: CType::Str,
                    regs: vec![p, l],
                })
            }
            "sqrt" | "sin" | "cos" | "abs" | "floor" => {
                let x = self.to_f64(&args[0])?;
                let intr = match name {
                    "sqrt" => "llvm.sqrt.f64",
                    "sin" => "llvm.sin.f64",
                    "cos" => "llvm.cos.f64",
                    "abs" => "llvm.fabs.f64",
                    _ => "llvm.floor.f64",
                };
                let t = self.t();
                self.line(format!("{} = call fast double @{}(double {})", t, intr, x));
                if args[0].ty == CType::F32 {
                    self.coerce_ev(
                        EV {
                            ty: CType::F64,
                            regs: vec![t],
                        },
                        &CType::F32,
                    )
                } else {
                    Ok(EV {
                        ty: CType::F64,
                        regs: vec![t],
                    })
                }
            }
            "acos" => {
                let x = self.to_f64(&args[0])?;
                let t = self.t();
                self.line(format!("{} = call fast double @acos(double {})", t, x));
                if args[0].ty == CType::F32 {
                    self.coerce_ev(
                        EV {
                            ty: CType::F64,
                            regs: vec![t],
                        },
                        &CType::F32,
                    )
                } else {
                    Ok(EV {
                        ty: CType::F64,
                        regs: vec![t],
                    })
                }
            }
            "min" | "max" | "pow" | "atan2" => {
                let a = self.to_f64(&args[0])?;
                let b = self.to_f64(&args[1])?;
                let t = self.t();
                let callee = match name {
                    "min" => "llvm.minnum.f64",
                    "max" => "llvm.maxnum.f64",
                    "pow" => "llvm.pow.f64",
                    _ => "atan2",
                };
                self.line(format!(
                    "{} = call fast double @{}(double {}, double {})",
                    t, callee, a, b
                ));
                if args.iter().all(|value| value.ty == CType::F32) {
                    self.coerce_ev(
                        EV {
                            ty: CType::F64,
                            regs: vec![t],
                        },
                        &CType::F32,
                    )
                } else {
                    Ok(EV {
                        ty: CType::F64,
                        regs: vec![t],
                    })
                }
            }
            "float" => {
                let x = self.to_f64(&args[0])?;
                Ok(EV {
                    ty: CType::F64,
                    regs: vec![x],
                })
            }
            "f32" => self.coerce_ev(args[0].clone(), &CType::F32),
            "int" => {
                if matches!(args[0].ty, CType::F32 | CType::F64) {
                    let t = self.t();
                    let source = if args[0].ty == CType::F32 {
                        "float"
                    } else {
                        "double"
                    };
                    self.line(format!(
                        "{} = fptosi {} {} to i64",
                        t, source, args[0].regs[0]
                    ));
                    Ok(EV {
                        ty: CType::I64,
                        regs: vec![t],
                    })
                } else {
                    // i64, bool, enum tag: already an integer register
                    Ok(EV {
                        ty: CType::I64,
                        regs: args[0].regs.clone(),
                    })
                }
            }
            "len" if args[0].ty == CType::Str => Ok(EV {
                ty: CType::I64,
                regs: vec![args[0].regs[1].clone()],
            }),
            "substr" => {
                let (p0, l0) = (args[0].regs[0].clone(), args[0].regs[1].clone());
                let (lo, hi) = (args[1].regs[0].clone(), args[2].regs[0].clone());
                let neg = self.t();
                self.line(format!("{} = icmp slt i64 {}, 0", neg, lo));
                let inv = self.t();
                self.line(format!("{} = icmp slt i64 {}, {}", inv, hi, lo));
                let over = self.t();
                self.line(format!("{} = icmp sgt i64 {}, {}", over, hi, l0));
                let b1 = self.t();
                self.line(format!("{} = or i1 {}, {}", b1, neg, inv));
                let bad = self.t();
                self.line(format!("{} = or i1 {}, {}", bad, b1, over));
                let lb = self.l();
                let lg = self.l();
                self.line(format!("br i1 {}, label %{}, label %{}", bad, lb, lg));
                self.label(&lb);
                self.line(format!("call void @lu_oob(i64 {}, i64 {})", hi, l0));
                self.line("unreachable".into());
                self.label(&lg);
                let np = self.t();
                self.line(format!("{} = getelementptr i8, ptr {}, i64 {}", np, p0, lo));
                let nl = self.t();
                self.line(format!("{} = sub i64 {}, {}", nl, hi, lo));
                Ok(EV {
                    ty: CType::Str,
                    regs: vec![np, nl],
                })
            }
            "len" => {
                let elem = match &args[0].ty {
                    CType::Arr(e) => e.as_ref().clone(),
                    CType::CSlice(_) | CType::CMutSlice(_) => {
                        return Ok(EV {
                            ty: CType::I64,
                            regs: vec![args[0].regs[1].clone()],
                        });
                    }
                    _ => return Err("`len` expects array".into()),
                };
                let stride = lty(self.p, &elem)?.len() as i64;
                let n = self.t();
                self.line(format!("{} = load i64, ptr {}", n, args[0].regs[0]));
                if stride == 1 {
                    Ok(EV {
                        ty: CType::I64,
                        regs: vec![n],
                    })
                } else {
                    let d = self.t();
                    self.line(format!("{} = sdiv i64 {}, {}", d, n, stride));
                    Ok(EV {
                        ty: CType::I64,
                        regs: vec![d],
                    })
                }
            }
            "arr" => {
                let n = &args[0].regs[0];
                let t = self.t();
                match &args[1].ty {
                    CType::F64 => {
                        self.line(format!(
                            "{} = call ptr @lu_arr_new_f64(i64 {}, double {})",
                            t, n, args[1].regs[0]
                        ));
                        Ok(EV {
                            ty: CType::Arr(Box::new(CType::F64)),
                            regs: vec![t],
                        })
                    }
                    CType::I64 => {
                        self.line(format!(
                            "{} = call ptr @lu_arr_new_i64(i64 {}, i64 {})",
                            t, n, args[1].regs[0]
                        ));
                        Ok(EV {
                            ty: CType::Arr(Box::new(CType::I64)),
                            regs: vec![t],
                        })
                    }
                    t @ (CType::Bool | CType::Enum(_)) => {
                        let elem = t.clone();
                        let r = self.t();
                        self.line(format!(
                            "{} = call ptr @lu_arr_new_i64(i64 {}, i64 {})",
                            r, n, args[1].regs[0]
                        ));
                        Ok(EV {
                            ty: CType::Arr(Box::new(elem)),
                            regs: vec![r],
                        })
                    }
                    t @ (CType::F32 | CType::Rec(_) | CType::Str) => {
                        let elem = t.clone();
                        let stride = lty(self.p, &elem)?.len() as i64;
                        let base = self.t();
                        self.line(format!(
                            "{} = call ptr @lu_arr_new_raw(i64 {}, i64 {})",
                            base, n, stride
                        ));
                        // fill loop over logical elements, SoA planes by default
                        let iptr = self.t();
                        self.line(format!("{} = alloca i64", iptr));
                        self.line(format!("store i64 0, ptr {}", iptr));
                        let lh = self.l();
                        let lb = self.l();
                        let lx = self.l();
                        let n = n.clone();
                        self.line(format!("br label %{}", lh));
                        self.label(&lh);
                        let iv = self.t();
                        self.line(format!("{} = load i64, ptr {}", iv, iptr));
                        let more = self.t();
                        self.line(format!("{} = icmp slt i64 {}, {}", more, iv, n));
                        self.line(format!("br i1 {}, label %{}, label %{}", more, lb, lx));
                        self.label(&lb);
                        let addrs = self.elem_addrs(&base, &iv, &elem, Some(n.clone()))?;
                        let comps = lty(self.p, &elem)?;
                        for ((c, r), ep) in comps.iter().zip(args[1].regs.iter()).zip(addrs.iter())
                        {
                            self.line(format!("store {} {}, ptr {}", c, r, ep));
                        }
                        let ivn = self.t();
                        self.line(format!("{} = add i64 {}, 1", ivn, iv));
                        self.line(format!("store i64 {}, ptr {}", ivn, iptr));
                        self.line(format!("br label %{}", lh));
                        self.label(&lx);
                        Ok(EV {
                            ty: CType::Arr(Box::new(elem)),
                            regs: vec![base],
                        })
                    }
                    t => Err(format!("arr of {:?} unsupported in AOT", t)),
                }
            }
            _ => Err(format!("unknown builtin `{}`", name)),
        }
    }
}
