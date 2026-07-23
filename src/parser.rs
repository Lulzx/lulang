use crate::ast::*;
use crate::lexer::Tok;
use std::collections::{HashMap, HashSet};

fn operator_ascii_name(glyphs: &[&str]) -> String {
    let mut name = String::from("operator");
    for glyph in glyphs {
        for scalar in glyph.chars() {
            name.push_str(&format!("_u{:x}", scalar as u32));
        }
    }
    name
}

pub struct Parser {
    toks: Vec<Tok>,
    pos: usize,
    pub prog: Program,
    prec: HashMap<String, u8>,
    type_names: HashSet<String>,
    enum_names: HashSet<String>,
    circum_close: HashSet<String>,
    match_ct: u32,
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
            enum_names: HashSet::new(),
            circum_close: HashSet::new(),
            match_ct: 0,
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
                    let f = self.parse_fn(true, false)?;
                    self.prog.fns.push(f);
                }
                Tok::Ident(k) if k == "export" => {
                    self.eat_kw("export")?;
                    if !matches!(self.peek(), Tok::Ident(k) if k == "fn") {
                        return Err("`export` must be followed by `fn`".into());
                    }
                    let f = self.parse_fn(true, true)?;
                    self.prog.fns.push(f);
                }
                Tok::Ident(k) if k == "extern" => self.parse_extern()?,
                Tok::Ident(k) if k == "type" => self.parse_type()?,
                Tok::Ident(k) if k == "enum" => self.parse_enum()?,
                Tok::Ident(k) if k == "operator" => self.parse_operator()?,
                Tok::Ident(k) if k == "property" => {
                    let f = self.parse_fn(false, false)?;
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

    fn parse_enum(&mut self) -> Result<(), String> {
        self.eat_kw("enum")?;
        let name = self.ident()?;
        self.eat_sym("{")?;
        self.skip_nl();
        let mut variants = Vec::new();
        while !self.is_sym("}") {
            variants.push(self.ident()?);
            if self.is_sym(",") {
                self.next();
            }
            self.skip_nl();
        }
        self.eat_sym("}")?;
        if variants.is_empty() {
            return Err(format!("enum `{}` has no variants", name));
        }
        self.enum_names.insert(name.clone());
        self.prog.enums.push(EnumDecl { name, variants });
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

    fn parse_params(&mut self) -> Result<(Vec<(String, String)>, Vec<bool>), String> {
        self.eat_sym("(")?;
        self.skip_nl();
        let mut ps = Vec::new();
        let mut inouts = Vec::new();
        while !self.is_sym(")") {
            let mut n = self.ident()?;
            let mut io = false;
            if n == "inout" {
                io = true;
                n = self.ident()?;
            }
            self.eat_sym(":")?;
            let t = self.parse_type_str()?;
            ps.push((n, t));
            inouts.push(io);
            if self.is_sym(",") {
                self.next();
                self.skip_nl();
            }
        }
        self.eat_sym(")")?;
        Ok((ps, inouts))
    }

    fn parse_fn(&mut self, has_kw_fn: bool, exported: bool) -> Result<FnDecl, String> {
        self.eat_kw(if has_kw_fn { "fn" } else { "property" })?;
        let name = self.ident()?;
        let (params, inouts) = self.parse_params()?;
        let ret = if self.is_sym(":") {
            self.next();
            self.parse_type_str()?
        } else if has_kw_fn {
            "()".into() // fn with no annotation returns unit
        } else {
            "bool".into() // property bodies are predicates
        };
        let body = self.parse_block()?;
        Ok(FnDecl {
            name,
            params,
            inouts,
            ret,
            body,
            exported,
        })
    }

    fn parse_extern(&mut self) -> Result<(), String> {
        self.eat_kw("extern")?;
        let lib = match self.peek().clone() {
            Tok::Str(lib) => {
                self.next();
                Some(lib)
            }
            _ => None,
        };
        self.eat_kw("fn")?;
        let name = self.ident()?;
        let (params, inouts) = self.parse_params()?;
        let ret = if self.is_sym(":") {
            self.next();
            self.parse_type_str()?
        } else {
            "()".into()
        };
        self.prog.externs.push(ExternDecl {
            name,
            lib,
            params,
            inouts,
            ret,
        });
        Ok(())
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
            // Stable ASCII-callable spelling required by the language spec.
            // Example: `⊕` is the ordinary function `operator_u2295`.
            let fname = operator_ascii_name(&[&glyph]);
            self.prec.insert(glyph.clone(), anchor_prec);
            self.prog.infix_ops.insert(glyph, fname.clone());
            let body = self.parse_block()?;
            self.prog.fns.push(FnDecl {
                name: fname,
                params: vec![a, b],
                inouts: vec![false, false],
                ret,
                body,
                exported: false,
            });
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
            let fname = operator_ascii_name(&[&open, &close]);
            self.circum_close.insert(close.clone());
            self.prog.circum_ops.insert(open, (close, fname.clone()));
            let body = self.parse_block()?;
            self.prog.fns.push(FnDecl {
                name: fname,
                params: vec![v],
                inouts: vec![false],
                ret,
                body,
                exported: false,
            });
            Ok(())
        }
    }

    fn parse_params_single(&mut self) -> Result<Vec<(String, String)>, String> {
        let (ps, inouts) = self.parse_params()?;
        if ps.len() != 1 {
            return Err("operator parameter list must have exactly one parameter".into());
        }
        if inouts[0] {
            return Err("operator parameters cannot be `inout`".into());
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
            self.parse_stmt(&mut out)?;
        }
    }

    fn parse_stmt(&mut self, out: &mut Vec<StmtId>) -> Result<(), String> {
        match self.peek().clone() {
            Tok::Ident(k) if k == "let" || k == "var" => {
                self.next();
                let name = self.ident()?;
                self.eat_sym("=")?;
                let e = self.parse_expr(0)?;
                out.push(self.alloc_stmt(if k == "let" { Stmt::Let(name, e) } else { Stmt::Var(name, e) }));
                Ok(())
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
                out.push(self.alloc_stmt(Stmt::If(c, then, els)));
                Ok(())
            }
            Tok::Ident(k) if k == "for" => {
                self.next();
                let v = self.ident()?;
                self.eat_kw("in")?;
                let lo = self.parse_expr(4)?;
                self.eat_sym("..")?;
                let hi = self.parse_expr(4)?;
                let body = self.parse_block()?;
                out.push(self.alloc_stmt(Stmt::For(v, lo, hi, body)));
                Ok(())
            }
            Tok::Ident(k) if k == "while" => {
                self.next();
                let c = self.parse_expr(0)?;
                let body = self.parse_block()?;
                out.push(self.alloc_stmt(Stmt::While(c, body)));
                Ok(())
            }
            Tok::Ident(k) if k == "match" => {
                self.next();
                let scrut = self.parse_expr(0)?;
                // bind the scrutinee once, then desugar arms to an `==` chain
                // (exhaustiveness is checked here, where the enum decl is known)
                let tmp = format!("__match{}", self.match_ct);
                self.match_ct += 1;
                self.skip_nl();
                self.eat_sym("{")?;
                let mut arms: Vec<(String, String, Vec<StmtId>)> = Vec::new();
                let mut else_arm: Option<Vec<StmtId>> = None;
                loop {
                    self.skip_nl();
                    if self.is_sym("}") {
                        self.next();
                        break;
                    }
                    if matches!(self.peek(), Tok::Ident(s) if s == "else") {
                        self.next();
                        else_arm = Some(self.parse_block()?);
                        continue;
                    }
                    let ename = self.ident()?;
                    self.eat_sym(".")?;
                    let vname = self.ident()?;
                    let body = self.parse_block()?;
                    arms.push((ename, vname, body));
                }
                if arms.is_empty() {
                    return Err("`match` needs at least one enum arm".into());
                }
                let ename = arms[0].0.clone();
                let decl = self
                    .prog
                    .enums
                    .iter()
                    .find(|e| e.name == ename)
                    .ok_or(format!("unknown enum `{}` in match", ename))?
                    .clone();
                let mut seen = Vec::new();
                for (en, vn, _) in &arms {
                    if *en != ename {
                        return Err(format!("match arms mix enums `{}` and `{}`", ename, en));
                    }
                    if !decl.variants.contains(vn) {
                        return Err(format!("enum `{}` has no variant `{}`", ename, vn));
                    }
                    if seen.contains(vn) {
                        return Err(format!("duplicate match arm `{}.{}`", ename, vn));
                    }
                    seen.push(vn.clone());
                }
                if else_arm.is_none() {
                    for v in &decl.variants {
                        if !seen.contains(v) {
                            return Err(format!(
                                "non-exhaustive match: missing `{}.{}` (or add `else`)",
                                ename, v
                            ));
                        }
                    }
                }
                let let_id = self.alloc_stmt(Stmt::Let(tmp.clone(), scrut));
                out.push(let_id);
                let mut chain = else_arm.unwrap_or_default();
                for (en, vn, body) in arms.into_iter().rev() {
                    let sv = self.alloc(Expr::Ident(tmp.clone()));
                    let pat = self.alloc(Expr::EnumVal(en, vn));
                    let cond = self.alloc(Expr::Bin("==".into(), sv, pat));
                    let if_id = self.alloc_stmt(Stmt::If(cond, body, chain));
                    chain = vec![if_id];
                }
                out.push(chain[0]);
                Ok(())
            }
            Tok::Ident(k) if k == "return" => {
                self.next();
                if matches!(self.peek(), Tok::Newline | Tok::Eof) || self.is_sym("}") {
                    out.push(self.alloc_stmt(Stmt::Return(None)));
                } else {
                    let e = self.parse_expr(0)?;
                    out.push(self.alloc_stmt(Stmt::Return(Some(e))));
                }
                Ok(())
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
                    out.push(self.alloc_stmt(Stmt::Assign(e, v)));
                } else {
                    out.push(self.alloc_stmt(Stmt::Expr(e)));
                }
                Ok(())
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
                if let Expr::Ident(n) = self.prog.expr(e) {
                    if self.enum_names.contains(n) {
                        let n = n.clone();
                        e = self.alloc(Expr::EnumVal(n, f));
                        continue;
                    }
                }
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
