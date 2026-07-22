use std::io::Write as _;
use std::process::{Command, Output, Stdio};

fn run(mode: &str, source: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lu"))
        .args([mode, "/dev/stdin"])
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
        assert!(!output.status.success(), "{mode} accepted an invalid record");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("initialized more than once"),
            "unexpected {mode} error: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
