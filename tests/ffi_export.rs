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

#[test]
fn exported_f32_is_exact_in_headers_manifests_and_c_callers() {
    let directory = std::env::temp_dir().join(format!("lulang_ffi_f32_{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create fixture directory");
    let source = directory.join("scalar_f32.lu");
    std::fs::write(
        &source,
        "export fn affine32(x: f32, y: f32): f32 {\n\
           return x * y + f32(0.5)\n\
         }\n\
         main { print(0) }\n",
    )
    .expect("write source");
    let base = directory.join("scalar_f32");

    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["build", "--lib", "-o"])
        .arg(&base)
        .arg(&source));
    let header = std::fs::read_to_string(directory.join("scalar_f32.h")).expect("read header");
    assert!(header.contains("float affine32(float x, float y);"));
    let manifest =
        std::fs::read_to_string(directory.join("scalar_f32.json")).expect("read manifest");
    assert!(manifest.contains("\"name\": \"x\", \"type\": \"f32\""));
    assert!(manifest.contains("\"ret\": \"f32\""));

    let c_source = directory.join("scalar_f32.c");
    std::fs::write(
        &c_source,
        "#include <stdio.h>\n\
         #include \"scalar_f32.h\"\n\
         int main(void) {\n\
           printf(\"%.1f\\n\", (double)affine32(2.0f, 4.0f));\n\
           return 0;\n\
         }\n",
    )
    .expect("write C harness");
    let c_binary = directory.join("scalar_f32_c");
    run(Command::new("clang")
        .arg("-O2")
        .arg("-I")
        .arg(&directory)
        .arg(&c_source)
        .arg(directory.join("libscalar_f32.a"))
        .arg("-o")
        .arg(&c_binary));
    let output = run(&mut Command::new(&c_binary));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "8.5\n");

    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let selfhost_ir = directory.join("selfhost_scalar_f32.ll");
    let generated = run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .arg("run")
        .arg(repository.join("selfhost/codegen.lu"))
        .arg(&source));
    std::fs::write(&selfhost_ir, generated.stdout).expect("write self-hosted LLVM IR");
    let selfhost_binary = directory.join("selfhost_scalar_f32_c");
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
    assert_eq!(String::from_utf8_lossy(&output.stdout), "8.5\n");

    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn exported_c_slice_borrows_c_memory_without_an_array_copy() {
    let directory = std::env::temp_dir().join(format!("lulang_ffi_c_slice_{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create fixture directory");
    let source = directory.join("borrowed_sum.lu");
    std::fs::write(
        &source,
        "export fn borrowed_sum(values: c_slice[f64]): f64 {\n\
           return sum(i in 0..len(values)) values[i]\n\
         }\n\
         main { print(0) }\n",
    )
    .expect("write source");
    let base = directory.join("borrowed_sum");

    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["build", "--lib", "-o"])
        .arg(&base)
        .arg(&source));
    let header = std::fs::read_to_string(directory.join("borrowed_sum.h")).expect("read header");
    assert!(header.contains("double borrowed_sum(const double *values_data, int64_t values_len);"));
    let manifest =
        std::fs::read_to_string(directory.join("borrowed_sum.json")).expect("read manifest");
    assert!(manifest.contains("\"type\": \"c_slice[f64]\""));

    let llvm_path = directory.join("borrowed_sum.ll");
    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["build", "--emit-llvm", "-o"])
        .arg(&llvm_path)
        .arg(&source));
    let llvm = std::fs::read_to_string(&llvm_path).expect("read LLVM");
    let wrapper = llvm
        .split("define dso_local double @\"borrowed_sum\"")
        .nth(1)
        .and_then(|rest| rest.split("\n}\n").next())
        .expect("borrowed_sum export wrapper");
    assert!(!wrapper.contains("lu_arr_new_raw"));
    assert!(wrapper.contains("ptr %c0, i64 %c1"));

    let c_source = directory.join("borrowed_sum.c");
    std::fs::write(
        &c_source,
        "#include <stdio.h>\n\
         #include \"borrowed_sum.h\"\n\
         int main(void) {\n\
           const double values[] = { 1.5, 2.5, 3.0 };\n\
           printf(\"%.1f\\n\", borrowed_sum(values, 3));\n\
           return 0;\n\
         }\n",
    )
    .expect("write C harness");
    let c_binary = directory.join("borrowed_sum_c");
    run(Command::new("clang")
        .arg("-O2")
        .arg("-I")
        .arg(&directory)
        .arg(&c_source)
        .arg(directory.join("libborrowed_sum.a"))
        .arg("-o")
        .arg(&c_binary));
    let output = run(&mut Command::new(&c_binary));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "7.0\n");

    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let selfhost_ir = directory.join("selfhost_borrowed_sum.ll");
    let generated = run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .arg("run")
        .arg(repository.join("selfhost/codegen.lu"))
        .arg(&source));
    std::fs::write(&selfhost_ir, generated.stdout).expect("write self-hosted LLVM IR");
    let selfhost_binary = directory.join("selfhost_borrowed_sum_c");
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
    assert_eq!(String::from_utf8_lossy(&output.stdout), "7.0\n");

    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn exported_opaque_pointer_uses_void_pointer_header_abi() {
    let directory = std::env::temp_dir().join(format!("lulang_ffi_pointer_{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create fixture directory");
    let source = directory.join("pointer_echo.lu");
    std::fs::write(
        &source,
        "@c_layout type Vec2 { x: f32, y: f32 }\n\
         @c_layout type Packet { position: Vec2, tag: i64 }\n\
         export fn pointer_echo(pointer: c_ptr[Packet]): c_ptr[Packet] {\n\
           return pointer\n\
         }\n",
    )
    .expect("write source");
    let base = directory.join("pointer_echo");

    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["build", "--lib", "-o"])
        .arg(&base)
        .arg(&source));

    let header = std::fs::read_to_string(directory.join("pointer_echo.h")).expect("read header");
    assert!(header.contains("typedef struct Vec2 {\n    float x;\n    float y;\n} Vec2;"));
    assert!(
        header.contains("typedef struct Packet {\n    Vec2 position;\n    int64_t tag;\n} Packet;")
    );
    assert!(header.contains("void * pointer_echo(void * pointer);"));
    let manifest =
        std::fs::read_to_string(directory.join("pointer_echo.json")).expect("read manifest");
    assert!(manifest.contains("\"Vec2\": [{\"name\": \"x\", \"type\": \"f32\"}"));
    assert!(manifest.contains("\"type\": \"c_ptr[Packet]\""));
    assert!(manifest.contains("\"ret\": \"c_ptr[Packet]\""));

    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn exported_c_layout_record_is_passed_by_value_without_an_adapter() {
    let directory =
        std::env::temp_dir().join(format!("lulang_ffi_c_layout_{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create fixture directory");
    let source = directory.join("c_layout_value.lu");
    std::fs::write(
        &source,
        "@c_layout type Vec2 { x: f64, y: f64 }\n\
         export fn vec2_sum(value: Vec2): f64 {\n\
           return value.x + value.y\n\
         }\n\
         main { print(0) }\n",
    )
    .expect("write source");
    let base = directory.join("c_layout_value");

    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["build", "--lib", "-o"])
        .arg(&base)
        .arg(&source));
    let header =
        std::fs::read_to_string(directory.join("c_layout_value.h")).expect("read generated header");
    assert!(header.contains("double vec2_sum(Vec2 value);"));
    let manifest = std::fs::read_to_string(directory.join("c_layout_value.json"))
        .expect("read generated manifest");
    assert!(manifest.contains("\"type\": \"Vec2\""));

    let c_source = directory.join("caller.c");
    std::fs::write(
        &c_source,
        "#include <stdio.h>\n\
         #include \"c_layout_value.h\"\n\
         int main(void) {\n\
           Vec2 value = { 2.5, 4.5 };\n\
           printf(\"%.1f\\n\", vec2_sum(value));\n\
           return 0;\n\
         }\n",
    )
    .expect("write C caller");
    let binary = directory.join("caller");
    run(Command::new("clang")
        .arg("-O2")
        .arg("-I")
        .arg(&directory)
        .arg(&c_source)
        .arg(directory.join("libc_layout_value.a"))
        .arg("-o")
        .arg(&binary));
    let output = run(&mut Command::new(&binary));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "7.0\n");

    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let selfhost_ir = directory.join("selfhost-c-layout-value.ll");
    let generated = run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .arg("run")
        .arg(repository.join("selfhost/codegen.lu"))
        .arg(&source));
    std::fs::write(&selfhost_ir, generated.stdout).expect("write self-hosted LLVM IR");
    let selfhost_binary = directory.join("selfhost-caller");
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
    assert_eq!(String::from_utf8_lossy(&output.stdout), "7.0\n");

    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn exported_string_return_uses_pointer_and_hidden_length() {
    let directory =
        std::env::temp_dir().join(format!("lulang_ffi_string_return_{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create fixture directory");
    let source = directory.join("string_return.lu");
    std::fs::write(
        &source,
        "export fn greeting(prefix: str): str {\n\
           return concat(prefix, \"\\0!\")\n\
         }\n\
         main { print(0) }\n",
    )
    .expect("write source");
    let base = directory.join("string_return");

    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["build", "--lib", "-o"])
        .arg(&base)
        .arg(&source));
    let header = std::fs::read_to_string(directory.join("string_return.h")).expect("read header");
    assert!(header.contains(
        "const char * greeting(const char *prefix_data, int64_t prefix_len, int64_t *out_len);"
    ));
    let manifest =
        std::fs::read_to_string(directory.join("string_return.json")).expect("read manifest");
    assert!(manifest.contains("\"ret\": \"str\""));

    let c_source = directory.join("caller.c");
    std::fs::write(
        &c_source,
        "#include <stdint.h>\n\
         #include <stdio.h>\n\
         #include \"string_return.h\"\n\
         int main(void) {\n\
           int64_t length = -1;\n\
           const char *value = greeting(\"A\", 1, &length);\n\
           printf(\"%lld %d %d %d\\n\", (long long)length,\n\
                  (unsigned char)value[0], (unsigned char)value[1],\n\
                  (unsigned char)value[2]);\n\
           return 0;\n\
         }\n",
    )
    .expect("write C caller");
    let binary = directory.join("caller");
    run(Command::new("clang")
        .arg("-O2")
        .arg("-I")
        .arg(&directory)
        .arg(&c_source)
        .arg(directory.join("libstring_return.a"))
        .arg("-o")
        .arg(&binary));
    let output = run(&mut Command::new(&binary));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "3 65 0 33\n");

    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let selfhost_ir = directory.join("selfhost-string-return.ll");
    let generated = run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .arg("run")
        .arg(repository.join("selfhost/codegen.lu"))
        .arg(&source));
    std::fs::write(&selfhost_ir, generated.stdout).expect("write self-hosted LLVM IR");
    let selfhost_binary = directory.join("selfhost-caller");
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
    assert_eq!(String::from_utf8_lossy(&output.stdout), "3 65 0 33\n");

    let _ = std::fs::remove_dir_all(directory);
}
