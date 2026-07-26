//! The same source, compiled twice by the same binary, must produce the same
//! bytes.
//!
//! Both compiled tiers read `analyze_cfg`, whose loop bodies are `HashSet`s.
//! Walking one unordered leaks the process's hash seed into generated code: the
//! order arrays are discovered in decides the order of their hoisted loop-entry
//! range checks, and therefore the numbering of every temporary after them. It
//! never changed behavior, which is why it survived — the four-tier corpus gate
//! passes either way.
//!
//! The if-converter has the same hazard over its set of assigned locals. That
//! one is ordered defensively rather than in response to a failure: the order
//! does vary per run, but the selects are independent and Cranelift's egraph
//! pass re-canonicalizes them, so it does not currently reach the output. These
//! tests would catch it if that ever stopped being true.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn lu() -> &'static str {
    env!("CARGO_BIN_EXE_lu")
}

/// Both sites, in one loop.
///
/// The arrays are first indexed in *different* blocks of the body, so which one
/// the analysis discovers first depends on the order the body's blocks are
/// walked — and the hoisted loop-entry range checks are emitted in that order.
/// (Two arrays indexed in the same block would not reproduce it: within a block,
/// instruction order settles the question.) Array accesses are not
/// speculation-safe, so this `if` survives if-conversion in both tiers. The
/// second `if` assigns two locals and is speculatable, which is the
/// if-converter's own ordering site.
const SOURCE: &str = "\
fn mix(a: [f64], b: [f64], n: i64, k: f64): f64 {
  var total = 0.0
  for i in 0..n {
    if k > 0.5 {
      total = total + a[i]
    } else {
      total = total + b[i]
    }
    var scale = 1.0
    var bias = 0.0
    if total > 10.0 {
      scale = 2.0
      bias = 0.5
    } else {
      scale = 3.0
      bias = 1.5
    }
    total = total + scale * bias
  }
  return total
}

main {
  let n = 64
  var a = arr(n, 0.0)
  var b = arr(n, 0.0)
  for i in 0..n {
    a[i] = float(i) * 0.5
    b[i] = float(n - i) * 0.25
  }
  print(mix(a, b, n, 0.75))
}
";

struct Case(PathBuf);

impl Case {
    fn new(name: &str) -> Case {
        let path = std::env::temp_dir().join(format!("lulang-det-{}-{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).expect("create case directory");
        fs::write(path.join("case.lu"), SOURCE).expect("write case source");
        Case(path)
    }

    fn source(&self) -> String {
        self.0.join("case.lu").to_string_lossy().into_owned()
    }

    /// Compile `runs` times in separate processes, returning the artifact bytes.
    /// Separate processes matter: the hash seed is per process, so repeating the
    /// work inside one would not reproduce the original failure faithfully.
    fn compile_repeatedly(&self, args: &[&str], artifact: &str, runs: usize) -> Vec<Vec<u8>> {
        (0..runs)
            .map(|_| {
                let built = Command::new(lu())
                    .args(args)
                    .arg(self.source())
                    .current_dir(&self.0)
                    .output()
                    .expect("run lu build");
                assert!(
                    built.status.success(),
                    "build failed: {}",
                    String::from_utf8_lossy(&built.stderr)
                );
                fs::read(self.0.join(artifact)).expect("read build artifact")
            })
            .collect()
    }
}

impl Drop for Case {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn assert_all_equal(outputs: &[Vec<u8>], what: &str) {
    for (index, output) in outputs.iter().enumerate().skip(1) {
        assert_eq!(
            output.len(),
            outputs[0].len(),
            "{what}: run {index} differs in length from run 0"
        );
        assert!(
            output == &outputs[0],
            "{what}: run {index} differs from run 0 — compilation is not deterministic"
        );
    }
}

#[test]
fn emitted_llvm_ir_is_byte_identical_across_runs() {
    let case = Case::new("llvm");
    let outputs = case.compile_repeatedly(&["build", "--emit-llvm", "-o", "out.ll"], "out.ll", 5);
    assert_all_equal(&outputs, "emitted LLVM IR");
}

#[test]
fn cranelift_binaries_are_byte_identical_across_runs() {
    let case = Case::new("fast");
    let outputs = case.compile_repeatedly(&["build", "--fast", "-o", "out"], "out", 5);
    assert_all_equal(&outputs, "`lu build --fast` binary");
}
