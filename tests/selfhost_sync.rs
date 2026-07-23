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
