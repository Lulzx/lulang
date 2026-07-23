use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[derive(Clone, Debug)]
struct Dependency {
    git: String,
    rev: String,
}

#[derive(Clone, Debug)]
struct Manifest {
    name: String,
    version: String,
    dependencies: BTreeMap<String, Dependency>,
}

#[derive(Clone, Debug)]
struct LockedPackage {
    name: String,
    git: String,
    rev: String,
    commit: String,
    tree: String,
}

pub struct LoadedProgram {
    pub label: String,
    pub source: String,
}

pub fn init(arguments: &[String]) -> Result<String, String> {
    if arguments.len() > 1 {
        return Err("usage: lu init [package-name]".into());
    }
    let directory = std::env::current_dir().map_err(|error| error.to_string())?;
    let default_name = directory
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("lulang-package");
    let name = arguments
        .first()
        .map(String::as_str)
        .unwrap_or(default_name);
    validate_package_name(name)?;
    let manifest_path = directory.join("lu.toml");
    if manifest_path.exists() {
        return Err(format!("{} already exists", manifest_path.display()));
    }
    let source_directory = directory.join("src");
    std::fs::create_dir_all(&source_directory).map_err(|error| error.to_string())?;
    std::fs::write(
        &manifest_path,
        render_manifest(&Manifest {
            name: name.into(),
            version: "0.1.0".into(),
            dependencies: BTreeMap::new(),
        }),
    )
    .map_err(|error| error.to_string())?;
    let main = source_directory.join("main.lu");
    if !main.exists() {
        std::fs::write(&main, "main {\n  print(\"hello from lulang\")\n}\n")
            .map_err(|error| error.to_string())?;
    }
    Ok(format!(
        "initialized package `{name}` in {}",
        directory.display()
    ))
}

pub fn add(arguments: &[String]) -> Result<String, String> {
    let Some(name) = arguments.first() else {
        return Err("usage: lu add <name> --git <url> --rev <revision>".into());
    };
    validate_package_name(name)?;
    let mut git = None;
    let mut rev = None;
    let mut index = 1;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--git" => {
                git = arguments.get(index + 1).cloned();
                index += 2;
            }
            "--rev" => {
                rev = arguments.get(index + 1).cloned();
                index += 2;
            }
            option => return Err(format!("unknown `lu add` option `{option}`")),
        }
    }
    let git = git.ok_or("`lu add` requires --git <url>")?;
    let rev = rev.ok_or("`lu add` requires --rev <revision>")?;
    let root = workspace_root(&std::env::current_dir().map_err(|error| error.to_string())?)?;
    let mut manifest = read_manifest(&root.join("lu.toml"))?;
    manifest
        .dependencies
        .insert(name.clone(), Dependency { git, rev });
    std::fs::write(root.join("lu.toml"), render_manifest(&manifest))
        .map_err(|error| error.to_string())?;
    let (_, packages) = resolve_workspace(&root)?;
    let locked = packages
        .iter()
        .find(|package| package.name == *name)
        .ok_or_else(|| format!("dependency `{name}` was not resolved"))?;
    Ok(format!(
        "added `{name}` at {} ({})",
        locked.commit, locked.tree
    ))
}

pub fn fetch() -> Result<String, String> {
    let root = workspace_root(&std::env::current_dir().map_err(|error| error.to_string())?)?;
    let (_, packages) = resolve_workspace(&root)?;
    Ok(format!("resolved {} package(s)", packages.len()))
}

pub fn load_workspace(mode: &str) -> Result<LoadedProgram, String> {
    let root = workspace_root(&std::env::current_dir().map_err(|error| error.to_string())?)?;
    let manifest = read_manifest(&root.join("lu.toml"))?;
    let (resolved, _) = resolve_workspace(&root)?;
    let mut source = String::new();
    let mut emitted = BTreeSet::new();
    for package in &resolved {
        append_package_source(&mut source, &package.path, false, mode, &mut emitted)?;
    }
    append_package_source(&mut source, &root, true, mode, &mut emitted)?;
    Ok(LoadedProgram {
        label: root
            .join(format!("{}.lu", manifest.name))
            .to_string_lossy()
            .into_owned(),
        source,
    })
}

#[derive(Clone, Debug)]
struct ResolvedPackage {
    path: PathBuf,
}

fn resolve_workspace(root: &Path) -> Result<(Vec<ResolvedPackage>, Vec<LockedPackage>), String> {
    let manifest = read_manifest(&root.join("lu.toml"))?;
    let lock_path = root.join("lu.lock");
    let old_lock = read_lock(&lock_path)?;
    let mut packages = Vec::new();
    let mut resolved = Vec::new();
    let mut visiting = Vec::new();
    let mut names = BTreeMap::new();
    resolve_dependencies(
        &manifest,
        &old_lock,
        &mut packages,
        &mut resolved,
        &mut visiting,
        &mut names,
    )?;
    packages.sort_by(|left, right| left.name.cmp(&right.name));
    std::fs::write(&lock_path, render_lock(&manifest, &packages))
        .map_err(|error| error.to_string())?;
    Ok((resolved, packages))
}

fn resolve_dependencies(
    manifest: &Manifest,
    old_lock: &[LockedPackage],
    packages: &mut Vec<LockedPackage>,
    resolved: &mut Vec<ResolvedPackage>,
    visiting: &mut Vec<String>,
    names: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    for (name, dependency) in &manifest.dependencies {
        if visiting.contains(name) {
            return Err(format!(
                "dependency cycle: {} -> {}",
                visiting.join(" -> "),
                name
            ));
        }
        if let Some(existing) = names.get(name) {
            if existing != &dependency.git {
                return Err(format!(
                    "dependency name `{name}` refers to both `{existing}` and `{}`",
                    dependency.git
                ));
            }
            continue;
        }
        visiting.push(name.clone());
        let locked = old_lock.iter().find(|package| {
            package.name == *name && package.git == dependency.git && package.rev == dependency.rev
        });
        let (path, package) = materialize(name, dependency, locked)?;
        let child_manifest = read_manifest(&path.join("lu.toml"))?;
        if child_manifest.name != *name {
            return Err(format!(
                "dependency key `{name}` resolves to package `{}`",
                child_manifest.name
            ));
        }
        names.insert(name.clone(), dependency.git.clone());
        resolve_dependencies(
            &child_manifest,
            old_lock,
            packages,
            resolved,
            visiting,
            names,
        )?;
        packages.push(package);
        resolved.push(ResolvedPackage { path });
        visiting.pop();
    }
    Ok(())
}

fn materialize(
    name: &str,
    dependency: &Dependency,
    locked: Option<&LockedPackage>,
) -> Result<(PathBuf, LockedPackage), String> {
    let cache = cache_root()?.join("git");
    std::fs::create_dir_all(&cache).map_err(|error| error.to_string())?;
    if let Some(locked) = locked {
        let path = cache.join(&locked.commit);
        if path.join("lu.toml").exists() {
            verify_cached_package(&path, locked)?;
            return Ok((path, locked.clone()));
        }
        return clone_package(name, dependency, Some(locked));
    }
    clone_package(name, dependency, None)
}

fn verify_cached_package(path: &Path, locked: &LockedPackage) -> Result<(), String> {
    let commit = command_text(
        Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["rev-parse", "HEAD^{commit}"]),
        "verify cached package commit",
    )?;
    let tree = command_text(
        Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["rev-parse", "HEAD^{tree}"]),
        "verify cached package tree",
    )?;
    let status = command_text(
        Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["status", "--porcelain"]),
        "verify cached package contents",
    )?;
    if commit != locked.commit || tree != locked.tree || !status.is_empty() {
        return Err(format!(
            "content-addressed cache entry {} was modified; remove that exact entry and run `lu fetch`",
            path.display()
        ));
    }
    Ok(())
}

fn clone_package(
    name: &str,
    dependency: &Dependency,
    locked: Option<&LockedPackage>,
) -> Result<(PathBuf, LockedPackage), String> {
    let cache = cache_root()?.join("git");
    std::fs::create_dir_all(&cache).map_err(|error| error.to_string())?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let temporary = cache.join(format!(".clone-{}-{nonce}", std::process::id()));
    command_ok(
        Command::new("git")
            .args(["clone", "--quiet", "--no-checkout"])
            .arg(&dependency.git)
            .arg(&temporary),
        "clone dependency",
    )?;
    let revision = locked
        .map(|package| package.commit.as_str())
        .unwrap_or(&dependency.rev);
    command_ok(
        Command::new("git")
            .arg("-C")
            .arg(&temporary)
            .args(["checkout", "--quiet", "--detach", revision]),
        "check out dependency revision",
    )?;
    let commit = command_text(
        Command::new("git")
            .arg("-C")
            .arg(&temporary)
            .args(["rev-parse", "HEAD^{commit}"]),
        "read dependency commit",
    )?;
    if let Some(locked) = locked {
        if commit != locked.commit {
            return Err(format!(
                "locked commit {} for `{name}` is unavailable from {}",
                locked.commit, dependency.git
            ));
        }
    }
    let tree = command_text(
        Command::new("git")
            .arg("-C")
            .arg(&temporary)
            .args(["rev-parse", "HEAD^{tree}"]),
        "read dependency tree",
    )?;
    let destination = cache.join(&commit);
    if destination.exists() {
        std::fs::remove_dir_all(&temporary).map_err(|error| error.to_string())?;
    } else {
        std::fs::rename(&temporary, &destination).map_err(|error| error.to_string())?;
    }
    Ok((
        destination,
        LockedPackage {
            name: name.into(),
            git: dependency.git.clone(),
            rev: dependency.rev.clone(),
            commit,
            tree,
        },
    ))
}

fn append_package_source(
    output: &mut String,
    directory: &Path,
    root: bool,
    mode: &str,
    emitted: &mut BTreeSet<PathBuf>,
) -> Result<(), String> {
    let manifest = read_manifest(&directory.join("lu.toml"))?;
    let mut files = Vec::new();
    if root && matches!(mode, "test" | "doc") {
        let library = directory.join("src/lib.lu");
        if library.exists() {
            files.push(library);
        }
        if mode == "doc" {
            let main = directory.join("src/main.lu");
            if main.exists() {
                files.push(main);
            }
        }
        let tests = directory.join("tests");
        if tests.exists() {
            let mut entries = std::fs::read_dir(&tests)
                .map_err(|error| error.to_string())?
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|extension| extension == "lu"))
                .collect::<Vec<_>>();
            entries.sort();
            files.extend(entries);
        }
    } else {
        let preferred = directory.join("src/lib.lu");
        let fallback = directory.join("lib.lu");
        if root {
            if preferred.exists() {
                files.push(preferred);
            }
            let main = directory.join("src/main.lu");
            if main.exists() {
                files.push(main);
            } else {
                return Err(format!("package `{}` has no src/main.lu", manifest.name));
            }
        } else if preferred.exists() {
            files.push(preferred);
        } else if fallback.exists() {
            files.push(fallback);
        } else {
            return Err(format!(
                "package `{}` has no {}",
                manifest.name, "src/lib.lu"
            ));
        }
    }
    if files.is_empty() {
        return Err(format!(
            "package `{}` has no {} sources",
            manifest.name, mode
        ));
    }
    for file in files {
        let canonical = std::fs::canonicalize(&file).unwrap_or_else(|_| file.clone());
        if !emitted.insert(canonical) {
            continue;
        }
        let source = std::fs::read_to_string(&file)
            .map_err(|error| format!("cannot read {}: {error}", file.display()))?;
        for line in source.lines() {
            let line = line.trim();
            let Some(import) = line.strip_prefix("use ") else {
                continue;
            };
            let import = import
                .split_whitespace()
                .next()
                .ok_or_else(|| format!("empty `use` in {}", file.display()))?;
            if import != "math" && !manifest.dependencies.contains_key(import) {
                return Err(format!(
                    "{} imports undeclared package `{import}`",
                    file.display()
                ));
            }
        }
        let _ = writeln!(output, "// package {}: {}", manifest.name, file.display());
        output.push_str(&source);
        if !source.ends_with('\n') {
            output.push('\n');
        }
    }
    Ok(())
}

fn read_manifest(path: &Path) -> Result<Manifest, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    parse_manifest(&source).map_err(|error| format!("{}: {error}", path.display()))
}

fn parse_manifest(source: &str) -> Result<Manifest, String> {
    let mut section = String::new();
    let mut name = None;
    let mut version = None;
    let mut dependencies = BTreeMap::new();
    for raw_line in source.lines() {
        let line = strip_toml_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].to_string();
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("invalid manifest line `{line}`"))?;
        let key = key.trim();
        let value = value.trim();
        match section.as_str() {
            "package" if key == "name" => name = Some(parse_quoted(value)?),
            "package" if key == "version" => version = Some(parse_quoted(value)?),
            "dependencies" => {
                validate_package_name(key)?;
                let table = value
                    .strip_prefix('{')
                    .and_then(|value| value.strip_suffix('}'))
                    .ok_or_else(|| format!("dependency `{key}` must use an inline table"))?;
                let mut git = None;
                let mut rev = None;
                for item in table.split(',') {
                    let (field, value) = item
                        .split_once('=')
                        .ok_or_else(|| format!("invalid dependency `{key}`"))?;
                    match field.trim() {
                        "git" => git = Some(parse_quoted(value.trim())?),
                        "rev" => rev = Some(parse_quoted(value.trim())?),
                        field => {
                            return Err(format!("unknown field `{field}` in dependency `{key}`"))
                        }
                    }
                }
                dependencies.insert(
                    key.into(),
                    Dependency {
                        git: git.ok_or_else(|| format!("dependency `{key}` needs `git`"))?,
                        rev: rev.ok_or_else(|| format!("dependency `{key}` needs `rev`"))?,
                    },
                );
            }
            _ => return Err(format!("unsupported manifest key `{key}` in [{section}]")),
        }
    }
    let name = name.ok_or("missing [package].name")?;
    validate_package_name(&name)?;
    Ok(Manifest {
        name,
        version: version.ok_or("missing [package].version")?,
        dependencies,
    })
}

fn strip_toml_comment(line: &str) -> &str {
    let mut quoted = false;
    let mut escaped = false;
    for (index, byte) in line.bytes().enumerate() {
        if escaped {
            escaped = false;
        } else if byte == b'\\' && quoted {
            escaped = true;
        } else if byte == b'"' {
            quoted = !quoted;
        } else if byte == b'#' && !quoted {
            return &line[..index];
        }
    }
    line
}

fn render_manifest(manifest: &Manifest) -> String {
    let mut output = format!(
        "[package]\nname = \"{}\"\nversion = \"{}\"\n\n[dependencies]\n",
        manifest.name, manifest.version
    );
    for (name, dependency) in &manifest.dependencies {
        let _ = writeln!(
            output,
            "{} = {{ git = \"{}\", rev = \"{}\" }}",
            name,
            escape_toml(&dependency.git),
            escape_toml(&dependency.rev)
        );
    }
    output
}

fn read_lock(path: &Path) -> Result<Vec<LockedPackage>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let source = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let mut packages = Vec::new();
    let mut fields = BTreeMap::new();
    let mut in_package = false;
    for line in source.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            if in_package && !fields.is_empty() {
                packages.push(lock_from_fields(&fields)?);
                fields.clear();
            }
            in_package = true;
        } else if let Some((key, value)) = line.split_once('=') {
            if in_package {
                fields.insert(key.trim().to_string(), parse_quoted(value.trim())?);
            }
        }
    }
    if in_package && !fields.is_empty() {
        packages.push(lock_from_fields(&fields)?);
    }
    Ok(packages)
}

fn lock_from_fields(fields: &BTreeMap<String, String>) -> Result<LockedPackage, String> {
    let field = |name: &str| {
        fields
            .get(name)
            .cloned()
            .ok_or_else(|| format!("lock package missing `{name}`"))
    };
    Ok(LockedPackage {
        name: field("name")?,
        git: field("git")?,
        rev: field("rev")?,
        commit: field("commit")?,
        tree: field("tree")?,
    })
}

fn render_lock(root: &Manifest, packages: &[LockedPackage]) -> String {
    let mut output = format!(
        "# Generated by lu. Do not edit.\nversion = 1\nroot = \"{}\"\n",
        root.name
    );
    for package in packages {
        let _ = write!(
            output,
            "\n[[package]]\nname = \"{}\"\ngit = \"{}\"\nrev = \"{}\"\ncommit = \"{}\"\ntree = \"{}\"\n",
            package.name,
            escape_toml(&package.git),
            escape_toml(&package.rev),
            package.commit,
            package.tree
        );
    }
    output
}

fn parse_quoted(value: &str) -> Result<String, String> {
    let value = value.trim();
    let inner = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .ok_or_else(|| format!("expected quoted string, found `{value}`"))?;
    Ok(inner.replace("\\\"", "\"").replace("\\\\", "\\"))
}

fn escape_toml(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn validate_package_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    if !chars
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic() || character == '_')
        || !chars.all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return Err(format!(
            "invalid package name `{name}`; use ASCII letters, digits, and underscores"
        ));
    }
    Ok(())
}

fn workspace_root(start: &Path) -> Result<PathBuf, String> {
    for directory in start.ancestors() {
        if directory.join("lu.toml").exists() {
            return Ok(directory.to_path_buf());
        }
    }
    Err("no lu.toml found in this directory or its parents".into())
}

fn cache_root() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("LULANG_CACHE") {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = std::env::var_os("XDG_CACHE_HOME") {
        return Ok(PathBuf::from(path).join("lulang"));
    }
    let home = std::env::var_os("HOME")
        .ok_or("cannot determine package cache; set LULANG_CACHE to an absolute directory")?;
    Ok(PathBuf::from(home).join(".cache/lulang"))
}

fn command_ok(command: &mut Command, action: &str) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|error| format!("cannot {action}: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(command_error(action, &output))
    }
}

fn command_text(command: &mut Command, action: &str) -> Result<String, String> {
    let output = command
        .output()
        .map_err(|error| format!("cannot {action}: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().into())
    } else {
        Err(command_error(action, &output))
    }
}

fn command_error(action: &str, output: &Output) -> String {
    format!(
        "failed to {action}: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    )
}
