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
         export fn twice(x: i64): i64 { x * 2 }\n\
         main { print(twice(21)) }\n",
        b"42\n",
    );
}

#[test]
fn ffi_boundary_subset_and_register_caps_are_checked() {
    let cases = [
        (
            "extern fn bad(x: f32): f32\nmain {}\n",
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
            "type P { x: i64 }\nexport fn bad(p: P): i64 { p.x }\nmain {}\n",
            "unsupported parameter",
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
