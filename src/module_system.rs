use lu_syntax::ast::{Expr, ExprId, FnDecl, Program, Stmt, StmtId};
use lu_syntax::{lexer, parser};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Clone, Debug)]
pub struct SourceFile {
    pub module: String,
    pub path: String,
    pub source: String,
    pub root: bool,
}

#[derive(Default)]
struct Symbols {
    functions: BTreeMap<String, BTreeSet<String>>,
    types: BTreeMap<String, BTreeSet<String>>,
    enums: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Clone, Debug, Default)]
struct Imports {
    // local namespace -> canonical package module
    aliases: BTreeMap<String, String>,
}

impl Imports {
    fn targets(&self) -> impl Iterator<Item = &String> {
        self.aliases.values()
    }

    fn resolve_namespace<'a>(&'a self, namespace: &'a str) -> &'a str {
        self.aliases
            .get(namespace)
            .map(String::as_str)
            .unwrap_or(namespace)
    }

    fn contains_target(&self, module: &str) -> bool {
        self.aliases.values().any(|target| target == module)
    }
}

const INTERNAL_PREFIX: &str = "__lu_module_";

pub fn parse_and_link(files: &[SourceFile]) -> Result<Program, String> {
    if files.is_empty() {
        return Err("module graph contains no source files".into());
    }
    let mut parsed = Vec::new();
    let mut known_types = BTreeSet::new();
    let mut known_enums = BTreeSet::new();
    let mut known_infix_precedence = BTreeMap::<String, HashMap<String, u8>>::new();
    let mut known_circum = BTreeMap::<String, HashMap<String, (String, String)>>::new();

    for file in files {
        let tokens = lexer::lex(&file.source).map_err(|error| format!("{}: {error}", file.path))?;
        let imports = source_imports(&tokens).map_err(|error| format!("{}: {error}", file.path))?;
        let mut visible_modules = imports.targets().cloned().collect::<BTreeSet<_>>();
        visible_modules.insert(file.module.clone());
        let namespaces = imports.aliases.keys().cloned().collect::<Vec<_>>();
        let visible_types = visible_names(&known_types, &visible_modules);
        let visible_enums = visible_names(&known_enums, &visible_modules);
        let mut visible_infix = HashMap::new();
        let mut visible_circum = HashMap::new();
        for visible in &visible_modules {
            if let Some(operators) = known_infix_precedence.get(visible) {
                visible_infix.extend(operators.clone());
            }
            if let Some(operators) = known_circum.get(visible) {
                visible_circum.extend(operators.clone());
            }
        }
        let mut parser = parser::Parser::new(tokens).with_prelude(
            namespaces,
            visible_types,
            visible_enums,
            &visible_infix,
            &visible_circum,
        );
        parser
            .parse()
            .map_err(|error| format!("{}: {error}", file.path))?;
        for ty in &parser.prog.types {
            known_types.insert(format!("{}.{}", file.module, ty.name));
        }
        for en in &parser.prog.enums {
            known_enums.insert(format!("{}.{}", file.module, en.name));
        }
        for (glyph, precedence) in &parser.prog.infix_precedence {
            known_infix_precedence
                .entry(file.module.clone())
                .or_default()
                .insert(glyph.clone(), *precedence);
        }
        for (open, pair) in &parser.prog.circum_ops {
            known_circum
                .entry(file.module.clone())
                .or_default()
                .insert(open.clone(), pair.clone());
        }
        parsed.push((file, parser.prog));
    }

    let symbols = collect_symbols(&parsed)?;
    let root_module = files
        .iter()
        .find(|file| file.root)
        .map(|file| file.module.as_str())
        .ok_or("module graph has no root module")?;
    let mut linked = Program::default();
    for (file, mut program) in parsed {
        let imports = imports_from_declarations(&program.uses)
            .map_err(|error| format!("{}: {error}", file.path))?;
        rewrite_program(&mut program, &file.module, root_module, &imports, &symbols)?;
        merge_program(&mut linked, program, file.root, &file.path)?;
    }
    Ok(linked)
}

fn source_imports(tokens: &[lexer::Tok]) -> Result<Imports, String> {
    let declarations = parser::scan_imports(tokens)?;
    imports_from_declarations(&declarations)
}

fn imports_from_declarations(declarations: &[lu_syntax::ast::UseDecl]) -> Result<Imports, String> {
    let mut imports = Imports::default();
    for declaration in declarations {
        if let Some(previous) = imports
            .aliases
            .insert(declaration.alias.clone(), declaration.module.clone())
        {
            if previous != declaration.module {
                return Err(format!(
                    "import alias `{}` refers to both `{previous}` and `{}`",
                    declaration.alias, declaration.module
                ));
            }
        }
    }
    Ok(imports)
}

fn visible_names(names: &BTreeSet<String>, imports: &BTreeSet<String>) -> Vec<String> {
    let mut short_counts = BTreeMap::<&str, usize>::new();
    for name in names {
        let Some((module, short)) = name.split_once('.') else {
            continue;
        };
        if imports.contains(module) {
            *short_counts.entry(short).or_default() += 1;
        }
    }
    let mut visible = Vec::new();
    for name in names {
        let Some((module, short)) = name.split_once('.') else {
            continue;
        };
        if imports.contains(module) {
            visible.push(name.clone());
            if short_counts.get(short) == Some(&1) {
                visible.push(short.to_string());
            }
        }
    }
    visible
}

fn collect_symbols(parsed: &[(&SourceFile, Program)]) -> Result<Symbols, String> {
    let mut symbols = Symbols::default();
    for (file, program) in parsed {
        let functions = symbols.functions.entry(file.module.clone()).or_default();
        for function in program.fns.iter().chain(&program.props) {
            reject_reserved_name(&file.path, &function.name)?;
            if !functions.insert(function.name.clone()) {
                return Err(format!(
                    "{}: duplicate function `{}` in module `{}`",
                    file.path, function.name, file.module
                ));
            }
        }
        let types = symbols.types.entry(file.module.clone()).or_default();
        for ty in &program.types {
            reject_reserved_name(&file.path, &ty.name)?;
            if !types.insert(ty.name.clone()) {
                return Err(format!(
                    "{}: duplicate type `{}` in module `{}`",
                    file.path, ty.name, file.module
                ));
            }
        }
        let enums = symbols.enums.entry(file.module.clone()).or_default();
        for en in &program.enums {
            reject_reserved_name(&file.path, &en.name)?;
            if !enums.insert(en.name.clone()) {
                return Err(format!(
                    "{}: duplicate enum `{}` in module `{}`",
                    file.path, en.name, file.module
                ));
            }
        }
    }
    Ok(symbols)
}

fn reject_reserved_name(path: &str, name: &str) -> Result<(), String> {
    if name.starts_with(INTERNAL_PREFIX) {
        Err(format!(
            "{path}: declaration `{name}` uses reserved module-linker prefix `{INTERNAL_PREFIX}`"
        ))
    } else {
        Ok(())
    }
}

fn internal_name(module: &str, root: &str, name: &str) -> String {
    if module == root {
        name.to_string()
    } else {
        format!("{INTERNAL_PREFIX}{}_{module}_{name}", module.len())
    }
}

fn resolve_name(
    name: &str,
    current: &str,
    root: &str,
    imports: &Imports,
    table: &BTreeMap<String, BTreeSet<String>>,
) -> Result<String, String> {
    if let Some((namespace, member)) = name.split_once('.') {
        let module = imports.resolve_namespace(namespace);
        if module != current && !imports.contains_target(module) {
            return Err(format!(
                "module `{current}` must import namespace `{namespace}` before referring to `{name}`"
            ));
        }
        if table
            .get(module)
            .is_some_and(|members| members.contains(member))
        {
            return Ok(internal_name(module, root, member));
        }
        return Ok(name.to_string());
    }
    if table
        .get(current)
        .is_some_and(|members| members.contains(name))
    {
        return Ok(internal_name(current, root, name));
    }
    let matches = table
        .iter()
        .filter(|(module, members)| imports.contains_target(module) && members.contains(name))
        .map(|(module, _)| module.as_str())
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(name.to_string()),
        [module] => Ok(internal_name(module, root, name)),
        _ => Err(format!(
            "ambiguous imported name `{name}`; qualify it as `module.{name}`"
        )),
    }
}

fn rewrite_type(
    source: &str,
    module: &str,
    root: &str,
    imports: &Imports,
    symbols: &Symbols,
) -> Result<String, String> {
    let mut output = String::with_capacity(source.len());
    let mut identifier = String::new();
    let flush = |identifier: &mut String, output: &mut String| -> Result<(), String> {
        if !identifier.is_empty() {
            let name = resolve_type_name(identifier, module, root, imports, symbols)?;
            output.push_str(&name);
            identifier.clear();
        }
        Ok(())
    };
    for ch in source.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' {
            identifier.push(ch);
        } else {
            flush(&mut identifier, &mut output)?;
            output.push(ch);
        }
    }
    flush(&mut identifier, &mut output)?;
    Ok(output)
}

fn resolve_type_name(
    name: &str,
    module: &str,
    root: &str,
    imports: &Imports,
    symbols: &Symbols,
) -> Result<String, String> {
    let table_contains = |table: &BTreeMap<String, BTreeSet<String>>| {
        if let Some((owner, member)) = name.split_once('.') {
            table
                .get(owner)
                .is_some_and(|members| members.contains(member))
        } else {
            table
                .get(module)
                .is_some_and(|members| members.contains(name))
                || table.iter().any(|(owner, members)| {
                    imports.contains_target(owner) && members.contains(name)
                })
        }
    };
    if table_contains(&symbols.types) {
        resolve_name(name, module, root, imports, &symbols.types)
    } else if table_contains(&symbols.enums) {
        resolve_name(name, module, root, imports, &symbols.enums)
    } else {
        Ok(name.to_string())
    }
}

fn rewrite_program(
    program: &mut Program,
    module: &str,
    root: &str,
    imports: &Imports,
    symbols: &Symbols,
) -> Result<(), String> {
    for expression in &mut program.exprs {
        match expression {
            Expr::Call(name, _) => {
                *name = resolve_name(name, module, root, imports, &symbols.functions)?;
            }
            Expr::Record(name, _) => {
                *name = resolve_name(name, module, root, imports, &symbols.types)?;
            }
            Expr::EnumVal(name, _) => {
                *name = resolve_name(name, module, root, imports, &symbols.enums)?;
            }
            _ => {}
        }
    }
    for function in &mut program.fns {
        rewrite_function(function, module, root, imports, symbols)?;
        function.name = internal_name(module, root, &function.name);
        if module != root {
            function.exported = false;
        }
    }
    for function in &mut program.props {
        rewrite_function(function, module, root, imports, symbols)?;
        function.name = internal_name(module, root, &function.name);
    }
    for external in &mut program.externs {
        for (_, ty) in &mut external.params {
            *ty = rewrite_type(ty, module, root, imports, symbols)?;
        }
        external.ret = rewrite_type(&external.ret, module, root, imports, symbols)?;
    }
    for ty in &mut program.types {
        for (_, field_type) in &mut ty.fields {
            *field_type = rewrite_type(field_type, module, root, imports, symbols)?;
        }
        ty.name = internal_name(module, root, &ty.name);
    }
    for en in &mut program.enums {
        en.name = internal_name(module, root, &en.name);
    }
    for function in program.infix_ops.values_mut() {
        *function = internal_name(module, root, function);
    }
    for (_, function) in program.circum_ops.values_mut() {
        *function = internal_name(module, root, function);
    }
    Ok(())
}

fn rewrite_function(
    function: &mut FnDecl,
    module: &str,
    root: &str,
    imports: &Imports,
    symbols: &Symbols,
) -> Result<(), String> {
    for (_, ty) in &mut function.params {
        *ty = rewrite_type(ty, module, root, imports, symbols)?;
    }
    function.ret = rewrite_type(&function.ret, module, root, imports, symbols)?;
    Ok(())
}

fn merge_program(
    destination: &mut Program,
    mut source: Program,
    root: bool,
    path: &str,
) -> Result<(), String> {
    let expr_offset = destination.exprs.len() as ExprId;
    let stmt_offset = destination.stmts.len() as StmtId;
    for expression in &mut source.exprs {
        remap_expr(expression, expr_offset);
    }
    for statement in &mut source.stmts {
        remap_stmt(statement, expr_offset, stmt_offset);
    }
    for function in source.fns.iter_mut().chain(&mut source.props) {
        for statement in &mut function.body {
            *statement += stmt_offset;
        }
    }
    if let Some(main) = &mut source.main {
        if !root {
            return Err(format!("{path}: dependency modules cannot declare `main`"));
        }
        if destination.main.is_some() {
            return Err(format!("{path}: root module has more than one `main`"));
        }
        for statement in main.iter_mut() {
            *statement += stmt_offset;
        }
        destination.main = Some(std::mem::take(main));
    }
    for (glyph, function) in source.infix_ops {
        if let Some(previous) = destination.infix_ops.insert(glyph.clone(), function) {
            return Err(format!(
                "{path}: operator `{glyph}` conflicts with `{previous}`"
            ));
        }
    }
    for (glyph, precedence) in source.infix_precedence {
        destination.infix_precedence.insert(glyph, precedence);
    }
    for (open, pair) in source.circum_ops {
        if destination.circum_ops.insert(open.clone(), pair).is_some() {
            return Err(format!("{path}: circumfix operator `{open}` is ambiguous"));
        }
    }
    destination.exprs.extend(source.exprs);
    destination.stmts.extend(source.stmts);
    destination.fns.extend(source.fns);
    destination.externs.extend(source.externs);
    destination.types.extend(source.types);
    destination.enums.extend(source.enums);
    destination.uses.extend(source.uses);
    destination.props.extend(source.props);
    Ok(())
}

fn remap_expr(expression: &mut Expr, offset: ExprId) {
    let add = |id: &mut ExprId| *id += offset;
    match expression {
        Expr::Bin(_, left, right) | Expr::Index(left, right) => {
            add(left);
            add(right);
        }
        Expr::Un(_, value) | Expr::Field(value, _) | Expr::Circum(_, value) => add(value),
        Expr::Call(_, arguments) | Expr::Array(arguments) => {
            for argument in arguments {
                add(argument);
            }
        }
        Expr::Record(_, fields) => {
            for (_, value) in fields {
                add(value);
            }
        }
        Expr::Sum { lo, hi, body, .. } => {
            add(lo);
            add(hi);
            add(body);
        }
        _ => {}
    }
}

fn remap_stmt(statement: &mut Stmt, expr_offset: ExprId, stmt_offset: StmtId) {
    let add_expr = |id: &mut ExprId| *id += expr_offset;
    let add_stmts = |statements: &mut Vec<StmtId>| {
        for statement in statements {
            *statement += stmt_offset;
        }
    };
    match statement {
        Stmt::Let(_, value) | Stmt::Var(_, value) | Stmt::Expr(value) => add_expr(value),
        Stmt::Assign(target, value) => {
            add_expr(target);
            add_expr(value);
        }
        Stmt::If(condition, yes, no) => {
            add_expr(condition);
            add_stmts(yes);
            add_stmts(no);
        }
        Stmt::For(_, lo, hi, body) => {
            add_expr(lo);
            add_expr(hi);
            add_stmts(body);
        }
        Stmt::While(condition, body) => {
            add_expr(condition);
            add_stmts(body);
        }
        Stmt::Return(Some(value)) => add_expr(value),
        Stmt::Return(None) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_aliases_are_structured_and_conflicts_are_rejected() {
        let tokens = lexer::lex("use geometry as geo // comment\nuse numerics\n").unwrap();
        let imports = source_imports(&tokens).unwrap();
        assert_eq!(imports.resolve_namespace("geo"), "geometry");
        assert_eq!(imports.resolve_namespace("numerics"), "numerics");
        let conflict = lexer::lex("use geometry as math\nuse numerics as math\n").unwrap();
        assert!(source_imports(&conflict).is_err());
        let invalid = lexer::lex("use geometry alias\n").unwrap();
        assert!(source_imports(&invalid).is_err());
    }

    #[test]
    fn module_mangling_cannot_confuse_delimiters_with_symbol_text() {
        let left = internal_name("a__b", "root", "c");
        let right = internal_name("a", "root", "b__c");
        assert_ne!(left, right);
        assert_eq!(left, "__lu_module_4_a__b_c");
        assert_eq!(right, "__lu_module_1_a_b__c");
    }
}
