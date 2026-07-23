pub use lu_syntax::{ast, lexer, parser};

pub mod check {
    pub use lu_check::check::*;
}

#[path = "../../../src/ir.rs"]
pub mod ir;
