use std::path::Path;
use std::process::Command;

#[test]
fn lsp_and_editor_assets_validate() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        let output = Command::new("python3")
            .args(["-m", "unittest", "discover", "-s"])
            .arg(repository.join("tools/tests"))
            .env("LULANG_BIN", env!("CARGO_BIN_EXE_lu"))
            .output()
            .expect("run LSP tests");
        assert!(
            output.status.success(),
            "LSP tests failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    if Command::new("node")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        for script in [
            "editors/vscode/extension.js",
            "editors/tree-sitter-lulang/grammar.js",
        ] {
            let output = Command::new("node")
                .arg("--check")
                .arg(repository.join(script))
                .output()
                .expect("check editor JavaScript");
            assert!(
                output.status.success(),
                "{script} is invalid:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let package = std::fs::read_to_string(repository.join("editors/vscode/package.json"))
            .expect("read VS Code manifest");
        assert!(package.contains("\"lulang.runProperty\""));
        assert!(package.contains("\"editor.formatOnSave\": true"));
        let snippets =
            std::fs::read_to_string(repository.join("editors/vscode/snippets/lulang.json"))
                .expect("read snippets");
        assert!(snippets.contains("‖${1:value}‖"));
        assert!(snippets.contains("\\\\dot"));
    }
}
