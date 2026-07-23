use std::collections::HashMap;

pub type ExprId = u32;
pub type StmtId = u32;

#[derive(Debug, Clone)]
pub enum Expr {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Ident(String),
    Bin(String, ExprId, ExprId),
    Un(String, ExprId),
    Call(String, Vec<ExprId>),
    Field(ExprId, String),
    Index(ExprId, ExprId),
    Record(String, Vec<(Option<String>, ExprId)>),
    Array(Vec<ExprId>),
    Sum {
        var: String,
        lo: ExprId,
        hi: ExprId,
        body: ExprId,
    },
    Circum(String, ExprId),  // key = open glyph, operand
    EnumVal(String, String), // EnumName.Variant
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Let(String, ExprId),
    Var(String, ExprId),
    Assign(ExprId, ExprId),
    If(ExprId, Vec<StmtId>, Vec<StmtId>),
    For(String, ExprId, ExprId, Vec<StmtId>),
    While(ExprId, Vec<StmtId>),
    Return(Option<ExprId>),
    Expr(ExprId),
}

#[derive(Debug, Clone)]
pub struct FnDecl {
    pub name: String,
    pub params: Vec<(String, String)>,
    // parallel to params: true = `inout` (copy-in/copy-out; no aliasing ever)
    pub inouts: Vec<bool>,
    pub ret: String,
    pub body: Vec<StmtId>,
    pub exported: bool,
}

impl FnDecl {
    pub fn has_inout(&self) -> bool {
        self.inouts.iter().any(|&b| b)
    }
}

#[derive(Debug, Clone)]
pub struct ExternDecl {
    pub name: String,
    pub lib: Option<String>,
    pub params: Vec<(String, String)>,
    pub inouts: Vec<bool>,
    pub ret: String,
}

#[derive(Debug, Clone)]
pub struct TypeDecl {
    pub name: String,
    pub fields: Vec<(String, String)>,
    /// Stable C field order/layout at an FFI boundary. Ordinary records keep
    /// compiler-owned layout and must never acquire an implicit C ABI.
    pub c_layout: bool,
}

#[derive(Debug, Clone)]
pub struct EnumDecl {
    pub name: String,
    pub variants: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseDecl {
    pub module: String,
    pub alias: String,
}

#[derive(Debug, Default)]
pub struct Program {
    pub exprs: Vec<Expr>,
    pub stmts: Vec<Stmt>,
    pub fns: Vec<FnDecl>,
    pub externs: Vec<ExternDecl>,
    pub types: Vec<TypeDecl>,
    pub enums: Vec<EnumDecl>,
    pub uses: Vec<UseDecl>,
    pub props: Vec<FnDecl>,
    pub main: Option<Vec<StmtId>>,
    // glyph -> function name (operators are ordinary functions after parsing)
    pub infix_ops: HashMap<String, String>,
    // glyph -> parser precedence, retained so imported modules seed parsing
    // without copying their declarations into another module's AST.
    pub infix_precedence: HashMap<String, u8>,
    // open glyph -> (close glyph, function name)
    pub circum_ops: HashMap<String, (String, String)>,
}

impl Program {
    pub fn expr(&self, id: ExprId) -> &Expr {
        &self.exprs[id as usize]
    }
    pub fn stmt(&self, id: StmtId) -> &Stmt {
        &self.stmts[id as usize]
    }
    /// (enum index, variant tag) for `EnumName.Variant`.
    pub fn enum_tag(&self, ename: &str, vname: &str) -> Option<(usize, i64)> {
        let ei = self.enums.iter().position(|e| e.name == ename)?;
        let tag = self.enums[ei].variants.iter().position(|v| v == vname)? as i64;
        Some((ei, tag))
    }
    pub fn find_fn(&self, name: &str) -> Option<&FnDecl> {
        self.fns.iter().find(|f| f.name == name)
    }
}
