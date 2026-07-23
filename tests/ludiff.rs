use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn run(command: &mut Command) -> Output {
    let output = command.output().expect("start command");
    assert!(
        output.status.success(),
        "command failed: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn library_path(base: &Path) -> PathBuf {
    let parent = base.parent().unwrap_or_else(|| Path::new(""));
    let name = base.file_name().and_then(|name| name.to_str()).unwrap();
    parent.join(format!(
        "lib{name}.{}",
        if cfg!(target_os = "macos") {
            "dylib"
        } else {
            "so"
        }
    ))
}

#[test]
fn forward_duals_are_library_code_across_all_execution_tiers() {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let package = repository.join("lib/ludiff");
    let directory = std::env::temp_dir().join(format!("lulang-ludiff-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create ludiff test directory");

    let properties = run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["test", "--runs", "75"])
        .current_dir(&package));
    let properties = String::from_utf8_lossy(&properties.stdout);
    for law in [
        "product_rule",
        "quotient_rule",
        "sine_chain_rule",
        "power_rule",
        "agrees_with_central_difference",
    ] {
        assert!(
            properties.contains(&format!("property {law} ... ok (75 runs)")),
            "missing law {law}:\n{properties}"
        );
    }

    let mut expected = None;
    for mode in ["interp", "run"] {
        let output = run(Command::new(env!("CARGO_BIN_EXE_lu"))
            .arg(mode)
            .current_dir(&package));
        assert!(String::from_utf8_lossy(&output.stdout).starts_with("ludiff\nf(2):"));
        if let Some(expected) = &expected {
            assert_eq!(&output.stdout, expected);
        } else {
            expected = Some(output.stdout);
        }
    }
    let expected = expected.expect("reference output");

    let native = directory.join("ludiff-demo");
    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["build", "-o"])
        .arg(&native)
        .current_dir(&package));
    assert_eq!(run(&mut Command::new(&native)).stdout, expected);

    let combined = directory.join("ludiff-combined.lu");
    let mut source =
        std::fs::read_to_string(package.join("src/lib.lu")).expect("read ludiff library");
    source.push_str(
        &std::fs::read_to_string(package.join("src/main.lu")).expect("read ludiff example"),
    );
    std::fs::write(&combined, source).expect("write combined selfhost source");
    let selfhost = run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .arg("run")
        .arg(repository.join("selfhost/interp.lu"))
        .arg(&combined));
    assert_eq!(selfhost.stdout, expected);

    let base = directory.join("ludiff");
    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["build", "--lib", "--shared", "-o"])
        .arg(&base)
        .arg(package.join("src/lib.lu")));
    let header = directory.join("ludiff.h");
    assert!(std::fs::read_to_string(&header)
        .expect("read ludiff header")
        .contains("double polynomial_derivative(double value);"));
    let harness = directory.join("embed.c");
    std::fs::write(
        &harness,
        "#include <stdio.h>\n#include \"ludiff.h\"\nint main(void) {\n  printf(\"%.6f\\n\", polynomial_derivative(2.0));\n}\n",
    )
    .expect("write C embedding harness");
    let embedded = directory.join("embed");
    run(Command::new("cc")
        .arg(&harness)
        .arg(library_path(&base))
        .arg("-o")
        .arg(&embedded));
    assert_eq!(run(&mut Command::new(&embedded)).stdout, b"9.385426\n");

    if Command::new("zig")
        .arg("version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        let wasm = directory.join("ludiff.wasm");
        run(Command::new(env!("CARGO_BIN_EXE_lu"))
            .args(["build", "--target", "wasm32-wasi", "-o"])
            .arg(&wasm)
            .current_dir(&package));
        assert_eq!(
            &std::fs::read(wasm).expect("read ludiff wasm")[..4],
            b"\0asm"
        );
    }

    let _ = std::fs::remove_dir_all(directory);
}
