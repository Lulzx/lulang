//! Runtime compilation and linking, shared by the AOT backends.
//!
//! Both AOT tiers produce an object that calls the same C runtime and needs the
//! same link line: the LLVM tier compiles textual IR with clang, the Cranelift
//! tier (`lu build --fast`) emits the object itself. Only the runtime object is
//! cached — it is identical for every program built from a given runtime
//! source, so the cost is paid once per machine rather than once per build.

use std::path::PathBuf;
use std::process::Command;

const RUNTIME_SOURCE: &str = include_str!("../lu_runtime.c");

/// Compile (or reuse) the C runtime object. The cache key is the runtime source
/// itself, so editing `src/lu_runtime.c` produces a different object rather than
/// silently reusing a stale one.
pub fn runtime_object(library: bool) -> Result<PathBuf, String> {
    let mut hash: u64 = RUNTIME_SOURCE.bytes().fold(1469598103934665603u64, |h, b| {
        (h ^ b as u64).wrapping_mul(1099511628211)
    });
    if library {
        hash ^= 0x4c55_5f4c_4942;
    }
    let cached = std::env::temp_dir().join(format!("lu_runtime_{:016x}.o", hash));
    if cached.exists() {
        return Ok(cached);
    }

    let pid = std::process::id();
    let source = std::env::temp_dir().join(format!("lu_runtime_{}.c", pid));
    let scratch = std::env::temp_dir().join(format!("lu_runtime_{:016x}_{}.o", hash, pid));
    std::fs::write(&source, RUNTIME_SOURCE).map_err(|error| error.to_string())?;
    let mut clang = Command::new("clang");
    clang.args(["-O3", "-mcpu=native", "-c"]);
    if library {
        clang.args(["-DLU_LIB", "-fPIC"]);
    }
    let status = clang
        .arg("-o")
        .arg(&scratch)
        .arg(&source)
        .status()
        .map_err(|error| format!("failed to invoke clang: {}", error))?;
    let _ = std::fs::remove_file(&source);
    if !status.success() {
        return Err("clang failed compiling the runtime".into());
    }
    // A concurrent build may have installed the same object first; either
    // outcome is fine as long as the cached path ends up present.
    if let Err(error) = std::fs::rename(&scratch, &cached) {
        if !cached.exists() {
            return Err(format!("failed to install runtime object: {}", error));
        }
        let _ = std::fs::remove_file(&scratch);
    }
    Ok(cached)
}

/// Extra libraries named by `extern "lib" fn` declarations, in first-seen order
/// and deduplicated. A value containing a path separator or a shared-object
/// extension is passed through verbatim; anything else becomes `-lname`.
pub fn library_arguments<'a>(libs: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut args = Vec::new();
    for lib in libs {
        if !seen.insert(lib) {
            continue;
        }
        if lib.contains('/') || lib.ends_with(".so") || lib.ends_with(".dylib") {
            args.push(lib.to_string());
        } else {
            args.push(format!("-l{}", lib));
        }
    }
    args
}

/// Link objects into an executable. `LU_LINK_FLAGS` is appended so callers can
/// add search paths without the compiler learning about them.
pub fn link_executable(
    output: &str,
    objects: &[PathBuf],
    libraries: &[String],
) -> Result<(), String> {
    let mut cc = Command::new("cc");
    cc.arg("-o").arg(output).args(objects).args(libraries);
    if let Ok(flags) = std::env::var("LU_LINK_FLAGS") {
        cc.args(flags.split_whitespace());
    }
    let status = cc
        .status()
        .map_err(|error| format!("failed to invoke the linker: {}", error))?;
    if !status.success() {
        return Err(format!("linker failed producing {}", output));
    }
    Ok(())
}
