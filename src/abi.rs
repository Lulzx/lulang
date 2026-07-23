use std::collections::BTreeMap;
use std::path::Path;

#[derive(Clone, Debug)]
enum Json {
    Null,
    Bool,
    Number(i64),
    String(String),
    Array(Vec<Json>),
    Object(BTreeMap<String, Json>),
}

struct Parser<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            bytes: source.as_bytes(),
            offset: 0,
        }
    }

    fn parse(mut self) -> Result<Json, String> {
        let value = self.value()?;
        self.space();
        if self.offset != self.bytes.len() {
            return Err(format!("unexpected input at byte {}", self.offset));
        }
        Ok(value)
    }

    fn space(&mut self) {
        while self
            .bytes
            .get(self.offset)
            .is_some_and(|byte| byte.is_ascii_whitespace())
        {
            self.offset += 1;
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        self.space();
        match self.bytes.get(self.offset).copied() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => self.string().map(Json::String),
            Some(b'-' | b'0'..=b'9') => self.number().map(Json::Number),
            Some(b't') => {
                self.keyword(b"true")?;
                Ok(Json::Bool)
            }
            Some(b'f') => {
                self.keyword(b"false")?;
                Ok(Json::Bool)
            }
            Some(b'n') => {
                self.keyword(b"null")?;
                Ok(Json::Null)
            }
            Some(byte) => Err(format!(
                "unexpected character `{}` at byte {}",
                byte as char, self.offset
            )),
            None => Err("unexpected end of JSON".into()),
        }
    }

    fn keyword(&mut self, keyword: &[u8]) -> Result<(), String> {
        if self.bytes.get(self.offset..self.offset + keyword.len()) == Some(keyword) {
            self.offset += keyword.len();
            Ok(())
        } else {
            Err(format!("invalid token at byte {}", self.offset))
        }
    }

    fn object(&mut self) -> Result<Json, String> {
        self.offset += 1;
        let mut values = BTreeMap::new();
        loop {
            self.space();
            if self.bytes.get(self.offset) == Some(&b'}') {
                self.offset += 1;
                return Ok(Json::Object(values));
            }
            let key = self.string()?;
            self.space();
            if self.bytes.get(self.offset) != Some(&b':') {
                return Err(format!("expected `:` at byte {}", self.offset));
            }
            self.offset += 1;
            let value = self.value()?;
            if values.insert(key.clone(), value).is_some() {
                return Err(format!("duplicate JSON key `{key}`"));
            }
            self.space();
            match self.bytes.get(self.offset) {
                Some(b',') => self.offset += 1,
                Some(b'}') => {
                    self.offset += 1;
                    return Ok(Json::Object(values));
                }
                _ => return Err(format!("expected `,` or `}}` at byte {}", self.offset)),
            }
        }
    }

    fn array(&mut self) -> Result<Json, String> {
        self.offset += 1;
        let mut values = Vec::new();
        loop {
            self.space();
            if self.bytes.get(self.offset) == Some(&b']') {
                self.offset += 1;
                return Ok(Json::Array(values));
            }
            values.push(self.value()?);
            self.space();
            match self.bytes.get(self.offset) {
                Some(b',') => self.offset += 1,
                Some(b']') => {
                    self.offset += 1;
                    return Ok(Json::Array(values));
                }
                _ => return Err(format!("expected `,` or `]` at byte {}", self.offset)),
            }
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.space();
        if self.bytes.get(self.offset) != Some(&b'"') {
            return Err(format!("expected string at byte {}", self.offset));
        }
        self.offset += 1;
        let mut value = String::new();
        while let Some(byte) = self.bytes.get(self.offset).copied() {
            self.offset += 1;
            match byte {
                b'"' => return Ok(value),
                b'\\' => {
                    let escaped = self
                        .bytes
                        .get(self.offset)
                        .copied()
                        .ok_or_else(|| "unterminated JSON escape".to_string())?;
                    self.offset += 1;
                    match escaped {
                        b'"' => value.push('"'),
                        b'\\' => value.push('\\'),
                        b'/' => value.push('/'),
                        b'b' => value.push('\u{0008}'),
                        b'f' => value.push('\u{000c}'),
                        b'n' => value.push('\n'),
                        b'r' => value.push('\r'),
                        b't' => value.push('\t'),
                        b'u' => {
                            let code = self.hex4()?;
                            let ch = char::from_u32(code)
                                .ok_or_else(|| format!("invalid Unicode escape {code:04x}"))?;
                            value.push(ch);
                        }
                        _ => return Err(format!("invalid escape at byte {}", self.offset - 1)),
                    }
                }
                0..=31 => return Err("control character in JSON string".into()),
                _ if byte.is_ascii() => value.push(byte as char),
                _ => {
                    let start = self.offset - 1;
                    let width =
                        utf8_width(byte).ok_or_else(|| format!("invalid UTF-8 at byte {start}"))?;
                    let end = start + width;
                    let text = std::str::from_utf8(
                        self.bytes
                            .get(start..end)
                            .ok_or_else(|| "truncated UTF-8 in JSON string".to_string())?,
                    )
                    .map_err(|_| format!("invalid UTF-8 at byte {start}"))?;
                    value.push_str(text);
                    self.offset = end;
                }
            }
        }
        Err("unterminated JSON string".into())
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let mut value = 0;
        for _ in 0..4 {
            let byte = self
                .bytes
                .get(self.offset)
                .copied()
                .ok_or_else(|| "truncated Unicode escape".to_string())?;
            self.offset += 1;
            value = value * 16
                + match byte {
                    b'0'..=b'9' => (byte - b'0') as u32,
                    b'a'..=b'f' => (byte - b'a' + 10) as u32,
                    b'A'..=b'F' => (byte - b'A' + 10) as u32,
                    _ => return Err(format!("invalid hex digit at byte {}", self.offset - 1)),
                };
        }
        Ok(value)
    }

    fn number(&mut self) -> Result<i64, String> {
        let start = self.offset;
        if self.bytes.get(self.offset) == Some(&b'-') {
            self.offset += 1;
        }
        while self
            .bytes
            .get(self.offset)
            .is_some_and(|byte| byte.is_ascii_digit())
        {
            self.offset += 1;
        }
        let text = std::str::from_utf8(&self.bytes[start..self.offset]).unwrap();
        text.parse()
            .map_err(|_| format!("invalid integer `{text}` at byte {start}"))
    }
}

fn utf8_width(first: u8) -> Option<usize> {
    match first {
        0xc2..=0xdf => Some(2),
        0xe0..=0xef => Some(3),
        0xf0..=0xf4 => Some(4),
        _ => None,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Field {
    pub name: String,
    pub ty: String,
}

#[derive(Clone, Debug)]
pub(crate) struct Function {
    pub params: Vec<Field>,
    pub ret: String,
}

#[derive(Clone, Debug)]
pub(crate) struct Manifest {
    pub library: String,
    pub enums: BTreeMap<String, Vec<String>>,
    pub records: BTreeMap<String, Vec<Field>>,
    pub exports: BTreeMap<String, Function>,
}

fn object(value: &Json) -> Result<&BTreeMap<String, Json>, String> {
    match value {
        Json::Object(value) => Ok(value),
        _ => Err("expected JSON object".into()),
    }
}

fn array(value: &Json) -> Result<&[Json], String> {
    match value {
        Json::Array(value) => Ok(value),
        _ => Err("expected JSON array".into()),
    }
}

fn string(value: &Json) -> Result<&str, String> {
    match value {
        Json::String(value) => Ok(value),
        _ => Err("expected JSON string".into()),
    }
}

fn member<'a>(object: &'a BTreeMap<String, Json>, name: &str) -> Result<&'a Json, String> {
    object
        .get(name)
        .ok_or_else(|| format!("manifest is missing `{name}`"))
}

fn fields(value: &Json, context: &str) -> Result<Vec<Field>, String> {
    array(value)?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let value = object(value).map_err(|error| format!("{context}[{index}]: {error}"))?;
            Ok(Field {
                name: string(member(value, "name")?)?.to_string(),
                ty: string(member(value, "type")?)?.to_string(),
            })
        })
        .collect()
}

impl Manifest {
    pub fn parse(source: &str) -> Result<Self, String> {
        let json = Parser::new(source).parse()?;
        let root = object(&json)?;
        match member(root, "abi_version")? {
            Json::Number(1) => {}
            Json::Number(version) => {
                return Err(format!("unsupported ABI manifest version {version}"))
            }
            _ => return Err("`abi_version` must be an integer".into()),
        }
        let library = string(member(root, "library")?)?.to_string();
        let enums = object(member(root, "enums")?)?
            .iter()
            .map(|(name, values)| {
                let values = array(values)?
                    .iter()
                    .map(|value| string(value).map(str::to_string))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok((name.clone(), values))
            })
            .collect::<Result<_, String>>()?;
        let records = object(member(root, "c_layout_records")?)?
            .iter()
            .map(|(name, value)| Ok((name.clone(), fields(value, name)?)))
            .collect::<Result<_, String>>()?;
        let mut exports = BTreeMap::new();
        for (index, value) in array(member(root, "exports")?)?.iter().enumerate() {
            let value = object(value).map_err(|error| format!("exports[{index}]: {error}"))?;
            let name = string(member(value, "name")?)?.to_string();
            let function = Function {
                params: fields(member(value, "params")?, &format!("export `{name}` params"))?,
                ret: string(member(value, "ret")?)?.to_string(),
            };
            if exports.insert(name.clone(), function).is_some() {
                return Err(format!("duplicate export `{name}`"));
            }
        }
        Ok(Self {
            library,
            enums,
            records,
            exports,
        })
    }
}

pub(crate) fn load(path: &Path) -> Result<Manifest, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    Manifest::parse(&source).map_err(|error| format!("cannot parse {}: {error}", path.display()))
}

#[derive(Default)]
struct Report {
    breaking: Vec<String>,
    notices: Vec<String>,
}

fn compare(old: &Manifest, new: &Manifest) -> Report {
    let mut report = Report::default();
    if old.library != new.library {
        report.breaking.push(format!(
            "library changed from `{}` to `{}`",
            old.library, new.library
        ));
    }
    compare_named_sequences("enum", &old.enums, &new.enums, &mut report);
    compare_named_sequences("C-layout record", &old.records, &new.records, &mut report);

    for (name, old_function) in &old.exports {
        let Some(new_function) = new.exports.get(name) else {
            report.breaking.push(format!("export `{name}` was removed"));
            continue;
        };
        if old_function.ret != new_function.ret {
            report.breaking.push(format!(
                "export `{name}` return changed from `{}` to `{}`",
                old_function.ret, new_function.ret
            ));
        }
        if old_function.params.len() != new_function.params.len() {
            report.breaking.push(format!(
                "export `{name}` parameter count changed from {} to {}",
                old_function.params.len(),
                new_function.params.len()
            ));
        }
        for (index, (old_param, new_param)) in old_function
            .params
            .iter()
            .zip(&new_function.params)
            .enumerate()
        {
            if old_param.ty != new_param.ty {
                report.breaking.push(format!(
                    "export `{name}` parameter {} type changed from `{}` to `{}`",
                    index + 1,
                    old_param.ty,
                    new_param.ty
                ));
            } else if old_param.name != new_param.name {
                report.notices.push(format!(
                    "export `{name}` parameter {} was renamed from `{}` to `{}`",
                    index + 1,
                    old_param.name,
                    new_param.name
                ));
            }
        }
    }
    for name in new.exports.keys() {
        if !old.exports.contains_key(name) {
            report.notices.push(format!("export `{name}` was added"));
        }
    }
    report
}

fn compare_named_sequences<T: Eq>(
    kind: &str,
    old: &BTreeMap<String, Vec<T>>,
    new: &BTreeMap<String, Vec<T>>,
    report: &mut Report,
) {
    for (name, old_items) in old {
        let Some(new_items) = new.get(name) else {
            report.breaking.push(format!("{kind} `{name}` was removed"));
            continue;
        };
        if new_items.len() < old_items.len() || new_items[..old_items.len()] != old_items[..] {
            report
                .breaking
                .push(format!("{kind} `{name}` changed existing layout or tags"));
        } else if new_items.len() > old_items.len() {
            if kind == "C-layout record" {
                report
                    .breaking
                    .push(format!("{kind} `{name}` gained fields and changed size"));
            } else {
                report
                    .notices
                    .push(format!("{kind} `{name}` gained values"));
            }
        }
    }
    for name in new.keys() {
        if !old.contains_key(name) {
            report.notices.push(format!("{kind} `{name}` was added"));
        }
    }
}

pub fn check(old_path: &Path, new_path: &Path) -> Result<bool, String> {
    let report = compare(&load(old_path)?, &load(new_path)?);
    for notice in &report.notices {
        println!("compatible: {notice}");
    }
    for breaking in &report.breaking {
        println!("breaking: {breaking}");
    }
    if report.breaking.is_empty() {
        println!("ABI compatible");
        Ok(true)
    } else {
        println!(
            "ABI incompatible: {} breaking change(s)",
            report.breaking.len()
        );
        Ok(false)
    }
}
