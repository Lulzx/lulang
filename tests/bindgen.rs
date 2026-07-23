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
        .args(["bindgen", "--no-shims", "--lib", "m", "-o"])
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
    assert!(source.contains("@c_layout type mini_vector"));
    assert!(source.contains("x: f64"));
    assert!(source.contains("extern \"m\" fn hypot(x: f64, y: f64): f64"));
    assert!(source.contains("extern \"m\" fn clamp_index(value: i64, low: i64, high: i64): i64"));
    assert!(source.contains("extern \"m\" fn narrow_float(value: f32): f32"));
    assert!(source.contains("extern \"m\" fn allocate_bytes(size: i64): c_ptr[()]"));
    assert!(!source.contains("extern \"m\" fn consume_vector"));

    let diagnostics = String::from_utf8_lossy(&generated.stderr);
    assert!(diagnostics.contains("@c_layout"));
    assert!(diagnostics.contains("generated 4 C import(s)"));

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
    assert!(
        String::from_utf8_lossy(&generated.stderr).contains("built 4 C adapter shim(s)"),
        "missing shim build report:\n{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let generated_source = std::fs::read_to_string(&bindings).expect("read generated bindings");
    assert!(generated_source.contains("fn bindgen_half(value: f32): f32"));
    assert!(generated_source.contains("fn bindgen_increment_i32(value: i64): i64"));
    assert!(generated_source.contains("fn bindgen_is_positive(value: i64): bool"));
    assert!(generated_source.contains("fn bindgen_pair_sum(value: bindgen_pair): f64"));
    assert!(generated_source.contains("type bindgen_mixed"));
    assert!(generated_source.contains("count: i64"));
    assert!(generated_source.contains("scale: f32"));
    let shim_source = std::fs::read_to_string(bindings.with_extension("bindgen.c"))
        .expect("read generated C adapter");
    assert!(shim_source.contains("(struct bindgen_pair){ .x = arg0_x, .y = arg0_y }"));
    let mut bindings_file = OpenOptions::new()
        .append(true)
        .open(&bindings)
        .expect("open generated bindings");
    write!(
        bindings_file,
        "\nmain {{\n  print(bindgen_add(20, 22))\n  print(bindgen_scale(1.5, 2.0))\n  print(bindgen_half(9.0))\n  print(bindgen_increment_i32(41))\n  print(bindgen_is_positive(3))\n  print(bindgen_pair_sum(bindgen_pair {{ x: 1.25, y: 2.75 }}))\n  print(bindgen_mixed_value(bindgen_mixed {{ count: 3, scale: 1.5 }}))\n  let box = bindgen_box_new(99)\n  print(bindgen_box_read(box))\n  bindgen_box_free(box)\n}}\n"
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
            "42\n3\n4.5\n42\ntrue\n4\n4.5\n99\n",
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
    assert_eq!(
        String::from_utf8_lossy(&executed.stdout),
        "42\n3\n4.5\n42\ntrue\n4\n4.5\n99\n"
    );

    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let selfhost_interp = Command::new(lu())
        .arg("run")
        .arg(repository.join("selfhost/interp.lu"))
        .arg(&bindings)
        .output()
        .expect("run bindings through self-hosted interpreter");
    assert!(
        selfhost_interp.status.success(),
        "self-hosted interpreter failed:\n{}",
        String::from_utf8_lossy(&selfhost_interp.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&selfhost_interp.stdout),
        "42\n3\n4.5\n42\ntrue\n4\n4.5\n99\n"
    );

    let selfhost_executable = directory.join("bindgen_runtime_selfhost");
    let selfhost_build = Command::new(repository.join("selfhost/build.sh"))
        .arg(&bindings)
        .arg("-o")
        .arg(&selfhost_executable)
        .current_dir(&repository)
        .output()
        .expect("build bindings with self-hosted compiler");
    assert!(
        selfhost_build.status.success(),
        "self-hosted build failed:\n{}",
        String::from_utf8_lossy(&selfhost_build.stderr)
    );
    let selfhost_output = Command::new(&selfhost_executable)
        .output()
        .expect("run self-hosted bindings");
    assert!(
        selfhost_output.status.success(),
        "self-hosted executable failed:\n{}",
        String::from_utf8_lossy(&selfhost_output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&selfhost_output.stdout),
        "42\n3\n4.5\n42\ntrue\n4\n4.5\n99\n"
    );

    let _ = std::fs::remove_file(&bindings);
    let _ = std::fs::remove_file(bindings.with_extension("bindgen.c"));
    let _ = std::fs::remove_file(bindings.with_extension(if cfg!(target_os = "macos") {
        "bindgen.dylib"
    } else {
        "bindgen.so"
    }));
    let _ = std::fs::remove_file(&library);
    let _ = std::fs::remove_file(&executable);
    let _ = std::fs::remove_file(&selfhost_executable);
    let _ = std::fs::remove_dir(&directory);
}
