use lu_ir::ir;
use lu_jit::{jit, runtime as jit_runtime};
use lu_llvm::llvm;
use lu_syntax::{fmt, lexer, parser};
use lu_test::{interp, runtime as test_runtime};

mod abi;
mod benchmark;
mod bindgen;
mod docgen;
mod module_system;
mod package;
mod sdk;

use std::process::ExitCode;

fn usage() -> ExitCode {
    eprintln!(
        "usage: lu <run|build|check|interp> <file.lu> [program args...]\n\
         \x20      lu init [package-name]\n\
         \x20      lu add <name> --git <url> --rev <revision>\n\
         \x20      lu fetch\n\
         \x20      lu lsp\n\
         \x20      lu bench [--runs N] [file.lu]\n\
         \x20      lu doc [--runs N] [-o directory] [file.lu]\n\
         \x20      lu build --lib [--shared] [-o name] <file.lu>\n\
         \x20      lu build --target <wasm32-wasi|wasm32-web> [-o file.wasm] <file.lu>\n\
         \x20      lu build --emit-llvm [-o file.ll] <file.lu>\n\
         \x20      lu abi check <old.json> <new.json>\n\
         \x20      lu sdk <rust|cpp|julia|node|go|swift|r> [-o path] <manifest.json>\n\
         \x20      lu bindgen [--lib name] [--no-shims] [-o file.lu] <header.h>\n\
         \x20      lu test [--runs N] [--property name] <file.lu>\n\
         \x20      lu fmt [--check] <file.lu>"
    );
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let explicit_mode = args.get(1).is_some_and(|argument| {
        matches!(
            argument.as_str(),
            "run"
                | "build"
                | "check"
                | "interp"
                | "test"
                | "fmt"
                | "bindgen"
                | "init"
                | "add"
                | "fetch"
                | "bench"
                | "doc"
                | "lsp"
                | "abi"
                | "sdk"
        )
    });
    let mode = match args.get(1) {
        None => return usage(),
        Some(_) if !explicit_mode => "run",
        Some(arg) => arg.as_str(),
    };
    if mode == "init" {
        return package_result(package::init(&args[2..]));
    }
    if mode == "add" {
        return package_result(package::add(&args[2..]));
    }
    if mode == "fetch" {
        if args.len() != 2 {
            return usage();
        }
        return package_result(package::fetch());
    }
    if mode == "bench" {
        return package_result(benchmark::run(&args[2..]));
    }
    if mode == "abi" {
        if args.get(2).map(String::as_str) != Some("check") || args.len() != 5 {
            return usage();
        }
        return match abi::check(
            std::path::Path::new(&args[3]),
            std::path::Path::new(&args[4]),
        ) {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::FAILURE,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if mode == "sdk" {
        return package_result(sdk::run(&args[2..]));
    }
    if mode == "lsp" {
        if args.len() != 2 {
            return usage();
        }
        return run_lsp();
    }
    let mut runs = 100u32;
    let mut check_format = false;
    let mut build_library = false;
    let mut build_shared = false;
    let mut build_target = None;
    let mut emit_llvm = false;
    let mut output_name = None;
    let mut property_name = None;
    let mut bindgen_library = None;
    let mut bindgen_shims = true;
    let mut positionals = Vec::new();
    let mut i = if explicit_mode { 2 } else { 1 };
    while i < args.len() {
        match args[i].as_str() {
            "--runs" if matches!(mode, "test" | "doc") => {
                let value = args.get(i + 1).ok_or("--runs needs a value");
                runs = match value
                    .and_then(|s| s.parse::<u32>().map_err(|_| "invalid --runs value"))
                {
                    Ok(n) if n > 0 => n,
                    _ => {
                        eprintln!("error: --runs must be a positive integer");
                        return ExitCode::FAILURE;
                    }
                };
                i += 2;
            }
            "--property" if mode == "test" => {
                let Some(value) = args.get(i + 1) else {
                    eprintln!("error: --property needs a name");
                    return ExitCode::FAILURE;
                };
                property_name = Some(value.clone());
                i += 2;
            }
            "--check" if mode == "fmt" => {
                check_format = true;
                i += 1;
            }
            "--lib" if mode == "build" => {
                build_library = true;
                i += 1;
            }
            "--lib" if mode == "bindgen" => {
                let Some(value) = args.get(i + 1) else {
                    eprintln!("error: --lib needs a value");
                    return ExitCode::FAILURE;
                };
                bindgen_library = Some(value.clone());
                i += 2;
            }
            "--no-shims" if mode == "bindgen" => {
                bindgen_shims = false;
                i += 1;
            }
            "--shared" if mode == "build" => {
                build_shared = true;
                i += 1;
            }
            "--target" if mode == "build" => {
                let Some(value) = args.get(i + 1) else {
                    eprintln!("error: --target needs a value");
                    return ExitCode::FAILURE;
                };
                build_target = Some(value.clone());
                i += 2;
            }
            "--emit-llvm" if mode == "build" => {
                emit_llvm = true;
                i += 1;
            }
            "-o" if matches!(mode, "build" | "bindgen" | "doc") => {
                let Some(value) = args.get(i + 1) else {
                    eprintln!("error: -o needs a value");
                    return ExitCode::FAILURE;
                };
                output_name = Some(value.clone());
                i += 2;
            }
            arg if arg.starts_with('-')
                && matches!(mode, "test" | "fmt" | "build" | "bindgen" | "doc") =>
            {
                eprintln!("error: unknown option `{}`", arg);
                return ExitCode::FAILURE;
            }
            _ => {
                positionals.push(args[i].clone());
                i += 1;
            }
        }
    }
    if mode == "bindgen" {
        let Some(path) = positionals.first() else {
            return usage();
        };
        if positionals.len() != 1 {
            return usage();
        }
        return run_bindgen(
            path,
            output_name.as_deref(),
            bindgen_library.as_deref(),
            bindgen_shims,
        );
    }
    let package_input = if positionals.is_empty()
        && matches!(mode, "run" | "build" | "check" | "interp" | "test" | "doc")
    {
        match package::load_workspace(mode) {
            Ok(program) => Some(program),
            Err(error) => {
                eprintln!("error: {error}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        None
    };
    let path_owned = package_input
        .as_ref()
        .map(|program| program.label.clone())
        .or_else(|| positionals.first().cloned());
    let Some(path_owned) = path_owned else {
        return usage();
    };
    let path = path_owned.as_str();
    if package_input.is_none() && !matches!(mode, "test" | "fmt") && positionals.len() > 1 {
        let program_args = positionals[1..].to_vec();
        jit_runtime::set_args(program_args.clone());
        test_runtime::set_args(program_args);
    }
    let (src, sources) = match package_input {
        Some(program) => {
            let source = program
                .sources
                .iter()
                .map(|file| file.source.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            (source, program.sources)
        }
        None => match std::fs::read_to_string(path) {
            Ok(s) => {
                let source_file = module_system::SourceFile {
                    module: "main".into(),
                    path: path.to_string(),
                    source: s.clone(),
                    root: true,
                };
                (s, vec![source_file])
            }
            Err(e) => {
                eprintln!("error: cannot read {}: {}", path, e);
                return ExitCode::FAILURE;
            }
        },
    };
    if mode == "fmt" {
        let formatted = fmt::format_source(&src);
        let tokens = match lexer::lex(&formatted) {
            Ok(tokens) => tokens,
            Err(error) => {
                eprintln!("error: {}", error);
                return ExitCode::FAILURE;
            }
        };
        let mut parser = parser::Parser::new(tokens);
        if let Err(error) = parser.parse() {
            eprintln!("error: {}", error);
            return ExitCode::FAILURE;
        }
        if check_format {
            if formatted == src {
                return ExitCode::SUCCESS;
            }
            eprintln!("error: {} is not canonically formatted", path);
            return ExitCode::FAILURE;
        }
        if formatted != src {
            if let Err(e) = std::fs::write(path, formatted) {
                eprintln!("error: cannot write {}: {}", path, e);
                return ExitCode::FAILURE;
            }
        }
        return ExitCode::SUCCESS;
    }
    // deep recursion (e.g. self-hosted interpreter towers) needs more than the
    // default 8 MiB main-thread stack; run the pipeline on a 512 MiB thread
    let src_owned = src.clone();
    let sources_owned = sources;
    let mode_owned = mode.to_string();
    let path_owned = path.to_string();
    let output_name_owned = output_name.clone();
    let build_target_owned = build_target.clone();
    let property_name_owned = property_name.clone();
    let result = std::thread::Builder::new()
        .stack_size(512 << 20)
        .spawn(move || {
            run_pipeline(
                &mode_owned,
                &path_owned,
                &src_owned,
                &sources_owned,
                runs,
                build_library,
                build_shared,
                build_target_owned.as_deref(),
                emit_llvm,
                output_name_owned.as_deref(),
                property_name_owned.as_deref(),
            )
        })
        .expect("spawn")
        .join()
        .expect("join");
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("error: {}", e);
            ExitCode::FAILURE
        }
    }
}

fn package_result(result: Result<String, String>) -> ExitCode {
    match result {
        Ok(message) => {
            eprintln!("{message}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_lsp() -> ExitCode {
    let repository_script =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/lulang_lsp.py");
    let installed_script = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
        .map(|path| path.join("../share/lulang/lulang_lsp.py"));
    let script = std::env::var_os("LULANG_LSP")
        .map(std::path::PathBuf::from)
        .or_else(|| repository_script.exists().then_some(repository_script))
        .or_else(|| installed_script.filter(|path| path.exists()));
    let Some(script) = script else {
        eprintln!("error: cannot find lulang_lsp.py; set LULANG_LSP");
        return ExitCode::FAILURE;
    };
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("error: cannot locate `lu`: {error}");
            return ExitCode::FAILURE;
        }
    };
    match std::process::Command::new("python3")
        .arg(script)
        .env("LULANG_BIN", executable)
        .status()
    {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => {
            eprintln!("error: language server exited with {status}");
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("error: cannot start language server: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_bindgen(
    path: &str,
    output: Option<&str>,
    library: Option<&str>,
    build_shims: bool,
) -> ExitCode {
    let header = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("error: cannot read {}: {}", path, error);
            return ExitCode::FAILURE;
        }
    };
    let input = std::path::Path::new(path);
    let inferred_library = input
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("c")
        .strip_prefix("lib")
        .unwrap_or_else(|| {
            input
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("c")
        });
    let library = library.unwrap_or(inferred_library);
    let default_output = input.with_extension("lu");
    let output = output
        .map(std::path::PathBuf::from)
        .unwrap_or(default_output);
    let shim_output = (build_shims && output.as_os_str() != "-").then(|| {
        output.with_extension(if cfg!(target_os = "macos") {
            "bindgen.dylib"
        } else {
            "bindgen.so"
        })
    });
    let shim_reference = shim_output.as_ref().map(|path| {
        if path.is_absolute() {
            path.clone()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."))
                .join(path)
        }
    });
    let include_path = std::fs::canonicalize(input).unwrap_or_else(|_| input.to_path_buf());
    let generated = match bindgen::generate(
        &header,
        &include_path,
        library,
        shim_reference.as_deref().and_then(std::path::Path::to_str),
    ) {
        Ok(generated) => generated,
        Err(error) => {
            eprintln!("error: cannot parse {}: {}", path, error);
            return ExitCode::FAILURE;
        }
    };
    if let (Some(shim_source), Some(shim_output)) =
        (generated.shim_source.as_deref(), shim_output.as_deref())
    {
        let shim_c = output.with_extension("bindgen.c");
        if let Err(error) = std::fs::write(&shim_c, shim_source) {
            eprintln!("error: cannot write {}: {}", shim_c.display(), error);
            return ExitCode::FAILURE;
        }
        if let Err(error) = compile_bindgen_shim(&shim_c, shim_output, library) {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
        eprintln!(
            "built {} C adapter shim(s) in {}",
            generated.shimmed_functions,
            shim_output.display()
        );
    }
    if output.as_os_str() == "-" {
        print!("{}", generated.source);
    } else if let Err(error) = std::fs::write(&output, &generated.source) {
        eprintln!("error: cannot write {}: {}", output.display(), error);
        return ExitCode::FAILURE;
    }
    for warning in &generated.warnings {
        eprintln!("warning: {}", warning);
    }
    eprintln!(
        "generated {} C import(s) in {}",
        generated.imported_functions,
        if output.as_os_str() == "-" {
            "stdout".into()
        } else {
            output.display().to_string()
        }
    );
    ExitCode::SUCCESS
}

fn compile_bindgen_shim(
    source: &std::path::Path,
    output: &std::path::Path,
    library: &str,
) -> Result<(), String> {
    let mut command = std::process::Command::new("cc");
    if cfg!(target_os = "macos") {
        command.arg("-dynamiclib");
    } else {
        command.args(["-shared", "-fPIC"]);
    }
    command
        .args(["-O2", "-std=c11"])
        .arg(source)
        .arg("-o")
        .arg(output);
    if library.contains('/') || library.ends_with(".so") || library.ends_with(".dylib") {
        command.arg(library);
    } else if !library.is_empty() && library != "c" {
        command.arg(format!("-l{library}"));
    }
    let result = command
        .output()
        .map_err(|error| format!("cannot start C compiler for bindgen shim: {error}"))?;
    if result.status.success() {
        Ok(())
    } else {
        Err(format!(
            "C adapter shim compilation failed:\n{}",
            String::from_utf8_lossy(&result.stderr)
        ))
    }
}

fn run_pipeline(
    mode: &str,
    path: &str,
    src: &str,
    sources: &[module_system::SourceFile],
    property_runs: u32,
    build_library: bool,
    build_shared: bool,
    build_target: Option<&str>,
    emit_llvm: bool,
    output_name: Option<&str>,
    property_name: Option<&str>,
) -> Result<bool, String> {
    (|| -> Result<bool, String> {
        let program = module_system::parse_and_link(sources)?;
        let ir = ir::LoweredProgram::lower(program)?;
        match mode {
            "run" => {
                jit::Jit::run(&ir)?;
                Ok(true)
            }
            "interp" => {
                interp::Interp::new(&ir).run_main()?;
                Ok(true)
            }
            "build" => {
                if emit_llvm && (build_target.is_some() || build_library || build_shared) {
                    return Err(
                        "`--emit-llvm` cannot be combined with --target, --lib, or --shared".into(),
                    );
                }
                if build_target.is_some() && (build_library || build_shared) {
                    return Err("`--target` cannot be combined with `--lib` or `--shared`".into());
                }
                if build_shared && !build_library {
                    return Err("`--shared` requires `--lib`".into());
                }
                if emit_llvm {
                    let out = llvm::emit_llvm(&ir, path, output_name)?;
                    eprintln!("built {}", out);
                } else if let Some(target) = build_target {
                    let target = match target {
                        "wasm32-wasi" => llvm::WasmTarget::Wasi,
                        "wasm32-web" => llvm::WasmTarget::Web,
                        target => {
                            return Err(format!(
                                "unsupported target `{target}`; expected wasm32-wasi or wasm32-web"
                            ))
                        }
                    };
                    for out in llvm::build_wasm(&ir, path, output_name, target)? {
                        eprintln!("built {}", out);
                    }
                } else if build_library {
                    for out in llvm::build_library(&ir, path, output_name, build_shared)? {
                        eprintln!("built {}", out);
                    }
                } else {
                    let out = llvm::build(&ir, path, output_name)?;
                    eprintln!("built {}", out);
                }
                Ok(true)
            }
            "test" => match property_name {
                Some(name) => interp::Interp::new(&ir).run_property(property_runs, name),
                None => interp::Interp::new(&ir).run_properties(property_runs),
            },
            "doc" => {
                for out in docgen::build(&ir, src, path, output_name, property_runs)? {
                    eprintln!("built {}", out);
                }
                Ok(true)
            }
            "check" => Ok(true),
            m => Err(format!("unknown mode `{}`", m)),
        }
    })()
}
