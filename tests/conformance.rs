use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);
static SELFHOST_INTERP: OnceLock<PathBuf> = OnceLock::new();

struct CaseDir(PathBuf);

impl CaseDir {
    fn new(name: &str, source: &str) -> Self {
        let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "lulang-conformance-{}-{}-{}",
            std::process::id(),
            id,
            name
        ));
        fs::create_dir(&path).expect("create conformance directory");
        fs::write(path.join("case.lu"), source).expect("write conformance source");
        Self(path)
    }

    fn source(&self) -> PathBuf {
        self.0.join("case.lu")
    }
}

impl Drop for CaseDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn lu() -> &'static str {
    env!("CARGO_BIN_EXE_lu")
}

fn run(command: &mut Command) -> Output {
    command.output().expect("run backend")
}

fn host(mode: &str, source: &Path) -> Output {
    run(Command::new(lu()).args([mode, source.to_str().unwrap()]))
}

fn aot(dir: &CaseDir) -> Output {
    let built = run(Command::new(lu())
        .args(["build", dir.source().to_str().unwrap()])
        .current_dir(&dir.0));
    assert!(
        built.status.success(),
        "AOT compilation failed: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    run(&mut Command::new(dir.0.join("case")))
}

fn selfhost(source: &Path) -> Output {
    let interpreter = SELFHOST_INTERP.get_or_init(|| {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../selfhost/interp.lu");
        let dir = std::env::temp_dir().join(format!("lulang-selfhost-{}", std::process::id()));
        fs::create_dir(&dir).expect("create self-host build directory");
        let built = run(Command::new(lu())
            .args(["build", source.to_str().unwrap()])
            .current_dir(&dir));
        assert!(
            built.status.success(),
            "compile self-hosted interpreter: {}",
            String::from_utf8_lossy(&built.stderr)
        );
        dir.join("interp")
    });
    run(Command::new(interpreter).arg(source))
}

fn assert_success(name: &str, source: &str, expected: &[u8]) {
    let dir = CaseDir::new(name, source);
    let outputs = [
        ("interpreter", host("interp", &dir.source())),
        ("JIT", host("run", &dir.source())),
        ("AOT", aot(&dir)),
    ];
    for (backend, output) in outputs {
        assert!(
            output.status.success(),
            "{name} failed in {backend}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, expected, "{name} disagreed in {backend}");
    }
}

fn assert_host_success(name: &str, source: &str, expected: &[u8]) {
    let dir = CaseDir::new(name, source);
    let outputs = [
        ("interpreter", host("interp", &dir.source())),
        ("JIT", host("run", &dir.source())),
        ("AOT", aot(&dir)),
    ];
    for (backend, output) in outputs {
        assert!(
            output.status.success(),
            "{name} failed in {backend}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, expected, "{name} disagreed in {backend}");
    }
}

fn assert_failure(name: &str, source: &str) {
    let dir = CaseDir::new(name, source);
    let outputs = [
        ("interpreter", host("interp", &dir.source())),
        ("JIT", host("run", &dir.source())),
        ("AOT", aot(&dir)),
        ("self-hosted", selfhost(&dir.source())),
    ];
    for (backend, output) in outputs {
        let reported_failure = !output.status.success()
            || output.stdout.windows(5).any(|part| part == b"error")
            || output.stderr.windows(5).any(|part| part == b"error");
        assert!(
            reported_failure,
            "{name} unexpectedly succeeded in {backend}"
        );
    }
}

macro_rules! conformance_cases {
    ($($name:ident: $source:expr => $expected:expr;)+) => {
        $(
            #[test]
            fn $name() {
                assert_success(stringify!($name), $source, $expected);
            }
        )+
    };
}

conformance_cases! {
    wrapping_integer_arithmetic:
        "main {\n print(9223372036854775807 + 1)\n}\n"
        => b"-9223372036854775808\n";
    raw_byte_substrings:
        "main {\n puts(substr(\"é\", 0, 1))\n}\n"
        => &[0xc3];
    record_array_layout:
        "type P { x: i64, y: i64 }\nmain {\n var a = arr(2, P { 1, 2 })\n a[1] = P { 3, 4 }\n print(len(a), a[1].x, a[0].y)\n}\n"
        => b"2 3 2\n";
    left_to_right_and_short_circuit:
        "fn bump(inout x: i64): i64 {\n x = x + 1\n return x\n}\nfn pair(a: i64, b: i64): i64 { return a * 10 + b }\nmain {\n var x = 0\n print(pair(bump(x), x), x)\n print(false and (1 / 0 == 0), true or (1 / 0 == 0))\n}\n"
        => b"11 1\nfalse true\n";
    string_constants_in_outlined_functions_stay_alive:
        "fn text(n: i64): str {\n if n == 0 { return \"stable string\" }\n return text(n - 1)\n}\nmain {\n print(text(1))\n}\n"
        => b"stable string\n";
}

#[test]
fn negative_array_length_traps_everywhere() {
    assert_failure(
        "negative_array_length",
        "main {\n let a = arr(-1, 0)\n print(len(a))\n}\n",
    );
}

#[test]
fn division_overflow_traps_everywhere() {
    assert_failure(
        "division_overflow",
        "main {\n let m = -9223372036854775807 - 1\n print(m / -1)\n}\n",
    );
}

#[test]
fn division_by_zero_traps_everywhere() {
    assert_failure("division_by_zero", "main {\n print(1 / 0)\n}\n");
}

#[test]
fn allocation_size_overflow_traps_everywhere() {
    assert_failure(
        "allocation_size_overflow",
        "type P { x: i64, y: i64 }\nmain {\n let a = arr(9223372036854775807, P { 0, 0 })\n print(len(a))\n}\n",
    );
}

#[test]
fn f32_is_a_distinct_width_in_all_host_tiers() {
    assert_host_success(
        "f32_width",
        "type P { x: f32 }\n\
         fn narrow(x: f32): f32 { x + f32(1) }\n\
         fn wrapped(x: f32): P { P { x } }\n\
         main {\n\
           var a = arr(2, f32(16777217))\n\
           print(narrow(16777217), wrapped(16777217).x, a[0])\n\
         }\n",
        b"16777216 16777216 16777216\n",
    );
}

#[test]
fn direct_f32_c_import_uses_single_precision_in_all_tiers() {
    assert_success(
        "ffi_f32",
        "extern \"m\" fn cbrtf(x: f32): f32\n\
         main { print(float(cbrtf(f32(27)))) }\n",
        b"3\n",
    );
}

#[test]
fn borrowed_c_slice_is_read_only_and_indexable_in_all_tiers() {
    assert_success(
        "c_slice",
        "fn slice_sum(values: c_slice[f64]): f64 {\n\
           return sum(i in 0..len(values)) values[i]\n\
         }\n\
         main {\n\
           let values = arr(3, 2.0)\n\
           print(slice_sum(values))\n\
         }\n",
        b"6\n",
    );
}

#[test]
fn mutable_c_slice_updates_the_borrowed_array_in_all_host_tiers() {
    assert_success(
        "c_mut_slice",
        "fn saxpy(a: f64, x: c_slice[f64], y: c_mut_slice[f64]) {\n\
           for i in 0..len(y) {\n\
             y[i] = a * x[i] + y[i]\n\
           }\n\
         }\n\
         fn set_count(values: c_mut_slice[i64]) {\n\
           values[1] = values[0] + 4\n\
         }\n\
         main {\n\
           let x = [1.0, 2.0, 3.0]\n\
           var y = [10.0, 20.0, 30.0]\n\
           let snapshot = y\n\
           saxpy(2.0, x, y)\n\
           print(y[0], y[1], y[2])\n\
           print(snapshot[0], snapshot[1], snapshot[2])\n\
           var counts = arr(2, 3)\n\
           set_count(counts)\n\
           print(counts[0], counts[1])\n\
         }\n",
        b"12 24 36\n10 20 30\n3 7\n",
    );
}

#[test]
fn borrowed_c_slice_imports_cross_all_tiers() {
    let dir = CaseDir::new("ffi_c_slice", "");
    let library = dir.0.join(if cfg!(target_os = "macos") {
        "libc_slice_fixture.dylib"
    } else {
        "libc_slice_fixture.so"
    });
    let c_source = dir.0.join("c_slice_fixture.c");
    fs::write(
        &c_source,
        "#include <stdint.h>\n\
         double c_slice_sum(const double *values, int64_t length) {\n\
           double total = 0.0;\n\
           for (int64_t i = 0; i < length; i++) total += values[i];\n\
           return total;\n\
         }\n",
    )
    .expect("write C fixture");
    let mut compiler = Command::new("cc");
    if cfg!(target_os = "macos") {
        compiler.arg("-dynamiclib");
    } else {
        compiler.args(["-shared", "-fPIC"]);
    }
    let compiled = run(compiler.arg(&c_source).arg("-o").arg(&library));
    assert!(
        compiled.status.success(),
        "compile c_slice fixture: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    fs::write(
        dir.source(),
        format!(
            "extern {:?} fn c_slice_sum(values: c_slice[f64]): f64\n\
             main {{\n\
               let values = arr(3, 2.0)\n\
               print(c_slice_sum(values))\n\
             }}\n",
            library.to_string_lossy()
        ),
    )
    .expect("write lulang fixture");

    let outputs = [
        ("interpreter", host("interp", &dir.source())),
        ("JIT", host("run", &dir.source())),
        ("AOT", aot(&dir)),
        ("self-hosted", selfhost(&dir.source())),
    ];
    for (backend, output) in outputs {
        assert!(
            output.status.success(),
            "c_slice import failed in {backend}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            output.stdout, b"6\n",
            "c_slice import disagreed in {backend}"
        );
    }
}

#[test]
fn mutable_c_slice_imports_copy_writes_back_in_all_tiers() {
    let dir = CaseDir::new("ffi_c_mut_slice", "");
    let library = dir.0.join(if cfg!(target_os = "macos") {
        "libc_mut_slice_fixture.dylib"
    } else {
        "libc_mut_slice_fixture.so"
    });
    let c_source = dir.0.join("c_mut_slice_fixture.c");
    fs::write(
        &c_source,
        "#include <stdint.h>\n\
         double c_mut_slice_bump(double *values, int64_t length) {\n\
           for (int64_t i = 0; i < length; i++) values[i] += 1.0;\n\
           return values[0];\n\
         }\n",
    )
    .expect("write C fixture");
    let mut compiler = Command::new("cc");
    if cfg!(target_os = "macos") {
        compiler.arg("-dynamiclib");
    } else {
        compiler.args(["-shared", "-fPIC"]);
    }
    let compiled = run(compiler.arg(&c_source).arg("-o").arg(&library));
    assert!(
        compiled.status.success(),
        "compile c_mut_slice fixture: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    fs::write(
        dir.source(),
        format!(
            "extern {:?} fn c_mut_slice_bump(values: c_mut_slice[f64]): f64\n\
             main {{\n\
               var values = arr(3, 2.0)\n\
               let snapshot = values\n\
               print(c_mut_slice_bump(values))\n\
               print(values[0], values[1], values[2])\n\
               print(snapshot[0], snapshot[1], snapshot[2])\n\
             }}\n",
            library.to_string_lossy()
        ),
    )
    .expect("write lulang fixture");

    let outputs = [
        ("interpreter", host("interp", &dir.source())),
        ("JIT", host("run", &dir.source())),
        ("AOT", aot(&dir)),
        ("self-hosted", selfhost(&dir.source())),
    ];
    for (backend, output) in outputs {
        assert!(
            output.status.success(),
            "c_mut_slice import failed in {backend}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            output.stdout, b"3\n3 3 3\n2 2 2\n",
            "c_mut_slice import disagreed in {backend}"
        );
    }
}

#[test]
fn by_value_c_layout_imports_use_the_portable_register_subset() {
    let dir = CaseDir::new("ffi_c_layout_value", "");
    let library = dir.0.join(if cfg!(target_os = "macos") {
        "libc_layout_fixture.dylib"
    } else {
        "libc_layout_fixture.so"
    });
    let c_source = dir.0.join("c_layout_fixture.c");
    fs::write(
        &c_source,
        "#include <stdint.h>\n\
         typedef struct Vec2 { double x; double y; } Vec2;\n\
         typedef struct Span { int64_t start; int64_t length; } Span;\n\
         double vec2_sum(Vec2 value) { return value.x + value.y; }\n\
         int64_t span_end(Span value) { return value.start + value.length; }\n\
         Vec2 vec2_make(double x, double y) { Vec2 value = {x, y}; return value; }\n\
         Span span_make(int64_t start, int64_t length) { Span value = {start, length}; return value; }\n",
    )
    .expect("write C fixture");
    let mut compiler = Command::new("cc");
    if cfg!(target_os = "macos") {
        compiler.arg("-dynamiclib");
    } else {
        compiler.args(["-shared", "-fPIC"]);
    }
    let compiled = run(compiler.arg(&c_source).arg("-o").arg(&library));
    assert!(
        compiled.status.success(),
        "compile c_layout fixture: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    fs::write(
        dir.source(),
        format!(
            "@c_layout type Vec2 {{ x: f64, y: f64 }}\n\
             @c_layout type Span {{ start: i64, length: i64 }}\n\
             extern {:?} fn vec2_sum(value: Vec2): f64\n\
             extern {:?} fn span_end(value: Span): i64\n\
             extern {:?} fn vec2_make(x: f64, y: f64): Vec2\n\
             extern {:?} fn span_make(start: i64, length: i64): Span\n\
             main {{\n\
               print(vec2_sum(Vec2 {{ 2.5, 4.5 }}), span_end(Span {{ 7, 8 }}))\n\
               let point = vec2_make(1.25, 3.75)\n\
               let span = span_make(11, 9)\n\
               print(point.x, point.y, span.start, span.length)\n\
             }}\n",
            library.to_string_lossy(),
            library.to_string_lossy(),
            library.to_string_lossy(),
            library.to_string_lossy()
        ),
    )
    .expect("write lulang fixture");

    let outputs = [
        ("interpreter", host("interp", &dir.source())),
        ("JIT", host("run", &dir.source())),
        ("AOT", aot(&dir)),
        ("self-hosted", selfhost(&dir.source())),
    ];
    for (backend, output) in outputs {
        assert!(
            output.status.success(),
            "c_layout value import failed in {backend}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            output.stdout, b"7 15\n1.25 3.75 11 9\n",
            "c_layout value import disagreed in {backend}"
        );
    }
}

#[test]
fn typed_c_function_pointers_cross_imports() {
    let dir = CaseDir::new("ffi_callback", "");
    let library = dir.0.join(if cfg!(target_os = "macos") {
        "libcallback_fixture.dylib"
    } else {
        "libcallback_fixture.so"
    });
    let c_source = dir.0.join("callback_fixture.c");
    fs::write(
        &c_source,
        "#include <stdint.h>\n\
         typedef int64_t (*unary_i64)(int64_t);\n\
         static int64_t increment(int64_t value) { return value + 1; }\n\
         unary_i64 get_increment(void) { return increment; }\n\
         int64_t apply_callback(unary_i64 callback, int64_t value) {\n\
           return callback(value);\n\
         }\n",
    )
    .expect("write callback C fixture");
    let mut compiler = Command::new("cc");
    if cfg!(target_os = "macos") {
        compiler.arg("-dynamiclib");
    } else {
        compiler.args(["-shared", "-fPIC"]);
    }
    let compiled = run(compiler.arg(&c_source).arg("-o").arg(&library));
    assert!(
        compiled.status.success(),
        "compile callback fixture: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    fs::write(
        dir.source(),
        format!(
            "extern {:?} fn get_increment(): c_fn[(i64) -> i64]\n\
             extern {:?} fn apply_callback(callback: c_fn[(i64) -> i64], value: i64): i64\n\
             main {{\n\
               let callback = get_increment()\n\
               print(apply_callback(callback, 41))\n\
             }}\n",
            library.to_string_lossy(),
            library.to_string_lossy()
        ),
    )
    .expect("write callback Lulang fixture");

    for (backend, output) in [
        ("interpreter", host("interp", &dir.source())),
        ("JIT", host("run", &dir.source())),
        ("AOT", aot(&dir)),
        ("self-hosted", selfhost(&dir.source())),
    ] {
        assert!(
            output.status.success(),
            "callback import failed in {backend}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"42\n", "callback disagreed in {backend}");
    }
}

#[test]
fn returned_ffi_strings_are_length_delimited_and_copied() {
    let dir = CaseDir::new("ffi_string_return", "");
    let library = dir.0.join(if cfg!(target_os = "macos") {
        "libstring_return_fixture.dylib"
    } else {
        "libstring_return_fixture.so"
    });
    let c_source = dir.0.join("string_return_fixture.c");
    fs::write(
        &c_source,
        "#include <stdint.h>\n\
         const char *make_label(const char *prefix, int64_t prefix_len, int64_t *out_len) {\n\
           static char bytes[3];\n\
           bytes[0] = prefix_len == 0 ? '?' : prefix[0];\n\
           bytes[1] = '\\0';\n\
           bytes[2] = '!';\n\
           *out_len = 3;\n\
           return bytes;\n\
         }\n",
    )
    .expect("write C fixture");
    let mut compiler = Command::new("cc");
    if cfg!(target_os = "macos") {
        compiler.arg("-dynamiclib");
    } else {
        compiler.args(["-shared", "-fPIC"]);
    }
    let compiled = run(compiler.arg(&c_source).arg("-o").arg(&library));
    assert!(
        compiled.status.success(),
        "compile string-return fixture: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    fs::write(
        dir.source(),
        format!(
            "extern {:?} fn make_label(prefix: str): str\n\
             main {{\n\
               let first = make_label(\"A\")\n\
               let second = make_label(\"B\")\n\
               print(len(first), first[0], first[1], first[2])\n\
               print(len(second), second[0], second[1], second[2])\n\
             }}\n",
            library.to_string_lossy()
        ),
    )
    .expect("write lulang fixture");

    let outputs = [
        ("interpreter", host("interp", &dir.source())),
        ("JIT", host("run", &dir.source())),
        ("AOT", aot(&dir)),
        ("self-hosted", selfhost(&dir.source())),
    ];
    for (backend, output) in outputs {
        assert!(
            output.status.success(),
            "string return failed in {backend}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            output.stdout, b"3 65 0 33\n3 66 0 33\n",
            "string return disagreed in {backend}"
        );
    }
}

#[test]
fn arrays_nested_in_records_still_have_value_semantics() {
    assert_host_success(
        "nested_array_value_semantics",
        "type Bag { values: [i64] }\n\
         main {\n\
           var original = Bag { arr(1, 0) }\n\
           let snapshot = original\n\
           original.values[0] = 7\n\
           print(original.values[0], snapshot.values[0])\n\
         }\n",
        b"7 0\n",
    );
}

#[test]
fn inlined_inout_record_mutation_preserves_array_snapshots() {
    assert_host_success(
        "inlined_inout_record_cow",
        "type Bag { values: [i64] }\n\
         fn mutate(inout bag: Bag) { bag.values[0] = 7 }\n\
         main {\n\
           var original = Bag { arr(1, 0) }\n\
           let snapshot = original\n\
           mutate(original)\n\
           print(original.values[0], snapshot.values[0])\n\
         }\n",
        b"7 0\n",
    );
}

#[test]
fn scalar_ffi_imports_match_across_all_tiers() {
    assert_success(
        "scalar_ffi_imports",
        "extern fn llabs(x: i64): i64\n\
         extern \"m\" fn cbrt(x: f64): f64\n\
         main { print(llabs(-42), cbrt(27.0)) }\n",
        b"42 3\n",
    );
}

#[test]
fn simd_reductions_do_not_treat_inlined_return_slots_as_invariants() {
    assert_success(
        "simd_inlined_return_slot",
        "fn one(): f64 { return 1.0 }\n\
         main { print(sum(i in 0..8) one()) }\n",
        b"8\n",
    );
}

#[test]
fn simd_reductions_handle_odd_lengths_with_a_scalar_tail() {
    assert_success(
        "simd_odd_length_tail",
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
        b"440\n",
    );
}

#[test]
fn integer_simd_reductions_remain_exact_above_f64_precision() {
    assert_success(
        "integer_simd_exact_odd_tail",
        "main {\n\
           let n = 11\n\
           let values = arr(n, 9007199254740992 + 1)\n\
           print(sum(i in 0..n) values[i])\n\
         }\n",
        b"99079191802150923\n",
    );
}

#[test]
fn unresolved_ffi_symbols_fail_cleanly_in_every_tier() {
    let source = "extern fn lulang_symbol_that_does_not_exist_7f42(x: i64): i64\n\
         main { print(lulang_symbol_that_does_not_exist_7f42(1)) }\n";
    let dir = CaseDir::new("ffi_unresolved_symbol", source);

    for (backend, output) in [
        ("interpreter", host("interp", &dir.source())),
        ("JIT", host("run", &dir.source())),
        ("self-hosted", selfhost(&dir.source())),
    ] {
        assert!(
            !output.status.success()
                || output.stdout.windows(5).any(|part| part == b"error")
                || output.stderr.windows(5).any(|part| part == b"error"),
            "unresolved symbol unexpectedly succeeded in {backend}"
        );
        let diagnostic = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            diagnostic.contains("lulang_symbol_that_does_not_exist_7f42")
                || diagnostic.contains("resolve")
                || diagnostic.contains("symbol"),
            "missing unresolved-symbol diagnostic in {backend}: {diagnostic}"
        );
    }

    let built = run(Command::new(lu())
        .args(["build", dir.source().to_str().unwrap()])
        .current_dir(&dir.0));
    assert!(
        !built.status.success(),
        "AOT linked an unresolved FFI symbol"
    );
    let diagnostic = String::from_utf8_lossy(&built.stderr);
    assert!(
        diagnostic.contains("lulang_symbol_that_does_not_exist_7f42")
            || diagnostic.contains("undefined"),
        "missing AOT linker diagnostic: {diagnostic}"
    );
}

#[test]
fn ffi_arrays_and_strings_match_across_all_tiers() {
    let dir = CaseDir::new("ffi_array_copyout", "");
    let extension = if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    };
    let library = dir.0.join(format!("libfixture.{}", extension));
    let fixture = dir.0.join("fixture.c");
    fs::write(
        &fixture,
        "#include <stdint.h>\n\
         void bump(int64_t *data, int64_t n) {\n\
           for (int64_t i = 0; i < n; ++i) data[i] += 10;\n\
         }\n\
         void scale(double *data, int64_t n, double factor) {\n\
           for (int64_t i = 0; i < n; ++i) data[i] *= factor;\n\
         }\n\
         int64_t byte_sum(const char *data, int64_t n) {\n\
           int64_t total = 0;\n\
           for (int64_t i = 0; i < n; ++i) total += (unsigned char)data[i];\n\
           return total;\n\
         }\n",
    )
    .expect("write FFI fixture");
    let compiled = run(Command::new("clang")
        .args(["-shared", "-o"])
        .arg(&library)
        .arg(&fixture));
    assert!(
        compiled.status.success(),
        "compile FFI fixture: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    fs::write(
        dir.source(),
        format!(
            "extern \"{}\" fn bump(data: [i64])\n\
             extern \"{}\" fn scale(data: [f64], factor: f64)\n\
             extern \"{}\" fn byte_sum(data: str): i64\n\
             main {{\n\
               var data = arr(3, 0)\n\
               data[0] = 1\n\
               data[1] = 2\n\
               data[2] = 3\n\
               bump(data)\n\
               var values = arr(2, 1.5)\n\
               scale(values, 2.0)\n\
               print(data[0], data[2], byte_sum(\"ABC\"))\n\
               print(values[0], values[1])\n\
             }}\n",
            library.display(),
            library.display(),
            library.display()
        ),
    )
    .expect("write FFI case");
    for (backend, output) in [
        ("interpreter", host("interp", &dir.source())),
        ("JIT", host("run", &dir.source())),
        ("AOT", aot(&dir)),
        ("self-hosted", selfhost(&dir.source())),
    ] {
        assert!(
            output.status.success(),
            "FFI array case failed in {backend}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            output.stdout, b"11 13 198\n3 3\n",
            "FFI array/string mismatch in {backend}"
        );
    }
}

#[test]
fn opaque_c_pointers_match_across_all_tiers() {
    let dir = CaseDir::new("ffi_opaque_pointer", "");
    let extension = if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    };
    let library = dir.0.join(format!("libpointer_fixture.{}", extension));
    let fixture = dir.0.join("pointer_fixture.c");
    fs::write(
        &fixture,
        "#include <stdint.h>\n\
         #include <stdlib.h>\n\
         typedef struct Box { int64_t value; } Box;\n\
         Box *box_new(int64_t value) {\n\
           Box *box = malloc(sizeof(*box));\n\
           if (box) box->value = value;\n\
           return box;\n\
         }\n\
         int64_t box_read(const Box *box) { return box ? box->value : -1; }\n\
         void box_free(Box *box) { free(box); }\n",
    )
    .expect("write pointer FFI fixture");
    let compiled = run(Command::new("clang")
        .args(["-shared", "-o"])
        .arg(&library)
        .arg(&fixture));
    assert!(
        compiled.status.success(),
        "compile pointer FFI fixture: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    fs::write(
        dir.source(),
        format!(
            "type Box {{}}\n\
             extern \"{}\" fn box_new(value: i64): c_ptr[Box]\n\
             extern \"{}\" fn box_read(box: c_ptr[Box]): i64\n\
             extern \"{}\" fn box_free(box: c_ptr[Box])\n\
             main {{\n\
               let box = box_new(73)\n\
               print(box == box, box_read(box))\n\
               box_free(box)\n\
             }}\n",
            library.display(),
            library.display(),
            library.display()
        ),
    )
    .expect("write pointer FFI case");
    for (backend, output) in [
        ("interpreter", host("interp", &dir.source())),
        ("JIT", host("run", &dir.source())),
        ("AOT", aot(&dir)),
        ("self-hosted", selfhost(&dir.source())),
    ] {
        assert!(
            output.status.success(),
            "opaque pointer case failed in {backend}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            output.stdout, b"true 73\n",
            "opaque pointer mismatch in {backend}"
        );
    }
}

#[test]
fn exported_array_body_is_mutable_without_changing_lulang_value_semantics() {
    assert_success(
        "exported_array_value_semantics",
        "export fn touch(data: [i64]) {\n\
           data[0] = 9\n\
         }\n\
         main {\n\
           var data = arr(1, 0)\n\
           touch(data)\n\
           print(data[0])\n\
         }\n",
        b"0\n",
    );
}
