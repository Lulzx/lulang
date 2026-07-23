use std::path::Path;
use std::process::Command;

#[test]
fn python_package_calls_generated_libraries() {
    let available = Command::new("python3").arg("--version").output();
    if !available.is_ok_and(|output| output.status.success()) {
        return;
    }
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new("python3")
        .args(["-m", "unittest", "discover", "-s"])
        .arg(repository.join("python/pylulang/tests"))
        .arg("-v")
        .env("PYTHONPATH", repository.join("python/pylulang"))
        .env("LULANG_BIN", env!("CARGO_BIN_EXE_lu"))
        .output()
        .expect("run pylulang tests");
    assert!(
        output.status.success(),
        "pylulang tests failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
