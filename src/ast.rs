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
    Sum { var: String, lo: ExprId, hi: ExprId, body: ExprId },
    Circum(String, ExprId), // key = open glyph, operand
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Let(String, ExprId),
    Var(String, ExprId),
    Assign(ExprId, ExprId),
    If(ExprId, Vec<StmtId>, Vec<StmtId>),
    For(String, ExprId, ExprId, Vec<StmtId>),
    Return(Option<ExprId>),
    Expr(ExprId),
}

#[derive(Debug, Clone)]
pub struct FnDecl {
    pub name: String,
    pub params: Vec<(String, String)>,
    pub ret: String,
    pub body: Vec<StmtId>,
}

#[derive(Debug, Clone)]
pub struct TypeDecl {
    pub name: String,
    pub fields: Vec<(String, String)>,
}

#[derive(Debug, Default)]
pub struct Program {
    pub exprs: Vec<Expr>,
    pub stmts: Vec<Stmt>,
    pub fns: Vec<FnDecl>,
    pub types: Vec<TypeDecl>,
    pub props: Vec<FnDecl>,
    pub main: Option<Vec<StmtId>>,
    // glyph -> function name (operators are ordinary functions after parsing)
    pub infix_ops: HashMap<String, String>,
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
}
