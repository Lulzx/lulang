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
fn luimage_runs_laws_all_tiers_and_a_real_c_render() {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let package = repository.join("lib/luimage");
    let directory = std::env::temp_dir().join(format!("lulang-luimage-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create luimage test directory");

    let laws = run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["test", "--runs", "30"])
        .current_dir(&package));
    let laws = String::from_utf8_lossy(&laws.stdout);
    for law in [
        "mandelbrot_pixels_are_normalized",
        "inversion_is_an_involution",
        "mandelbrot_is_symmetric_across_the_real_axis",
        "zero_exposure_produces_black",
    ] {
        assert!(laws.contains(&format!("property {law} ... ok (30 runs)")));
    }

    let interpreted = run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .arg("interp")
        .current_dir(&package))
    .stdout;
    assert!(String::from_utf8_lossy(&interpreted).starts_with("luimage\nsize: 80 40\n"));
    assert_eq!(
        run(Command::new(env!("CARGO_BIN_EXE_lu"))
            .arg("run")
            .current_dir(&package))
        .stdout,
        interpreted
    );
    let combined = directory.join("luimage-combined.lu");
    let mut combined_source =
        std::fs::read_to_string(package.join("src/lib.lu")).expect("read luimage library");
    combined_source.push_str(
        &std::fs::read_to_string(package.join("src/main.lu")).expect("read luimage main"),
    );
    std::fs::write(&combined, combined_source).expect("write selfhost fixture");
    assert_eq!(
        run(Command::new(env!("CARGO_BIN_EXE_lu"))
            .arg("run")
            .arg(repository.join("selfhost/interp.lu"))
            .arg(&combined))
        .stdout,
        interpreted
    );
    let native = directory.join("luimage-demo");
    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["build", "-o"])
        .arg(&native)
        .current_dir(&package));
    assert_eq!(run(&mut Command::new(&native)).stdout, interpreted);

    let base = directory.join("luimage");
    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["build", "--lib", "-o"])
        .arg(&base)
        .arg(package.join("src/lib.lu")));
    let header = std::fs::read_to_string(directory.join("luimage.h")).expect("read header");
    assert!(header
        .contains("void render_mandelbrot(double *pixels_data, int64_t pixels_len, int64_t width"));
    assert!(header.contains("double image_checksum(const double *pixels_data"));

    let renderer = directory.join("render");
    run(Command::new("cc")
        .arg("-O2")
        .arg("-I")
        .arg(&directory)
        .arg(package.join("examples/render.c"))
        .arg(directory.join("libluimage.a"))
        .arg("-o")
        .arg(&renderer));
    let preview = directory.join("mandelbrot.pgm");
    let output = run(Command::new(&renderer).arg(&preview));
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("16000 "));
    let image = std::fs::read(&preview).expect("read rendered PGM");
    let header = b"P5\n160 100\n255\n";
    assert!(image.starts_with(header));
    assert_eq!(image.len(), header.len() + 160 * 100);
    let pixels = &image[header.len()..];
    assert!(pixels.iter().any(|pixel| *pixel == 0));
    assert!(pixels.iter().any(|pixel| *pixel > 200));

    let docs = directory.join("docs");
    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["doc", "--runs", "10", "-o"])
        .arg(&docs)
        .current_dir(&package));
    assert!(docs.join("module.h").exists());
    assert!(docs.join("module.json").exists());
    assert!(docs.join("observatory.html").exists());

    let _ = std::fs::remove_dir_all(directory);
}
