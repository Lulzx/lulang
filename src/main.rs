use lu_ir::ir;
use lu_jit::{jit, runtime as jit_runtime};
use lu_llvm::llvm;
use lu_syntax::{fmt, lexer, parser};
use lu_test::{interp, runtime as test_runtime};

mod bindgen;

use std::process::ExitCode;

fn usage() -> ExitCode {
    eprintln!(
        "usage: lu <run|build|check|interp> <file.lu> [program args...]\n\
         \x20      lu build --lib [--shared] [-o name] <file.lu>\n\
         \x20      lu bindgen [--lib name] [-o file.lu] <header.h>\n\
         \x20      lu test [--runs N] <file.lu>\n\
         \x20      lu fmt [--check] <file.lu>"
    );
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let mode = match args.get(1) {
        None => return usage(),
        Some(arg) if args.len() == 2 => "run",
        Some(arg) => arg.as_str(),
    };
    let mut runs = 100u32;
    let mut check_format = false;
    let mut build_library = false;
    let mut build_shared = false;
    let mut output_name = None;
    let mut bindgen_library = None;
    let mut positionals = Vec::new();
    let mut i = if args.len() == 2 { 1 } else { 2 };
    while i < args.len() {
        match args[i].as_str() {
            "--runs" if mode == "test" => {
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
            "--shared" if mode == "build" => {
                build_shared = true;
                i += 1;
            }
            "-o" if matches!(mode, "build" | "bindgen") => {
                let Some(value) = args.get(i + 1) else {
                    eprintln!("error: -o needs a value");
                    return ExitCode::FAILURE;
                };
                output_name = Some(value.clone());
                i += 2;
            }
            arg if arg.starts_with('-') && matches!(mode, "test" | "fmt" | "build" | "bindgen") => {
                eprintln!("error: unknown option `{}`", arg);
                return ExitCode::FAILURE;
            }
            _ => {
                positionals.push(args[i].clone());
                i += 1;
            }
        }
    }
    let Some(path) = positionals.first() else {
        return usage();
    };
    if mode == "bindgen" {
        if positionals.len() != 1 {
            return usage();
        }
        return run_bindgen(path, output_name.as_deref(), bindgen_library.as_deref());
    }
    if !matches!(mode, "test" | "fmt") && positionals.len() > 1 {
        let program_args = positionals[1..].to_vec();
        jit_runtime::set_args(program_args.clone());
        test_runtime::set_args(program_args);
    }
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {}: {}", path, e);
            return ExitCode::FAILURE;
        }
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
    let mode_owned = mode.to_string();
    let path_owned = path.to_string();
    let output_name_owned = output_name.clone();
    let result = std::thread::Builder::new()
        .stack_size(512 << 20)
        .spawn(move || {
            run_pipeline(
                &mode_owned,
                &path_owned,
                &src_owned,
                runs,
                build_library,
                build_shared,
                output_name_owned.as_deref(),
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

fn run_bindgen(path: &str, output: Option<&str>, library: Option<&str>) -> ExitCode {
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
    let generated = match bindgen::generate(&header, input, library) {
        Ok(generated) => generated,
        Err(error) => {
            eprintln!("error: cannot parse {}: {}", path, error);
            return ExitCode::FAILURE;
        }
    };
    let default_output = input.with_extension("lu");
    let output = output
        .map(std::path::PathBuf::from)
        .unwrap_or(default_output);
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

fn run_pipeline(
    mode: &str,
    path: &str,
    src: &str,
    property_runs: u32,
    build_library: bool,
    build_shared: bool,
    output_name: Option<&str>,
) -> Result<bool, String> {
    (|| -> Result<bool, String> {
        let toks = lexer::lex(src)?;
        let mut p = parser::Parser::new(toks);
        p.parse()?;
        let ir = ir::LoweredProgram::lower(p.prog)?;
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
                if build_shared && !build_library {
                    return Err("`--shared` requires `--lib`".into());
                }
                if build_library {
                    for out in llvm::build_library(&ir, path, output_name, build_shared)? {
                        eprintln!("built {}", out);
                    }
                } else {
                    let out = llvm::build(&ir, path, output_name)?;
                    eprintln!("built ./{}", out);
                }
                Ok(true)
            }
            "test" => interp::Interp::new(&ir).run_properties(property_runs),
            "check" => Ok(true),
            m => Err(format!("unknown mode `{}`", m)),
        }
    })()
}
