//! `lu build --fast` emits an object through Cranelift instead of handing
//! textual LLVM IR to clang. It shares the JIT's code generator but not its
//! runtime: the JIT resolves `lu_*` against `src/runtime.rs` in process, while a
//! built binary links `src/lu_runtime.c`. The two must agree — in particular on
//! array ownership (`lu_arr_share`/`lu_arr_cow`), which is the C runtime's newer
//! half — and string literals become data symbols rather than baked-in host
//! addresses. These cases cover both.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn lu() -> &'static str {
    env!("CARGO_BIN_EXE_lu")
}

struct Case(PathBuf);

impl Case {
    fn new(name: &str, source: &str) -> Case {
        let path =
            std::env::temp_dir().join(format!("lulang-fast-{}-{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).expect("create case directory");
        fs::write(path.join("case.lu"), source).expect("write case source");
        Case(path)
    }

    fn source(&self) -> String {
        self.0.join("case.lu").to_string_lossy().into_owned()
    }
}

impl Drop for Case {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Build with `--fast`, run it, and return stdout alongside the JIT's.
fn fast_and_jit(name: &str, source: &str) -> (String, String) {
    let case = Case::new(name, source);
    let built = Command::new(lu())
        .args(["build", "--fast", "-o", "case", &case.source()])
        .current_dir(&case.0)
        .output()
        .expect("run lu build --fast");
    assert!(
        built.status.success(),
        "--fast build failed: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    let fast = Command::new(case.0.join("case"))
        .output()
        .expect("run the built binary");
    assert!(
        fast.status.success(),
        "--fast binary failed: {}",
        String::from_utf8_lossy(&fast.stderr)
    );
    let jit = Command::new(lu())
        .args(["run", &case.source()])
        .output()
        .expect("run lu run");
    assert!(
        jit.status.success(),
        "JIT run failed: {}",
        String::from_utf8_lossy(&jit.stderr)
    );
    (
        String::from_utf8_lossy(&fast.stdout).into_owned(),
        String::from_utf8_lossy(&jit.stdout).into_owned(),
    )
}

#[test]
fn fast_binaries_match_the_jit_on_strings_and_arrays() {
    // String literals reached from an outlined function are the case that
    // cannot use baked pointers; array writes after a copy are the case that
    // needs the runtime's ownership table.
    let source = "\
fn label(n: i64): str {
  if n == 0 { return \"zero\" }
  return label(n - 1)
}

fn bump(xs: [i64]): [i64] {
  var ys = xs
  ys[0] = 99
  return ys
}

main {
  print(label(3))
  var a = arr(3, 1)
  var b = a
  b[0] = 2
  print(a[0], b[0])
  let c = bump(a)
  print(a[0], c[0])
  var total = 0
  for i in 0..64 { a[i % 3] = i }
  for i in 0..3 { total = total + a[i] }
  print(total)
}
";
    let (fast, jit) = fast_and_jit("strings-arrays", source);
    assert_eq!(fast, jit, "--fast output diverged from the JIT");
    assert_eq!(fast, "zero\n1 2\n1 99\n186\n");
}

#[test]
fn fast_binaries_match_the_jit_on_records_and_reductions() {
    // Record arrays exercise the SoA layout and the `sum` vectorizer, the parts
    // of the shared middle-end most sensitive to a backend swap.
    let source = "\
type Quat { w: f64, x: f64, y: f64, z: f64 }

fn qq(qs: [Quat], n: i64): f64 {
  return sum(i in 0..n) qs[i].w * qs[i].w + qs[i].x * qs[i].x
}

main {
  let n = 1000
  var qs = arr(n, Quat { 0.0, 0.0, 0.0, 0.0 })
  for i in 0..n {
    let f = float(i) * 0.001
    qs[i] = Quat { w: 1.0 + f, x: 2.0 - f, y: f, z: 0.5 }
  }
  print(qq(qs, n))
  print(sqrt(2.0), sin(0.5))
}
";
    let (fast, jit) = fast_and_jit("records", source);
    assert_eq!(fast, jit, "--fast output diverged from the JIT");
}

#[test]
fn fast_rejects_flags_it_cannot_honor() {
    let case = Case::new("flags", "main { print(1) }\n");
    for extra in ["--emit-llvm", "--lib", "--target"] {
        let output = Command::new(lu())
            .args(["build", "--fast", extra, &case.source()])
            .current_dir(&case.0)
            .output()
            .expect("run lu build");
        assert!(
            !output.status.success(),
            "`--fast {extra}` should be rejected"
        );
    }
}
