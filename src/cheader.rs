use crate::check::Type;
use crate::ir::LoweredProgram;
use std::collections::BTreeSet;
use std::fmt::Write as _;

fn c_scalar_type(ty: &Type) -> Result<&'static str, String> {
    match ty {
        Type::I64 | Type::Bool | Type::Enum(_) => Ok("int64_t"),
        Type::CPtr(_) => Ok("void *"),
        Type::F32 => Ok("float"),
        Type::F64 => Ok("double"),
        Type::Unit => Ok("void"),
        _ => Err(format!("unsupported C ABI type {:?}", ty)),
    }
}

fn c_layout_field_type(program: &LoweredProgram, ty: &Type) -> Result<String, String> {
    match ty {
        Type::Rec(index) if program.records[*index].c_layout => {
            Ok(program.records[*index].name.clone())
        }
        _ => c_scalar_type(ty).map(String::from),
    }
}

fn emit_c_layout_record(
    program: &LoweredProgram,
    index: usize,
    emitted: &mut BTreeSet<usize>,
    out: &mut String,
) -> Result<(), String> {
    if !emitted.insert(index) {
        return Ok(());
    }
    let record = &program.records[index];
    for (_, field) in &record.fields {
        if let Type::Rec(nested) = field {
            if program.records[*nested].c_layout {
                emit_c_layout_record(program, *nested, emitted, out)?;
            }
        }
    }
    let _ = writeln!(out, "typedef struct {} {{", record.name);
    for (name, ty) in &record.fields {
        let _ = writeln!(out, "    {} {};", c_layout_field_type(program, ty)?, name);
    }
    let _ = writeln!(out, "}} {};\n", record.name);
    Ok(())
}

fn json_string(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c < ' ' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn lu_type_name(program: &LoweredProgram, ty: &Type) -> String {
    match ty {
        Type::I64 => "i64".into(),
        Type::F32 => "f32".into(),
        Type::F64 => "f64".into(),
        Type::Bool => "bool".into(),
        Type::Str => "str".into(),
        Type::Unit => "()".into(),
        Type::Arr(element) => format!("[{}]", lu_type_name(program, element)),
        Type::CSlice(element) => format!("c_slice[{}]", lu_type_name(program, element)),
        Type::CMutSlice(element) => {
            format!("c_mut_slice[{}]", lu_type_name(program, element))
        }
        Type::CPtr(element) => format!("c_ptr[{}]", lu_type_name(program, element)),
        Type::CFn(params, ret) => format!(
            "c_fn[({})->{}]",
            params
                .iter()
                .map(|ty| lu_type_name(program, ty))
                .collect::<Vec<_>>()
                .join(","),
            lu_type_name(program, ret)
        ),
        Type::Rec(index) => program.records[*index].name.clone(),
        Type::Enum(index) => program.enums[*index].name.clone(),
    }
}

fn c_params(
    program: &LoweredProgram,
    function: &crate::ir::Function,
    callbacks: &[(Type, String)],
) -> Result<Vec<String>, String> {
    let mut params = Vec::new();
    for &local in &function.params {
        let local = &function.locals[local as usize];
        match &local.ty {
            Type::Str => {
                params.push(format!("const char *{}_data", local.name));
                params.push(format!("int64_t {}_len", local.name));
            }
            Type::Arr(element) => {
                let element = c_scalar_type(element)?;
                params.push(format!("{} *{}_data", element, local.name));
                params.push(format!("int64_t {}_len", local.name));
            }
            Type::CSlice(element) => {
                let element = c_scalar_type(element)?;
                params.push(format!("const {} *{}_data", element, local.name));
                params.push(format!("int64_t {}_len", local.name));
            }
            Type::CMutSlice(element) => {
                let element = c_scalar_type(element)?;
                params.push(format!("{} *{}_data", element, local.name));
                params.push(format!("int64_t {}_len", local.name));
            }
            Type::Rec(index) if program.records[*index].c_layout => {
                params.push(format!("{} {}", program.records[*index].name, local.name))
            }
            Type::CFn(_, _) => params.push(format!(
                "{} {}",
                callback_name(callbacks, &local.ty)?,
                local.name
            )),
            ty => params.push(format!("{} {}", c_scalar_type(ty)?, local.name)),
        }
    }
    if function.ret == Type::Str {
        params.push("int64_t *out_len".into());
    }
    if params.is_empty() {
        params.push("void".into());
    }
    Ok(params)
}

fn callback_name<'a>(callbacks: &'a [(Type, String)], ty: &Type) -> Result<&'a str, String> {
    callbacks
        .iter()
        .find_map(|(candidate, name)| (candidate == ty).then_some(name.as_str()))
        .ok_or_else(|| "missing generated callback typedef".into())
}

fn collect_callbacks(ty: &Type, callbacks: &mut Vec<(Type, String)>) {
    if let Type::CFn(params, ret) = ty {
        for param in params {
            collect_callbacks(param, callbacks);
        }
        collect_callbacks(ret, callbacks);
        if !callbacks.iter().any(|(candidate, _)| candidate == ty) {
            callbacks.push((ty.clone(), format!("lu_callback_{}", callbacks.len())));
        }
    }
}

fn callback_typedef_name(type_name: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in type_name.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("lu_callback_{hash:016x}")
}

fn callback_c_type(
    program: &LoweredProgram,
    ty: &Type,
    callbacks: &[(Type, String)],
) -> Result<String, String> {
    Ok(match ty {
        Type::Rec(index) if program.records[*index].c_layout => {
            program.records[*index].name.clone()
        }
        Type::CFn(_, _) => callback_name(callbacks, ty)?.into(),
        _ => c_scalar_type(ty)?.into(),
    })
}

pub fn emit_header(program: &LoweredProgram, guard_name: &str) -> Result<String, String> {
    let guard = guard_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    let mut out = format!(
        "#ifndef {guard}_H\n#define {guard}_H\n\n#include <stdint.h>\n\n#ifdef __cplusplus\nextern \"C\" {{\n#endif\n\n"
    );
    let mut emitted_records = BTreeSet::new();
    for (index, record) in program.records.iter().enumerate() {
        if record.c_layout {
            emit_c_layout_record(program, index, &mut emitted_records, &mut out)?;
        }
    }
    let mut callbacks = Vec::new();
    for function in program
        .functions
        .iter()
        .filter(|function| function.exported)
    {
        for &local in &function.params {
            collect_callbacks(&function.locals[local as usize].ty, &mut callbacks);
        }
        collect_callbacks(&function.ret, &mut callbacks);
    }
    for (callback, name) in &mut callbacks {
        *name = callback_typedef_name(&lu_type_name(program, callback));
    }
    for (callback, name) in &callbacks {
        let Type::CFn(params, ret) = callback else {
            unreachable!()
        };
        let mut c_params = Vec::new();
        for ty in params {
            match ty {
                Type::Str => c_params.extend(["const char *".into(), "int64_t".into()]),
                Type::Arr(element) | Type::CMutSlice(element) => {
                    c_params.push(format!("{} *", c_scalar_type(element)?));
                    c_params.push("int64_t".into());
                }
                Type::CSlice(element) => {
                    c_params.push(format!("const {} *", c_scalar_type(element)?));
                    c_params.push("int64_t".into());
                }
                _ => c_params.push(callback_c_type(program, ty, &callbacks)?),
            }
        }
        if ret.as_ref() == &Type::Str {
            c_params.push("int64_t *".into());
        }
        if c_params.is_empty() {
            c_params.push("void".into());
        }
        let ret = if ret.as_ref() == &Type::Str {
            "const char *".into()
        } else {
            callback_c_type(program, ret, &callbacks)?
        };
        let _ = writeln!(out, "typedef {ret} (*{name})({});", c_params.join(", "));
    }
    if !callbacks.is_empty() {
        out.push('\n');
    }
    if program.records.iter().any(|record| {
        record.c_layout
            && record.fields.len() == 2
            && record.fields[0].0 == "status"
            && record.fields[0].1 == Type::I64
            && record.fields[1].0 == "value"
            && record.fields[1].1 == Type::I64
    }) {
        out.push_str("#define LU_STATUS_OK INT64_C(0)\n\n");
    }
    for definition in &program.enums {
        for (tag, variant) in definition.variants.iter().enumerate() {
            let _ = writeln!(
                out,
                "#define {}_{} INT64_C({})",
                definition.name, variant, tag
            );
        }
        if !definition.variants.is_empty() {
            out.push('\n');
        }
    }
    let owned_i64 = program.functions.iter().any(|function| {
        function.exported
            && matches!(&function.ret, Type::Arr(element) if element.as_ref() == &Type::I64)
    });
    let owned_f64 = program.functions.iter().any(|function| {
        function.exported
            && matches!(&function.ret, Type::Arr(element) if element.as_ref() == &Type::F64)
    });
    if owned_i64 {
        out.push_str(
            "typedef struct lu_owned_i64 lu_owned_i64;\n\
             int64_t *lu_owned_i64_data(lu_owned_i64 *handle);\n\
             int64_t lu_owned_i64_len(const lu_owned_i64 *handle);\n\
             void lu_owned_i64_release(lu_owned_i64 *handle);\n\n",
        );
    }
    if owned_f64 {
        out.push_str(
            "typedef struct lu_owned_f64 lu_owned_f64;\n\
             double *lu_owned_f64_data(lu_owned_f64 *handle);\n\
             int64_t lu_owned_f64_len(const lu_owned_f64 *handle);\n\
             void lu_owned_f64_release(lu_owned_f64 *handle);\n\n",
        );
    }
    for function in program
        .functions
        .iter()
        .filter(|function| function.exported)
    {
        let params = function
            .params
            .iter()
            .map(|&local| {
                let local = &function.locals[local as usize];
                format!("{}: {}", local.name, lu_type_name(program, &local.ty))
            })
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            out,
            "/* export fn {}({}): {} */",
            function.name,
            params,
            lu_type_name(program, &function.ret)
        );
        let return_type = if function.ret == Type::Str {
            "const char *".to_string()
        } else if let Type::Arr(element) = &function.ret {
            match element.as_ref() {
                Type::I64 => "lu_owned_i64 *".into(),
                Type::F64 => "lu_owned_f64 *".into(),
                _ => return Err("owned C result arrays require i64 or f64 elements".into()),
            }
        } else if let Type::Rec(index) = &function.ret {
            if program.records[*index].c_layout {
                program.records[*index].name.clone()
            } else {
                return Err("compiler-layout record cannot be returned through C".into());
            }
        } else if matches!(&function.ret, Type::CFn(_, _)) {
            callback_name(&callbacks, &function.ret)?.to_string()
        } else {
            c_scalar_type(&function.ret)?.to_string()
        };
        let _ = writeln!(
            out,
            "{} {}({});\n",
            return_type,
            function.name,
            c_params(program, function, &callbacks)?.join(", ")
        );
    }
    out.push_str("#ifdef __cplusplus\n}\n#endif\n\n#endif\n");
    Ok(out)
}

pub fn emit_manifest(program: &LoweredProgram, library: &str) -> String {
    let mut out = format!(
        "{{\n  \"abi_version\": 1,\n  \"library\": {},\n  \"enums\": {{",
        json_string(library)
    );
    for (enum_index, definition) in program.enums.iter().enumerate() {
        if enum_index == 0 {
            out.push('\n');
        } else {
            out.push_str(",\n");
        }
        let _ = write!(out, "    {}: [", json_string(&definition.name));
        for (variant_index, variant) in definition.variants.iter().enumerate() {
            if variant_index != 0 {
                out.push_str(", ");
            }
            out.push_str(&json_string(variant));
        }
        out.push(']');
    }
    if !program.enums.is_empty() {
        out.push('\n');
    }
    out.push_str("  },\n  \"c_layout_records\": {");
    let records = program
        .records
        .iter()
        .filter(|record| record.c_layout)
        .collect::<Vec<_>>();
    for (record_index, record) in records.iter().enumerate() {
        if record_index == 0 {
            out.push('\n');
        } else {
            out.push_str(",\n");
        }
        let _ = write!(out, "    {}: [", json_string(&record.name));
        for (field_index, (name, ty)) in record.fields.iter().enumerate() {
            if field_index != 0 {
                out.push_str(", ");
            }
            let _ = write!(
                out,
                "{{\"name\": {}, \"type\": {}}}",
                json_string(name),
                json_string(&lu_type_name(program, ty))
            );
        }
        out.push(']');
    }
    if !records.is_empty() {
        out.push('\n');
    }
    out.push_str("  },\n  \"exports\": [");
    let exports = program
        .functions
        .iter()
        .filter(|function| function.exported)
        .collect::<Vec<_>>();
    for (function_index, function) in exports.iter().enumerate() {
        if function_index == 0 {
            out.push('\n');
        } else {
            out.push_str(",\n");
        }
        let _ = write!(
            out,
            "    {{\"name\": {}, \"params\": [",
            json_string(&function.name)
        );
        for (param_index, &local) in function.params.iter().enumerate() {
            if param_index != 0 {
                out.push_str(", ");
            }
            let local = &function.locals[local as usize];
            let _ = write!(
                out,
                "{{\"name\": {}, \"type\": {}}}",
                json_string(&local.name),
                json_string(&lu_type_name(program, &local.ty))
            );
        }
        let _ = write!(
            out,
            "], \"ret\": {}}}",
            json_string(&lu_type_name(program, &function.ret))
        );
    }
    if !exports.is_empty() {
        out.push('\n');
    }
    out.push_str("  ]\n}\n");
    out
}
