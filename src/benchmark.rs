use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

pub fn run(arguments: &[String]) -> Result<String, String> {
    let mut runs = 5u32;
    let mut source = None;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--runs" => {
                runs = arguments
                    .get(index + 1)
                    .ok_or("`lu bench --runs` needs a value")?
                    .parse()
                    .map_err(|_| "`lu bench --runs` must be a positive integer")?;
                if runs == 0 {
                    return Err("`lu bench --runs` must be a positive integer".into());
                }
                index += 2;
            }
            argument if argument.starts_with('-') => {
                return Err(format!("unknown `lu bench` option `{argument}`"))
            }
            argument if source.is_none() => {
                source = Some(PathBuf::from(argument));
                index += 1;
            }
            _ => return Err("usage: lu bench [--runs N] [file.lu]".into()),
        }
    }

    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let current = std::env::current_dir().map_err(|error| error.to_string())?;
    let root = find_workspace(&current).unwrap_or(current);
    let label = source
        .as_ref()
        .and_then(|path| path.file_stem())
        .and_then(|name| name.to_str())
        .map(String::from)
        .or_else(|| package_name(&root))
        .unwrap_or_else(|| "program".into());

    let mode_command = |mode: &str| {
        let mut command = Command::new(&executable);
        command.arg(mode);
        if let Some(source) = &source {
            command.arg(source);
        }
        command.current_dir(&root);
        command
    };
    let interp_ms = measure(mode_command("interp"), runs)?;
    let jit_ms = measure(mode_command("run"), runs)?;

    let native =
        std::env::temp_dir().join(format!("lulang-bench-{}-{}", std::process::id(), label));
    let mut build = Command::new(&executable);
    build.args(["build", "-o"]).arg(&native);
    if let Some(source) = &source {
        build.arg(source);
    }
    build.current_dir(&root);
    let compile_start = Instant::now();
    command_success(&mut build, "build benchmark program")?;
    let compile_ms = compile_start.elapsed().as_secs_f64() * 1000.0;
    let mut native_command = Command::new(&native);
    native_command.current_dir(&root);
    let aot_ms = measure(native_command, runs)?;
    let _ = std::fs::remove_file(&native);

    let history_directory = root.join("benchmarks");
    std::fs::create_dir_all(&history_directory).map_err(|error| error.to_string())?;
    let history = history_directory.join("history.csv");
    if !history.exists() {
        std::fs::write(
            &history,
            "timestamp,label,runs,interp_ms,jit_ms,aot_ms,compile_ms\n",
        )
        .map_err(|error| error.to_string())?;
    }
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_secs();
    let mut row = String::new();
    let _ = writeln!(
        row,
        "{timestamp},{label},{runs},{interp_ms:.6},{jit_ms:.6},{aot_ms:.6},{compile_ms:.6}"
    );
    use std::io::Write as _;
    std::fs::OpenOptions::new()
        .append(true)
        .open(&history)
        .and_then(|mut file| file.write_all(row.as_bytes()))
        .map_err(|error| error.to_string())?;

    Ok(format!(
        "benchmark `{label}` (mean of {runs})\n\
         interp  {interp_ms:.3} ms\n\
         jit     {jit_ms:.3} ms\n\
         aot     {aot_ms:.3} ms\n\
         compile {compile_ms:.3} ms\n\
         history {}",
        history.display()
    ))
}

fn measure(mut command: Command, runs: u32) -> Result<f64, String> {
    command.stdout(Stdio::null()).stderr(Stdio::piped());
    command_success(&mut command, "warm benchmark program")?;
    let start = Instant::now();
    for _ in 0..runs {
        command_success(&mut command, "run benchmark program")?;
    }
    Ok(start.elapsed().as_secs_f64() * 1000.0 / f64::from(runs))
}

fn command_success(command: &mut Command, action: &str) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|error| format!("cannot {action}: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "failed to {action}: {}",
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

fn find_workspace(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|directory| directory.join("lu.toml").exists())
        .map(Path::to_path_buf)
}

fn package_name(root: &Path) -> Option<String> {
    let source = std::fs::read_to_string(root.join("lu.toml")).ok()?;
    let mut package = false;
    for line in source.lines() {
        let line = line.trim();
        if line == "[package]" {
            package = true;
        } else if line.starts_with('[') {
            package = false;
        } else if package && line.starts_with("name") {
            return line
                .split_once('=')
                .map(|(_, value)| value.trim().trim_matches('"').to_string());
        }
    }
    None
}
