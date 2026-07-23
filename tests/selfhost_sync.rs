use std::path::Path;
use std::process::{Command, Output};

fn run(command: &mut Command) -> Output {
    command.output().expect("run command")
}

fn between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let start = source.find(start).expect("shared-region start marker");
    let end = source[start..]
        .find(end)
        .map(|offset| start + offset)
        .expect("shared-region end marker");
    &source[start..end]
}

fn emit_host_ir(source: &Path, output: &Path) -> String {
    let built = run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["build", "--emit-llvm", "-o"])
        .arg(output)
        .arg(source));
    assert!(
        built.status.success(),
        "host LLVM emission failed: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    std::fs::read_to_string(output).expect("read host LLVM")
}

fn emit_selfhost_ir(repository: &Path, source: &Path, triple: &str) -> String {
    let emitted = run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .arg("run")
        .arg(repository.join("selfhost/codegen.lu"))
        .arg(source)
        .arg(triple));
    assert!(
        emitted.status.success(),
        "self-hosted LLVM emission failed: {}",
        String::from_utf8_lossy(&emitted.stderr)
    );
    String::from_utf8(emitted.stdout).expect("self-hosted LLVM is UTF-8")
}

fn target_triple(module: &str) -> &str {
    module
        .lines()
        .find_map(|line| {
            line.strip_prefix("target triple = \"")
                .and_then(|rest| rest.strip_suffix('"'))
        })
        .expect("target triple in host LLVM")
}

fn compile_ir(repository: &Path, ir: &Path, output: &Path) {
    let compiled = run(Command::new("clang")
        .args(["-O3", "-mcpu=native"])
        .arg("-o")
        .arg(output)
        .arg(ir)
        .arg(repository.join("src/lu_runtime.c")));
    assert!(
        compiled.status.success(),
        "LLVM fixture compilation failed: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
}

#[test]
fn host_and_selfhost_emit_correct_explicit_simd_with_scalar_tails() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let directory =
        std::env::temp_dir().join(format!("lulang-selfhost-simd-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create SIMD sync directory");
    let source = directory.join("odd-dot.lu");
    std::fs::write(
        &source,
        "main {\n\
           let n = 11\n\
           var a = arr(n, 0.0)\n\
           var b = arr(n, 0.0)\n\
           for i in 0..n {\n\
             a[i] = float(i)\n\
             b[i] = float(i + 1)\n\
           }\n\
           print(sum(i in 0..n) a[i] * b[i])\n\
         }\n",
    )
    .expect("write SIMD fixture");

    let host_path = directory.join("host.ll");
    let host = emit_host_ir(&source, &host_path);
    let selfhost = emit_selfhost_ir(&repository, &source, target_triple(&host));
    assert!(
        host.contains("fmul fast <2 x double>"),
        "host did not lower the shared SIMD plan"
    );
    assert!(
        selfhost.contains("fmul fast <2 x double>"),
        "selfhost did not mirror the shared SIMD plan"
    );
    assert!(
        host.contains("icmp slt i64") && selfhost.contains("icmp slt i64"),
        "both backends need a scalar tail"
    );
    let scalar_path = directory.join("host-scalar.ll");
    let scalar_build = run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .env("LU_SIMD", "off")
        .args(["build", "--emit-llvm", "-o"])
        .arg(&scalar_path)
        .arg(&source));
    assert!(
        scalar_build.status.success(),
        "scalar fallback emission failed: {}",
        String::from_utf8_lossy(&scalar_build.stderr)
    );
    let scalar = std::fs::read_to_string(&scalar_path).expect("read scalar fallback IR");
    assert!(
        !scalar.contains("fmul fast <2 x double>"),
        "LU_SIMD=off must preserve the scalar fallback"
    );

    let selfhost_path = directory.join("selfhost.ll");
    std::fs::write(&selfhost_path, selfhost).expect("write selfhost SIMD IR");
    for (name, ir) in [("host", &host_path), ("selfhost", &selfhost_path)] {
        let binary = directory.join(name);
        compile_ir(&repository, ir, &binary);
        let output = run(&mut Command::new(binary));
        assert!(output.status.success(), "{name} SIMD fixture failed");
        assert_eq!(output.stdout, b"440\n", "{name} scalar tail disagreed");
    }
}

#[test]
fn selfhost_frontends_are_byte_identical() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let interpreter =
        std::fs::read_to_string(repository.join("selfhost/interp.lu")).expect("read interp.lu");
    let codegen =
        std::fs::read_to_string(repository.join("selfhost/codegen.lu")).expect("read codegen.lu");

    let interpreter_frontend = between(
        &interpreter,
        "// ---------- lexer",
        "// ---------- evaluator",
    );
    let codegen_frontend = between(
        &codegen,
        "// ---------- lexer",
        "// ---------- LLVM IR emitter",
    );
    assert_eq!(
        interpreter_frontend, codegen_frontend,
        "the self-hosted parser/checker region must be copied byte-for-byte"
    );
}

#[test]
fn ffi_import_and_export_ir_match_the_host_byte_for_byte() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let directory =
        std::env::temp_dir().join(format!("lulang-selfhost-sync-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create selfhost sync directory");

    let import_source = repository.join("corpus/ffi_cbrt.lu");
    let host_import_path = directory.join("host-import.ll");
    let host_import = emit_host_ir(&import_source, &host_import_path);
    let triple = target_triple(&host_import);
    let selfhost_import = emit_selfhost_ir(&repository, &import_source, triple);
    assert_eq!(
        host_import, selfhost_import,
        "extern declarations and link metadata drifted between host and selfhost"
    );

    let export_source = directory.join("scalar-export.lu");
    std::fs::write(
        &export_source,
        "export fn twice(x: i64): i64 { return x * 2 }\n\
         main { print(twice(21)) }\n",
    )
    .expect("write export fixture");
    let host_export_path = directory.join("host-export.ll");
    let host_export = emit_host_ir(&export_source, &host_export_path);
    let selfhost_export = emit_selfhost_ir(&repository, &export_source, triple);
    assert_eq!(
        host_export, selfhost_export,
        "export wrappers drifted between host and selfhost"
    );

    let f32_source = directory.join("f32-boundary.lu");
    std::fs::write(
        &f32_source,
        "extern \"m\" fn cbrtf(x: f32): f32\n\
         export fn half32(x: f32): f32 {\n\
           return x * f32(0.5)\n\
         }\n\
         main { print(float(cbrtf(f32(27)))) }\n",
    )
    .expect("write f32 boundary fixture");
    let host_f32_path = directory.join("host-f32.ll");
    let host_f32 = emit_host_ir(&f32_source, &host_f32_path);
    let selfhost_f32 = emit_selfhost_ir(&repository, &f32_source, triple);
    assert_eq!(
        host_f32, selfhost_f32,
        "direct f32 imports and exports drifted between host and selfhost"
    );

    let c_slice_source = directory.join("c-slice-boundary.lu");
    std::fs::write(
        &c_slice_source,
        "export fn borrowed_sum(values: c_slice[f64]): f64 {\n\
           return sum(i in 0..len(values)) values[i]\n\
         }\n\
         main {\n\
           let values = arr(3, 2.0)\n\
           print(borrowed_sum(values))\n\
         }\n",
    )
    .expect("write c_slice boundary fixture");
    let host_c_slice_path = directory.join("host-c-slice.ll");
    let host_c_slice = emit_host_ir(&c_slice_source, &host_c_slice_path);
    let selfhost_c_slice = emit_selfhost_ir(&repository, &c_slice_source, triple);
    let wrapper_start = "define dso_local double @\"borrowed_sum\"";
    let host_wrapper = host_c_slice
        .split(wrapper_start)
        .nth(1)
        .and_then(|rest| rest.split("\n}\n").next())
        .expect("host c_slice wrapper");
    let selfhost_wrapper = selfhost_c_slice
        .split(wrapper_start)
        .nth(1)
        .and_then(|rest| rest.split("\n}\n").next())
        .expect("selfhost c_slice wrapper");
    assert_eq!(
        host_wrapper, selfhost_wrapper,
        "borrowed c_slice export wrapper drifted between host and selfhost"
    );
    assert!(host_wrapper.contains("(ptr %c0, i64 %c1)"));
    assert!(!host_wrapper.contains("lu_arr_new_raw"));

    let c_mut_slice_source = directory.join("c-mut-slice-boundary.lu");
    std::fs::write(
        &c_mut_slice_source,
        "export fn borrowed_bump(values: c_mut_slice[f64]): f64 {\n\
           for i in 0..len(values) {\n\
             values[i] = values[i] + 1.0\n\
           }\n\
           return values[0]\n\
         }\n\
         main { print(0) }\n",
    )
    .expect("write c_mut_slice boundary fixture");
    let host_c_mut_slice_path = directory.join("host-c-mut-slice.ll");
    let host_c_mut_slice = emit_host_ir(&c_mut_slice_source, &host_c_mut_slice_path);
    let selfhost_c_mut_slice = emit_selfhost_ir(&repository, &c_mut_slice_source, triple);
    let wrapper_start = "define dso_local double @\"borrowed_bump\"";
    let host_wrapper = host_c_mut_slice
        .split(wrapper_start)
        .nth(1)
        .and_then(|rest| rest.split("\n}\n").next())
        .expect("host c_mut_slice wrapper");
    let selfhost_wrapper = selfhost_c_mut_slice
        .split(wrapper_start)
        .nth(1)
        .and_then(|rest| rest.split("\n}\n").next())
        .expect("selfhost c_mut_slice wrapper");
    assert_eq!(
        host_wrapper, selfhost_wrapper,
        "mutable c_mut_slice export wrapper drifted between host and selfhost"
    );
    assert!(host_wrapper.contains("(ptr %c0, i64 %c1)"));
    assert!(!host_wrapper.contains("lu_arr_new_raw"));
    assert!(!host_wrapper.contains("lu_arr_cow"));

    let c_layout_source = directory.join("c-layout-value-boundary.lu");
    std::fs::write(
        &c_layout_source,
        "@c_layout type Vec2 { x: f64, y: f64 }\n\
         extern \"m\" fn vec2_sum(value: Vec2): f64\n\
         export fn local_sum(value: Vec2): f64 {\n\
           return value.x + value.y\n\
         }\n\
         main { print(local_sum(Vec2 { 2.5, 4.5 })) }\n",
    )
    .expect("write c_layout value boundary fixture");
    let host_c_layout_path = directory.join("host-c-layout-value.ll");
    let host_c_layout = emit_host_ir(&c_layout_source, &host_c_layout_path);
    let selfhost_c_layout = emit_selfhost_ir(&repository, &c_layout_source, triple);
    let declaration = "declare double @\"vec2_sum\"({ double, double })";
    assert_eq!(
        host_c_layout.lines().find(|line| *line == declaration),
        selfhost_c_layout.lines().find(|line| *line == declaration),
        "direct @c_layout import declarations drifted between host and selfhost"
    );
    let wrapper_start = "define dso_local double @\"local_sum\"({ double, double } %c0)";
    let host_c_layout_wrapper = host_c_layout
        .split(wrapper_start)
        .nth(1)
        .and_then(|rest| rest.split("\n}\n").next())
        .expect("host c_layout wrapper");
    let selfhost_c_layout_wrapper = selfhost_c_layout
        .split(wrapper_start)
        .nth(1)
        .and_then(|rest| rest.split("\n}\n").next())
        .expect("selfhost c_layout wrapper");
    assert_eq!(
        host_c_layout_wrapper, selfhost_c_layout_wrapper,
        "direct @c_layout export wrappers drifted between host and selfhost"
    );

    let string_return_source = directory.join("string-return-boundary.lu");
    std::fs::write(
        &string_return_source,
        "extern \"labels\" fn make_label(prefix: str): str\n\
         export fn greeting(prefix: str): str {\n\
           return concat(prefix, \"!\")\n\
         }\n\
         main { print(greeting(\"hi\")) }\n",
    )
    .expect("write string return boundary fixture");
    let host_string_return_path = directory.join("host-string-return.ll");
    let host_string_return = emit_host_ir(&string_return_source, &host_string_return_path);
    let selfhost_string_return = emit_selfhost_ir(&repository, &string_return_source, triple);
    let declaration = "declare ptr @\"make_label\"(ptr, i64, ptr)";
    assert_eq!(
        host_string_return.lines().find(|line| *line == declaration),
        selfhost_string_return
            .lines()
            .find(|line| *line == declaration),
        "string-return import declarations drifted between host and selfhost"
    );
    let wrapper_start = "define dso_local ptr @\"greeting\"(ptr %c0, i64 %c1, ptr %c2)";
    let host_string_wrapper = host_string_return
        .split(wrapper_start)
        .nth(1)
        .and_then(|rest| rest.split("\n}\n").next())
        .expect("host string-return wrapper");
    let selfhost_string_wrapper = selfhost_string_return
        .split(wrapper_start)
        .nth(1)
        .and_then(|rest| rest.split("\n}\n").next())
        .expect("selfhost string-return wrapper");
    assert_eq!(
        host_string_wrapper, selfhost_string_wrapper,
        "string-return export wrappers drifted between host and selfhost"
    );

    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn selfhost_preserves_array_value_semantics() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let directory =
        std::env::temp_dir().join(format!("lulang-selfhost-values-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create selfhost value directory");
    let source = directory.join("array-values.lu");
    std::fs::write(
        &source,
        "type Holder { values: [i64] }\n\
         main {\n\
           var original = arr(1, 1)\n\
           var direct = original\n\
           direct[0] = 2\n\
           var left = Holder { values: original }\n\
           var right = left\n\
           right.values[0] = 3\n\
           var assigned = arr(1, 0)\n\
           assigned = original\n\
           assigned[0] = 4\n\
           print(original[0], direct[0], left.values[0], right.values[0], assigned[0])\n\
         }\n",
    )
    .expect("write array value fixture");

    let host_ir_path = directory.join("host.ll");
    let host_ir = emit_host_ir(&source, &host_ir_path);
    let triple = target_triple(&host_ir);
    let selfhost_ir = emit_selfhost_ir(&repository, &source, triple);
    assert!(
        selfhost_ir.matches("call ptr @lu_arr_clone").count() >= 5,
        "selfhost did not retain persistent array values"
    );
    let selfhost_ir_path = directory.join("selfhost.ll");
    std::fs::write(&selfhost_ir_path, selfhost_ir).expect("write selfhost LLVM");
    let selfhost_binary = directory.join("selfhost");
    compile_ir(&repository, &selfhost_ir_path, &selfhost_binary);

    let host_binary = directory.join("host");
    compile_ir(&repository, &host_ir_path, &host_binary);
    let host = run(&mut Command::new(&host_binary));
    let selfhost = run(&mut Command::new(&selfhost_binary));
    assert!(host.status.success());
    assert!(selfhost.status.success());
    assert_eq!(host.stdout, b"1 2 1 3 4\n");
    assert_eq!(
        selfhost.stdout, host.stdout,
        "selfhost violated array value semantics"
    );

    let _ = std::fs::remove_dir_all(directory);
}
