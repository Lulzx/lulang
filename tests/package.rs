use std::path::Path;
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

fn git(directory: &Path, arguments: &[&str]) -> Output {
    run(Command::new("git").arg("-C").arg(directory).args(arguments))
}

fn write_dependency(directory: &Path, body: &str) {
    std::fs::create_dir_all(directory.join("src")).expect("create dependency source directory");
    std::fs::write(
        directory.join("lu.toml"),
        "[package]\nname = \"numerics\"\nversion = \"0.1.0\"\n\n[dependencies]\n",
    )
    .expect("write dependency manifest");
    std::fs::write(directory.join("src/lib.lu"), body).expect("write dependency source");
}

fn commit_dependency(directory: &Path) {
    run(Command::new("git")
        .args(["init", "--quiet", "-b", "main"])
        .arg(directory));
    git(
        directory,
        &["config", "user.email", "package-test@example.com"],
    );
    git(directory, &["config", "user.name", "Package Test"]);
    git(directory, &["add", "."]);
    git(directory, &["commit", "--quiet", "-m", "initial"]);
}

#[test]
fn git_dependencies_are_locked_cached_and_composed_whole_program() {
    let directory = std::env::temp_dir().join(format!("lulang-package-{}", std::process::id()));
    let dependency = directory.join("numerics");
    let root = directory.join("orbit");
    let cache = directory.join("cache");
    std::fs::create_dir_all(&dependency).expect("create dependency repository");
    std::fs::create_dir_all(&root).expect("create root package");
    write_dependency(&dependency, "fn triple(x: i64): i64 { return x * 3 }\n");
    commit_dependency(&dependency);
    let first_commit = String::from_utf8_lossy(&git(&dependency, &["rev-parse", "HEAD"]).stdout)
        .trim()
        .to_string();

    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["init", "orbit"])
        .current_dir(&root)
        .env("LULANG_CACHE", &cache));
    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["add", "numerics", "--git"])
        .arg(&dependency)
        .args(["--rev", "main"])
        .current_dir(&root)
        .env("LULANG_CACHE", &cache));
    std::fs::write(
        root.join("src/main.lu"),
        "use numerics\nmain { print(numerics.triple(14)) }\n",
    )
    .expect("write root source");

    let lock = std::fs::read_to_string(root.join("lu.lock")).expect("read lock");
    assert!(lock.contains(&format!("commit = \"{first_commit}\"")));
    assert!(lock.contains("tree = \""));
    assert!(cache
        .join("git")
        .join(&first_commit)
        .join("lu.toml")
        .exists());

    let interpreted = run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .arg("run")
        .current_dir(&root)
        .env("LULANG_CACHE", &cache));
    assert_eq!(interpreted.stdout, b"42\n");

    std::fs::write(
        root.join("src/main.lu"),
        "use numerics as numbers\nfn triple(x: i64): i64 { return x + 1 }\nmain { print(triple(14), numbers.triple(14)) }\n",
    )
    .expect("write collision-safe module source");
    let namespaced = run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .arg("run")
        .current_dir(&root)
        .env("LULANG_CACHE", &cache));
    assert_eq!(namespaced.stdout, b"15 42\n");

    write_dependency(&dependency, "fn triple(x: i64): i64 { return x * 4 }\n");
    git(&dependency, &["add", "."]);
    git(&dependency, &["commit", "--quiet", "-m", "move main"]);
    std::fs::write(
        root.join("src/main.lu"),
        "use numerics\nmain { print(numerics.triple(14)) }\n",
    )
    .expect("restore namespaced package source");
    let still_locked = run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .arg("run")
        .current_dir(&root)
        .env("LULANG_CACHE", &cache));
    assert_eq!(
        still_locked.stdout, b"42\n",
        "a movable branch changed a locked build"
    );

    let executable = root.join("orbit");
    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["build", "-o"])
        .arg(&executable)
        .current_dir(&root)
        .env("LULANG_CACHE", &cache));
    assert_eq!(run(&mut Command::new(&executable)).stdout, b"42\n");

    std::fs::write(
        root.join("src/lib.lu"),
        "fn identity(x: i64): i64 { return x }\n",
    )
    .expect("write package library");
    std::fs::create_dir_all(root.join("tests")).expect("create package tests");
    std::fs::write(
        root.join("tests/laws.lu"),
        "property identity_law(x: i64) { identity(x) == x }\n",
    )
    .expect("write package property");
    let tested = run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["test", "--runs", "5"])
        .current_dir(&root)
        .env("LULANG_CACHE", &cache));
    assert_eq!(tested.stdout, b"property identity_law ... ok (5 runs)\n");

    std::fs::write(root.join("src/main.lu"), "use missing\nmain {}\n")
        .expect("write undeclared import");
    let undeclared = Command::new(env!("CARGO_BIN_EXE_lu"))
        .arg("check")
        .current_dir(&root)
        .env("LULANG_CACHE", &cache)
        .output()
        .expect("check undeclared package");
    assert!(!undeclared.status.success());
    assert!(String::from_utf8_lossy(&undeclared.stderr).contains("imports undeclared package"));

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn imported_records_and_enums_link_with_qualified_names() {
    let directory =
        std::env::temp_dir().join(format!("lulang-module-types-{}", std::process::id()));
    let dependency = directory.join("geometry");
    let root = directory.join("viewer");
    let cache = directory.join("cache");
    std::fs::create_dir_all(&dependency).expect("create dependency");
    std::fs::create_dir_all(&root).expect("create root");
    write_dependency(
        &dependency,
        "type Point { x: i64, y: i64 }\nenum Axis { X, Y }\n\
         fn point_sum(value: Point): i64 { return value.x + value.y }\n\
         fn axis_tag(value: Axis): i64 {\n\
           if value == Axis.X { return 10 }\n\
           return 20\n\
         }\n",
    );
    std::fs::write(
        dependency.join("lu.toml"),
        "[package]\nname = \"geometry\"\nversion = \"0.1.0\"\n\n[dependencies]\n",
    )
    .expect("write geometry manifest");
    commit_dependency(&dependency);

    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["init", "viewer"])
        .current_dir(&root)
        .env("LULANG_CACHE", &cache));
    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["add", "geometry", "--git"])
        .arg(&dependency)
        .args(["--rev", "main"])
        .current_dir(&root)
        .env("LULANG_CACHE", &cache));
    std::fs::write(
        root.join("src/main.lu"),
        "use geometry\n\
         fn local_sum(value: geometry.Point): i64 { return value.x + value.y + 1 }\n\
         main {\n\
           let point = geometry.Point { x: 2, y: 3 }\n\
           print(geometry.point_sum(point), local_sum(point), geometry.axis_tag(geometry.Axis.Y))\n\
         }\n",
    )
    .expect("write qualified module program");
    let output = run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .arg("run")
        .current_dir(&root)
        .env("LULANG_CACHE", &cache));
    assert_eq!(output.stdout, b"5 6 20\n");

    let _ = std::fs::remove_dir_all(&directory);
}
