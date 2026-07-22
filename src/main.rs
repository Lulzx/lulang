mod ast;
mod check;
mod interp;
mod jit;
mod lexer;
mod llvm;
mod parser;
mod runtime;

use std::process::ExitCode;

fn usage() -> ExitCode {
    eprintln!("usage: lu <run|build|test|interp> <file.lu> [program args...]");
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let (mode, path) = match args.len() {
        0 | 1 => return usage(),
        2 => ("run", args[1].as_str()),
        _ => (args[1].as_str(), args[2].as_str()),
    };
    if args.len() > 3 {
        runtime::set_args(args[3..].to_vec());
    }
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {}: {}", path, e);
            return ExitCode::FAILURE;
        }
    };
    // deep recursion (e.g. self-hosted interpreter towers) needs more than the
    // default 8 MiB main-thread stack; run the pipeline on a 512 MiB thread
    let src_owned = src.clone();
    let mode_owned = mode.to_string();
    let path_owned = path.to_string();
    let result = std::thread::Builder::new()
        .stack_size(512 << 20)
        .spawn(move || run_pipeline(&mode_owned, &path_owned, &src_owned))
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

fn run_pipeline(mode: &str, path: &str, src: &str) -> Result<bool, String> {
    (|| -> Result<bool, String> {
        let toks = lexer::lex(&src)?;
        let mut p = parser::Parser::new(toks);
        p.parse()?;
        check::Checker::check(&p.prog)?;
        match mode {
            "run" => {
                jit::Jit::run(&p.prog)?;
                Ok(true)
            }
            "interp" => {
                interp::Interp::new(&p.prog).run_main()?;
                Ok(true)
            }
            "build" => {
                let out = llvm::build(&p.prog, path, None)?;
                eprintln!("built ./{}", out);
                Ok(true)
            }
            "test" => interp::Interp::new(&p.prog).run_properties(100),
            m => Err(format!("unknown mode `{}`", m)),
        }
    })()
}
