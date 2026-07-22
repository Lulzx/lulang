use crate::ast::*;
use crate::lexer::Tok;
use std::collections::{HashMap, HashSet};

pub struct Parser {
    toks: Vec<Tok>,
    pos: usize,
    pub prog: Program,
    prec: HashMap<String, u8>,
    type_names: HashSet<String>,
    circum_close: HashSet<String>,
}

impl Parser {
    pub fn new(toks: Vec<Tok>) -> Self {
        let mut prec = HashMap::new();
        for s in ["==", "!=", "<", "<=", ">", ">=", "~=", "\u{2248}"] {
            prec.insert(s.to_string(), 3);
        }
        prec.insert("+".into(), 5);
        prec.insert("-".into(), 5);
        for s in ["*", "/", "%"] {
            prec.insert(s.to_string(), 6);
        }
        Parser {
            toks,
            pos: 0,
            prog: Program::default(),
            prec,
            type_names: HashSet::new(),
            circum_close: HashSet::new(),
        }
    }

    fn peek(&self) -> &Tok {
        &self.toks[self.pos]
    }
    fn peek2(&self) -> &Tok {
        self.toks.get(self.pos + 1).unwrap_or(&Tok::Eof)
    }
    fn next(&mut self) -> Tok {
        let t = self.toks[self.pos].clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }
    fn skip_nl(&mut self) {
        while matches!(self.peek(), Tok::Newline) {
            self.pos += 1;
        }
    }
    fn is_sym(&self, s: &str) -> bool {
        matches!(self.peek(), Tok::Sym(x) if x == s)
    }
    fn eat_sym(&mut self, s: &str) -> Result<(), String> {
        if self.is_sym(s) {
            self.next();
            Ok(())
        } else {
            Err(format!("expected `{}`, found {:?}", s, self.peek()))
        }
    }
    fn eat_kw(&mut self, k: &str) -> Result<(), String> {
        match self.next() {
            Tok::Ident(s) if s == k => Ok(()),
            t => Err(format!("expected `{}`, found {:?}", k, t)),
        }
    }
    fn ident(&mut self) -> Result<String, String> {
        match self.next() {
            Tok::Ident(s) => Ok(s),
            t => Err(format!("expected identifier, found {:?}", t)),
        }
    }
    fn alloc(&mut self, e: Expr) -> ExprId {
        self.prog.exprs.push(e);
        (self.prog.exprs.len() - 1) as ExprId
    }
    fn alloc_stmt(&mut self, s: Stmt) -> StmtId {
        self.prog.stmts.push(s);
        (self.prog.stmts.len() - 1) as StmtId
    }

    pub fn parse(&mut self) -> Result<(), String> {
        loop {
            self.skip_nl();
            match self.peek().clone() {
                Tok::Eof => return Ok(()),
                Tok::Ident(k) if k == "fn" => {
                    let f = self.parse_fn(true)?;
                    self.prog.fns.push(f);
                }
                Tok::Ident(k) if k == "type" => self.parse_type()?,
                Tok::Ident(k) if k == "operator" => self.parse_operator()?,
                Tok::Ident(k) if k == "property" => {
                    let f = self.parse_fn(false)?;
                    self.prog.props.push(f);
                }
                Tok::Ident(k) if k == "main" => {
                    self.eat_kw("main")?;
                    let body = self.parse_block()?;
                    self.prog.main = Some(body);
                }
                Tok::Ident(k) if k == "use" => {
                    while !matches!(self.peek(), Tok::Newline | Tok::Eof) {
                        self.next();
                    }
                }
                t => return Err(format!("unexpected top-level token {:?}", t)),
            }
        }
    }

    fn parse_type(&mut self) -> Result<(), String> {
        self.eat_kw("type")?;
        let name = self.ident()?;
        self.eat_sym("{")?;
        self.skip_nl();
        let mut fields = Vec::new();
        while !self.is_sym("}") {
            let f = self.ident()?;
            self.eat_sym(":")?;
            let t = self.parse_type_str()?;
            fields.push((f, t));
            if self.is_sym(",") {
                self.next();
            }
            self.skip_nl();
        }
        self.eat_sym("}")?;
        self.type_names.insert(name.clone());
        self.prog.types.push(TypeDecl { name, fields });
        Ok(())
    }

    fn parse_type_str(&mut self) -> Result<String, String> {
        if self.is_sym("[") {
            self.next();
            let inner = self.parse_type_str()?;
            self.eat_sym("]")?;
            Ok(format!("[{}]", inner))
        } else {
            self.ident()
        }
    }

    fn parse_params(&mut self) -> Result<Vec<(String, String)>, String> {
        self.eat_sym("(")?;
        self.skip_nl();
        let mut ps = Vec::new();
        while !self.is_sym(")") {
            let n = self.ident()?;
            self.eat_sym(":")?;
            let t = self.parse_type_str()?;
            ps.push((n, t));
            if self.is_sym(",") {
                self.next();
                self.skip_nl();
            }
        }
        self.eat_sym(")")?;
        Ok(ps)
    }

    fn parse_fn(&mut self, has_kw_fn: bool) -> Result<FnDecl, String> {
        self.eat_kw(if has_kw_fn { "fn" } else { "property" })?;
        let name = self.ident()?;
        let params = self.parse_params()?;
        let ret = if self.is_sym(":") {
            self.next();
            self.parse_type_str()?
        } else {
            "bool".into()
        };
        let body = self.parse_block()?;
        Ok(FnDecl { name, params, ret, body })
    }

    fn parse_operator(&mut self) -> Result<(), String> {
        self.eat_kw("operator")?;
        // infix: `operator<anchor> (a: T) <glyph> (b: U): R { .. }` — anchor is a
        // known-precedence symbol immediately followed by `(`.
        // circumfix: `operator <open>(v: T)<close>: R { .. }`.
        let first = match self.next() {
            Tok::Sym(s) => s,
            t => return Err(format!("expected operator glyph after `operator`, found {:?}", t)),
        };
        if self.prec.contains_key(&first) && matches!(self.peek(), Tok::Sym(s) if s == "(") {
            let anchor_prec = self.prec[&first];
            let a = {
                let mut ps = self.parse_params_single()?;
                ps.remove(0)
            };
            let glyph = match self.next() {
                Tok::Sym(s) => s,
                t => return Err(format!("expected infix glyph, found {:?}", t)),
            };
            let b = {
                let mut ps = self.parse_params_single()?;
                ps.remove(0)
            };
            self.eat_sym(":")?;
            let ret = self.parse_type_str()?;
            let fname = format!("operator{}", glyph);
            self.prec.insert(glyph.clone(), anchor_prec);
            self.prog.infix_ops.insert(glyph, fname.clone());
            let body = self.parse_block()?;
            self.prog.fns.push(FnDecl { name: fname, params: vec![a, b], ret, body });
            Ok(())
        } else {
            let open = first;
            let mut ps = self.parse_params_single()?;
            let v = ps.remove(0);
            let close = match self.next() {
                Tok::Sym(s) => s,
                t => return Err(format!("expected closing glyph, found {:?}", t)),
            };
            self.eat_sym(":")?;
            let ret = self.parse_type_str()?;
            let fname = format!("operator{}{}", open, close);
            self.circum_close.insert(close.clone());
            self.prog.circum_ops.insert(open, (close, fname.clone()));
            let body = self.parse_block()?;
            self.prog.fns.push(FnDecl { name: fname, params: vec![v], ret, body });
            Ok(())
        }
    }

    fn parse_params_single(&mut self) -> Result<Vec<(String, String)>, String> {
        let ps = self.parse_params()?;
        if ps.len() != 1 {
            return Err("operator parameter list must have exactly one parameter".into());
        }
        Ok(ps)
    }

    fn parse_block(&mut self) -> Result<Vec<StmtId>, String> {
        self.skip_nl();
        self.eat_sym("{")?;
        let mut out = Vec::new();
        loop {
            self.skip_nl();
            if self.is_sym("}") {
                self.next();
                return Ok(out);
            }
            if matches!(self.peek(), Tok::Eof) {
                return Err("unexpected EOF in block".into());
            }
            let s = self.parse_stmt()?;
            out.push(s);
        }
    }

    fn parse_stmt(&mut self) -> Result<StmtId, String> {
        match self.peek().clone() {
            Tok::Ident(k) if k == "let" || k == "var" => {
                self.next();
                let name = self.ident()?;
                self.eat_sym("=")?;
                let e = self.parse_expr(0)?;
                Ok(self.alloc_stmt(if k == "let" { Stmt::Let(name, e) } else { Stmt::Var(name, e) }))
            }
            Tok::Ident(k) if k == "if" => {
                self.next();
                let c = self.parse_expr(0)?;
                let then = self.parse_block()?;
                let els = if matches!(self.peek(), Tok::Ident(s) if s == "else") {
                    self.next();
                    self.parse_block()?
                } else {
                    Vec::new()
                };
                Ok(self.alloc_stmt(Stmt::If(c, then, els)))
            }
            Tok::Ident(k) if k == "for" => {
                self.next();
                let v = self.ident()?;
                self.eat_kw("in")?;
                let lo = self.parse_expr(4)?;
                self.eat_sym("..")?;
                let hi = self.parse_expr(4)?;
                let body = self.parse_block()?;
                Ok(self.alloc_stmt(Stmt::For(v, lo, hi, body)))
            }
            Tok::Ident(k) if k == "return" => {
                self.next();
                if matches!(self.peek(), Tok::Newline | Tok::Eof) || self.is_sym("}") {
                    Ok(self.alloc_stmt(Stmt::Return(None)))
                } else {
                    let e = self.parse_expr(0)?;
                    Ok(self.alloc_stmt(Stmt::Return(Some(e))))
                }
            }
            _ => {
                let e = self.parse_expr(0)?;
                if self.is_sym("=") {
                    self.next();
                    let v = self.parse_expr(0)?;
                    match self.prog.expr(e) {
                        Expr::Ident(_) | Expr::Index(_, _) | Expr::Field(_, _) => {}
                        _ => return Err("invalid assignment target".into()),
                    }
                    Ok(self.alloc_stmt(Stmt::Assign(e, v)))
                } else {
                    Ok(self.alloc_stmt(Stmt::Expr(e)))
                }
            }
        }
    }

    fn parse_expr(&mut self, min_prec: u8) -> Result<ExprId, String> {
        let mut lhs = self.parse_prefix()?;
        loop {
            let (op, p) = match self.peek() {
                Tok::Sym(s) if s == "=" || s == ".." => break,
                Tok::Sym(s) if self.circum_close.contains(s) => break,
                Tok::Sym(s) => match self.prec.get(s) {
                    Some(&p) => (s.clone(), p),
                    None => break,
                },
                Tok::Ident(k) if k == "and" => ("and".to_string(), 2),
                Tok::Ident(k) if k == "or" => ("or".to_string(), 1),
                _ => break,
            };
            if p < min_prec {
                break;
            }
            self.next();
            self.skip_nl();
            let rhs = self.parse_expr(p + 1)?;
            lhs = self.alloc(Expr::Bin(op, lhs, rhs));
        }
        Ok(lhs)
    }

    fn parse_prefix(&mut self) -> Result<ExprId, String> {
        match self.peek().clone() {
            Tok::Sym(s) if s == "-" => {
                self.next();
                let e = self.parse_prefix()?;
                Ok(self.alloc(Expr::Un("-".into(), e)))
            }
            Tok::Ident(k) if k == "not" => {
                self.next();
                let e = self.parse_prefix()?;
                Ok(self.alloc(Expr::Un("not".into(), e)))
            }
            Tok::Sym(s) if self.prog.circum_ops.contains_key(&s) => {
                self.next();
                let close = self.prog.circum_ops[&s].0.clone();
                let e = self.parse_expr(0)?;
                self.eat_sym(&close)?;
                Ok(self.alloc(Expr::Circum(s, e)))
            }
            _ => {
                let a = self.parse_atom()?;
                self.parse_postfix(a)
            }
        }
    }

    fn parse_postfix(&mut self, mut e: ExprId) -> Result<ExprId, String> {
        loop {
            if self.is_sym(".") {
                self.next();
                let f = self.ident()?;
                e = self.alloc(Expr::Field(e, f));
            } else if self.is_sym("[") {
                self.next();
                let i = self.parse_expr(0)?;
                self.eat_sym("]")?;
                e = self.alloc(Expr::Index(e, i));
            } else {
                return Ok(e);
            }
        }
    }

    fn parse_atom(&mut self) -> Result<ExprId, String> {
        match self.next() {
            Tok::Int(v) => Ok(self.alloc(Expr::Int(v))),
            Tok::Float(v) => Ok(self.alloc(Expr::Float(v))),
            Tok::Str(s) => Ok(self.alloc(Expr::Str(s))),
            Tok::Ident(name) => match name.as_str() {
                "true" => Ok(self.alloc(Expr::Bool(true))),
                "false" => Ok(self.alloc(Expr::Bool(false))),
                "sum" => {
                    self.eat_sym("(")?;
                    let var = self.ident()?;
                    self.eat_kw("in")?;
                    let lo = self.parse_expr(4)?;
                    self.eat_sym("..")?;
                    let hi = self.parse_expr(4)?;
                    self.eat_sym(")")?;
                    let body = self.parse_expr(4)?;
                    Ok(self.alloc(Expr::Sum { var, lo, hi, body }))
                }
                _ => {
                    if self.is_sym("(") {
                        self.next();
                        self.skip_nl();
                        let mut args = Vec::new();
                        while !self.is_sym(")") {
                            args.push(self.parse_expr(0)?);
                            if self.is_sym(",") {
                                self.next();
                                self.skip_nl();
                            }
                        }
                        self.next();
                        Ok(self.alloc(Expr::Call(name, args)))
                    } else if self.is_sym("{") && self.type_names.contains(&name) {
                        self.next();
                        self.skip_nl();
                        let mut inits = Vec::new();
                        while !self.is_sym("}") {
                            let field = if let (Tok::Ident(f), Tok::Sym(c)) = (self.peek(), self.peek2()) {
                                if c == ":" {
                                    let f = f.clone();
                                    self.next();
                                    self.next();
                                    Some(f)
                                } else {
                                    None
                                }
                            } else {
                                None
                            };
                            let e = self.parse_expr(0)?;
                            inits.push((field, e));
                            if self.is_sym(",") {
                                self.next();
                                self.skip_nl();
                            }
                            self.skip_nl();
                        }
                        self.next();
                        Ok(self.alloc(Expr::Record(name, inits)))
                    } else {
                        Ok(self.alloc(Expr::Ident(name)))
                    }
                }
            },
            Tok::Sym(s) if s == "(" => {
                self.skip_nl();
                let e = self.parse_expr(0)?;
                self.skip_nl();
                self.eat_sym(")")?;
                Ok(e)
            }
            Tok::Sym(s) if s == "[" => {
                self.skip_nl();
                let mut items = Vec::new();
                while !self.is_sym("]") {
                    items.push(self.parse_expr(0)?);
                    if self.is_sym(",") {
                        self.next();
                        self.skip_nl();
                    }
                }
                self.next();
                Ok(self.alloc(Expr::Array(items)))
            }
            t => Err(format!("unexpected token in expression: {:?}", t)),
        }
    }
}
