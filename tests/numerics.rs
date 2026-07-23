use std::path::PathBuf;
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
fn first_party_numerical_package_has_laws_docs_comparisons_and_all_tiers() {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let package = repository.join("lib/lu-numerics");
    let directory = std::env::temp_dir().join(format!("lulang-numerics-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create numerics test directory");

    let properties = run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["test", "--runs", "25"])
        .current_dir(&package));
    let properties = String::from_utf8_lossy(&properties.stdout);
    for law in [
        "vector_laws",
        "statistics_laws",
        "integration_is_exact_for_a_line",
        "random_kernels_are_deterministic_and_bounded",
        "special_function_symmetries",
        "combinatoric_recurrences",
    ] {
        assert!(
            properties.contains(&format!("property {law} ... ok (25 runs)")),
            "missing law {law}:\n{properties}"
        );
    }

    let reference = run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .arg("interp")
        .current_dir(&package))
    .stdout;
    assert!(String::from_utf8_lossy(&reference).starts_with("lu-numerics\ndot: 20"));
    assert_eq!(
        run(Command::new(env!("CARGO_BIN_EXE_lu"))
            .arg("run")
            .current_dir(&package))
        .stdout,
        reference
    );

    let native = directory.join("numerics-demo");
    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["build", "-o"])
        .arg(&native)
        .current_dir(&package));
    assert_eq!(run(&mut Command::new(&native)).stdout, reference);

    let combined = directory.join("numerics-combined.lu");
    let mut source =
        std::fs::read_to_string(package.join("src/lib.lu")).expect("read numerics library");
    source.push_str(
        &std::fs::read_to_string(package.join("src/main.lu")).expect("read numerics example"),
    );
    std::fs::write(&combined, source).expect("write selfhost source");
    assert_eq!(
        run(Command::new(env!("CARGO_BIN_EXE_lu"))
            .arg("run")
            .arg(repository.join("selfhost/interp.lu"))
            .arg(&combined))
        .stdout,
        reference
    );

    let docs = directory.join("docs");
    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["doc", "--runs", "20", "-o"])
        .arg(&docs)
        .current_dir(&package));
    let mut pages = 0;
    for entry in std::fs::read_dir(docs.join("functions")).expect("read function docs") {
        let path = entry.expect("function page").path();
        let page = std::fs::read_to_string(path).expect("read function page");
        assert!(!page.contains("No prose description was provided."));
        assert!(!page.contains("No property reaches this function."));
        pages += 1;
    }
    assert!(pages >= 27, "expected complete API docs, got {pages} pages");
    assert!(docs.join("program.ll").exists());
    assert!(docs.join("module.h").exists());
    assert!(docs.join("observatory.html").exists());

    let benchmark_rows =
        std::fs::read_to_string(package.join("benchmarks/functions.tsv")).expect("benchmarks");
    let comparison_rows =
        std::fs::read_to_string(package.join("comparisons/functions.tsv")).expect("comparisons");
    for function in [
        "dot",
        "matmul_square_inplace",
        "convolution_inplace",
        "monte_carlo_pi",
        "bisect_sqrt",
        "normal_pdf",
        "distance3",
        "binomial",
    ] {
        assert!(benchmark_rows
            .lines()
            .any(|line| line.starts_with(function)));
        assert!(comparison_rows
            .lines()
            .any(|line| line.starts_with(function)));
    }

    if Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        let output = Command::new("python3")
            .arg(package.join("test_numerics.py"))
            .env("PYTHONPATH", repository.join("python/pylulang"))
            .env("LULANG_BIN", env!("CARGO_BIN_EXE_lu"))
            .output()
            .expect("run lu-numerics Python comparison");
        assert!(
            output.status.success(),
            "lu-numerics failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let _ = std::fs::remove_dir_all(directory);
}
