# Ecosystem roadmap completion audit

This is the evidence map for [ROADMAP.md](ROADMAP.md) and
[M8-PLAN.md](M8-PLAN.md). A roadmap item counts as shipped only when its
user-visible artifact and its executable verification are both present.
Items that the roadmap explicitly labels “next”, “later”, or a non-goal remain
outside the shipped slice and are listed at the end.

## Boundary invariant

The stable C ABI is a boundary representation, not the compiler ABI.
`src/check.rs` admits only the scalar, string, array, enum, and opaque-pointer
subset and enforces the 6 integer / 8 float register cap. Ordinary records
remain compiler-owned. `@c_layout` records provide explicit C metadata;
bindgen uses generated adapters for by-value C records rather than passing
ordinary lulang records across the boundary. The invariant is exercised by
`tests/regressions.rs`, `tests/ffi_export.rs`, and `tests/bindgen.rs`.

## Ordered roadmap evidence

| # | Shipped slice | Authoritative evidence |
|---:|---|---|
| 1 | Stable C import/export ABI | Parser/checker/IR in `src/{ast,parser,check,ir}.rs`; interpreter, JIT, LLVM, and selfhost execution; four-tier scalar/array/string/pointer and unresolved-symbol cases in `tests/conformance.rs`. |
| 2 | C headers and ABI manifests | `src/cheader.rs`; static/shared C and ctypes callers in `tests/ffi_export.rs`; generated Embedded `.h`/`.json` drift check in `tests/luphysics.rs`. |
| 3 | `pylulang` v0.1 | Installable package under `python/pylulang`; writable contiguous NumPy buffers cross directly into the C shim; list, scalar, mutation, boolean, and NumPy coverage in `tests/pylulang.rs`. |
| 4 | LSP and VS Code v0.1 | Tree-sitter grammar, VS Code grammar/snippets/extension, `lu lsp`, typed hover/completion, function/operator definitions, format-on-save, property lenses and inline counterexamples; `tests/lsp.rs` plus `tools/tests/test_lulang_lsp.py`. |
| 5 | `lu-numerics` v0.1 | 26 kernels, 11 law groups, per-function benchmark registry, C++/NumPy/Julia twins, generated docs and all-tier/generated-Python execution; enforced by `tests/numerics.rs` and `lib/lu-numerics/test_numerics.py`. |
| 6 | Public playground v0.1 | Local browser interpreter with editable examples and no server execution in `playground/app/playground.tsx`; rendered-route tests in `playground/tests`; production site at `lulang.lulzx.space`. |
| 7 | `lu bindgen` foundation | Dependency-free C parser, typedefs/macros/enums/functions, opaque pointers, exact-layout metadata and generated narrow/by-value adapters in `src/bindgen.rs`; checker-valid and real-library integration in `tests/bindgen.rs`. Unsupported unions, bitfields, variadics, callbacks, and aggregate returns are explicit diagnostics. |
| 8 | WASI and web targets | `lu build --target wasm32-wasi|wasm32-web`, web loader, executable parity and native-extern rejection in `tests/wasm.rs`. |
| 9 | Git package foundation | `lu init/add/fetch`, immutable commit/tree lockfile, content-addressed cache, dependency graph composition and package-default commands in `src/package.rs`; moving-revision and whole-program verification in `tests/package.rs`. A registry is an explicit non-goal before a real package corpus exists. |
| 10 | `luphysics` showcase | Value vectors/bodies, N-body gravity, integration, impulses, conservation laws, C SoA export, WASI and raylib adapter under `lib/luphysics`; all-tier, property, C, WASM and notebook checks in `tests/luphysics.rs`. |
| 11 | Executable docs and observatory | Package-aware `lu doc`/`lu bench` in `src/{docgen,benchmark}.rs`; laws, status, source, C ABI, history and LLVM verified by `tests/docs.rs`; reproducible cross-language runner/workflow under `benchmarks/` and `.github/workflows/observatory.yml`; public `/observatory` route. |
| 12 | Forward autodiff | `lib/ludiff` implements dual numbers and operators as ordinary library code with nine laws; interpreter/JIT/AOT/selfhost/WASI/C coverage in `tests/ludiff.rs`. |

## M8 plan evidence

- Slices A, B, and C are represented by four-tier import conformance,
  static/shared export integration, and selfhost extern execution.
- Invalid records, `[f32]`, `inout`, register overflow, symbol collisions,
  reserved names, nested declarations, and unresolved symbols are negative
  tests.
- Direct scalar `f32` is a shipped compatible extension: `cbrtf` executes in
  all four tiers, C callers exercise generated `float` exports, bindgen maps
  C `float` without a shim, and host/selfhost LLVM is byte-identical.
- `tests/selfhost_sync.rs` byte-compares the shared frontend region and the
  host/selfhost LLVM for an import and a scalar export.
- `selfhost/build.sh --bootstrap` proves stage 1 = stage 2 and stage 2 =
  stage 3 after the ABI changes.
- The adoption corpus remains `corpus/ffi_cbrt.lu` and
  `corpus/kernel_saxpy.lu`, with real C and Python callers.

## Embedded adoption loop

| Step | Evidence |
|---|---|
| `extern` and `export fn` | M8 conformance and export integration |
| Checker-enforced boundary types | `src/check.rs` plus negative tests |
| Generated header and manifest | `examples/embedded_slerp.{h,json}`, checked against fresh compiler output |
| Python and NumPy bridge | `python/pylulang` and its NumPy no-boundary-copy test |
| Notebook result | `examples/lulang_embedded.ipynb` checks lulang vs NumPy and asserts that the compiled 2M-slerp kernel wins; `benchmarks/embedded.tsv` records the reproducible snapshot |
| VS Code syntax and diagnostics | editor and LSP integration tests |
| Public source/header/benchmark page | `https://lulang.lulzx.space/observatory` |

This closes the intended loop: discover a source-linked result, try the local
web interpreter, install `pylulang`, accelerate one function, reuse the C
library, and publish a Git-pinned source package.

## Release gates

On 2026-07-23 the closeout branch passed:

```text
cargo test --release --workspace
selfhost/build.sh --bootstrap
python3 tools/verify_corpus.py
python3 examples/run_embedded_notebook.py
npm test && npm run lint                 # from playground/
```

The release suite includes all four native tiers, generated C and Python
interfaces, package/docs/editor/numerics/physics/autodiff tests, WASI/web
targets when Zig is available, the M8 byte-identity checks, and full compiled
plus scaled reference-interpreter corpus agreement.

## Explicitly deferred by the roadmap

`c_slice[T]`, string returns, callbacks, zero-copy export handles,
reverse-mode AD, native WASM SIMD parity, a package registry, and the broader
bindgen C surface remain later work. The browser's current
interpreter is local TypeScript; compiling the reference CFG evaluator to
WASM plus property/IR panels and permalinks is the next playground increment.
These are not silently claimed by the shipped v0.1/foundation slices.
