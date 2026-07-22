use crate::ast::Program;
use crate::check::{Checker, Type};

/// The sole execution input: an owned AST plus a complete type assignment for
/// every reachable expression (parser arena artifacts remain `None`).
/// Construction performs all name, type, mutation, and
/// no-aliasing validation, so no backend can accidentally execute unchecked
/// parser output.
pub struct TypedProgram {
    program: Program,
    expr_types: Vec<Option<Type>>,
}

impl TypedProgram {
    pub fn lower(program: Program) -> Result<Self, String> {
        let expr_types = Checker::check_types(&program)?;
        Ok(Self {
            program,
            expr_types,
        })
    }

    pub fn program(&self) -> &Program {
        &self.program
    }

    pub fn expression_types(&self) -> &[Option<Type>] {
        &self.expr_types
    }
}
