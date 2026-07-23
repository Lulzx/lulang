#![allow(clippy::approx_constant, clippy::not_unsafe_ptr_arg_deref)]

pub use lu_syntax::{ast, lexer, parser};

pub mod check {
    pub use lu_check::check::*;
}
pub mod ir {
    pub use lu_ir::ir::*;
}

#[path = "../../../src/backend/mod.rs"]
pub mod backend;
#[path = "../../../src/ffi.rs"]
pub mod ffi;
#[path = "../../../src/jit.rs"]
pub mod jit;
#[path = "../../../src/runtime.rs"]
pub mod runtime;
