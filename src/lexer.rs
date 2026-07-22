#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Ident(String),
    Int(i64),
    Float(f64),
    Str(String),
    Sym(String),
    Newline,
    Eof,
}

pub fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let cs: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < cs.len() {
        let c = cs[i];
        if c == ' ' || c == '\t' || c == '\r' {
            i += 1;
        } else if c == '\n' {
            out.push(Tok::Newline);
            i += 1;
        } else if c == '/' && i + 1 < cs.len() && cs[i + 1] == '/' {
            while i < cs.len() && cs[i] != '\n' {
                i += 1;
            }
        } else if c.is_ascii_alphabetic() || c == '_' {
            let s = i;
            while i < cs.len() && (cs[i].is_ascii_alphanumeric() || cs[i] == '_') {
                i += 1;
            }
            out.push(Tok::Ident(cs[s..i].iter().collect()));
        } else if c.is_ascii_digit() {
            let s = i;
            while i < cs.len() && cs[i].is_ascii_digit() {
                i += 1;
            }
            if i + 1 < cs.len() && cs[i] == '.' && cs[i + 1].is_ascii_digit() {
                i += 1;
                while i < cs.len() && cs[i].is_ascii_digit() {
                    i += 1;
                }
                let t: String = cs[s..i].iter().collect();
                out.push(Tok::Float(t.parse().map_err(|e| format!("bad float: {e}"))?));
            } else {
                let t: String = cs[s..i].iter().collect();
                out.push(Tok::Int(t.parse().map_err(|e| format!("bad int: {e}"))?));
            }
        } else if c == '\'' {
            // byte-char literal: 'a', '\n', '\'' — value is the byte as i64
            i += 1;
            if i >= cs.len() {
                return Err("unterminated char literal".into());
            }
            let mut ch = cs[i];
            if ch == '\\' {
                i += 1;
                if i >= cs.len() {
                    return Err("unterminated char literal".into());
                }
                ch = match cs[i] {
                    'n' => '\n',
                    't' => '\t',
                    x => x,
                };
            }
            if !ch.is_ascii() {
                return Err("char literal must be a single byte".into());
            }
            i += 1;
            if i >= cs.len() || cs[i] != '\'' {
                return Err("unterminated char literal".into());
            }
            i += 1;
            out.push(Tok::Int(ch as i64));
        } else if c == '"' {
            i += 1;
            let mut s = String::new();
            while i < cs.len() && cs[i] != '"' {
                if cs[i] == '\\' && i + 1 < cs.len() {
                    i += 1;
                    s.push(match cs[i] {
                        'n' => '\n',
                        't' => '\t',
                        x => x,
                    });
                } else {
                    s.push(cs[i]);
                }
                i += 1;
            }
            if i >= cs.len() {
                return Err("unterminated string literal".into());
            }
            i += 1;
            out.push(Tok::Str(s));
        } else {
            if i + 1 < cs.len() {
                let two: String = [c, cs[i + 1]].iter().collect();
                if ["==", "!=", "<=", ">=", "~=", "..", "|>"].contains(&two.as_str()) {
                    out.push(Tok::Sym(two));
                    i += 2;
                    continue;
                }
            }
            out.push(Tok::Sym(c.to_string()));
            i += 1;
        }
    }
    out.push(Tok::Eof);
    Ok(out)
}
