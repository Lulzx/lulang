use lu_ir::ir::LoweredProgram;
use lu_syntax::{lexer, parser};
use std::time::{Duration, Instant};

fn compile_frontend(source: &str) {
    let tokens = lexer::lex(source).expect("lex generated source");
    let mut parser = parser::Parser::new(tokens);
    parser.parse().expect("parse generated source");
    LoweredProgram::lower(parser.prog).expect("check and lower generated source");
}

#[test]
fn one_thousand_line_frontend_budget() {
    let mut source = String::from("main {\n");
    for index in 0..998 {
        source.push_str(&format!("  let value{} = {} + 1\n", index, index));
    }
    source.push_str("}\n");
    assert_eq!(source.lines().count(), 1000);

    compile_frontend(&source);
    let mut best = Duration::MAX;
    for _ in 0..5 {
        let start = Instant::now();
        compile_frontend(&source);
        best = best.min(start.elapsed());
    }

    let budget = if cfg!(debug_assertions) {
        Duration::from_millis(100)
    } else {
        Duration::from_millis(10)
    };
    assert!(
        best < budget,
        "1k-line frontend took {:?}, budget is {:?}",
        best,
        budget
    );
}
