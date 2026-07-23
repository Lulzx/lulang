use std::path::PathBuf;
use std::process::Command;
use std::{fs::OpenOptions, io::Write};

fn lu() -> &'static str {
    env!("CARGO_BIN_EXE_lu")
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/mini_bindgen.h")
}

fn fixture_named(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

#[test]
fn bindgen_generates_checker_valid_imports_and_reports_deferred_types() {
    let directory = std::env::temp_dir().join(format!("lulang-bindgen-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create temporary directory");
    let output = directory.join("mini_bindgen.lu");

    let generated = Command::new(lu())
        .args(["bindgen", "--lib", "m", "-o"])
        .arg(&output)
        .arg(fixture())
        .output()
        .expect("run lu bindgen");
    assert!(
        generated.status.success(),
        "bindgen failed:\n{}",
        String::from_utf8_lossy(&generated.stderr)
    );

    let source = std::fs::read_to_string(&output).expect("read generated source");
    assert!(source.contains("enum mini_state"));
    assert!(source.contains("fn MINI_LIMIT(): i64"));
    assert!(source.contains("fn MINI_RATE(): f64"));
    assert!(source.contains("extern \"m\" fn hypot(x: f64, y: f64): f64"));
    assert!(source.contains("extern \"m\" fn clamp_index(value: i64, low: i64, high: i64): i64"));
    assert!(!source.contains("extern \"m\" fn narrow_float"));
    assert!(source.contains("extern \"m\" fn allocate_bytes(size: i64): c_ptr[()]"));
    assert!(!source.contains("extern \"m\" fn consume_vector"));

    let diagnostics = String::from_utf8_lossy(&generated.stderr);
    assert!(diagnostics.contains("C float requires direct f32 boundary support"));
    assert!(diagnostics.contains("@c_layout"));
    assert!(diagnostics.contains("generated 3 C import(s)"));

    let checked = Command::new(lu())
        .args(["check"])
        .arg(&output)
        .output()
        .expect("check generated bindings");
    assert!(
        checked.status.success(),
        "generated source failed to check:\n{}",
        String::from_utf8_lossy(&checked.stderr)
    );

    let _ = std::fs::remove_file(&output);
    let _ = std::fs::remove_dir(&directory);
}

#[test]
fn generated_imports_call_a_compiled_c_library() {
    let directory =
        std::env::temp_dir().join(format!("lulang-bindgen-runtime-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create temporary directory");
    let library = if cfg!(target_os = "macos") {
        directory.join("libbindgen_runtime.dylib")
    } else {
        directory.join("libbindgen_runtime.so")
    };
    let mut compiler = Command::new("cc");
    if cfg!(target_os = "macos") {
        compiler.arg("-dynamiclib");
    } else {
        compiler.args(["-shared", "-fPIC"]);
    }
    let compiled = compiler
        .arg(fixture_named("bindgen_runtime.c"))
        .arg("-o")
        .arg(&library)
        .output()
        .expect("compile C fixture");
    assert!(
        compiled.status.success(),
        "C fixture failed to compile:\n{}",
        String::from_utf8_lossy(&compiled.stderr)
    );

    let bindings = directory.join("bindgen_runtime.lu");
    let generated = Command::new(lu())
        .args(["bindgen", "--lib"])
        .arg(&library)
        .args(["-o"])
        .arg(&bindings)
        .arg(fixture_named("bindgen_runtime.h"))
        .output()
        .expect("generate runtime bindings");
    assert!(
        generated.status.success(),
        "bindgen failed:\n{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let mut bindings_file = OpenOptions::new()
        .append(true)
        .open(&bindings)
        .expect("open generated bindings");
    write!(
        bindings_file,
        "\nmain {{\n  print(bindgen_add(20, 22))\n  print(bindgen_scale(1.5, 2.0))\n  let box = bindgen_box_new(99)\n  print(bindgen_box_read(box))\n  bindgen_box_free(box)\n}}\n"
    )
    .expect("append test program");

    for mode in ["interp", "run"] {
        let executed = Command::new(lu())
            .args([mode])
            .arg(&bindings)
            .output()
            .expect("execute generated bindings");
        assert!(
            executed.status.success(),
            "{mode} generated bindings failed:\n{}",
            String::from_utf8_lossy(&executed.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&executed.stdout),
            "42\n3\n99\n",
            "unexpected {mode} output"
        );
    }

    let executable = directory.join("bindgen_runtime_aot");
    let built = Command::new(lu())
        .args(["build", "-o"])
        .arg(&executable)
        .arg(&bindings)
        .output()
        .expect("build generated bindings");
    assert!(
        built.status.success(),
        "AOT build failed:\n{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let executed = Command::new(&executable)
        .output()
        .expect("execute AOT generated bindings");
    assert!(
        executed.status.success(),
        "AOT generated bindings failed:\n{}",
        String::from_utf8_lossy(&executed.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&executed.stdout), "42\n3\n99\n");

    let _ = std::fs::remove_file(&bindings);
    let _ = std::fs::remove_file(&library);
    let _ = std::fs::remove_file(&executable);
    let _ = std::fs::remove_dir(&directory);
}
