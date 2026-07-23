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

fn tool_exists(tool: &str) -> bool {
    Command::new(tool)
        .arg(if tool == "zig" {
            "version"
        } else {
            "--version"
        })
        .output()
        .is_ok_and(|output| output.status.success())
}

#[test]
fn wasi_and_web_targets_execute_the_same_program() {
    if !tool_exists("zig") || !tool_exists("node") {
        eprintln!("skipping wasm integration test: zig and node are required");
        return;
    }

    let directory = std::env::temp_dir().join(format!("lulang-wasm-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create wasm test directory");
    let source = directory.join("answer.lu");
    std::fs::write(
        &source,
        "main {\n\
           let n = 11\n\
           var a = arr(n, 0.0)\n\
           var b = arr(n, 0.0)\n\
           for i in 0..n {\n\
             a[i] = float(i)\n\
             b[i] = float(i + 1)\n\
           }\n\
           print(\"wasm\", 6 * 7, sum(i in 0..n) a[i] * b[i])\n\
         }\n",
    )
    .expect("write source");

    let wasi = directory.join("answer-wasi.wasm");
    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["build", "--target", "wasm32-wasi", "-o"])
        .arg(&wasi)
        .arg(&source));
    assert_eq!(
        &std::fs::read(&wasi).expect("read WASI module")[..4],
        b"\0asm"
    );
    let wasi_runner = r#"
const fs = require("fs");
const { WASI } = require("wasi");
const wasi = new WASI({ version: "preview1", args: ["answer"], env: {} });
WebAssembly.instantiate(fs.readFileSync(process.argv[1]), {
  wasi_snapshot_preview1: wasi.wasiImport
}).then(({ instance }) => wasi.start(instance));
"#;
    let wasi_output = run(Command::new("node").args(["-e", wasi_runner]).arg(&wasi));
    assert_eq!(wasi_output.stdout, b"wasm 42 440\n");

    let web = directory.join("answer-web.wasm");
    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["build", "--target", "wasm32-web", "-o"])
        .arg(&web)
        .arg(&source));
    let loader = web.with_extension("js");
    assert!(loader.exists(), "web target did not emit its JS loader");
    let module_loader = web.with_extension("mjs");
    std::fs::copy(&loader, &module_loader).expect("copy loader as an ESM module");
    let web_runner = directory.join("run-web.mjs");
    std::fs::write(
        &web_runner,
        r#"
import fs from "node:fs";
import { pathToFileURL } from "node:url";
const { instantiateLulang } = await import(pathToFileURL(process.argv[2]));
const lines = [];
const app = await instantiateLulang(fs.readFileSync(process.argv[3]), line => lines.push(line));
const status = app.run();
if (status !== 0) process.exit(status);
process.stdout.write(lines.join("\n") + "\n");
"#,
    )
    .expect("write web runner");
    let web_output = run(Command::new("node")
        .arg(&web_runner)
        .arg(&module_loader)
        .arg(&web));
    assert_eq!(web_output.stdout, b"wasm 42 440\n");

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn wasm_target_rejects_native_externs() {
    if !tool_exists("zig") {
        return;
    }
    let output = Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["build", "--target", "wasm32-wasi", "/dev/stdin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write as _;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(b"extern \"m\" fn cbrt(x: f64): f64\nmain {}\n")?;
            child.wait_with_output()
        })
        .expect("run wasm rejection");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot use native `extern`"));
}
