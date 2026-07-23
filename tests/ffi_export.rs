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
fn exported_c_mut_slice_writes_caller_memory_without_an_array_copy() {
    let directory =
        std::env::temp_dir().join(format!("lulang_ffi_c_mut_slice_{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create fixture directory");
    let source = directory.join("borrowed_bump.lu");
    std::fs::write(
        &source,
        "export fn borrowed_bump(values: c_mut_slice[f64]): f64 {\n\
           for i in 0..len(values) {\n\
             values[i] = values[i] + 1.0\n\
           }\n\
           return values[0]\n\
         }\n\
         main { print(0) }\n",
    )
    .expect("write source");
    let base = directory.join("borrowed_bump");

    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["build", "--lib", "-o"])
        .arg(&base)
        .arg(&source));
    let header = std::fs::read_to_string(directory.join("borrowed_bump.h")).expect("read header");
    assert!(header.contains("double borrowed_bump(double *values_data, int64_t values_len);"));
    let manifest =
        std::fs::read_to_string(directory.join("borrowed_bump.json")).expect("read manifest");
    assert!(manifest.contains("\"type\": \"c_mut_slice[f64]\""));

    let llvm_path = directory.join("borrowed_bump.ll");
    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["build", "--emit-llvm", "-o"])
        .arg(&llvm_path)
        .arg(&source));
    let llvm = std::fs::read_to_string(&llvm_path).expect("read LLVM");
    let wrapper = llvm
        .split("define dso_local double @\"borrowed_bump\"")
        .nth(1)
        .and_then(|rest| rest.split("\n}\n").next())
        .expect("borrowed_bump export wrapper");
    assert!(!wrapper.contains("lu_arr_new_raw"));
    assert!(!wrapper.contains("lu_arr_cow"));
    assert!(wrapper.contains("ptr %c0, i64 %c1"));

    let c_source = directory.join("borrowed_bump.c");
    std::fs::write(
        &c_source,
        "#include <stdio.h>\n\
         #include \"borrowed_bump.h\"\n\
         int main(void) {\n\
           double values[] = { 1.5, 2.5, 3.0 };\n\
           double first = borrowed_bump(values, 3);\n\
           printf(\"%.1f %.1f %.1f %.1f\\n\", first, values[0], values[1], values[2]);\n\
           return 0;\n\
         }\n",
    )
    .expect("write C harness");
    let c_binary = directory.join("borrowed_bump_c");
    run(Command::new("clang")
        .arg("-O2")
        .arg("-I")
        .arg(&directory)
        .arg(&c_source)
        .arg(directory.join("libborrowed_bump.a"))
        .arg("-o")
        .arg(&c_binary));
    let output = run(&mut Command::new(&c_binary));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "2.5 2.5 3.5 4.0\n");

    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let selfhost_ir = directory.join("selfhost_borrowed_bump.ll");
    let generated = run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .arg("run")
        .arg(repository.join("selfhost/codegen.lu"))
        .arg(&source));
    assert!(
        generated.status.success(),
        "self-hosted c_mut_slice codegen failed: {}",
        String::from_utf8_lossy(&generated.stderr)
    );
    std::fs::write(&selfhost_ir, generated.stdout).expect("write self-hosted LLVM IR");
    let selfhost_binary = directory.join("selfhost_borrowed_bump_c");
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
    assert_eq!(String::from_utf8_lossy(&output.stdout), "2.5 2.5 3.5 4.0\n");

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
         @c_layout type Span { start: i64, length: i64 }\n\
         @c_layout type LuResultI64 { status: i64, value: i64 }\n\
         export fn vec2_sum(value: Vec2): f64 {\n\
           return value.x + value.y\n\
         }\n\
         export fn make_vec2(x: f64, y: f64): Vec2 {\n\
           return Vec2 { x, y }\n\
         }\n\
         export fn make_span(start: i64, length: i64): Span {\n\
           return Span { start, length }\n\
         }\n\
         export fn checked_div(numerator: i64, denominator: i64): LuResultI64 {\n\
           if denominator == 0 { return LuResultI64 { 1, 0 } }\n\
           return LuResultI64 { 0, numerator / denominator }\n\
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
    assert!(header.contains("Vec2 make_vec2(double x, double y);"));
    assert!(header.contains("Span make_span(int64_t start, int64_t length);"));
    assert!(header.contains("#define LU_STATUS_OK INT64_C(0)"));
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
           Vec2 made = make_vec2(1.25, 3.75);\n\
           Span span = make_span(11, 9);\n\
           printf(\"%.2f %.2f %lld %lld\\n\", made.x, made.y,\n\
                  (long long)span.start, (long long)span.length);\n\
           LuResultI64 ok = checked_div(12, 3);\n\
           LuResultI64 failed = checked_div(12, 0);\n\
           printf(\"%lld %lld %lld %lld\\n\", (long long)ok.status,\n\
                  (long long)ok.value, (long long)failed.status,\n\
                  (long long)failed.value);\n\
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
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "7.0\n1.25 3.75 11 9\n0 4 1 0\n"
    );

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
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "7.0\n1.25 3.75 11 9\n0 4 1 0\n"
    );

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

#[test]
fn exported_scalar_arrays_use_opaque_owned_result_handles() {
    let directory =
        std::env::temp_dir().join(format!("lulang_ffi_owned_result_{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create fixture directory");
    let source = directory.join("owned_result.lu");
    std::fs::write(
        &source,
        "export fn integer_sequence(count: i64): [i64] {\n\
           var values = arr(count, 0)\n\
           for i in 0..count { values[i] = i * i }\n\
           return values\n\
         }\n\
         export fn float_sequence(count: i64): [f64] {\n\
           var values = arr(count, 0.0)\n\
           for i in 0..count { values[i] = float(i) * 0.5 }\n\
           return values\n\
         }\n\
         main { print(0) }\n",
    )
    .expect("write source");
    let base = directory.join("owned_result");

    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["build", "--lib", "-o"])
        .arg(&base)
        .arg(&source));
    let header = std::fs::read_to_string(directory.join("owned_result.h")).expect("read header");
    assert!(header.contains("typedef struct lu_owned_i64 lu_owned_i64;"));
    assert!(header.contains("typedef struct lu_owned_f64 lu_owned_f64;"));
    assert!(header.contains("lu_owned_i64 * integer_sequence(int64_t count);"));
    assert!(header.contains("lu_owned_f64 * float_sequence(int64_t count);"));

    let llvm = directory.join("owned_result.ll");
    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["build", "--emit-llvm", "-o"])
        .arg(&llvm)
        .arg(&source));
    let llvm = std::fs::read_to_string(llvm).expect("read LLVM");
    let integer_wrapper = llvm
        .split("define dso_local ptr @\"integer_sequence\"")
        .nth(1)
        .and_then(|source| source.split("\n}\n").next())
        .expect("integer owned-result wrapper");
    assert!(integer_wrapper.contains("call ptr @lu_owned_i64_wrap(ptr %wrapper_result)"));
    assert!(!integer_wrapper.contains("lu_arr_clone"));
    assert!(!integer_wrapper.contains("memcpy"));

    let caller = directory.join("caller.c");
    std::fs::write(
        &caller,
        "#include <stdio.h>\n\
         #include \"owned_result.h\"\n\
         int main(void) {\n\
           lu_owned_i64 *ints = integer_sequence(5);\n\
           lu_owned_f64 *floats = float_sequence(4);\n\
           if (!ints || !floats) return 2;\n\
           int64_t *idata = lu_owned_i64_data(ints);\n\
           double *fdata = lu_owned_f64_data(floats);\n\
           printf(\"%lld %lld %lld %.1f %.1f\\n\",\n\
                  (long long)lu_owned_i64_len(ints),\n\
                  (long long)idata[3],\n\
                  (long long)lu_owned_f64_len(floats), fdata[1], fdata[3]);\n\
           idata[0] = 99;\n\
           printf(\"%lld\\n\", (long long)idata[0]);\n\
           lu_owned_i64_release(ints);\n\
           lu_owned_f64_release(floats);\n\
           return 0;\n\
         }\n",
    )
    .expect("write C caller");
    let binary = directory.join("caller");
    run(Command::new("clang")
        .arg("-O2")
        .arg("-I")
        .arg(&directory)
        .arg(&caller)
        .arg(directory.join("libowned_result.a"))
        .arg("-o")
        .arg(&binary));
    assert_eq!(
        run(&mut Command::new(&binary)).stdout,
        b"5 9 4 0.5 1.5\n99\n"
    );

    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let selfhost_ir = directory.join("selfhost-owned-result.ll");
    let generated = run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .arg("run")
        .arg(repository.join("selfhost/codegen.lu"))
        .arg(&source));
    std::fs::write(&selfhost_ir, generated.stdout).expect("write selfhost LLVM");
    let selfhost_binary = directory.join("selfhost-caller");
    run(Command::new("clang")
        .arg("-O2")
        .arg("-DLU_LIB")
        .arg(&selfhost_ir)
        .arg(repository.join("src/lu_runtime.c"))
        .arg(&caller)
        .arg("-I")
        .arg(&directory)
        .arg("-o")
        .arg(&selfhost_binary));
    assert_eq!(
        run(&mut Command::new(&selfhost_binary)).stdout,
        b"5 9 4 0.5 1.5\n99\n"
    );

    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn exported_callback_types_preserve_the_function_pointer_signature() {
    let directory =
        std::env::temp_dir().join(format!("lulang_ffi_callback_{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create callback directory");
    let source = directory.join("callback_export.lu");
    std::fs::write(
        &source,
        "export fn callback_identity(\n\
           callback: c_fn[(i64) -> i64],\n\
         ): c_fn[(i64) -> i64] {\n\
           return callback\n\
         }\n\
         export fn twice(value: i64): i64 { return value * 2 }\n\
         main { print(0) }\n",
    )
    .expect("write callback export");
    let base = directory.join("callback_export");
    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["build", "--lib", "-o"])
        .arg(&base)
        .arg(&source));
    let header = std::fs::read_to_string(directory.join("callback_export.h")).expect("read header");
    assert!(header.contains("typedef int64_t (*lu_callback_"));
    assert!(header.contains("callback_identity("));

    let caller = directory.join("caller.c");
    std::fs::write(
        &caller,
        "#include <stdint.h>\n\
         #include <stdio.h>\n\
         #include \"callback_export.h\"\n\
         static int64_t increment(int64_t value) { return value + 1; }\n\
         static int64_t apply(int64_t (*callback)(int64_t), int64_t value) {\n\
           return callback(value);\n\
         }\n\
         int main(void) {\n\
           printf(\"%lld %lld\\n\",\n\
                  (long long)apply(callback_identity(increment), 41),\n\
                  (long long)apply(twice, 21));\n\
           return 0;\n\
         }\n",
    )
    .expect("write callback caller");
    let binary = directory.join("caller");
    run(Command::new("clang")
        .arg("-O2")
        .arg("-I")
        .arg(&directory)
        .arg(&caller)
        .arg(directory.join("libcallback_export.a"))
        .arg("-o")
        .arg(&binary));
    assert_eq!(run(&mut Command::new(&binary)).stdout, b"42 42\n");

    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let selfhost_ir = directory.join("selfhost-callback.ll");
    let generated = run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .arg("run")
        .arg(repository.join("selfhost/codegen.lu"))
        .arg(&source));
    std::fs::write(&selfhost_ir, generated.stdout).expect("write selfhost callback LLVM");
    let selfhost_binary = directory.join("selfhost-caller");
    run(Command::new("clang")
        .arg("-O2")
        .arg("-DLU_LIB")
        .arg(&selfhost_ir)
        .arg(repository.join("src/lu_runtime.c"))
        .arg(&caller)
        .arg("-I")
        .arg(&directory)
        .arg("-o")
        .arg(&selfhost_binary));
    assert_eq!(run(&mut Command::new(&selfhost_binary)).stdout, b"42 42\n");

    let _ = std::fs::remove_dir_all(directory);
}
