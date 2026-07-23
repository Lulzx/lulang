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

    let _ = std::fs::remove_dir_all(directory);
}
