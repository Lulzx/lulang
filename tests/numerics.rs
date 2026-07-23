use std::path::Path;
use std::process::Command;

#[test]
fn first_party_numerical_kernels_pass_python_integration() {
    if !Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        return;
    }
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new("python3")
        .arg(repository.join("lib/lu-numerics/test_numerics.py"))
        .env("PYTHONPATH", repository.join("python/pylulang"))
        .env("LULANG_BIN", env!("CARGO_BIN_EXE_lu"))
        .output()
        .expect("run lu-numerics tests");
    assert!(
        output.status.success(),
        "lu-numerics failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
