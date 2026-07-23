use crate::ast::{self, ExprId, FnDecl, StmtId};
use crate::check::{Checker, Type};
use std::collections::HashMap;

pub type ValueId = u32;
pub type LocalId = u32;
pub type BlockId = u32;
pub type FunctionId = u32;
pub type ExternId = u32;

#[derive(Clone, Debug, PartialEq)]
pub enum Constant {
    I64(i64),
    F32(f32),
    F64(f64),
    Bool(bool),
    Bytes(Vec<u8>),
    Unit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    ApproxEq,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Callee {
    Function(FunctionId),
    Extern(ExternId),
    Builtin(String),
}

#[derive(Clone, Debug)]
pub enum InstKind {
    Constant(Constant),
    Load(LocalId),
    Store {
        local: LocalId,
        value: ValueId,
        /// Whether this store creates a persistent language-level value copy.
        /// Inliner parameter/result shuttles are call-scoped borrows.
        retain_arrays: bool,
    },
    Unary {
        op: UnaryOp,
        value: ValueId,
    },
    Binary {
        op: BinaryOp,
        lhs: ValueId,
        rhs: ValueId,
    },
    Select {
        condition: ValueId,
        then_value: ValueId,
        else_value: ValueId,
    },
    Call {
        callee: Callee,
        args: Vec<ValueId>,
        /// Copy-out destinations, parallel to `args`. The checker guarantees
        /// these are mutable plain locals and do not alias another argument.
        inout: Vec<Option<LocalId>>,
    },
    Field {
        base: ValueId,
        record: usize,
        field: usize,
    },
    Index {
        base: ValueId,
        index: ValueId,
    },
    Array(Vec<ValueId>),
    Record {
        record: usize,
        fields: Vec<ValueId>,
    },
    Enum {
        enumeration: usize,
        tag: i64,
    },
    /// Array update with an explicit owning local. Compiled backends update the
    /// materialized storage; the interpreter copy-on-writes and stores to root.
    SetIndex {
        root: LocalId,
        path: Vec<usize>,
        base: ValueId,
        index: ValueId,
        value: ValueId,
    },
    SetField {
        root: LocalId,
        path: Vec<usize>,
        value: ValueId,
    },
}

#[derive(Clone, Debug)]
pub struct Inst {
    pub result: Option<ValueId>,
    pub ty: Type,
    pub kind: InstKind,
}

#[derive(Clone, Debug)]
pub enum Terminator {
    Jump(BlockId),
    Branch {
        condition: ValueId,
        then_block: BlockId,
        else_block: BlockId,
    },
    Return(ValueId),
    Unreachable,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub instructions: Vec<Inst>,
    pub terminator: Terminator,
}

#[derive(Clone, Debug)]
pub struct Local {
    pub name: String,
    pub ty: Type,
    pub mutable: bool,
}

#[derive(Clone, Debug)]
pub struct Function {
    pub name: String,
    pub exported: bool,
    pub params: Vec<LocalId>,
    pub inouts: Vec<bool>,
    pub ret: Type,
    pub locals: Vec<Local>,
    pub values: Vec<Type>,
    pub blocks: Vec<Block>,
    pub entry: BlockId,
}

#[derive(Clone, Debug)]
pub struct ExternDef {
    pub name: String,
    pub lib: Option<String>,
    pub params: Vec<(String, Type)>,
    pub ret: Type,
}

#[derive(Clone, Debug)]
pub struct RecordDef {
    pub name: String,
    pub fields: Vec<(String, Type)>,
    pub c_layout: bool,
}

#[derive(Clone, Debug)]
pub struct EnumDef {
    pub name: String,
    pub variants: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Property {
    pub function: FunctionId,
    pub name: String,
    pub params: Vec<(String, Type)>,
}

/// Backend-neutral, checked compiler IR. Execution semantics and optimization
/// inputs live in `functions` and `main`; source declarations supply names and
/// user-defined layout metadata.
pub struct LoweredProgram {
    source: ast::Program,
    pub records: Vec<RecordDef>,
    pub enums: Vec<EnumDef>,
    pub externs: Vec<ExternDef>,
    pub functions: Vec<Function>,
    pub properties: Vec<Property>,
    pub main: Option<Function>,
}

impl LoweredProgram {
    pub fn lower(source: ast::Program) -> Result<Self, String> {
        let expr_types = Checker::check_types(&source)?;
        let function_names: HashMap<_, _> = source
            .fns
            .iter()
            .enumerate()
            .map(|(i, f)| (f.name.clone(), i as FunctionId))
            .collect();
        let extern_names: HashMap<_, _> = source
            .externs
            .iter()
            .enumerate()
            .map(|(i, e)| (e.name.clone(), i as ExternId))
            .collect();
        let externs = source
            .externs
            .iter()
            .map(|e| {
                Ok(ExternDef {
                    name: e.name.clone(),
                    lib: e.lib.clone(),
                    params: e
                        .params
                        .iter()
                        .map(|(name, ty)| {
                            Ok((name.clone(), crate::check::resolve_type(&source, ty)?))
                        })
                        .collect::<Result<_, String>>()?,
                    ret: crate::check::resolve_type(&source, &e.ret)?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut functions = Vec::with_capacity(source.fns.len() + source.props.len());
        for f in &source.fns {
            functions.push(
                Builder::new(&source, &expr_types, &function_names, &extern_names, f)?
                    .finish(&f.body)?,
            );
        }
        let mut properties = Vec::with_capacity(source.props.len());
        for f in &source.props {
            let id = functions.len() as FunctionId;
            functions.push(
                Builder::new(&source, &expr_types, &function_names, &extern_names, f)?
                    .finish(&f.body)?,
            );
            properties.push(Property {
                function: id,
                name: f.name.clone(),
                params: f
                    .params
                    .iter()
                    .map(|(name, ty)| Ok((name.clone(), crate::check::resolve_type(&source, ty)?)))
                    .collect::<Result<_, String>>()?,
            });
        }
        let main = source
            .main
            .as_ref()
            .map(|body| {
                let decl = FnDecl {
                    name: "main".into(),
                    params: Vec::new(),
                    inouts: Vec::new(),
                    ret: "()".into(),
                    body: body.clone(),
                    exported: false,
                };
                Builder::new(&source, &expr_types, &function_names, &extern_names, &decl)?
                    .finish(body)
            })
            .transpose()?;
        let records = source
            .types
            .iter()
            .map(|record| {
                Ok(RecordDef {
                    name: record.name.clone(),
                    c_layout: record.c_layout,
                    fields: record
                        .fields
                        .iter()
                        .map(|(name, ty)| {
                            Ok((name.clone(), crate::check::resolve_type(&source, ty)?))
                        })
                        .collect::<Result<_, String>>()?,
                })
            })
            .collect::<Result<_, String>>()?;
        let enums = source
            .enums
            .iter()
            .map(|e| EnumDef {
                name: e.name.clone(),
                variants: e.variants.clone(),
            })
            .collect();
        let ir = Self {
            source,
            records,
            enums,
            externs,
            functions,
            properties,
            main,
        };
        ir.validate()?;
        Ok(ir)
    }

    /// Source declarations remain available for names and user-defined layout;
    /// executable semantics and optimization facts live in the lowered CFG.
    pub fn source(&self) -> &ast::Program {
        &self.source
    }

    pub fn validate(&self) -> Result<(), String> {
        for f in self.functions.iter().chain(self.main.iter()) {
            if f.entry as usize >= f.blocks.len() {
                return Err(format!("IR `{}` has invalid entry", f.name));
            }
            if f.params.len() != f.inouts.len()
                || f.locals.iter().any(|local| local.name.is_empty())
            {
                return Err(format!("IR `{}` has invalid local metadata", f.name));
            }
            for (&param, &inout) in f.params.iter().zip(&f.inouts) {
                let local = f
                    .locals
                    .get(param as usize)
                    .ok_or_else(|| format!("IR `{}` has invalid parameter", f.name))?;
                if inout && !local.mutable {
                    return Err(format!("IR `{}` has immutable inout parameter", f.name));
                }
            }
            for (bi, block) in f.blocks.iter().enumerate() {
                for inst in &block.instructions {
                    if let Some(v) = inst.result {
                        if f.values.get(v as usize) != Some(&inst.ty) {
                            return Err(format!(
                                "IR `{}` block {} has mistyped value %{}",
                                f.name, bi, v
                            ));
                        }
                    }
                    for v in operands(&inst.kind) {
                        if v as usize >= f.values.len() {
                            return Err(format!(
                                "IR `{}` block {} uses invalid value %{}",
                                f.name, bi, v
                            ));
                        }
                    }
                    match &inst.kind {
                        InstKind::Load(local) => {
                            let local = f
                                .locals
                                .get(*local as usize)
                                .ok_or_else(|| format!("IR `{}` loads invalid local", f.name))?;
                            if local.ty != inst.ty {
                                return Err(format!("IR `{}` load type mismatch", f.name));
                            }
                        }
                        InstKind::Store { local, value, .. } => {
                            let local = f
                                .locals
                                .get(*local as usize)
                                .ok_or_else(|| format!("IR `{}` stores invalid local", f.name))?;
                            if !compatible(&local.ty, &f.values[*value as usize]) {
                                return Err(format!("IR `{}` store type mismatch", f.name));
                            }
                        }
                        InstKind::Select {
                            condition,
                            then_value,
                            else_value,
                        } => {
                            if f.values.get(*condition as usize) != Some(&Type::Bool)
                                || f.values.get(*then_value as usize) != Some(&inst.ty)
                                || f.values.get(*else_value as usize) != Some(&inst.ty)
                            {
                                return Err(format!("IR `{}` select type mismatch", f.name));
                            }
                        }
                        InstKind::SetField { root, .. } => {
                            if *root as usize >= f.locals.len() {
                                return Err(format!("IR `{}` updates invalid local", f.name));
                            }
                        }
                        InstKind::Call {
                            callee: Callee::Function(id),
                            args,
                            inout,
                        } => {
                            let callee = self
                                .functions
                                .get(*id as usize)
                                .ok_or_else(|| format!("IR `{}` calls invalid function", f.name))?;
                            if args.len() != callee.params.len() || inout.len() != args.len() {
                                return Err(format!("IR `{}` call arity mismatch", f.name));
                            }
                            for (i, (&arg, target)) in args.iter().zip(inout).enumerate() {
                                let want = &callee.locals[callee.params[i] as usize].ty;
                                if !compatible(want, &f.values[arg as usize]) {
                                    return Err(format!(
                                        "IR `{}` call argument type mismatch",
                                        f.name
                                    ));
                                }
                                if target.is_some() != callee.inouts[i] {
                                    return Err(format!("IR `{}` call inout mismatch", f.name));
                                }
                            }
                        }
                        InstKind::Call {
                            callee: Callee::Extern(id),
                            args,
                            inout,
                        } => {
                            let callee = self
                                .externs
                                .get(*id as usize)
                                .ok_or_else(|| format!("IR `{}` calls invalid extern", f.name))?;
                            if args.len() != callee.params.len() || inout.len() != args.len() {
                                return Err(format!("IR `{}` extern call arity mismatch", f.name));
                            }
                            for ((arg, target), (_, want)) in
                                args.iter().zip(inout).zip(&callee.params)
                            {
                                if !compatible(want, &f.values[*arg as usize]) {
                                    return Err(format!(
                                        "IR `{}` extern call argument type mismatch",
                                        f.name
                                    ));
                                }
                                if target.is_some() != matches!(want, Type::Arr(_)) {
                                    return Err(format!(
                                        "IR `{}` extern array copy-out mismatch",
                                        f.name
                                    ));
                                }
                            }
                        }
                        _ => {}
                    }
                }
                let target = |b: BlockId| -> Result<(), String> {
                    if (b as usize) < f.blocks.len() {
                        Ok(())
                    } else {
                        Err(format!(
                            "IR `{}` block {} jumps to invalid block {}",
                            f.name, bi, b
                        ))
                    }
                };
                match block.terminator {
                    Terminator::Jump(b) => target(b)?,
                    Terminator::Branch {
                        condition,
                        then_block,
                        else_block,
                    } => {
                        if f.values.get(condition as usize) != Some(&Type::Bool) {
                            return Err(format!(
                                "IR `{}` block {} branches on non-bool",
                                f.name, bi
                            ));
                        }
                        target(then_block)?;
                        target(else_block)?;
                    }
                    Terminator::Return(v) => {
                        let got = f
                            .values
                            .get(v as usize)
                            .ok_or_else(|| format!("IR `{}` returns invalid value", f.name))?;
                        if !compatible(&f.ret, got) {
                            return Err(format!(
                                "IR `{}` returns {:?}, expected {:?}",
                                f.name, got, f.ret
                            ));
                        }
                    }
                    Terminator::Unreachable => {}
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lower(source: &str) -> LoweredProgram {
        let tokens = crate::lexer::lex(source).unwrap();
        let mut parser = crate::parser::Parser::new(tokens);
        parser.parse().unwrap();
        LoweredProgram::lower(parser.prog).unwrap()
    }

    #[test]
    fn lowering_resolves_calls_fields_and_short_circuit_control() {
        let ir = lower(
            r#"
            type Pair { left: i64, right: i64 }
            fn id(x: i64): i64 { x }
            main {
                let p = Pair { right: id(2), left: 1 }
                let ok = false and (p.right / 0 == 0)
                print(p.left, ok)
            }
        "#,
        );
        let main = ir.main.as_ref().unwrap();
        assert!(main
            .blocks
            .iter()
            .any(|b| matches!(b.terminator, Terminator::Branch { .. })));
        assert!(main.blocks.iter().flat_map(|b| &b.instructions).any(|i| {
            matches!(
                i.kind,
                InstKind::Call {
                    callee: Callee::Function(0),
                    ..
                }
            )
        }));
        assert!(main.blocks.iter().flat_map(|b| &b.instructions).any(|i| {
            matches!(
                i.kind,
                InstKind::Field {
                    record: 0,
                    field: 0,
                    ..
                }
            )
        }));
    }

    #[test]
    fn lowering_turns_sums_into_explicit_back_edges() {
        let ir = lower("main { print(sum(i in 0..4) i * i) }");
        let main = ir.main.as_ref().unwrap();
        assert!(main.blocks.iter().enumerate().any(|(from, block)| {
            matches!(block.terminator, Terminator::Jump(to) if to as usize <= from)
        }));
    }
}

fn operands(k: &InstKind) -> Vec<ValueId> {
    match k {
        InstKind::Store { value, .. }
        | InstKind::Unary { value, .. }
        | InstKind::SetField { value, .. } => vec![*value],
        InstKind::Binary { lhs, rhs, .. } => vec![*lhs, *rhs],
        InstKind::Select {
            condition,
            then_value,
            else_value,
        } => vec![*condition, *then_value, *else_value],
        InstKind::Call { args, .. }
        | InstKind::Array(args)
        | InstKind::Record { fields: args, .. } => args.clone(),
        InstKind::Field { base, .. } => vec![*base],
        InstKind::Index { base, index } => vec![*base, *index],
        InstKind::SetIndex {
            base, index, value, ..
        } => vec![*base, *index, *value],
        InstKind::Constant(_) | InstKind::Load(_) | InstKind::Enum { .. } => Vec::new(),
    }
}

fn compatible(expected: &Type, actual: &Type) -> bool {
    expected == actual
        || (matches!(expected, Type::F32 | Type::F64) && *actual == Type::I64)
        || (*expected == Type::F32 && *actual == Type::F64)
        || (*expected == Type::F64 && *actual == Type::F32)
}

struct Builder<'a> {
    p: &'a ast::Program,
    types: &'a [Option<Type>],
    functions: &'a HashMap<String, FunctionId>,
    externs: &'a HashMap<String, ExternId>,
    name: String,
    exported: bool,
    params: Vec<LocalId>,
    inouts: Vec<bool>,
    ret: Type,
    locals: Vec<Local>,
    scopes: Vec<HashMap<String, LocalId>>,
    values: Vec<Type>,
    blocks: Vec<Block>,
    current: BlockId,
}

impl<'a> Builder<'a> {
    fn new(
        p: &'a ast::Program,
        types: &'a [Option<Type>],
        functions: &'a HashMap<String, FunctionId>,
        externs: &'a HashMap<String, ExternId>,
        f: &FnDecl,
    ) -> Result<Self, String> {
        let mut b = Self {
            p,
            types,
            functions,
            externs,
            name: f.name.clone(),
            exported: f.exported,
            params: Vec::new(),
            inouts: f.inouts.clone(),
            ret: crate::check::resolve_type(p, &f.ret)?,
            locals: Vec::new(),
            scopes: vec![HashMap::new()],
            values: Vec::new(),
            blocks: vec![Block {
                instructions: Vec::new(),
                terminator: Terminator::Unreachable,
            }],
            current: 0,
        };
        for (i, (name, ty)) in f.params.iter().enumerate() {
            let ty = crate::check::resolve_type(p, ty)?;
            let mutable = f.inouts.get(i).copied().unwrap_or(false)
                || (f.exported && matches!(&ty, Type::Arr(_)));
            let id = b.add_local(name, ty, mutable);
            b.params.push(id);
        }
        Ok(b)
    }

    fn finish(mut self, body: &[StmtId]) -> Result<Function, String> {
        let last = self.block(body)?;
        if matches!(
            self.blocks[self.current as usize].terminator,
            Terminator::Unreachable
        ) {
            let value = match last {
                Some(v) if compatible(&self.ret, &self.values[v as usize]) => v,
                _ if self.ret == Type::Unit => self.constant(Constant::Unit, Type::Unit),
                _ => {
                    return Err(format!(
                        "lowering `{}` found no fallthrough value",
                        self.name
                    ))
                }
            };
            self.terminate(Terminator::Return(value));
        }
        Ok(Function {
            name: self.name,
            exported: self.exported,
            params: self.params,
            inouts: self.inouts,
            ret: self.ret,
            locals: self.locals,
            values: self.values,
            blocks: self.blocks,
            entry: 0,
        })
    }

    fn add_local(&mut self, name: &str, ty: Type, mutable: bool) -> LocalId {
        let id = self.locals.len() as LocalId;
        self.locals.push(Local {
            name: name.into(),
            ty,
            mutable,
        });
        self.scopes.last_mut().unwrap().insert(name.into(), id);
        id
    }
    fn temp_local(&mut self, ty: Type) -> LocalId {
        let id = self.locals.len() as LocalId;
        self.locals.push(Local {
            name: format!("$tmp{}", id),
            ty,
            mutable: true,
        });
        id
    }
    fn lookup(&self, name: &str) -> Result<LocalId, String> {
        self.scopes
            .iter()
            .rev()
            .find_map(|s| s.get(name).copied())
            .ok_or_else(|| format!("lowering: unknown local `{}`", name))
    }
    fn new_block(&mut self) -> BlockId {
        let id = self.blocks.len() as BlockId;
        self.blocks.push(Block {
            instructions: Vec::new(),
            terminator: Terminator::Unreachable,
        });
        id
    }
    fn switch(&mut self, id: BlockId) {
        self.current = id;
    }
    fn terminate(&mut self, term: Terminator) {
        self.blocks[self.current as usize].terminator = term;
    }
    fn emit(&mut self, kind: InstKind, ty: Type) -> ValueId {
        let id = self.values.len() as ValueId;
        self.values.push(ty.clone());
        self.blocks[self.current as usize].instructions.push(Inst {
            result: Some(id),
            ty,
            kind,
        });
        id
    }
    fn effect(&mut self, kind: InstKind) {
        self.blocks[self.current as usize].instructions.push(Inst {
            result: None,
            ty: Type::Unit,
            kind,
        });
    }
    fn constant(&mut self, c: Constant, ty: Type) -> ValueId {
        self.emit(InstKind::Constant(c), ty)
    }
    fn load(&mut self, local: LocalId) -> ValueId {
        self.emit(
            InstKind::Load(local),
            self.locals[local as usize].ty.clone(),
        )
    }
    fn store(&mut self, local: LocalId, value: ValueId) {
        self.effect(InstKind::Store {
            local,
            value,
            retain_arrays: true,
        });
    }
    fn expr_type(&self, e: ExprId) -> Result<Type, String> {
        self.types[e as usize]
            .clone()
            .ok_or_else(|| format!("lowering: expression {} has no type", e))
    }

    fn block(&mut self, stmts: &[StmtId]) -> Result<Option<ValueId>, String> {
        self.scopes.push(HashMap::new());
        let mut last = None;
        for &s in stmts {
            if !matches!(
                self.blocks[self.current as usize].terminator,
                Terminator::Unreachable
            ) {
                break;
            }
            last = self.stmt(s)?;
        }
        self.scopes.pop();
        Ok(last)
    }

    fn stmt(&mut self, sid: StmtId) -> Result<Option<ValueId>, String> {
        match self.p.stmt(sid) {
            ast::Stmt::Let(name, e) | ast::Stmt::Var(name, e) => {
                let value = self.expr(*e)?;
                let local = self.add_local(
                    name,
                    self.expr_type(*e)?,
                    matches!(self.p.stmt(sid), ast::Stmt::Var(..)),
                );
                self.store(local, value);
                Ok(None)
            }
            ast::Stmt::Assign(target, e) => {
                let value = self.expr(*e)?; // language order: RHS before place computation
                match self.p.expr(*target) {
                    ast::Expr::Ident(name) => {
                        let l = self.lookup(name)?;
                        self.store(l, value);
                    }
                    ast::Expr::Index(base, index) => {
                        let mut names = Vec::new();
                        let mut current = *base;
                        let root = loop {
                            match self.p.expr(current) {
                                ast::Expr::Ident(name) => break self.lookup(name)?,
                                ast::Expr::Field(parent, name) => {
                                    names.push(name.as_str());
                                    current = *parent;
                                }
                                _ => return Err("lowering: invalid indexed assignment root".into()),
                            }
                        };
                        names.reverse();
                        let mut current_ty = self.locals[root as usize].ty.clone();
                        let mut path = Vec::new();
                        for name in names {
                            let Type::Rec(record) = current_ty else {
                                return Err("lowering: indexed path crosses non-record".into());
                            };
                            let field = self.p.types[record]
                                .fields
                                .iter()
                                .position(|(field, _)| field == name)
                                .ok_or_else(|| format!("unknown field `{}`", name))?;
                            current_ty = crate::check::resolve_type(
                                self.p,
                                &self.p.types[record].fields[field].1,
                            )?;
                            path.push(field);
                        }
                        let b = self.expr(*base)?;
                        let i = self.expr(*index)?;
                        self.effect(InstKind::SetIndex {
                            root,
                            path,
                            base: b,
                            index: i,
                            value,
                        });
                    }
                    ast::Expr::Field(..) => {
                        let mut names = Vec::new();
                        let mut cur = *target;
                        let root = loop {
                            match self.p.expr(cur) {
                                ast::Expr::Field(base, name) => {
                                    names.push(name.as_str());
                                    cur = *base;
                                }
                                ast::Expr::Ident(name) => break self.lookup(name)?,
                                _ => return Err("lowering: invalid field assignment".into()),
                            }
                        };
                        names.reverse();
                        let mut ty = self.locals[root as usize].ty.clone();
                        let mut path = Vec::new();
                        for name in names {
                            let Type::Rec(ti) = ty else {
                                return Err("lowering: field on non-record".into());
                            };
                            let fi = self.p.types[ti]
                                .fields
                                .iter()
                                .position(|(n, _)| n == name)
                                .ok_or_else(|| format!("unknown field `{}`", name))?;
                            ty =
                                crate::check::resolve_type(self.p, &self.p.types[ti].fields[fi].1)?;
                            path.push(fi);
                        }
                        self.effect(InstKind::SetField { root, path, value });
                    }
                    _ => return Err("lowering: invalid assignment target".into()),
                }
                Ok(None)
            }
            ast::Stmt::If(c, yes, no) => {
                let condition = self.expr(*c)?;
                let tb = self.new_block();
                let fb = self.new_block();
                let merge = self.new_block();
                self.terminate(Terminator::Branch {
                    condition,
                    then_block: tb,
                    else_block: fb,
                });
                self.switch(tb);
                self.block(yes)?;
                if matches!(
                    self.blocks[self.current as usize].terminator,
                    Terminator::Unreachable
                ) {
                    self.terminate(Terminator::Jump(merge));
                }
                self.switch(fb);
                self.block(no)?;
                if matches!(
                    self.blocks[self.current as usize].terminator,
                    Terminator::Unreachable
                ) {
                    self.terminate(Terminator::Jump(merge));
                }
                self.switch(merge);
                Ok(None)
            }
            ast::Stmt::While(c, body) => {
                let head = self.new_block();
                let loop_body = self.new_block();
                let exit = self.new_block();
                self.terminate(Terminator::Jump(head));
                self.switch(head);
                let condition = self.expr(*c)?;
                self.terminate(Terminator::Branch {
                    condition,
                    then_block: loop_body,
                    else_block: exit,
                });
                self.switch(loop_body);
                self.block(body)?;
                if matches!(
                    self.blocks[self.current as usize].terminator,
                    Terminator::Unreachable
                ) {
                    self.terminate(Terminator::Jump(head));
                }
                self.switch(exit);
                Ok(None)
            }
            ast::Stmt::For(name, lo, hi, body) => {
                let lo = self.expr(*lo)?;
                let hi = self.expr(*hi)?;
                self.scopes.push(HashMap::new());
                let index = self.add_local(name, Type::I64, false);
                self.store(index, lo);
                let head = self.new_block();
                let loop_body = self.new_block();
                let exit = self.new_block();
                self.terminate(Terminator::Jump(head));
                self.switch(head);
                let iv = self.load(index);
                let cond = self.emit(
                    InstKind::Binary {
                        op: BinaryOp::Lt,
                        lhs: iv,
                        rhs: hi,
                    },
                    Type::Bool,
                );
                self.terminate(Terminator::Branch {
                    condition: cond,
                    then_block: loop_body,
                    else_block: exit,
                });
                self.switch(loop_body);
                self.block(body)?;
                if matches!(
                    self.blocks[self.current as usize].terminator,
                    Terminator::Unreachable
                ) {
                    let cur = self.load(index);
                    let one = self.constant(Constant::I64(1), Type::I64);
                    let next = self.emit(
                        InstKind::Binary {
                            op: BinaryOp::Add,
                            lhs: cur,
                            rhs: one,
                        },
                        Type::I64,
                    );
                    self.store(index, next);
                    self.terminate(Terminator::Jump(head));
                }
                self.scopes.pop();
                self.switch(exit);
                Ok(None)
            }
            ast::Stmt::Return(e) => {
                let value = match e {
                    Some(e) => self.expr(*e)?,
                    None => self.constant(Constant::Unit, Type::Unit),
                };
                self.terminate(Terminator::Return(value));
                Ok(Some(value))
            }
            ast::Stmt::Expr(e) => Ok(Some(self.expr(*e)?)),
        }
    }

    fn expr(&mut self, eid: ExprId) -> Result<ValueId, String> {
        let ty = self.expr_type(eid)?;
        match self.p.expr(eid) {
            ast::Expr::Int(v) => Ok(self.constant(Constant::I64(*v), ty)),
            ast::Expr::Float(v) => Ok(self.constant(Constant::F64(*v), ty)),
            ast::Expr::Str(v) => Ok(self.constant(Constant::Bytes(v.as_bytes().to_vec()), ty)),
            ast::Expr::Bool(v) => Ok(self.constant(Constant::Bool(*v), ty)),
            ast::Expr::Ident(name) => {
                let l = self.lookup(name)?;
                Ok(self.load(l))
            }
            ast::Expr::Un(op, e) => {
                let value = self.expr(*e)?;
                let op = match op.as_str() {
                    "-" => UnaryOp::Neg,
                    "not" => UnaryOp::Not,
                    _ => return Err(format!("lowering: unknown unary `{}`", op)),
                };
                Ok(self.emit(InstKind::Unary { op, value }, ty))
            }
            ast::Expr::Bin(op, l, r) if op == "and" || op == "or" => {
                self.short_circuit(op == "and", *l, *r)
            }
            ast::Expr::Bin(op, l, r) => {
                let lhs = self.expr(*l)?;
                let rhs = self.expr(*r)?;
                if let Some(name) = self.p.infix_ops.get(op) {
                    let callee = Callee::Function(
                        *self
                            .functions
                            .get(name)
                            .ok_or_else(|| format!("unknown operator fn `{}`", name))?,
                    );
                    Ok(self.emit(
                        InstKind::Call {
                            callee,
                            args: vec![lhs, rhs],
                            inout: vec![None, None],
                        },
                        ty,
                    ))
                } else {
                    let op = match op.as_str() {
                        "+" => BinaryOp::Add,
                        "-" => BinaryOp::Sub,
                        "*" => BinaryOp::Mul,
                        "/" => BinaryOp::Div,
                        "%" => BinaryOp::Rem,
                        "==" => BinaryOp::Eq,
                        "!=" => BinaryOp::Ne,
                        "<" => BinaryOp::Lt,
                        "<=" => BinaryOp::Le,
                        ">" => BinaryOp::Gt,
                        ">=" => BinaryOp::Ge,
                        "~=" | "≈" => BinaryOp::ApproxEq,
                        _ => return Err(format!("lowering: unknown binary `{}`", op)),
                    };
                    Ok(self.emit(InstKind::Binary { op, lhs, rhs }, ty))
                }
            }
            ast::Expr::Circum(open, e) => {
                let value = self.expr(*e)?;
                let name = &self.p.circum_ops[open].1;
                let callee = Callee::Function(
                    *self
                        .functions
                        .get(name)
                        .ok_or_else(|| format!("unknown operator fn `{}`", name))?,
                );
                Ok(self.emit(
                    InstKind::Call {
                        callee,
                        args: vec![value],
                        inout: vec![None],
                    },
                    ty,
                ))
            }
            ast::Expr::Field(base, name) => {
                let base_ty = self.expr_type(*base)?;
                let Type::Rec(record) = base_ty else {
                    return Err("lowering: field on non-record".into());
                };
                let field = self.p.types[record]
                    .fields
                    .iter()
                    .position(|(n, _)| n == name)
                    .ok_or_else(|| format!("unknown field `{}`", name))?;
                let base = self.expr(*base)?;
                Ok(self.emit(
                    InstKind::Field {
                        base,
                        record,
                        field,
                    },
                    ty,
                ))
            }
            ast::Expr::Index(base, index) => {
                let base = self.expr(*base)?;
                let index = self.expr(*index)?;
                Ok(self.emit(InstKind::Index { base, index }, ty))
            }
            ast::Expr::Array(items) => {
                let values = items
                    .iter()
                    .map(|e| self.expr(*e))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(self.emit(InstKind::Array(values), ty))
            }
            ast::Expr::Record(name, fields) => {
                let record = self
                    .p
                    .types
                    .iter()
                    .position(|t| t.name == *name)
                    .ok_or_else(|| format!("unknown record `{}`", name))?;
                let mut ordered = vec![None; self.p.types[record].fields.len()];
                for (pos, (name, e)) in fields.iter().enumerate() {
                    let field = name
                        .as_ref()
                        .map(|n| {
                            self.p.types[record]
                                .fields
                                .iter()
                                .position(|(f, _)| f == n)
                                .unwrap()
                        })
                        .unwrap_or(pos);
                    ordered[field] = Some(self.expr(*e)?);
                }
                Ok(self.emit(
                    InstKind::Record {
                        record,
                        fields: ordered.into_iter().map(Option::unwrap).collect(),
                    },
                    ty,
                ))
            }
            ast::Expr::EnumVal(en, var) => {
                let (enumeration, tag) = self
                    .p
                    .enum_tag(en, var)
                    .ok_or_else(|| format!("unknown enum `{}.{}`", en, var))?;
                Ok(self.emit(InstKind::Enum { enumeration, tag }, ty))
            }
            ast::Expr::Sum { var, lo, hi, body } => self.sum(var, *lo, *hi, *body, ty),
            ast::Expr::Call(name, args) => {
                let mut values = Vec::with_capacity(args.len());
                let mut inout = vec![None; args.len()];
                for (i, e) in args.iter().enumerate() {
                    values.push(self.expr(*e)?);
                    let copy_out =
                        self.functions.get(name).is_some_and(|fid| {
                            self.p.fns[*fid as usize].inouts.get(i) == Some(&true)
                        }) || self.externs.get(name).is_some_and(|id| {
                            self.p.externs[*id as usize]
                                .params
                                .get(i)
                                .and_then(|(_, ty)| crate::check::resolve_type(self.p, ty).ok())
                                .is_some_and(|ty| matches!(ty, Type::Arr(_)))
                        });
                    if copy_out {
                        let ast::Expr::Ident(n) = self.p.expr(*e) else {
                            return Err("lowering: copy-out argument is not a local".into());
                        };
                        inout[i] = Some(self.lookup(n)?);
                    }
                }
                let callee = self
                    .functions
                    .get(name)
                    .copied()
                    .map(Callee::Function)
                    .or_else(|| self.externs.get(name).copied().map(Callee::Extern))
                    .unwrap_or_else(|| Callee::Builtin(name.clone()));
                Ok(self.emit(
                    InstKind::Call {
                        callee,
                        args: values,
                        inout,
                    },
                    ty,
                ))
            }
        }
    }

    fn short_circuit(&mut self, and: bool, lhs: ExprId, rhs: ExprId) -> Result<ValueId, String> {
        let left = self.expr(lhs)?;
        let slot = self.temp_local(Type::Bool);
        self.store(slot, left);
        let right_block = self.new_block();
        let merge = self.new_block();
        self.terminate(if and {
            Terminator::Branch {
                condition: left,
                then_block: right_block,
                else_block: merge,
            }
        } else {
            Terminator::Branch {
                condition: left,
                then_block: merge,
                else_block: right_block,
            }
        });
        self.switch(right_block);
        let right = self.expr(rhs)?;
        self.store(slot, right);
        self.terminate(Terminator::Jump(merge));
        self.switch(merge);
        Ok(self.load(slot))
    }

    fn sum(
        &mut self,
        var: &str,
        lo: ExprId,
        hi: ExprId,
        body: ExprId,
        ty: Type,
    ) -> Result<ValueId, String> {
        let lo = self.expr(lo)?;
        let hi = self.expr(hi)?;
        self.scopes.push(HashMap::new());
        let index = self.add_local(var, Type::I64, false);
        let acc = self.temp_local(ty.clone());
        self.store(index, lo);
        let zero = match ty {
            Type::I64 => self.constant(Constant::I64(0), Type::I64),
            Type::F32 => self.constant(Constant::F32(0.0), Type::F32),
            Type::F64 => self.constant(Constant::F64(0.0), Type::F64),
            _ => return Err("lowering: non-numeric sum".into()),
        };
        self.store(acc, zero);
        let head = self.new_block();
        let loop_body = self.new_block();
        let exit = self.new_block();
        self.terminate(Terminator::Jump(head));
        self.switch(head);
        let iv = self.load(index);
        let cond = self.emit(
            InstKind::Binary {
                op: BinaryOp::Lt,
                lhs: iv,
                rhs: hi,
            },
            Type::Bool,
        );
        self.terminate(Terminator::Branch {
            condition: cond,
            then_block: loop_body,
            else_block: exit,
        });
        self.switch(loop_body);
        let value = self.expr(body)?;
        let old = self.load(acc);
        let next = self.emit(
            InstKind::Binary {
                op: BinaryOp::Add,
                lhs: old,
                rhs: value,
            },
            ty.clone(),
        );
        self.store(acc, next);
        let cur = self.load(index);
        let one = self.constant(Constant::I64(1), Type::I64);
        let inc = self.emit(
            InstKind::Binary {
                op: BinaryOp::Add,
                lhs: cur,
                rhs: one,
            },
            Type::I64,
        );
        self.store(index, inc);
        self.terminate(Terminator::Jump(head));
        self.scopes.pop();
        self.switch(exit);
        Ok(self.load(acc))
    }
}
