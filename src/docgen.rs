use crate::interp::{Interp, PropertyStatus};
use crate::ir::{Callee, InstKind, LoweredProgram};
use crate::llvm;
use lu_llvm::cheader;
use lu_syntax::ast;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub fn build(
    ir: &LoweredProgram,
    source: &str,
    source_path: &str,
    output: Option<&str>,
    property_runs: u32,
) -> Result<Vec<String>, String> {
    let project_root = project_root(source_path)?;
    let output = output
        .map(PathBuf::from)
        .unwrap_or_else(|| project_root.join("target/doc"));
    let functions_directory = output.join("functions");
    std::fs::create_dir_all(&functions_directory).map_err(|error| error.to_string())?;
    std::fs::write(output.join("style.css"), STYLE).map_err(|error| error.to_string())?;
    std::fs::write(output.join("source.lu"), source).map_err(|error| error.to_string())?;

    let ir_path = output.join("program.ll");
    llvm::emit_llvm(ir, source_path, ir_path.to_str())?;
    let header = cheader::emit_header(ir, "module")?;
    std::fs::write(output.join("module.h"), &header).map_err(|error| error.to_string())?;
    std::fs::write(
        output.join("module.json"),
        cheader::emit_manifest(ir, "module"),
    )
    .map_err(|error| error.to_string())?;

    let statuses = Interp::new(ir).property_statuses(property_runs)?;
    write_property_json(&output.join("properties.json"), &statuses)?;
    let descriptions = doc_comments(source);
    let program = ir.source();
    let title = project_title(&project_root, source_path);
    let history = read_history(&project_root.join("benchmarks/history.csv"))?;
    let c_signatures = c_signatures(&header);

    for (function_index, function) in program.fns.iter().enumerate() {
        let related = related_properties(ir, function_index as u32, &statuses);
        let description = descriptions
            .get(&function.name)
            .cloned()
            .unwrap_or_else(|| "No prose description was provided.".into());
        let signature = function_signature(function);
        let abi = c_signatures
            .get(&function.name)
            .cloned()
            .unwrap_or_else(|| "Internal function; no stable C ABI symbol.".into());
        let page = function_page(
            &title,
            &function.name,
            &signature,
            &description,
            &example_call(function),
            &related,
            &abi,
            &history,
        );
        std::fs::write(
            functions_directory.join(format!("{}.html", safe_name(&function.name))),
            page,
        )
        .map_err(|error| error.to_string())?;
    }

    let observatory = build_observatory(&project_root, &output, &history)?;
    std::fs::write(output.join("observatory.html"), observatory)
        .map_err(|error| error.to_string())?;
    std::fs::write(
        output.join("index.html"),
        index_page(&title, ir, &statuses, property_runs),
    )
    .map_err(|error| error.to_string())?;

    Ok(vec![
        output.join("index.html").to_string_lossy().into_owned(),
        output
            .join("observatory.html")
            .to_string_lossy()
            .into_owned(),
        ir_path.to_string_lossy().into_owned(),
    ])
}

fn index_page(
    title: &str,
    ir: &LoweredProgram,
    statuses: &[PropertyStatus],
    property_runs: u32,
) -> String {
    let mut functions = String::new();
    for function in &ir.source().fns {
        let _ = writeln!(
            functions,
            "<tr><td><a href=\"functions/{}.html\">{}</a></td><td><code>{}</code></td><td>{}</td></tr>",
            safe_name(&function.name),
            escape_html(&function.name),
            escape_html(&function_signature(function)),
            if function.exported { "exported" } else { "internal" }
        );
    }
    let mut properties = String::new();
    for status in statuses {
        let _ = writeln!(
            properties,
            "<tr><td>{}</td><td class=\"{}\">{}</td><td>{}</td></tr>",
            escape_html(&status.name),
            if status.passed { "pass" } else { "fail" },
            if status.passed { "PASS" } else { "FAIL" },
            status.runs
        );
    }
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>{0}</title><link rel=\"stylesheet\" href=\"style.css\"></head><body><header><h1>{0}</h1><p>Generated lulang API and executable claims.</p><nav><a href=\"source.lu\">source</a> · <a href=\"program.ll\">LLVM IR</a> · <a href=\"module.h\">C header</a> · <a href=\"module.json\">ABI manifest</a> · <a href=\"observatory.html\">benchmark observatory</a></nav></header><main><section><h2>Functions</h2><table><thead><tr><th>Name</th><th>Signature</th><th>Boundary</th></tr></thead><tbody>{1}</tbody></table></section><section><h2>Properties</h2><p>Executed deterministically for {3} generated cases per property while building these docs.</p><table><thead><tr><th>Property</th><th>Status</th><th>Runs</th></tr></thead><tbody>{2}</tbody></table></section></main><footer>Generated by <code>lu doc</code>.</footer></body></html>",
        escape_html(title),
        functions,
        properties,
        property_runs
    )
}

fn function_page(
    title: &str,
    name: &str,
    signature: &str,
    description: &str,
    example: &str,
    properties: &[PropertyStatus],
    abi: &str,
    history: &[HistoryRow],
) -> String {
    let mut property_rows = String::new();
    if properties.is_empty() {
        property_rows
            .push_str("<tr><td colspan=\"3\">No property reaches this function.</td></tr>");
    } else {
        for property in properties {
            let _ = writeln!(
                property_rows,
                "<tr><td>{}</td><td class=\"{}\">{}</td><td>{}</td></tr>",
                escape_html(&property.name),
                if property.passed { "pass" } else { "fail" },
                if property.passed { "PASS" } else { "FAIL" },
                property.runs
            );
        }
    }
    let mut benchmark_rows = String::new();
    for row in history {
        let _ = writeln!(
            benchmark_rows,
            "<tr><td>{}</td><td>{:.3}</td><td>{:.3}</td><td>{:.3}</td><td>{:.3}</td></tr>",
            escape_html(&row.label),
            row.interp_ms,
            row.jit_ms,
            row.aot_ms,
            row.compile_ms
        );
    }
    if benchmark_rows.is_empty() {
        benchmark_rows.push_str(
            "<tr><td colspan=\"5\">No local history yet. Run <code>lu bench</code>.</td></tr>",
        );
    }
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>{0} — {1}</title><link rel=\"stylesheet\" href=\"../style.css\"></head><body><header><p><a href=\"../index.html\">← {0}</a></p><h1>{1}</h1><pre>{2}</pre></header><main><section><h2>Description</h2><p>{3}</p></section><section><h2>Example</h2><pre>{4}</pre></section><section><h2>Executable properties</h2><table><thead><tr><th>Property</th><th>Status</th><th>Runs</th></tr></thead><tbody>{5}</tbody></table></section><section><h2>C ABI</h2><pre>{6}</pre></section><section><h2>Local benchmark history (ms)</h2><table><thead><tr><th>Program</th><th>Interp</th><th>JIT</th><th>AOT</th><th>Compile</th></tr></thead><tbody>{7}</tbody></table></section></main></body></html>",
        escape_html(title),
        escape_html(name),
        escape_html(signature),
        escape_html(description),
        escape_html(example),
        property_rows,
        escape_html(abi),
        benchmark_rows
    )
}

fn related_properties(
    ir: &LoweredProgram,
    target: u32,
    statuses: &[PropertyStatus],
) -> Vec<PropertyStatus> {
    let status_by_name = statuses
        .iter()
        .map(|status| (status.name.as_str(), status))
        .collect::<BTreeMap<_, _>>();
    ir.properties
        .iter()
        .filter(|property| reaches_function(ir, property.function, target, &mut BTreeSet::new()))
        .filter_map(|property| {
            status_by_name
                .get(property.name.as_str())
                .map(|status| (*status).clone())
        })
        .collect()
}

fn reaches_function(
    ir: &LoweredProgram,
    current: u32,
    target: u32,
    seen: &mut BTreeSet<u32>,
) -> bool {
    if current == target {
        return true;
    }
    if !seen.insert(current) {
        return false;
    }
    ir.functions[current as usize].blocks.iter().any(|block| {
        block.instructions.iter().any(|instruction| {
            if let InstKind::Call {
                callee: Callee::Function(next),
                ..
            } = &instruction.kind
            {
                reaches_function(ir, *next, target, seen)
            } else {
                false
            }
        })
    })
}

fn function_signature(function: &ast::FnDecl) -> String {
    let params = function
        .params
        .iter()
        .zip(&function.inouts)
        .map(|((name, ty), inout)| {
            format!("{}{}: {}", if *inout { "inout " } else { "" }, name, ty)
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{}fn {}({}): {}",
        if function.exported { "export " } else { "" },
        function.name,
        params,
        function.ret
    )
}

fn example_call(function: &ast::FnDecl) -> String {
    let args = function
        .params
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "// Define values for the parameters above.\n{}({})",
        function.name, args
    )
}

fn doc_comments(source: &str) -> BTreeMap<String, String> {
    let mut docs = Vec::new();
    let mut result = BTreeMap::new();
    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(comment) = trimmed.strip_prefix("///") {
            docs.push(comment.trim().to_string());
            continue;
        }
        let declaration = trimmed
            .strip_prefix("export ")
            .unwrap_or(trimmed)
            .strip_prefix("fn ")
            .or_else(|| trimmed.strip_prefix("property "));
        let mut documented_name = None;
        if let Some(declaration) = declaration {
            if let Some(name) = declaration.split('(').next() {
                documented_name = Some(name.trim().to_string());
            }
        } else {
            documented_name = operator_doc_name(trimmed);
        }
        if let Some(name) = documented_name {
            if !docs.is_empty() {
                result.insert(name, docs.join(" "));
            }
        }
        if !trimmed.is_empty() {
            docs.clear();
        }
    }
    result
}

fn operator_doc_name(line: &str) -> Option<String> {
    let rest = line.strip_prefix("operator")?;
    let glyphs = if rest.starts_with(char::is_whitespace) {
        let rest = rest.trim_start();
        let open = rest.chars().next()?.to_string();
        let close = rest
            .split_once(')')?
            .1
            .trim_start()
            .chars()
            .next()?
            .to_string();
        vec![open, close]
    } else {
        let after_first_parameter = rest.split_once(')')?.1.trim_start();
        vec![after_first_parameter.split_whitespace().next()?.to_string()]
    };
    let mut name = String::from("operator");
    for glyph in glyphs {
        for scalar in glyph.chars() {
            let _ = write!(name, "_u{:x}", scalar as u32);
        }
    }
    Some(name)
}

fn c_signatures(header: &str) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    let mut export = None;
    for line in header.lines() {
        if let Some(comment) = line.strip_prefix("/* export fn ") {
            export = comment.split('(').next().map(String::from);
        } else if let Some(name) = export.take() {
            result.insert(name, line.to_string());
        }
    }
    result
}

fn write_property_json(path: &Path, statuses: &[PropertyStatus]) -> Result<(), String> {
    let mut output = String::from("{\n  \"properties\": [");
    for (index, status) in statuses.iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        let _ = write!(
            output,
            "\n    {{\"name\": \"{}\", \"passed\": {}, \"runs\": {}}}",
            escape_json(&status.name),
            status.passed,
            status.runs
        );
    }
    if !statuses.is_empty() {
        output.push('\n');
    }
    output.push_str("  ]\n}\n");
    std::fs::write(path, output).map_err(|error| error.to_string())
}

#[derive(Clone, Debug)]
struct HistoryRow {
    label: String,
    interp_ms: f64,
    jit_ms: f64,
    aot_ms: f64,
    compile_ms: f64,
}

fn read_history(path: &Path) -> Result<Vec<HistoryRow>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let source = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    source
        .lines()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let fields = line.split(',').collect::<Vec<_>>();
            if fields.len() != 7 {
                return Err(format!("invalid benchmark history row `{line}`"));
            }
            Ok(HistoryRow {
                label: fields[1].into(),
                interp_ms: fields[3].parse().map_err(|_| "invalid interp time")?,
                jit_ms: fields[4].parse().map_err(|_| "invalid JIT time")?,
                aot_ms: fields[5].parse().map_err(|_| "invalid AOT time")?,
                compile_ms: fields[6].parse().map_err(|_| "invalid compile time")?,
            })
        })
        .collect()
}

fn build_observatory(
    project_root: &Path,
    output: &Path,
    history: &[HistoryRow],
) -> Result<String, String> {
    let observatory = project_root
        .ancestors()
        .map(|root| root.join("benchmarks/observatory.tsv"))
        .find(|path| path.exists());
    let mut rows = String::new();
    let mut environment_link =
        "No measurement environment was published for this record.".to_string();
    let sources = output.join("sources");
    std::fs::create_dir_all(&sources).map_err(|error| error.to_string())?;
    if let Some(path) = observatory {
        let repository = path.parent().and_then(Path::parent).unwrap_or(project_root);
        let source = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
        let mut lines = source.lines();
        let headers = lines
            .next()
            .unwrap_or_default()
            .split('\t')
            .enumerate()
            .map(|(index, name)| (name, index))
            .collect::<BTreeMap<_, _>>();
        for (index, line) in lines.enumerate() {
            let fields = line.split('\t').collect::<Vec<_>>();
            let field = |name: &str| {
                headers
                    .get(name)
                    .and_then(|index| fields.get(*index))
                    .copied()
                    .unwrap_or("")
            };
            if field("kernel").is_empty() {
                continue;
            }
            let mut links = Vec::new();
            for (language, column) in [
                ("lu", "lu_source"),
                ("C++", "cpp_source"),
                ("Rust", "rust_source"),
                ("Julia", "julia_source"),
                ("NumPy", "numpy_source"),
                ("JS", "js_source"),
            ] {
                let source_path = field(column);
                if source_path.is_empty() {
                    continue;
                }
                let input = repository.join(source_path);
                if input.exists() {
                    let file = format!(
                        "{}-{}-{}",
                        safe_name(field("kernel")),
                        index,
                        input.file_name().unwrap().to_string_lossy()
                    );
                    std::fs::copy(&input, sources.join(&file))
                        .map_err(|error| error.to_string())?;
                    links.push(format!("<a href=\"sources/{file}\">{language}</a>"));
                }
            }
            let _ = writeln!(
                rows,
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                escape_html(field("kernel")),
                display_measurement(field("lulang_aot_ms")),
                display_measurement(field("lulang_jit_ms")),
                display_measurement(field("lulang_selfhost_ms")),
                display_measurement(field("cpp_o3_ms")),
                display_measurement(field("cpp_fast_ms")),
                display_measurement(field("rust_ms")),
                display_measurement(field("julia_ms")),
                display_measurement(field("numpy_ms")),
                display_measurement(field("js_ms")),
                links.join(" · "),
                escape_html(field("assumptions_layout"))
            );
        }
        let environment = path.with_file_name("environment.json");
        if environment.exists() {
            std::fs::copy(&environment, output.join("environment.json"))
                .map_err(|error| error.to_string())?;
            environment_link =
                "<a href=\"environment.json\">Measurement environment</a>".to_string();
        }
    }
    if rows.is_empty() {
        rows.push_str(
            "<tr><td colspan=\"12\">No cross-language observations were found.</td></tr>",
        );
    }
    let mut local = String::new();
    for row in history {
        let _ = writeln!(
            local,
            "<tr><td>{}</td><td>{:.3}</td><td>{:.3}</td><td>{:.3}</td><td>{:.3}</td></tr>",
            escape_html(&row.label),
            row.interp_ms,
            row.jit_ms,
            row.aot_ms,
            row.compile_ms
        );
    }
    if local.is_empty() {
        local.push_str(
            "<tr><td colspan=\"5\">Run <code>lu bench</code> to start local history.</td></tr>",
        );
    }
    Ok(format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>lulang benchmark observatory</title><link rel=\"stylesheet\" href=\"style.css\"></head><body><header><p><a href=\"index.html\">← API docs</a></p><h1>Benchmark observatory</h1><p>Numbers are evidence with their source and semantic assumptions attached.</p></header><main><section><h2>Cross-language record (ms, lower is better)</h2><table><thead><tr><th>Kernel</th><th>lulang AOT</th><th>lulang JIT</th><th>selfhost</th><th>C++ -O3</th><th>C++ fast</th><th>Rust</th><th>Julia</th><th>NumPy</th><th>JS</th><th>Sources</th><th>Assumptions / layout</th></tr></thead><tbody>{rows}</tbody></table><p>{environment_link}; an em dash means the runtime was unavailable.</p></section><section><h2>Local continuous history</h2><table><thead><tr><th>Program</th><th>Interp</th><th>JIT</th><th>AOT</th><th>Compile</th></tr></thead><tbody>{local}</tbody></table></section><section><h2>Ablations</h2><p><code>LU_MATH</code> · <code>LU_IFCONV</code> · <code>LU_LICM</code> · <code>LU_SIMD</code> · <code>LU_LAYOUT</code></p><p>Generated LLVM is published with every documentation build: <a href=\"program.ll\">program.ll</a>.</p></section></main></body></html>"
    ))
}

fn display_measurement(value: &str) -> String {
    if value.trim().is_empty() {
        "—".into()
    } else {
        escape_html(value)
    }
}

fn project_root(source_path: &str) -> Result<PathBuf, String> {
    let source = Path::new(source_path);
    let start = source
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    for directory in start.ancestors() {
        if directory.join("lu.toml").exists() {
            return Ok(directory.to_path_buf());
        }
    }
    std::env::current_dir().map_err(|error| error.to_string())
}

fn project_title(root: &Path, source_path: &str) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .or_else(|| {
            Path::new(source_path)
                .file_stem()
                .and_then(|name| name.to_str())
        })
        .unwrap_or("lulang module")
        .to_string()
}

fn safe_name(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn escape_json(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

const STYLE: &str = r#"
:root { color-scheme: light; font-family: ui-monospace, SFMono-Regular, Menlo, monospace; color: #111; background: #fff; }
body { max-width: 1120px; margin: 0 auto; padding: 36px 24px 72px; line-height: 1.5; }
header, main, footer { border-top: 1px solid #bbb; padding-top: 18px; margin-top: 18px; }
h1 { font-size: 28px; margin: 0 0 8px; } h2 { font-size: 18px; margin-top: 30px; }
a { color: #0645ad; } code, pre { background: #f5f5f5; } pre { padding: 14px; overflow: auto; border: 1px solid #ddd; }
table { width: 100%; border-collapse: collapse; font-size: 13px; }
th, td { text-align: left; padding: 8px 10px; border: 1px solid #ccc; vertical-align: top; }
th { background: #f2f2f2; } .pass { color: #086b27; font-weight: 700; } .fail { color: #a00; font-weight: 700; }
nav { margin: 14px 0; } footer { color: #555; font-size: 12px; }
"#;
