use crate::ast::*;
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Type {
    I64,
    F32,
    F64,
    Bool,
    Str,
    Unit,
    Arr(Box<Type>),
    CSlice(Box<Type>),
    CMutSlice(Box<Type>),
    CPtr(Box<Type>),
    CFn(Vec<Type>, Box<Type>),
    Rec(usize),
    Enum(usize),
}

/// True if evaluating `e` can write variable `name` through an `inout`
/// parameter of some call nested inside it. Used by the no-aliasing rule:
/// copy-in for an inout argument snapshots the variable at its argument
/// position, so a mutation from a sibling argument would be silently lost.
fn writes_var(p: &Program, e: ExprId, name: &str) -> bool {
    match p.expr(e) {
        Expr::Call(fname, args) => {
            if let Some(f) = p.find_fn(fname) {
                for (j, &a) in args.iter().enumerate() {
                    let mutable_borrow = f.inouts.get(j).copied().unwrap_or(false)
                        || f.params
                            .get(j)
                            .and_then(|(_, ty)| resolve_type(p, ty).ok())
                            .is_some_and(|ty| matches!(ty, Type::CMutSlice(_)));
                    if mutable_borrow {
                        if let Expr::Ident(n) = p.expr(a) {
                            if n == name {
                                return true;
                            }
                        }
                    }
                }
            }
            if let Some(f) = p.externs.iter().find(|f| f.name == *fname) {
                for (j, &a) in args.iter().enumerate() {
                    let mutable_borrow = f
                        .params
                        .get(j)
                        .and_then(|(_, ty)| resolve_type(p, ty).ok())
                        .is_some_and(|ty| matches!(ty, Type::Arr(_) | Type::CMutSlice(_)));
                    if mutable_borrow && matches!(p.expr(a), Expr::Ident(n) if n == name) {
                        return true;
                    }
                }
            }
            args.iter().any(|&a| writes_var(p, a, name))
        }
        Expr::Bin(_, l, r) | Expr::Index(l, r) => {
            writes_var(p, *l, name) || writes_var(p, *r, name)
        }
        Expr::Un(_, x) | Expr::Circum(_, x) | Expr::Field(x, _) => writes_var(p, *x, name),
        Expr::Record(_, inits) => inits.iter().any(|(_, a)| writes_var(p, *a, name)),
        Expr::Array(items) => items.iter().any(|&a| writes_var(p, a, name)),
        Expr::Sum { lo, hi, body, .. } => {
            writes_var(p, *lo, name) || writes_var(p, *hi, name) || writes_var(p, *body, name)
        }
        _ => false,
    }
}

pub fn resolve_type(p: &Program, s: &str) -> Result<Type, String> {
    match s {
        "f32" => Ok(Type::F32),
        "f64" => Ok(Type::F64),
        "i64" | "i32" => Ok(Type::I64),
        "bool" => Ok(Type::Bool),
        "str" => Ok(Type::Str),
        "()" => Ok(Type::Unit),
        _ => {
            if let Some(inner) = s.strip_prefix('[').and_then(|x| x.strip_suffix(']')) {
                Ok(Type::Arr(Box::new(resolve_type(p, inner)?)))
            } else if let Some(inner) = s
                .strip_prefix("c_slice[")
                .and_then(|inner| inner.strip_suffix(']'))
            {
                Ok(Type::CSlice(Box::new(resolve_type(p, inner)?)))
            } else if let Some(inner) = s
                .strip_prefix("c_mut_slice[")
                .and_then(|inner| inner.strip_suffix(']'))
            {
                Ok(Type::CMutSlice(Box::new(resolve_type(p, inner)?)))
            } else if let Some(inner) = s
                .strip_prefix("c_ptr[")
                .and_then(|inner| inner.strip_suffix(']'))
            {
                Ok(Type::CPtr(Box::new(resolve_type(p, inner)?)))
            } else if let Some(inner) = s
                .strip_prefix("c_fn[(")
                .and_then(|inner| inner.strip_suffix(']'))
            {
                parse_c_fn_type(p, inner)
            } else if let Some(ei) = p.enums.iter().position(|e| e.name == s) {
                Ok(Type::Enum(ei))
            } else {
                p.types
                    .iter()
                    .position(|t| t.name == s)
                    .map(Type::Rec)
                    .ok_or(format!("unknown type `{}`", s))
            }
        }
    }
}

fn parse_c_fn_type(p: &Program, inner: &str) -> Result<Type, String> {
    let close = inner
        .find(")->")
        .ok_or_else(|| format!("invalid callback type `c_fn[({inner}]`"))?;
    let params = &inner[..close];
    let ret = &inner[close + 3..];
    let params = if params.is_empty() {
        Vec::new()
    } else {
        split_type_list(params)
            .into_iter()
            .map(|ty| resolve_type(p, ty))
            .collect::<Result<Vec<_>, _>>()?
    };
    let ret = resolve_type(p, ret)?;
    Ok(Type::CFn(params, Box::new(ret)))
}

fn split_type_list(source: &str) -> Vec<&str> {
    let mut depth = 0i32;
    let mut start = 0usize;
    let mut values = Vec::new();
    for (index, byte) in source.bytes().enumerate() {
        match byte {
            b'[' | b'(' => depth += 1,
            b']' | b')' => depth -= 1,
            b',' if depth == 0 => {
                values.push(&source[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    values.push(&source[start..]);
    values
}

pub struct Checker<'a> {
    p: &'a Program,
    type_ids: HashMap<String, usize>,
    sigs: HashMap<String, (Vec<Type>, Vec<bool>, Type)>,
    expr_types: RefCell<Vec<Option<Type>>>,
}

type Scope = HashMap<String, (Type, bool)>; // type, is-mutable

fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "print"
            | "puti"
            | "putf"
            | "putb"
            | "puts"
            | "putsp"
            | "putnl"
            | "nargs"
            | "arg"
            | "chr"
            | "concat"
            | "read_file"
            | "write_file"
            | "sqrt"
            | "sin"
            | "cos"
            | "acos"
            | "abs"
            | "floor"
            | "min"
            | "max"
            | "pow"
            | "atan2"
            | "float"
            | "f32"
            | "int"
            | "len"
            | "substr"
            | "arr"
    )
}

impl<'a> Checker<'a> {
    pub fn check_types(p: &'a Program) -> Result<Vec<Option<Type>>, String> {
        let mut type_ids = HashMap::new();
        for (i, t) in p.types.iter().enumerate() {
            type_ids.insert(t.name.clone(), i);
        }
        let mut c = Checker {
            p,
            type_ids,
            sigs: HashMap::new(),
            expr_types: RefCell::new(vec![None; p.exprs.len()]),
        };
        for (record_index, record) in p.types.iter().enumerate() {
            for (field, source) in &record.fields {
                match c.resolve(source)? {
                    Type::CSlice(_) => {
                        return Err(format!(
                            "record field `{}.{}` cannot store a borrowed c_slice",
                            record.name, field
                        ))
                    }
                    Type::CMutSlice(_) => {
                        return Err(format!(
                            "record field `{}.{}` cannot store a borrowed c_mut_slice",
                            record.name, field
                        ))
                    }
                    _ => {}
                }
            }
            if record.c_layout {
                c.validate_c_layout_record(record_index, &mut Vec::new())?;
            }
        }
        for e in &p.externs {
            let selfhost_bridge = matches!(
                e.name.as_str(),
                "lu_ffi_prepare"
                    | "lu_ffi_call_i"
                    | "lu_ffi_call_f"
                    | "lu_ffi_call_f32"
                    | "lu_ffi_call_str"
                    | "lu_ffi_call_record_i"
                    | "lu_ffi_call_record_f"
            );
            if e.name.starts_with("lu_") && !selfhost_bridge {
                return Err(format!(
                    "extern name `{}` uses reserved `lu_` prefix",
                    e.name
                ));
            }
            if is_builtin(&e.name) || p.fns.iter().any(|f| f.name == e.name) {
                return Err(format!(
                    "extern name `{}` collides with an existing function",
                    e.name
                ));
            }
            if e.inouts.iter().any(|&inout| inout) {
                return Err(format!(
                    "extern `{}` cannot have `inout` parameters",
                    e.name
                ));
            }
            let params = e
                .params
                .iter()
                .map(|(_, t)| c.resolve(t))
                .collect::<Result<Vec<_>, _>>()?;
            let ret = c.resolve(&e.ret)?;
            c.validate_ffi_signature(&e.name, &params, &ret, false)?;
            if c.sigs
                .insert(e.name.clone(), (params, e.inouts.clone(), ret))
                .is_some()
            {
                return Err(format!("duplicate extern `{}`", e.name));
            }
        }
        for f in &p.fns {
            let params: Result<Vec<Type>, String> =
                f.params.iter().map(|(_, t)| c.resolve(t)).collect();
            let params = params?;
            let ret = c.resolve(&f.ret)?;
            for param in &params {
                if let Type::CSlice(element) | Type::CMutSlice(element) = param {
                    if !matches!(element.as_ref(), Type::I64 | Type::F64) {
                        return Err(format!(
                            "function `{}` has unsupported borrowed C slice element {}; allowed elements are i64 and f64",
                            f.name,
                            c.name(element)
                        ));
                    }
                }
            }
            match ret {
                Type::CSlice(_) => {
                    return Err(format!(
                        "function `{}` cannot return a borrowed c_slice",
                        f.name
                    ))
                }
                Type::CMutSlice(_) => {
                    return Err(format!(
                        "function `{}` cannot return a borrowed c_mut_slice",
                        f.name
                    ))
                }
                _ => {}
            }
            if f.exported {
                if f.has_inout() {
                    return Err(format!(
                        "export `{}` cannot have `inout` parameters",
                        f.name
                    ));
                }
                c.validate_ffi_signature(&f.name, &params, &ret, true)?;
            }
            if c.sigs
                .insert(f.name.clone(), (params, f.inouts.clone(), ret))
                .is_some()
            {
                return Err(format!("duplicate function `{}`", f.name));
            }
        }
        for prop in &p.props {
            if prop.has_inout() {
                return Err(format!(
                    "property `{}` cannot take `inout` parameters",
                    prop.name
                ));
            }
        }
        for f in &p.fns {
            c.check_fn(f)?;
        }
        for prop in &p.props {
            let ret = c.check_fn_body(prop, &Type::Bool)?;
            if ret != Type::Bool {
                return Err(format!(
                    "property `{}` body must be bool, got {}",
                    prop.name,
                    c.name(&ret)
                ));
            }
        }
        if let Some(main) = &p.main {
            let mut scopes: Vec<Scope> = vec![HashMap::new()];
            c.check_block(main, &mut scopes, &Type::Unit)?;
        }
        Ok(c.expr_types.into_inner())
    }

    fn resolve(&self, s: &str) -> Result<Type, String> {
        match s {
            "f32" => Ok(Type::F32),
            "f64" => Ok(Type::F64),
            "i64" | "i32" => Ok(Type::I64),
            "bool" => Ok(Type::Bool),
            "str" => Ok(Type::Str),
            "()" => Ok(Type::Unit),
            _ => {
                if let Some(inner) = s.strip_prefix('[').and_then(|x| x.strip_suffix(']')) {
                    Ok(Type::Arr(Box::new(self.resolve(inner)?)))
                } else if let Some(inner) = s
                    .strip_prefix("c_slice[")
                    .and_then(|inner| inner.strip_suffix(']'))
                {
                    Ok(Type::CSlice(Box::new(self.resolve(inner)?)))
                } else if let Some(inner) = s
                    .strip_prefix("c_mut_slice[")
                    .and_then(|inner| inner.strip_suffix(']'))
                {
                    Ok(Type::CMutSlice(Box::new(self.resolve(inner)?)))
                } else if let Some(inner) = s
                    .strip_prefix("c_ptr[")
                    .and_then(|inner| inner.strip_suffix(']'))
                {
                    Ok(Type::CPtr(Box::new(self.resolve(inner)?)))
                } else if let Some(inner) = s
                    .strip_prefix("c_fn[(")
                    .and_then(|inner| inner.strip_suffix(']'))
                {
                    let close = inner
                        .find(")->")
                        .ok_or_else(|| format!("invalid callback type `{s}`"))?;
                    let params = if close == 0 {
                        Vec::new()
                    } else {
                        split_type_list(&inner[..close])
                            .into_iter()
                            .map(|ty| self.resolve(ty))
                            .collect::<Result<Vec<_>, _>>()?
                    };
                    Ok(Type::CFn(
                        params,
                        Box::new(self.resolve(&inner[close + 3..])?),
                    ))
                } else if let Some(ei) = self.p.enums.iter().position(|e| e.name == s) {
                    Ok(Type::Enum(ei))
                } else {
                    self.type_ids
                        .get(s)
                        .map(|&i| Type::Rec(i))
                        .ok_or(format!("unknown type `{}`", s))
                }
            }
        }
    }

    fn validate_ffi_signature(
        &self,
        name: &str,
        params: &[Type],
        ret: &Type,
        exported: bool,
    ) -> Result<(), String> {
        for param in params {
            self.validate_callback_types(name, param)?;
            self.ffi_param_classes(param).map_err(|why| {
                format!(
                    "FFI signature `{}` has unsupported parameter: {}",
                    name, why
                )
            })?;
        }
        self.validate_callback_types(name, ret)?;
        if exported
            && matches!(
                ret,
                Type::Arr(element) if matches!(element.as_ref(), Type::I64 | Type::F64)
            )
        {
            // Export-only owned result handles. The generated C wrapper owns
            // the returned allocation and never exposes compiler array layout.
        } else if matches!(ret, Type::Rec(record) if self.ffi_record_classes(*record).is_ok()) {
            // The same homogeneous one/two-register aggregate subset is
            // portable in both argument and result position.
        } else if !matches!(
            ret,
            Type::Unit
                | Type::I64
                | Type::F32
                | Type::F64
                | Type::Bool
                | Type::Enum(_)
                | Type::CPtr(_)
                | Type::CFn(_, _)
                | Type::Str
        ) {
            return Err(format!(
                "FFI signature `{}` has unsupported return type; returns are limited to (), scalars, enums, c_ptr[T], c_fn[(...) -> T], str, supported @c_layout records, and exported [i64]/[f64] owned results",
                name
            ));
        }
        let (mut ints, floats) = params.iter().try_fold((0usize, 0usize), |acc, ty| {
            let classes = self.ffi_param_classes(ty)?;
            Ok::<_, String>((acc.0 + classes.0, acc.1 + classes.1))
        })?;
        if *ret == Type::Str {
            ints += 1;
        }
        if ints > 6 || floats > 8 {
            return Err(format!(
                "FFI signature `{}` needs {} integer-class and {} float-class argument registers; maximum is 6 and 8",
                name, ints, floats
            ));
        }
        Ok(())
    }

    fn validate_callback_types(&self, owner: &str, ty: &Type) -> Result<(), String> {
        if let Type::CFn(params, ret) = ty {
            self.validate_ffi_signature(
                &format!("callback nested in `{owner}`"),
                params,
                ret,
                false,
            )?;
        }
        Ok(())
    }

    fn ffi_param_classes(&self, ty: &Type) -> Result<(usize, usize), String> {
        match ty {
            Type::I64 | Type::Bool | Type::Enum(_) | Type::CPtr(_) | Type::CFn(_, _) => Ok((1, 0)),
            Type::F32 | Type::F64 => Ok((0, 1)),
            Type::Str => Ok((2, 0)),
            Type::CSlice(element) if matches!(element.as_ref(), Type::I64 | Type::F64) =>
            {
                Ok((2, 0))
            }
            Type::CMutSlice(element) if matches!(element.as_ref(), Type::I64 | Type::F64) => {
                Ok((2, 0))
            }
            Type::Arr(element) if matches!(element.as_ref(), Type::I64 | Type::F64) => Ok((2, 0)),
            Type::Rec(record) => self.ffi_record_classes(*record),
            _ => Err(
                "allowed boundary types are i64, f32, f64, bool, enums, c_ptr[T], c_slice[i64|f64], c_mut_slice[i64|f64], str, [i64], [f64], and supported @c_layout records"
                    .into(),
            ),
        }
    }

    fn ffi_record_classes(&self, record_index: usize) -> Result<(usize, usize), String> {
        let record = &self.p.types[record_index];
        if !record.c_layout {
            return Err(format!(
                "record `{}` is compiler-layout; add @c_layout for boundary use",
                record.name
            ));
        }
        fn collect(
            checker: &Checker<'_>,
            record_index: usize,
            classes: &mut Vec<bool>,
        ) -> Result<(), String> {
            for (_, source) in &checker.p.types[record_index].fields {
                match checker.resolve(source)? {
                    Type::I64 | Type::Bool | Type::CPtr(_) | Type::CFn(_, _) => {
                        classes.push(false)
                    }
                    Type::F64 => classes.push(true),
                    _ => {
                        return Err(
                            "direct @c_layout parameters require flat records containing only 64-bit integer/pointer fields or only f64 fields"
                                .into(),
                        )
                    }
                }
            }
            Ok(())
        }
        let mut classes = Vec::new();
        collect(self, record_index, &mut classes)?;
        if classes.is_empty() || classes.len() > 2 {
            return Err(format!(
                "direct @c_layout parameter `{}` must flatten to one or two 64-bit fields",
                record.name
            ));
        }
        if classes.iter().any(|class| *class != classes[0]) {
            return Err(format!(
                "direct @c_layout parameter `{}` cannot mix integer/pointer and f64 fields",
                record.name
            ));
        }
        Ok(if classes[0] {
            (0, classes.len())
        } else {
            (classes.len(), 0)
        })
    }

    fn validate_c_layout_record(
        &self,
        record_index: usize,
        stack: &mut Vec<usize>,
    ) -> Result<(), String> {
        let record = &self.p.types[record_index];
        if record.fields.is_empty() {
            return Err(format!(
                "`@c_layout` type `{}` must contain at least one field",
                record.name
            ));
        }
        if stack.contains(&record_index) {
            return Err(format!(
                "`@c_layout` type `{}` contains a by-value layout cycle",
                record.name
            ));
        }
        stack.push(record_index);
        let mut field_names = HashMap::new();
        for (field_name, field_source) in &record.fields {
            if field_names.insert(field_name, ()).is_some() {
                return Err(format!(
                    "`@c_layout` type `{}` repeats field `{}`",
                    record.name, field_name
                ));
            }
            let field = self.resolve(field_source)?;
            match field {
                Type::I64 | Type::F32 | Type::F64 | Type::Bool | Type::CPtr(_) => {}
                Type::Rec(nested) if self.p.types[nested].c_layout => {
                    self.validate_c_layout_record(nested, stack)?;
                }
                _ => {
                    return Err(format!(
                        "`@c_layout` field `{}.{}` has unsupported type {}; allowed fields are i64, f32, f64, bool, c_ptr[T], and nested @c_layout records",
                        record.name,
                        field_name,
                        self.name(&field)
                    ))
                }
            }
        }
        stack.pop();
        Ok(())
    }

    fn name(&self, t: &Type) -> String {
        match t {
            Type::I64 => "i64".into(),
            Type::F32 => "f32".into(),
            Type::F64 => "f64".into(),
            Type::Bool => "bool".into(),
            Type::Str => "str".into(),
            Type::Unit => "()".into(),
            Type::Arr(t) => format!("[{}]", self.name(t)),
            Type::CSlice(t) => format!("c_slice[{}]", self.name(t)),
            Type::CMutSlice(t) => format!("c_mut_slice[{}]", self.name(t)),
            Type::CPtr(t) => format!("c_ptr[{}]", self.name(t)),
            Type::CFn(params, ret) => format!(
                "c_fn[({})->{}]",
                params
                    .iter()
                    .map(|ty| self.name(ty))
                    .collect::<Vec<_>>()
                    .join(","),
                self.name(ret)
            ),
            Type::Rec(i) => self.p.types[*i].name.clone(),
            Type::Enum(i) => self.p.enums[*i].name.clone(),
        }
    }

    fn compat(expected: &Type, actual: &Type) -> bool {
        expected == actual
            || (matches!(expected, Type::F32 | Type::F64) && *actual == Type::I64)
            || (*expected == Type::F32 && *actual == Type::F64)
            || (*expected == Type::F64 && *actual == Type::F32)
            || matches!(
                (expected, actual),
                (Type::CSlice(expected), Type::Arr(actual)) if expected == actual
            )
            || matches!(
                (expected, actual),
                (Type::CMutSlice(expected), Type::Arr(actual)) if expected == actual
            )
    }

    fn check_fn(&self, f: &FnDecl) -> Result<(), String> {
        let ret = self.resolve(&f.ret)?;
        let last = self.check_fn_body(f, &ret)?;
        if ret != Type::Unit && !Self::compat(&ret, &last) {
            return Err(format!(
                "function `{}` declares return type {} but its body produces {}",
                f.name,
                self.name(&ret),
                self.name(&last)
            ));
        }
        Ok(())
    }

    fn check_fn_body(&self, f: &FnDecl, ret: &Type) -> Result<Type, String> {
        let mut scope = HashMap::new();
        for (i, (n, t)) in f.params.iter().enumerate() {
            let ty = self.resolve(t)?;
            let mutable = f.inouts.get(i).copied().unwrap_or(false)
                || (f.exported && matches!(&ty, Type::Arr(_)))
                || matches!(&ty, Type::CMutSlice(_));
            scope.insert(n.clone(), (ty, mutable));
        }
        let mut scopes = vec![scope];
        self.check_block(&f.body, &mut scopes, ret)
    }

    fn check_block(
        &self,
        stmts: &[StmtId],
        scopes: &mut Vec<Scope>,
        ret: &Type,
    ) -> Result<Type, String> {
        scopes.push(HashMap::new());
        let mut last = Type::Unit;
        for &sid in stmts {
            last = self.check_stmt(sid, scopes, ret)?;
        }
        scopes.pop();
        Ok(last)
    }

    fn lookup(&self, scopes: &[Scope], n: &str) -> Option<(Type, bool)> {
        scopes.iter().rev().find_map(|s| s.get(n).cloned())
    }

    fn check_stmt(&self, sid: StmtId, scopes: &mut Vec<Scope>, ret: &Type) -> Result<Type, String> {
        match self.p.stmt(sid) {
            Stmt::Let(n, e) | Stmt::Var(n, e) => {
                let t = self.check_expr(*e, scopes)?;
                if t == Type::Unit {
                    return Err(format!("cannot bind `{}` to a unit value", n));
                }
                let mutable = matches!(self.p.stmt(sid), Stmt::Var(_, _));
                scopes.last_mut().unwrap().insert(n.clone(), (t, mutable));
                Ok(Type::Unit)
            }
            Stmt::Assign(target, e) => {
                let vt = self.check_expr(*e, scopes)?;
                match self.p.expr(*target) {
                    Expr::Ident(n) => {
                        let (t, mutable) = self
                            .lookup(scopes, n)
                            .ok_or(format!("unknown variable `{}`", n))?;
                        if !mutable {
                            return Err(format!(
                                "cannot assign to immutable binding `{}` (use `var`)",
                                n
                            ));
                        }
                        if !Self::compat(&t, &vt) {
                            return Err(format!(
                                "cannot assign {} to `{}: {}`",
                                self.name(&vt),
                                n,
                                self.name(&t)
                            ));
                        }
                    }
                    Expr::Index(a, i) => {
                        if self.check_expr(*i, scopes)? != Type::I64 {
                            return Err("array index must be i64".into());
                        }
                        match self.check_expr(*a, scopes)? {
                            Type::Arr(el) => {
                                if !Self::compat(&el, &vt) {
                                    return Err(format!(
                                        "cannot store {} in [{}]",
                                        self.name(&vt),
                                        self.name(&el)
                                    ));
                                }
                            }
                            Type::CSlice(_) => {
                                return Err("borrowed c_slice values are read-only".into())
                            }
                            Type::CMutSlice(el) => {
                                if !Self::compat(&el, &vt) {
                                    return Err(format!(
                                        "cannot store {} in c_mut_slice[{}]",
                                        self.name(&vt),
                                        self.name(&el)
                                    ));
                                }
                            }
                            t => return Err(format!("cannot index into {}", self.name(&t))),
                        }
                        let mut current = *a;
                        let root =
                            loop {
                                match self.p.expr(current) {
                                    Expr::Ident(name) => break name,
                                    Expr::Field(base, _) => current = *base,
                                    _ => return Err(
                                        "indexed assignment root must be a variable or its field"
                                            .into(),
                                    ),
                                }
                            };
                        let (_, mutable) = self
                            .lookup(scopes, root)
                            .ok_or(format!("unknown variable `{}`", root))?;
                        if !mutable {
                            return Err(format!(
                                "cannot write through immutable binding `{}`",
                                root
                            ));
                        }
                    }
                    Expr::Field(_, _) => {
                        // x.f (possibly nested) = v — root must be a mutable var
                        let mut path = Vec::new();
                        let mut cur = *target;
                        let root = loop {
                            match self.p.expr(cur) {
                                Expr::Field(b, f) => {
                                    path.push(f.clone());
                                    cur = *b;
                                }
                                Expr::Ident(n) => break n.clone(),
                                _ => return Err("field assignment root must be a variable".into()),
                            }
                        };
                        path.reverse();
                        let (mut t, mutable) = self
                            .lookup(scopes, &root)
                            .ok_or(format!("unknown variable `{}`", root))?;
                        if !mutable {
                            return Err(format!(
                                "cannot assign through immutable binding `{}`",
                                root
                            ));
                        }
                        for f in &path {
                            t = match t {
                                Type::Rec(ti) => self.p.types[ti]
                                    .fields
                                    .iter()
                                    .find(|(n, _)| n == f)
                                    .map(|(_, ft)| self.resolve(ft))
                                    .transpose()?
                                    .ok_or(format!(
                                        "type `{}` has no field `{}`",
                                        self.p.types[ti].name, f
                                    ))?,
                                t => {
                                    return Err(format!(
                                        "cannot access field `{}` on {}",
                                        f,
                                        self.name(&t)
                                    ))
                                }
                            };
                        }
                        if !Self::compat(&t, &vt) {
                            return Err(format!(
                                "cannot assign {} to field of type {}",
                                self.name(&vt),
                                self.name(&t)
                            ));
                        }
                    }
                    _ => return Err("invalid assignment target".into()),
                }
                Ok(Type::Unit)
            }
            Stmt::If(c, then, els) => {
                if self.check_expr(*c, scopes)? != Type::Bool {
                    return Err("`if` condition must be bool".into());
                }
                self.check_block(then, scopes, ret)?;
                self.check_block(els, scopes, ret)?;
                Ok(Type::Unit)
            }
            Stmt::While(c, body) => {
                if self.check_expr(*c, scopes)? != Type::Bool {
                    return Err("`while` condition must be bool".into());
                }
                self.check_block(body, scopes, ret)?;
                Ok(Type::Unit)
            }
            Stmt::For(v, lo, hi, body) => {
                if self.check_expr(*lo, scopes)? != Type::I64
                    || self.check_expr(*hi, scopes)? != Type::I64
                {
                    return Err("`for` bounds must be i64".into());
                }
                scopes.push(HashMap::new());
                scopes
                    .last_mut()
                    .unwrap()
                    .insert(v.clone(), (Type::I64, false));
                self.check_block(body, scopes, ret)?;
                scopes.pop();
                Ok(Type::Unit)
            }
            Stmt::Return(e) => {
                let t = match e {
                    Some(e) => self.check_expr(*e, scopes)?,
                    None => Type::Unit,
                };
                if !Self::compat(ret, &t) {
                    return Err(format!(
                        "return type mismatch: expected {}, got {}",
                        self.name(ret),
                        self.name(&t)
                    ));
                }
                Ok(ret.clone())
            }
            Stmt::Expr(e) => self.check_expr(*e, scopes),
        }
    }

    fn numeric(&self, t: &Type) -> bool {
        matches!(t, Type::I64 | Type::F32 | Type::F64)
    }

    fn check_expr(&self, eid: ExprId, scopes: &mut Vec<Scope>) -> Result<Type, String> {
        let result = match self.p.expr(eid) {
            Expr::Int(_) => Ok(Type::I64),
            Expr::Float(_) => Ok(Type::F64),
            Expr::Str(_) => Ok(Type::Str),
            Expr::Bool(_) => Ok(Type::Bool),
            Expr::Ident(n) => self
                .lookup(scopes, n)
                .map(|(t, _)| t)
                .ok_or(format!("unknown variable `{}`", n)),
            Expr::Un(op, e) => {
                let t = self.check_expr(*e, scopes)?;
                match op.as_str() {
                    "-" if self.numeric(&t) => Ok(t),
                    "not" if t == Type::Bool => Ok(Type::Bool),
                    op => Err(format!("cannot apply `{}` to {}", op, self.name(&t))),
                }
            }
            Expr::Bin(op, l, r) => {
                let lt = self.check_expr(*l, scopes)?;
                let rt = self.check_expr(*r, scopes)?;
                if let Some(fname) = self.p.infix_ops.get(op) {
                    let (params, _, ret) =
                        self.sigs.get(fname).ok_or(format!("unknown op `{}`", op))?;
                    if !Self::compat(&params[0], &lt) || !Self::compat(&params[1], &rt) {
                        return Err(format!(
                            "operator `{}` expects ({}, {}), got ({}, {})",
                            op,
                            self.name(&params[0]),
                            self.name(&params[1]),
                            self.name(&lt),
                            self.name(&rt)
                        ));
                    }
                    let ty = ret.clone();
                    self.expr_types.borrow_mut()[eid as usize] = Some(ty.clone());
                    return Ok(ty);
                }
                match op.as_str() {
                    "and" | "or" => {
                        if lt == Type::Bool && rt == Type::Bool {
                            Ok(Type::Bool)
                        } else {
                            Err(format!("`{}` needs bool operands", op))
                        }
                    }
                    "+" | "-" | "*" | "/" | "%" => {
                        if !self.numeric(&lt) || !self.numeric(&rt) {
                            return Err(format!(
                                "cannot apply `{}` to {} and {}",
                                op,
                                self.name(&lt),
                                self.name(&rt)
                            ));
                        }
                        Ok(if lt == Type::F64 || rt == Type::F64 {
                            Type::F64
                        } else if lt == Type::F32 || rt == Type::F32 {
                            Type::F32
                        } else {
                            Type::I64
                        })
                    }
                    "==" | "!=" => {
                        if lt == rt || (self.numeric(&lt) && self.numeric(&rt)) {
                            Ok(Type::Bool)
                        } else {
                            Err(format!(
                                "cannot compare {} with {}",
                                self.name(&lt),
                                self.name(&rt)
                            ))
                        }
                    }
                    "<" | "<=" | ">" | ">=" | "~=" | "\u{2248}" => {
                        if self.numeric(&lt) && self.numeric(&rt) {
                            Ok(Type::Bool)
                        } else {
                            Err(format!(
                                "cannot compare {} with {}",
                                self.name(&lt),
                                self.name(&rt)
                            ))
                        }
                    }
                    op => Err(format!("unknown operator `{}`", op)),
                }
            }
            Expr::Circum(open, e) => {
                let t = self.check_expr(*e, scopes)?;
                let (close, fname) = &self.p.circum_ops[open];
                let (params, _, ret) = self
                    .sigs
                    .get(fname)
                    .ok_or(format!("unknown op `{}…{}`", open, close))?;
                if !Self::compat(&params[0], &t) {
                    return Err(format!(
                        "operator `{}…{}` expects {}, got {}",
                        open,
                        close,
                        self.name(&params[0]),
                        self.name(&t)
                    ));
                }
                Ok(ret.clone())
            }
            Expr::Field(e, f) => match self.check_expr(*e, scopes)? {
                Type::Rec(ti) => self.p.types[ti]
                    .fields
                    .iter()
                    .find(|(n, _)| n == f)
                    .map(|(_, t)| self.resolve(t))
                    .transpose()?
                    .ok_or(format!(
                        "type `{}` has no field `{}`",
                        self.p.types[ti].name, f
                    )),
                t => Err(format!("cannot access field `{}` on {}", f, self.name(&t))),
            },
            Expr::Index(a, i) => {
                if self.check_expr(*i, scopes)? != Type::I64 {
                    return Err("array index must be i64".into());
                }
                match self.check_expr(*a, scopes)? {
                    Type::Arr(el) | Type::CSlice(el) | Type::CMutSlice(el) => Ok(*el),
                    Type::Str => Ok(Type::I64), // byte access
                    t => Err(format!("cannot index into {}", self.name(&t))),
                }
            }
            Expr::Array(items) => {
                let mut it = items.iter();
                let first = match it.next() {
                    Some(&e) => self.check_expr(e, scopes)?,
                    None => return Err("cannot infer element type of empty array literal".into()),
                };
                for &e in it {
                    let t = self.check_expr(e, scopes)?;
                    if t != first {
                        return Err(format!(
                            "mixed array literal: {} and {}",
                            self.name(&first),
                            self.name(&t)
                        ));
                    }
                }
                Ok(Type::Arr(Box::new(first)))
            }
            Expr::Record(name, inits) => {
                let ti = *self
                    .type_ids
                    .get(name)
                    .ok_or(format!("unknown type `{}`", name))?;
                let decl = &self.p.types[ti];
                if inits.len() != decl.fields.len() {
                    return Err(format!(
                        "`{}` has {} fields, literal provides {}",
                        name,
                        decl.fields.len(),
                        inits.len()
                    ));
                }
                let mut initialized = vec![false; decl.fields.len()];
                for (pos, (fname, e)) in inits.iter().enumerate() {
                    let idx = match fname {
                        Some(f) => decl
                            .fields
                            .iter()
                            .position(|(n, _)| n == f)
                            .ok_or(format!("type `{}` has no field `{}`", name, f))?,
                        None => pos,
                    };
                    if initialized[idx] {
                        return Err(format!(
                            "field `{}` of `{}` is initialized more than once",
                            decl.fields[idx].0, name
                        ));
                    }
                    initialized[idx] = true;
                    let expect = self.resolve(&decl.fields[idx].1)?;
                    let got = self.check_expr(*e, scopes)?;
                    if !Self::compat(&expect, &got) {
                        return Err(format!(
                            "field `{}` of `{}` is {}, got {}",
                            decl.fields[idx].0,
                            name,
                            self.name(&expect),
                            self.name(&got)
                        ));
                    }
                }
                if let Some((idx, _)) = initialized.iter().enumerate().find(|(_, set)| !**set) {
                    return Err(format!(
                        "record literal for `{}` is missing field `{}`",
                        name, decl.fields[idx].0
                    ));
                }
                Ok(Type::Rec(ti))
            }
            Expr::EnumVal(en, vn) => {
                let (ei, _) = self
                    .p
                    .enum_tag(en, vn)
                    .ok_or(format!("unknown enum value `{}.{}`", en, vn))?;
                Ok(Type::Enum(ei))
            }
            Expr::Sum { var, lo, hi, body } => {
                if self.check_expr(*lo, scopes)? != Type::I64
                    || self.check_expr(*hi, scopes)? != Type::I64
                {
                    return Err("`sum` bounds must be i64".into());
                }
                scopes.push(HashMap::new());
                scopes
                    .last_mut()
                    .unwrap()
                    .insert(var.clone(), (Type::I64, false));
                let t = self.check_expr(*body, scopes)?;
                scopes.pop();
                if !self.numeric(&t) {
                    return Err(format!("`sum` body must be numeric, got {}", self.name(&t)));
                }
                Ok(t)
            }
            Expr::Call(name, args) => {
                let ats: Result<Vec<Type>, String> =
                    args.iter().map(|&e| self.check_expr(e, scopes)).collect();
                let ats = ats?;
                let need = |n: usize| -> Result<(), String> {
                    if ats.len() == n {
                        Ok(())
                    } else {
                        Err(format!("`{}` expects {} args, got {}", name, n, ats.len()))
                    }
                };
                match name.as_str() {
                    "print" => Ok(Type::Unit),
                    "putsp" | "putnl" => {
                        need(0)?;
                        Ok(Type::Unit)
                    }
                    "puti" => {
                        need(1)?;
                        if ats[0] != Type::I64 {
                            return Err("`puti` expects an i64".into());
                        }
                        Ok(Type::Unit)
                    }
                    "putf" => {
                        need(1)?;
                        if !matches!(ats[0], Type::F32 | Type::F64) {
                            return Err("`putf` expects a float".into());
                        }
                        Ok(Type::Unit)
                    }
                    "putb" => {
                        need(1)?;
                        if ats[0] != Type::Bool {
                            return Err("`putb` expects a bool".into());
                        }
                        Ok(Type::Unit)
                    }
                    "puts" => {
                        need(1)?;
                        if ats[0] != Type::Str {
                            return Err("`puts` expects a str".into());
                        }
                        Ok(Type::Unit)
                    }
                    "nargs" => {
                        need(0)?;
                        Ok(Type::I64)
                    }
                    "arg" => {
                        need(1)?;
                        if ats[0] != Type::I64 {
                            return Err("`arg` expects an i64".into());
                        }
                        Ok(Type::Str)
                    }
                    "chr" => {
                        need(1)?;
                        if ats[0] != Type::I64 {
                            return Err("`chr` expects an i64".into());
                        }
                        Ok(Type::Str)
                    }
                    "concat" => {
                        need(2)?;
                        if ats[0] != Type::Str || ats[1] != Type::Str {
                            return Err("`concat` expects two strs".into());
                        }
                        Ok(Type::Str)
                    }
                    "read_file" => {
                        need(1)?;
                        if ats[0] != Type::Str {
                            return Err("`read_file` expects a str".into());
                        }
                        Ok(Type::Str)
                    }
                    "write_file" => {
                        need(2)?;
                        if ats[0] != Type::Str || ats[1] != Type::Str {
                            return Err("`write_file` expects (str, str)".into());
                        }
                        Ok(Type::Unit)
                    }
                    "sqrt" | "sin" | "cos" | "acos" | "abs" | "floor" => {
                        need(1)?;
                        if !self.numeric(&ats[0]) {
                            return Err(format!("`{}` needs a numeric arg", name));
                        }
                        Ok(if ats[0] == Type::F32 {
                            Type::F32
                        } else {
                            Type::F64
                        })
                    }
                    "min" | "max" | "pow" | "atan2" => {
                        need(2)?;
                        if !self.numeric(&ats[0]) || !self.numeric(&ats[1]) {
                            return Err(format!("`{}` needs numeric args", name));
                        }
                        Ok(if ats.iter().all(|t| *t == Type::F32) {
                            Type::F32
                        } else {
                            Type::F64
                        })
                    }
                    "float" => {
                        need(1)?;
                        Ok(Type::F64)
                    }
                    "f32" => {
                        need(1)?;
                        if !self.numeric(&ats[0]) {
                            return Err("`f32` expects a numeric argument".into());
                        }
                        Ok(Type::F32)
                    }
                    "int" => {
                        need(1)?;
                        Ok(Type::I64)
                    }
                    "len" => {
                        need(1)?;
                        match &ats[0] {
                            Type::Arr(_) | Type::CSlice(_) | Type::CMutSlice(_) | Type::Str => {
                                Ok(Type::I64)
                            }
                            t => Err(format!(
                                "`len` expects an array or str, got {}",
                                self.name(t)
                            )),
                        }
                    }
                    "substr" => {
                        need(3)?;
                        if ats[0] != Type::Str || ats[1] != Type::I64 || ats[2] != Type::I64 {
                            return Err("`substr` expects (str, i64, i64)".into());
                        }
                        Ok(Type::Str)
                    }
                    "arr" => {
                        need(2)?;
                        if ats[0] != Type::I64 {
                            return Err("`arr` length must be i64".into());
                        }
                        Ok(Type::Arr(Box::new(ats[1].clone())))
                    }
                    _ => {
                        let (params, inouts, ret) = self
                            .sigs
                            .get(name)
                            .ok_or(format!("unknown function `{}`", name))?;
                        let extern_decl = self
                            .p
                            .externs
                            .iter()
                            .find(|declaration| declaration.name == *name);
                        if params.len() != ats.len() {
                            return Err(format!(
                                "`{}` expects {} args, got {}",
                                name,
                                params.len(),
                                ats.len()
                            ));
                        }
                        for (i, (p, a)) in params.iter().zip(ats.iter()).enumerate() {
                            if inouts[i] {
                                // inout: the arg must be a mutable variable of the
                                // exact type (copy-out has nowhere to widen to)
                                match self.p.expr(args[i]) {
                                    Expr::Ident(n) => {
                                        let (t, mutable) = self
                                            .lookup(scopes, n)
                                            .ok_or(format!("unknown variable `{}`", n))?;
                                        if !mutable {
                                            return Err(format!(
                                                "inout arg {} of `{}` needs a `var`, `{}` is immutable",
                                                i + 1, name, n
                                            ));
                                        }
                                        if t != *p {
                                            return Err(format!(
                                                "inout arg {} of `{}` must be exactly {}, got {}",
                                                i + 1,
                                                name,
                                                self.name(p),
                                                self.name(&t)
                                            ));
                                        }
                                        // no aliasing: no other argument may pass or
                                        // mutate the same variable — copy-in/copy-out
                                        // would silently drop one of the writes
                                        for (j, &aj) in args.iter().enumerate() {
                                            if j == i {
                                                continue;
                                            }
                                            let dup = inouts[j]
                                                && matches!(self.p.expr(aj), Expr::Ident(m) if m == n);
                                            if dup || writes_var(self.p, aj, n) {
                                                return Err(format!(
                                                    "inout arg {} of `{}` aliases `{}` in arg {}",
                                                    i + 1,
                                                    name,
                                                    n,
                                                    j + 1
                                                ));
                                            }
                                        }
                                    }
                                    _ => {
                                        return Err(format!(
                                            "inout arg {} of `{}` must be a variable",
                                            i + 1,
                                            name
                                        ))
                                    }
                                }
                            } else if matches!(p, Type::CMutSlice(_)) {
                                match self.p.expr(args[i]) {
                                    Expr::Ident(n) => {
                                        let (actual, mutable) = self
                                            .lookup(scopes, n)
                                            .ok_or(format!("unknown variable `{}`", n))?;
                                        if !mutable {
                                            return Err(format!(
                                                "c_mut_slice arg {} of `{}` needs a `var`, `{}` is immutable",
                                                i + 1,
                                                name,
                                                n
                                            ));
                                        }
                                        if !Self::compat(p, &actual) {
                                            return Err(format!(
                                                "c_mut_slice arg {} of `{}` expects {}, got {}",
                                                i + 1,
                                                name,
                                                self.name(p),
                                                self.name(&actual)
                                            ));
                                        }
                                        for (j, &other) in args.iter().enumerate() {
                                            if j == i {
                                                continue;
                                            }
                                            let direct = matches!(
                                                self.p.expr(other),
                                                Expr::Ident(other_name) if other_name == n
                                            );
                                            if direct || writes_var(self.p, other, n) {
                                                return Err(format!(
                                                    "c_mut_slice arg {} of `{}` aliases `{}` in arg {}",
                                                    i + 1,
                                                    name,
                                                    n,
                                                    j + 1
                                                ));
                                            }
                                        }
                                    }
                                    _ => {
                                        return Err(format!(
                                            "c_mut_slice arg {} of `{}` must be a variable",
                                            i + 1,
                                            name
                                        ))
                                    }
                                }
                            } else if extern_decl.is_some() && matches!(p, Type::Arr(_)) {
                                match self.p.expr(args[i]) {
                                    Expr::Ident(variable) => {
                                        let (actual, mutable) = self
                                            .lookup(scopes, variable)
                                            .ok_or(format!("unknown variable `{}`", variable))?;
                                        if !mutable {
                                            return Err(format!(
                                                "array arg {} of extern `{}` needs a `var` for copy-out",
                                                i + 1,
                                                name
                                            ));
                                        }
                                        if actual != *p {
                                            return Err(format!(
                                                "array arg {} of extern `{}` must be exactly {}, got {}",
                                                i + 1,
                                                name,
                                                self.name(p),
                                                self.name(&actual)
                                            ));
                                        }
                                    }
                                    _ => {
                                        return Err(format!(
                                            "array arg {} of extern `{}` must be a variable",
                                            i + 1,
                                            name
                                        ))
                                    }
                                }
                            } else if !Self::compat(p, a) {
                                return Err(format!(
                                    "arg {} of `{}`: expected {}, got {}",
                                    i + 1,
                                    name,
                                    self.name(p),
                                    self.name(a)
                                ));
                            }
                        }
                        Ok(ret.clone())
                    }
                }
            }
        };
        if let Ok(ty) = &result {
            self.expr_types.borrow_mut()[eid as usize] = Some(ty.clone());
        }
        result
    }
}
