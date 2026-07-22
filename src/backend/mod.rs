//! Shared backend layers. `ir` owns validated lowering; this module owns the
//! target-independent ABI, layout, and optimization contracts consumed by the
//! Cranelift and LLVM emission modules.

pub mod abi;
pub mod layout;
pub mod optimization;
