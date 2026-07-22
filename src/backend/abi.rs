use crate::ast::{FnDecl, Program};
use crate::backend::layout::{components, Component};
use crate::check::resolve_type;

/// Flattened AOT return ABI: declared result followed by each `inout` copy-out.
pub fn return_components(p: &Program, function: &FnDecl) -> Result<Vec<Component>, String> {
    let mut out = components(p, &resolve_type(p, &function.ret)?)?;
    for ((_, ty), inout) in function.params.iter().zip(function.inouts.iter()) {
        if *inout {
            out.extend(components(p, &resolve_type(p, ty)?)?);
        }
    }
    Ok(out)
}
