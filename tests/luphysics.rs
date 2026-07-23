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
fn flagship_physics_package_runs_properties_targets_and_c_embedding() {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let package = repository.join("lib/luphysics");
    let directory = std::env::temp_dir().join(format!("lulang-luphysics-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create luphysics test directory");

    let properties = run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["test", "--runs", "50"])
        .current_dir(&package));
    let properties = String::from_utf8_lossy(&properties.stdout);
    assert!(properties.contains("elastic_circle_collision_preserves_momentum ... ok (50 runs)"));
    assert!(properties.contains("elastic_circle_collision_preserves_energy ... ok (50 runs)"));

    for mode in ["interp", "run"] {
        let output = run(Command::new(env!("CARGO_BIN_EXE_lu"))
            .arg(mode)
            .current_dir(&package));
        let output = String::from_utf8_lossy(&output.stdout);
        assert!(output.starts_with("luphysics\nbody 0:"));
        assert!(output.contains("\nmomentum drift:"));
        assert!(output.contains("\nkinetic energy:"));
    }

    let native = directory.join("luphysics-demo");
    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["build", "-o"])
        .arg(&native)
        .current_dir(&package));
    let native_output = run(&mut Command::new(&native));
    assert!(String::from_utf8_lossy(&native_output.stdout).starts_with("luphysics\n"));

    let base = directory.join("luphysics");
    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["build", "--lib", "--shared", "-o"])
        .arg(&base)
        .arg(package.join("src/lib.lu")));
    let header = directory.join("luphysics.h");
    assert!(std::fs::read_to_string(&header)
        .expect("read luphysics header")
        .contains("void integrate_axis(double *position_data"));
    let harness = directory.join("embed.c");
    std::fs::write(
        &harness,
        "#include <stdio.h>\n#include \"luphysics.h\"\nint main(void) {\n  double p[2] = {0, 1};\n  double v[2] = {1, 2};\n  integrate_axis(p, 2, v, 2, 2, 0.5, 2);\n  printf(\"%.1f %.1f\\n\", p[0], p[1]);\n}\n",
    )
    .expect("write C embedding harness");
    let embedded = directory.join("embed");
    run(Command::new("cc")
        .arg(&harness)
        .arg(library_path(&base))
        .arg("-o")
        .arg(&embedded));
    assert_eq!(run(&mut Command::new(&embedded)).stdout, b"1.0 2.5\n");

    if Command::new("zig")
        .arg("version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        let wasm = directory.join("luphysics.wasm");
        run(Command::new(env!("CARGO_BIN_EXE_lu"))
            .args(["build", "--target", "wasm32-wasi", "-o"])
            .arg(&wasm)
            .current_dir(&package));
        assert_eq!(
            &std::fs::read(wasm).expect("read luphysics wasm")[..4],
            b"\0asm"
        );
    }

    let _ = std::fs::remove_dir_all(directory);
}
