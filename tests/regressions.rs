use std::io::Write as _;
use std::process::{Command, Output, Stdio};

fn run(mode: &str, source: &str) -> Output {
    run_args(&[mode, "/dev/stdin"], source)
}

fn run_args(args: &[&str], source: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn lu");
    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(source.as_bytes())
        .expect("write source");
    child.wait_with_output().expect("wait for lu")
}

fn assert_modes(source: &str, expected: &[u8]) {
    for mode in ["interp", "run"] {
        let output = run(mode, source);
        assert!(
            output.status.success(),
            "{mode} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, expected, "unexpected {mode} output");
    }
}

#[test]
fn compiled_array_literals_match_the_interpreter() {
    assert_modes("main {\n print([1, 2][0])\n}\n", b"1\n");
}

#[test]
fn integer_sum_remains_exact_above_f64_precision() {
    assert_modes(
        "main {\n print(sum(i in 0..2) 9007199254740993)\n}\n",
        b"18014398509481986\n",
    );
}

#[test]
fn byte_substrings_are_not_lossily_decoded() {
    assert_modes("main {\n puts(substr(\"é\", 0, 1))\n}\n", &[0xc3]);
}

#[test]
fn duplicate_record_fields_are_rejected_by_the_checker() {
    let source = "type P { x: i64, y: i64 }\nmain {\n let p = P { x: 1, x: 2 }\n print(p.y)\n}\n";
    for mode in ["interp", "run"] {
        let output = run(mode, source);
        assert!(
            !output.status.success(),
            "{mode} accepted an invalid record"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("initialized more than once"),
            "unexpected {mode} error: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn c_layout_is_explicit_and_rejects_non_boundary_fields() {
    assert_modes(
        "@c_layout type Pair { x: i64, y: f64 }\n\
         main {\n let p = Pair { x: 7, y: 2.5 }\n print(p.x, p.y)\n }\n",
        b"7 2.5\n",
    );

    for source in [
        "@c_layout type Empty {}\nmain {}\n",
        "@c_layout type Bad { values: [i64] }\nmain {}\n",
        "@c_layout type Cycle { next: Cycle }\nmain {}\n",
    ] {
        let output = run("check", source);
        assert!(
            !output.status.success(),
            "accepted invalid @c_layout record: {source}"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("`@c_layout`"),
            "unexpected error: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn array_assignment_has_unobservable_aliasing() {
    assert_modes(
        "main {\n var a = arr(2, 0)\n let snapshot = a\n a[0] = 9\n print(a[0], snapshot[0])\n}\n",
        b"9 0\n",
    );
}

#[test]
fn a_function_may_return_its_final_expression() {
    assert_modes(
        "fn twice(x: i64): i64 {\n x * 2\n}\nmain {\n print(twice(21))\n}\n",
        b"42\n",
    );
}

#[test]
fn unicode_operators_have_stable_ascii_callable_names() {
    assert_modes(
        "operator+ (a: i64) ⊕ (b: i64): i64 { a + b }\n\
         operator ‖(x: i64)‖: i64 { x * x }\n\
         main {\n\
           print(2 ⊕ 3, operator_u2295(2, 3))\n\
           print(‖4‖, operator_u2016_u2016(4))\n\
         }\n",
        b"5 5\n16 16\n",
    );
}

#[test]
fn property_run_count_is_configurable() {
    let output = run_args(
        &["test", "--runs", "7", "/dev/stdin"],
        "property reflexive(x: i64) { x == x }\n",
    );
    assert!(output.status.success());
    assert_eq!(output.stdout, b"property reflexive ... ok (7 runs)\n");
}

#[test]
fn one_property_can_be_selected_for_editor_lenses() {
    let output = run_args(
        &[
            "test",
            "--runs",
            "9",
            "--property",
            "selected",
            "/dev/stdin",
        ],
        "property skipped(x: i64) { false }\nproperty selected(x: i64) { x == x }\n",
    );
    assert!(output.status.success());
    assert_eq!(output.stdout, b"property selected ... ok (9 runs)\n");

    let missing = run_args(
        &["test", "--property", "missing", "/dev/stdin"],
        "property selected(x: i64) { x == x }\n",
    );
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("unknown property `missing`"));
}

#[test]
fn ffi_declarations_parse_and_exports_remain_callable_in_host_tiers() {
    assert_modes(
        "extern \"m\" fn cbrt(x: f64): f64\n\
         extern \"m\" fn cbrtf(x: f32): f32\n\
         export fn twice(x: i64): i64 { x * 2 }\n\
         main { print(twice(21)) }\n",
        b"42\n",
    );
}

#[test]
fn ffi_boundary_subset_and_register_caps_are_checked() {
    let cases = [
        (
            "extern fn bad(x: [f32]): f32\nmain {}\n",
            "unsupported parameter",
        ),
        (
            "extern fn bad(inout x: i64)\nmain {}\n",
            "cannot have `inout`",
        ),
        (
            "extern fn bad(a: i64, b: i64, c: i64, d: i64, e: i64, f: i64, g: i64)\nmain {}\n",
            "maximum is 6 and 8",
        ),
        (
            "extern fn bad(a: f32, b: f32, c: f32, d: f32, e: f32, f: f32, g: f32, h: f32, i: f32)\nmain {}\n",
            "maximum is 6 and 8",
        ),
        (
            "type P { x: i64 }\nexport fn bad(p: P): i64 { p.x }\nmain {}\n",
            "unsupported parameter",
        ),
        (
            "extern fn bad(values: c_slice[f32]): f64\nmain {}\n",
            "unsupported parameter",
        ),
        (
            "extern fn bad(a: c_slice[f64], b: c_slice[f64], c: c_slice[f64], d: c_slice[f64])\nmain {}\n",
            "maximum is 6 and 8",
        ),
        (
            "@c_layout type Mixed { count: i64, scale: f64 }\nextern fn bad(value: Mixed)\nmain {}\n",
            "cannot mix integer/pointer and f64 fields",
        ),
        (
            "@c_layout type Narrow { x: f32, y: f32 }\nextern fn bad(value: Narrow)\nmain {}\n",
            "only 64-bit",
        ),
        (
            "@c_layout type Wide { x: i64, y: i64, z: i64 }\nextern fn bad(value: Wide)\nmain {}\n",
            "one or two 64-bit fields",
        ),
        (
            "extern fn bad(a: i64, b: i64, c: i64, d: i64, e: i64, f: i64): str\nmain {}\n",
            "maximum is 6 and 8",
        ),
    ];
    for (source, message) in cases {
        let output = run("interp", source);
        assert!(!output.status.success(), "accepted invalid FFI signature");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(message),
            "unexpected error: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn borrowed_c_slices_are_read_only_and_cannot_escape() {
    let cases = [
        (
            "fn bad(values: c_slice[f64]) {\n values[0] = 1.0\n}\nmain {}\n",
            "read-only",
        ),
        (
            "fn bad(values: c_slice[f64]): c_slice[f64] { return values }\nmain {}\n",
            "cannot return a borrowed c_slice",
        ),
        (
            "type Bad { values: c_slice[f64] }\nmain {}\n",
            "cannot store a borrowed c_slice",
        ),
    ];
    for (source, message) in cases {
        let output = run("check", source);
        assert!(!output.status.success(), "accepted escaping c_slice");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(message),
            "unexpected error: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn owned_array_results_are_export_only_and_scalar() {
    let cases = [
        (
            "extern fn foreign_values(): [f64]\nmain {}\n",
            "exported [i64]/[f64] owned results",
        ),
        (
            "export fn bad(): [bool] { return [true] }\nmain {}\n",
            "exported [i64]/[f64] owned results",
        ),
    ];
    for (source, message) in cases {
        let output = run("check", source);
        assert!(!output.status.success(), "accepted invalid owned result");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(message),
            "unexpected error: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn callback_signatures_are_typed_and_boundary_checked() {
    let valid = run("check", "extern fn install(cb: c_fn[() -> ()])\nmain {}\n");
    assert!(
        valid.status.success(),
        "rejected unit callback: {}",
        String::from_utf8_lossy(&valid.stderr)
    );

    for source in [
        "extern fn apply(cb: c_fn[(i64) -> i64], x: i64): i64\n\
         extern fn get(): c_fn[(f64) -> f64]\n\
         main { print(apply(get(), 1)) }\n",
        "extern fn bad(cb: c_fn[() -> [i64]])\nmain {}\n",
    ] {
        let output = run("check", source);
        assert!(
            !output.status.success(),
            "accepted invalid callback signature: {source}"
        );
    }
}

#[test]
fn mutable_c_slices_require_unique_mutable_variables_and_cannot_escape() {
    let cases = [
        (
            "fn write(values: c_mut_slice[f64]) { values[0] = 1.0 }\n\
             main { let values = [0.0]\n write(values) }\n",
            "needs a `var`",
        ),
        (
            "fn write(a: c_mut_slice[f64], b: c_slice[f64]) { a[0] = b[0] }\n\
             main { var values = [0.0]\n write(values, values) }\n",
            "aliases `values`",
        ),
        (
            "fn touch(values: c_mut_slice[f64]): f64 { values[0] = 1.0\n return values[0] }\n\
             fn combine(values: c_mut_slice[f64], amount: f64) { values[0] = amount }\n\
             main { var values = [0.0]\n combine(values, touch(values)) }\n",
            "aliases `values`",
        ),
        (
            "fn bad(values: c_mut_slice[f64]): c_mut_slice[f64] { return values }\nmain {}\n",
            "cannot return a borrowed c_mut_slice",
        ),
        (
            "type Bad { values: c_mut_slice[f64] }\nmain {}\n",
            "cannot store a borrowed c_mut_slice",
        ),
    ];
    for (source, message) in cases {
        let output = run("check", source);
        assert!(!output.status.success(), "accepted invalid c_mut_slice use");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(message),
            "unexpected error: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn ffi_names_cannot_collide_or_use_the_runtime_namespace() {
    let cases = [
        (
            "extern fn print(x: i64)\nmain {}\n",
            "collides with an existing function",
        ),
        (
            "fn local(x: i64): i64 { x }\nextern fn local(x: i64): i64\nmain {}\n",
            "collides with an existing function",
        ),
        (
            "extern fn lu_private(x: i64): i64\nmain {}\n",
            "uses reserved `lu_` prefix",
        ),
        (
            "extern fn same(x: i64): i64\nextern fn same(x: i64): i64\nmain {}\n",
            "duplicate extern",
        ),
    ];
    for (source, message) in cases {
        let output = run("check", source);
        assert!(!output.status.success(), "accepted colliding FFI name");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(message),
            "unexpected error: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn extern_declarations_are_top_level_only() {
    let output = run("check", "main { extern fn hidden(x: i64): i64 }\n");
    assert!(
        !output.status.success(),
        "accepted a nested extern declaration"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unexpected") || stderr.contains("expected"),
        "unexpected error: {stderr}"
    );
}

#[test]
fn check_mode_validates_without_executing_main() {
    let output = run("check", "main { print(1 / 0) }\n");
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    let output = run("check", "main { print(unknown) }\n");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown variable"));
}

/// A literal divisor that is neither 0 nor -1 lowers to a plain `sdiv`/`srem`
/// instead of a call into the runtime helper (the helper is an opaque
/// cross-translation-unit call, so it blocked strength reduction and
/// vectorization of any loop containing an integer divide). The results must
/// stay identical to the helper's, including the sign rules and the
/// `i64::MIN` edges.
#[test]
fn constant_divisors_match_the_runtime_helper() {
    // `opaque` keeps the divisor out of reach of constant folding, so each
    // pair compares the new inline path against the helper path.
    let source = "\
fn opaque(x: i64): i64 { return x }
main {
  print(7 / 2, 7 % 2, -7 / 2, -7 % 2, 7 / -2, 7 % -2, -7 / -2, -7 % -2)
  print(7 / opaque(2), 7 % opaque(2), -7 / opaque(2), -7 % opaque(2), 7 / opaque(-2), 7 % opaque(-2), -7 / opaque(-2), -7 % opaque(-2))
  print((-9223372036854775807 - 1) / 2, (-9223372036854775807 - 1) % 2)
  print((-9223372036854775807 - 1) / opaque(2), (-9223372036854775807 - 1) % opaque(2))
}
";
    let expected = b"3 1 -3 -1 -3 1 3 -1\n\
3 1 -3 -1 -3 1 3 -1\n\
-4611686018427387904 0\n\
-4611686018427387904 0\n";
    assert_modes(source, expected);
}

/// The traps from SPEC 3.1 must survive the inline-divide path: a zero
/// divisor and `i64::MIN / -1` are both excluded from it and still abort.
#[test]
fn integer_division_traps_still_fire() {
    for expr in [
        "1 / 0",
        "1 % 0",
        "(-9223372036854775807 - 1) / -1",
        "(-9223372036854775807 - 1) % -1",
    ] {
        let source = format!("main {{ print({expr}) }}\n");
        for mode in ["interp", "run"] {
            let output = run(mode, &source);
            assert!(
                !output.status.success(),
                "{mode} accepted `{expr}` instead of trapping"
            );
        }
    }
}

/// Ordering comparisons on i64 must compare as integers. They used to convert
/// both operands to f64 in the interpreter and the LLVM tier (the Cranelift
/// tier always did it right), so every i64 past 2^53 lost its low bits and
/// `i64::MAX > i64::MAX - 1` evaluated to false — a wrong answer *and* a
/// tier disagreement.
#[test]
fn i64_ordering_does_not_round_through_f64() {
    let source = "\
main {
  print(9007199254740993 > 9007199254740992)
  print(9007199254740993 < 9007199254740992)
  print(9007199254740993 >= 9007199254740992)
  print(9223372036854775807 > 9223372036854775806)
  print(-9223372036854775807 - 1 < -9223372036854775807)
}
";
    assert_modes(source, b"true\nfalse\ntrue\ntrue\ntrue\n");
}

/// `~=` keeps its relative-epsilon float semantics even when both operands are
/// integers; the integer-comparison path above must not capture it.
#[test]
fn approx_eq_stays_float_on_integer_operands() {
    assert_modes(
        "main {\n print(1 ~= 1, 1 ~= 2, 1 ~= 1.0)\n}\n",
        b"true false true\n",
    );
}

/// Array lengths are loaded with `!invariant.load`, which lets LLVM hoist the
/// length out of an indexed `while` loop and CSE the repeated bounds compares
/// (the element store and the length live in one allocation, so without the
/// metadata every store forced a reload and a re-check). The checks themselves
/// must still fire — on reads, and on writes from a loop that walks off the
/// end.
#[test]
fn bounds_checks_survive_the_invariant_length_load() {
    let read = "\
fn five(): i64 { return 5 }
main {
  var a = arr(3, 0)
  print(a[five()])
}
";
    let write = "\
main {
  var a = arr(3, 0)
  var i = 0
  while i < 20 {
    a[i] = i
    i = i + 1
  }
}
";
    for source in [read, write] {
        for mode in ["interp", "run"] {
            let output = run(mode, source);
            assert!(
                !output.status.success(),
                "{mode} did not trap on an out-of-bounds access"
            );
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("out of bounds"),
                "{mode} reported the wrong error: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

/// `str_from_bytes` is the language's only linear way to turn computed bytes
/// into a str; `chr` + `concat` is quadratic. It takes the low byte of each
/// i64 element and is bounds-checked on both ends.
#[test]
fn str_from_bytes_builds_and_checks_its_range() {
    let source = "\
main {
  var b = arr(5, 0)
  b[0] = 'h'
  b[1] = 'e'
  b[2] = 'l'
  b[3] = 'l'
  b[4] = 'o'
  puts(str_from_bytes(b, 0, 5))
  putnl()
  puts(str_from_bytes(b, 1, 3))
  putnl()
  print(len(str_from_bytes(b, 0, 0)), str_from_bytes(b, 0, 5) == \"hello\")
  var t = arr(1, 0)
  t[0] = 256 + 'A'
  print(str_from_bytes(t, 0, 1) == \"A\")
}
";
    assert_modes(source, b"hello\nel\n0 true\ntrue\n");

    for range in ["0, 9", "-1, 2", "3, 1"] {
        let bad = format!("main {{ var b = arr(4, 65)  puts(str_from_bytes(b, {range})) }}\n");
        for mode in ["interp", "run"] {
            let output = run(mode, &bad);
            assert!(
                !output.status.success(),
                "{mode} accepted str_from_bytes range {range}"
            );
        }
    }
}

/// A variable divisor is guarded inline and divided with a real instruction
/// rather than an opaque runtime call. The guard must still catch both traps,
/// and the results must match the helper's sign rules exactly.
#[test]
fn variable_divisors_divide_inline_and_still_trap() {
    let source = "\
fn opaque(x: i64): i64 { return x }
main {
  print(7 / opaque(2), 7 % opaque(2), -7 / opaque(2), -7 % opaque(2))
  print(7 / opaque(-2), 7 % opaque(-2), -7 / opaque(-2), -7 % opaque(-2))
  print(7 / opaque(1), -7 / opaque(1), 7 / opaque(7))
}
";
    assert_modes(source, b"3 1 -3 -1\n-3 1 3 -1\n7 -7 1\n");

    for expr in [
        "7 / opaque(0)",
        "7 % opaque(0)",
        "(-9223372036854775807 - 1) / opaque(-1)",
        "(-9223372036854775807 - 1) % opaque(-1)",
    ] {
        let bad = format!("fn opaque(x: i64): i64 {{ return x }}\nmain {{ print({expr}) }}\n");
        for mode in ["interp", "run"] {
            let output = run(mode, &bad);
            assert!(!output.status.success(), "{mode} accepted `{expr}`");
        }
    }
}

/// Zero-initialized arrays take a `calloc` path so large allocations are not
/// eagerly memset. The observable contents must be unchanged, and a non-zero
/// init (including -0.0, whose bits are not all zero) must still be filled.
#[test]
fn zero_initialized_arrays_still_read_back_as_zero() {
    let source = "\
main {
  var a = arr(1000, 0)
  var f = arr(1000, 0.0)
  var g = arr(4, -0.0)
  var h = arr(4, 7)
  print(a[0], a[999], f[0], f[999])
  print(g[0], g[3], h[0], h[3])
  print(sum(i in 0..1000) a[i], sum(i in 0..1000) f[i])
}
";
    assert_modes(source, b"0 0 0 0\n-0 -0 7 7\n0 0\n");
}

/// `i8` is a one-byte storage type: it widens to i64 in every arithmetic,
/// comparison, and reduction context, narrows only through `i8(x)`, and stores
/// one byte per array element.
#[test]
fn i8_is_a_byte_wide_storage_type() {
    let source = "\
fn dbl(x: i8): i64 { return x + x }
fn narrow(x: i64): i8 { return i8(x) }
main {
  print(dbl(i8(21)), narrow(300), dbl(narrow(300)))
  var a = arr(4, i8(0))
  a[0] = i8(127)
  a[1] = i8(128)
  a[2] = i8(-1)
  print(a[0], a[1], a[2], a[0] + a[1], a[2] * 3)
  print(a[0] > a[1], a[2] < 0, a[0] == 127)
  print(float(a[0]) / 2.0, int(a[2]))
  print(sum(i in 0..4) a[i])
  var b = arr(3, i8(7))
  print(b[0], b[2], len(b))
}
";
    assert_modes(
        source,
        b"42 44 88\n127 -128 -1 -1 -3\ntrue true true\n63.5 -1\n-2\n7 7 3\n",
    );
}

/// i8 is deliberately not allowed in records or across the C boundary yet:
/// record arrays allocate through a layout ABI that only knows 4- and 8-byte
/// components. Both must be rejected rather than silently miscompiled.
#[test]
fn i8_is_rejected_where_it_is_not_supported() {
    let record = "type R { a: i8, b: i64 }\nmain { var r = R { i8(1), 2 }  print(r.b) }\n";
    let output = run("check", record);
    assert!(!output.status.success(), "accepted an i8 record field");
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be i8"));

    let boundary = "extern fn f(x: c_slice[i8]): i64\nmain { var a = arr(4, i8(0))  print(f(a)) }\n";
    let output = run("check", boundary);
    assert!(!output.status.success(), "accepted i8 at the C boundary");
}

/// `break` leaves the innermost loop; `continue` skips to the next iteration
/// and, in a `for`, must still advance the index (it targets the latch block,
/// not the condition head).
#[test]
fn break_and_continue_target_the_innermost_loop() {
    let source = "\
main {
  var first = -1
  for i in 0..100 {
    if i * i > 50 { first = i  break }
  }
  var odds = 0
  for i in 0..10 {
    if i % 2 == 0 { continue }
    odds = odds + i
  }
  var j = 0
  var acc = 0
  while true {
    j = j + 1
    if j > 20 { break }
    if j % 3 != 0 { continue }
    acc = acc + j
  }
  var pairs = 0
  for a in 0..5 {
    for b in 0..5 {
      if b > a { break }
      pairs = pairs + 1
    }
  }
  print(first, odds, acc, j, pairs)
}
";
    assert_modes(source, b"8 25 63 21 15\n");

    for bad in [
        "main { break }\n",
        "main { continue }\n",
        "fn f(): i64 { break  return 1 }\nmain { print(f()) }\n",
    ] {
        let output = run("check", bad);
        assert!(!output.status.success(), "accepted `{bad}` outside a loop");
        assert!(String::from_utf8_lossy(&output.stderr).contains("outside of a loop"));
    }
}

/// Bitwise operators are i64-only, `>>` is arithmetic, shift counts are masked
/// to 0..63, and precedence is | < ^ < & < shifts < additive < multiplicative
/// (so bitwise binds tighter than comparison, unlike C).
#[test]
fn bitwise_operators_and_their_precedence() {
    let source = "\
main {
  print(12 & 10, 12 | 10, 12 ^ 10)
  print(1 << 10, 1024 >> 3, -8 >> 1)
  print(1 << 3 == 8, 12 & 10 == 8)
  print(1 << 2 + 1, 3 & 1 + 1)
  print(1 | 2 ^ 3 & 3)
  print(1 << 62, -1 >> 60)
}
";
    assert_modes(
        source,
        b"8 14 6\n1024 128 -4\ntrue true\n8 2\n1\n4611686018427387904 -1\n",
    );

    let output = run("check", "main { print(1.5 & 2) }\n");
    assert!(!output.status.success(), "accepted a float bitwise operand");
    assert!(String::from_utf8_lossy(&output.stderr).contains("needs two i64 operands"));
}

/// `putbytes` writes a span of an `[i64]` or `[i8]` straight to stdout with no
/// intermediate str, and is bounds-checked.
#[test]
fn putbytes_writes_spans_without_allocating() {
    let source = "\
main {
  var a = arr(5, i8(0))
  a[0] = i8('h')
  a[1] = i8('i')
  a[2] = i8(10)
  putbytes(a, 0, 3)
  var b = arr(3, 0)
  b[0] = 'o'
  b[1] = 'k'
  b[2] = 10
  putbytes(b, 0, 3)
}
";
    assert_modes(source, b"hi\nok\n");

    let bad = "main { var a = arr(3, i8(0))  putbytes(a, 0, 9) }\n";
    for mode in ["interp", "run"] {
        let output = run(mode, bad);
        assert!(!output.status.success(), "{mode} accepted an out-of-range putbytes");
    }
}
