use std::path::Path;
use std::process::{Command, Output};

fn run(old: &Path, new: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["abi", "check"])
        .arg(old)
        .arg(new)
        .output()
        .expect("run lu abi check")
}

fn manifest(library: &str, enums: &str, records: &str, exports: &str) -> String {
    format!(
        "{{\n\
         \"abi_version\": 1,\n\
         \"library\": \"{library}\",\n\
         \"enums\": {enums},\n\
         \"c_layout_records\": {records},\n\
         \"exports\": {exports}\n\
         }}\n"
    )
}

#[test]
fn abi_check_accepts_additive_changes_and_parameter_renames() {
    let directory =
        std::env::temp_dir().join(format!("lulang-abi-compatible-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let old = directory.join("old.json");
    let new = directory.join("new.json");
    std::fs::write(
        &old,
        manifest(
            "kernel",
            r#"{"Mode":["Fast"]}"#,
            "{}",
            r#"[{"name":"dot","params":[{"name":"n","type":"i64"}],"ret":"f64"}]"#,
        ),
    )
    .unwrap();
    std::fs::write(
        &new,
        manifest(
            "kernel",
            r#"{"Mode":["Fast","Strict"]}"#,
            r#"{"Pair":[{"name":"x","type":"f64"},{"name":"y","type":"f64"}]}"#,
            r#"[{"name":"dot","params":[{"name":"count","type":"i64"}],"ret":"f64"},{"name":"sum","params":[],"ret":"f64"}]"#,
        ),
    )
    .unwrap();

    let output = run(&old, &new);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("compatible: enum `Mode` gained values"));
    assert!(stdout.contains("compatible: export `sum` was added"));
    assert!(stdout.contains("parameter 1 was renamed"));
    assert!(stdout.contains("ABI compatible"));
}

#[test]
fn abi_check_rejects_binary_breaks() {
    let directory =
        std::env::temp_dir().join(format!("lulang-abi-breaking-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let old = directory.join("old.json");
    let new = directory.join("new.json");
    std::fs::write(
        &old,
        manifest(
            "kernel",
            r#"{"Mode":["Fast","Strict"]}"#,
            r#"{"Pair":[{"name":"x","type":"f64"},{"name":"y","type":"f64"}]}"#,
            r#"[{"name":"dot","params":[{"name":"n","type":"i64"}],"ret":"f64"},{"name":"gone","params":[],"ret":"i64"}]"#,
        ),
    )
    .unwrap();
    std::fs::write(
        &new,
        manifest(
            "kernel2",
            r#"{"Mode":["Strict","Fast"]}"#,
            r#"{"Pair":[{"name":"x","type":"f64"},{"name":"y","type":"f64"},{"name":"tag","type":"i64"}]}"#,
            r#"[{"name":"dot","params":[{"name":"n","type":"f64"}],"ret":"i64"}]"#,
        ),
    )
    .unwrap();

    let output = run(&old, &new);
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("breaking: library changed"));
    assert!(stdout.contains("breaking: enum `Mode` changed existing layout or tags"));
    assert!(stdout.contains("breaking: C-layout record `Pair` gained fields"));
    assert!(stdout.contains("breaking: export `dot` return changed"));
    assert!(stdout.contains("breaking: export `dot` parameter 1 type changed"));
    assert!(stdout.contains("breaking: export `gone` was removed"));
    assert!(stdout.contains("ABI incompatible: 6 breaking change(s)"));
}

#[test]
fn abi_check_reports_malformed_and_future_manifests() {
    let directory = std::env::temp_dir().join(format!("lulang-abi-invalid-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let invalid = directory.join("invalid.json");
    let future = directory.join("future.json");
    std::fs::write(&invalid, "{not json}").unwrap();
    std::fs::write(
        &future,
        manifest("kernel", "{}", "{}", "[]").replace("\"abi_version\": 1", "\"abi_version\": 2"),
    )
    .unwrap();
    let output = run(&invalid, &future);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot parse"));

    let output = run(&future, &future);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unsupported ABI manifest version 2"));
}
