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
    eprintln!("usage: lu <run|build|test|interp> <file.lu>");
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let (mode, path) = match args.len() {
        2 => ("run", args[1].as_str()),
        3 => (args[1].as_str(), args[2].as_str()),
        _ => return usage(),
    };
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {}: {}", path, e);
            return ExitCode::FAILURE;
        }
    };
    let result = (|| -> Result<bool, String> {
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
    })();
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("error: {}", e);
            ExitCode::FAILURE
        }
    }
}
