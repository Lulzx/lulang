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

#[test]
fn docs_publish_signatures_properties_abi_ir_and_benchmarks() {
    let directory = std::env::temp_dir().join(format!("lulang-docs-{}", std::process::id()));
    let source_directory = directory.join("src");
    let benchmark_directory = directory.join("benchmarks");
    std::fs::create_dir_all(&source_directory).expect("create source directory");
    std::fs::create_dir_all(&benchmark_directory).expect("create benchmark directory");
    std::fs::write(
        directory.join("lu.toml"),
        "[package]\nname = \"documented\"\nversion = \"0.1.0\"\n\n[dependencies]\n",
    )
    .expect("write manifest");
    std::fs::write(
        source_directory.join("lib.lu"),
        "/// Returns the square of a scalar.\nexport fn square(x: f64): f64 { return x * x }\n",
    )
    .expect("write library");
    let tests_directory = directory.join("tests");
    std::fs::create_dir_all(&tests_directory).expect("create tests directory");
    std::fs::write(
        tests_directory.join("laws.lu"),
        "property square_nonnegative(x: f64) { square(x) >= 0.0 }\n",
    )
    .expect("write package property");
    std::fs::write(
        source_directory.join("main.lu"),
        "main { print(square(12.0)) }\n",
    )
    .expect("write main");
    for (name, source) in [
        ("sample.lu", "main { print(1) }\n"),
        ("sample.cpp", "int main() { return 0; }\n"),
        ("sample.rs", "fn main() {}\n"),
        ("sample.jl", "println(1)\n"),
        ("sample.py", "print(1)\n"),
        ("sample.ts", "console.log(1);\n"),
    ] {
        std::fs::write(directory.join(name), source).expect("write observatory source");
    }
    std::fs::write(
        benchmark_directory.join("observatory.tsv"),
        "date\tkernel\tlulang_aot_ms\tlulang_jit_ms\tlulang_selfhost_ms\tcpp_o3_ms\tcpp_fast_ms\trust_ms\tjulia_ms\tnumpy_ms\tjs_ms\tlu_source\tcpp_source\trust_source\tjulia_source\tnumpy_source\tjs_source\tassumptions_layout\n2026-07-23\tsample\t1.0\t2.0\t1.1\t3.0\t2.5\t2.8\t\t4.2\t4.0\tsample.lu\tsample.cpp\tsample.rs\tsample.jl\tsample.py\tsample.ts\torder-free sum; SoA\n",
    )
    .expect("write observatory data");
    std::fs::write(
        benchmark_directory.join("environment.json"),
        "{\"machine\":\"test\"}\n",
    )
    .expect("write observatory environment");

    let benchmark = run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["bench", "--runs", "1"])
        .current_dir(&directory));
    assert!(String::from_utf8_lossy(&benchmark.stderr).contains("history"));
    assert!(benchmark_directory.join("history.csv").exists());

    let site = directory.join("site");
    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["doc", "--runs", "25", "-o"])
        .arg(&site)
        .current_dir(&directory));
    for artifact in [
        "index.html",
        "observatory.html",
        "program.ll",
        "module.h",
        "module.json",
        "properties.json",
        "source.lu",
        "style.css",
    ] {
        assert!(site.join(artifact).exists(), "missing {artifact}");
    }
    let index = std::fs::read_to_string(site.join("index.html")).expect("read index");
    assert!(index.contains("square_nonnegative"));
    assert!(index.contains("PASS"));
    assert!(index.contains("benchmark observatory"));
    let function =
        std::fs::read_to_string(site.join("functions/square.html")).expect("read function page");
    assert!(function.contains("Returns the square of a scalar."));
    assert!(function.contains("square_nonnegative"));
    assert!(function.contains("double square(double x);"));
    assert!(function.contains("Local benchmark history"));
    let observatory =
        std::fs::read_to_string(site.join("observatory.html")).expect("read observatory");
    assert!(observatory.contains("order-free sum; SoA"));
    assert!(observatory.contains("LU_LAYOUT"));
    assert!(observatory.contains("selfhost"));
    assert!(observatory.contains("NumPy"));
    assert!(site.join("environment.json").exists());
    assert!(
        std::fs::read_dir(site.join("sources"))
            .expect("read copied sources")
            .count()
            >= 6
    );
    assert!(std::fs::read_to_string(site.join("program.ll"))
        .expect("read LLVM")
        .contains("define"));

    let standalone = directory.join("standalone.lu");
    let standalone_ir = directory.join("standalone.ll");
    std::fs::write(&standalone, "main { print(1) }\n").expect("write standalone program");
    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["build", "--emit-llvm", "-o"])
        .arg(&standalone_ir)
        .arg(&standalone)
        .current_dir(&directory));
    assert!(std::fs::read_to_string(standalone_ir)
        .expect("read explicitly emitted LLVM")
        .contains("define"));

    let _ = std::fs::remove_dir_all(directory);
}
