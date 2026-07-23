#![allow(clippy::not_unsafe_ptr_arg_deref)]

pub mod check {
    pub use lu_check::check::*;
}
pub mod ir {
    pub use lu_ir::ir::*;
}

#[path = "../../../src/ffi.rs"]
pub mod ffi;
#[path = "../../../src/interp.rs"]
pub mod interp;
#[path = "../../../src/runtime.rs"]
pub mod runtime;
