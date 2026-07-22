use crate::ast::Program;
use crate::check::{resolve_type, Type};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Component {
    I64,
    F64,
    Ptr,
}

pub fn components(p: &Program, ty: &Type) -> Result<Vec<Component>, String> {
    Ok(match ty {
        Type::F64 => vec![Component::F64],
        Type::I64 | Type::Bool | Type::Enum(_) => vec![Component::I64],
        Type::Str => vec![Component::Ptr, Component::I64],
        Type::Arr(_) => vec![Component::Ptr],
        Type::Unit => vec![],
        Type::Rec(ti) => {
            let mut out = Vec::new();
            for (_, field_ty) in &p.types[*ti].fields {
                out.extend(components(p, &resolve_type(p, field_ty)?)?);
            }
            out
        }
    })
}

pub fn field_offset(p: &Program, type_id: usize, field: &str) -> Result<(usize, Type), String> {
    let mut offset = 0;
    for (name, field_ty) in &p.types[type_id].fields {
        let ty = resolve_type(p, field_ty)?;
        let width = components(p, &ty)?.len();
        if name == field {
            return Ok((offset, ty));
        }
        offset += width;
    }
    Err(format!(
        "type `{}` has no field `{}`",
        p.types[type_id].name, field
    ))
}
