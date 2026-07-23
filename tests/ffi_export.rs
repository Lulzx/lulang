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

fn library_path(directory: &Path, shared: bool) -> PathBuf {
    if shared {
        directory.join(if cfg!(target_os = "macos") {
            "libkernel_saxpy.dylib"
        } else {
            "libkernel_saxpy.so"
        })
    } else {
        directory.join("libkernel_saxpy.a")
    }
}

#[test]
fn exported_library_works_from_c_and_ctypes() {
    let directory = std::env::temp_dir().join(format!("lulang_ffi_export_{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create fixture directory");
    let source = directory.join("kernel_saxpy.lu");
    std::fs::write(&source, include_str!("../corpus/kernel_saxpy.lu")).expect("write source");
    let base = directory.join("kernel_saxpy");

    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["build", "--lib", "-o"])
        .arg(&base)
        .arg(&source));
    let header = std::fs::read_to_string(directory.join("kernel_saxpy.h")).expect("read header");
    assert!(header.contains(
        "double saxpy(double a, double *x_data, int64_t x_len, double *y_data, int64_t y_len, int64_t n);"
    ));
    let manifest =
        std::fs::read_to_string(directory.join("kernel_saxpy.json")).expect("read manifest");
    assert!(manifest.contains("\"abi_version\": 1"));
    assert!(manifest.contains("\"type\": \"[f64]\""));

    let c_source = directory.join("saxpy.c");
    std::fs::write(&c_source, include_str!("fixtures/saxpy.c")).expect("write C harness");
    let c_binary = directory.join("saxpy_c");
    run(Command::new("clang")
        .arg("-O2")
        .arg("-I")
        .arg(&directory)
        .arg(&c_source)
        .arg(library_path(&directory, false))
        .arg("-o")
        .arg(&c_binary));
    let output = run(&mut Command::new(&c_binary));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "72 12 24 36\n");

    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let selfhost_ir = directory.join("selfhost_kernel_saxpy.ll");
    let generated = run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .arg("run")
        .arg(repository.join("selfhost/codegen.lu"))
        .arg(&source));
    std::fs::write(&selfhost_ir, generated.stdout).expect("write self-hosted LLVM IR");
    let selfhost_binary = directory.join("selfhost_saxpy_c");
    run(Command::new("clang")
        .arg("-O2")
        .arg("-DLU_LIB")
        .arg(&selfhost_ir)
        .arg(repository.join("src/lu_runtime.c"))
        .arg(&c_source)
        .arg("-I")
        .arg(&directory)
        .arg("-o")
        .arg(&selfhost_binary));
    let output = run(&mut Command::new(&selfhost_binary));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "72 12 24 36\n");

    if Command::new("python3").arg("--version").output().is_ok() {
        run(Command::new(env!("CARGO_BIN_EXE_lu"))
            .args(["build", "--lib", "--shared", "-o"])
            .arg(&base)
            .arg(&source));
        let python = directory.join("saxpy.py");
        std::fs::write(&python, include_str!("fixtures/saxpy.py")).expect("write ctypes harness");
        let output = run(Command::new("python3")
            .arg(&python)
            .arg(library_path(&directory, true)));
        assert_eq!(String::from_utf8_lossy(&output.stdout), "72 12 24 36\n");
    }

    let _ = std::fs::remove_dir_all(directory);
}
