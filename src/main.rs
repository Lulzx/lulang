mod ast;
mod interp;
mod lexer;
mod parser;

use std::process::ExitCode;

fn usage() -> ExitCode {
    eprintln!("usage: lu <run|test> <file.lu>");
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
        let it = interp::Interp::new(&p.prog);
        match mode {
            "run" => {
                it.run_main()?;
                Ok(true)
            }
            "test" => it.run_properties(100),
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
