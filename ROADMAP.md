# lulang Ecosystem Roadmap

lulang today is deep on one axis: a numerics-first language with four verified
execution tiers (reference interpreter, Cranelift JIT, LLVM AOT, self-hosted
`selfhost/codegen.lu` with a 3-stage bootstrap fixpoint) and a measured
performance story. What it deliberately lacks is ecosystem surface. This
document is the plan for growing that surface, in leverage order.

The strategic thesis: **lulang should become a numerical kernel language that
can live inside every existing ecosystem** — Python, C/C++, Rust, Julia, the
browser — before trying to convince anyone to write whole applications in it.
Young languages grow when people can use 5% of them inside their existing
stack. Every item below is an interop or low-commitment entry point.

## Guiding principle: the C ABI is a boundary, not a window

lulang's performance thesis depends on compiler-owned layout (SoA record
arrays, flattened calling conventions, whole-program optimization). The FFI
must therefore be a **boundary representation**: a deliberately small,
checker-enforced set of C-compatible types crosses the boundary; ordinary
lulang records and arrays keep unstable, compiler-controlled layout forever.
We never promise a stable internal ABI. Boundary-only types (`c_ptr[T]`,
`c_slice[T]`, `@c_layout` records, opaque handles) are added as needed rather
than exposing internals.

## Ordered roadmap

| # | Project | Ecosystem leverage | Effort |
|--:|---------|--------------------|--------|
| 1 | Stable C import/export ABI (M8) — **complete** | Exceptional | Medium |
| 2 | Generated C headers + machine-readable ABI manifest — **complete** | Exceptional | Low–medium |
| 3 | `pylulang`: NumPy/Python bindings — **complete (v0.1)** | Exceptional | Medium |
| 4 | LSP + VS Code extension (tree-sitter first) — **complete (v0.1)** | Very high | Medium |
| 5 | `lu-numerics` first-party library corpus — **foundation shipped** | Very high | Continuous |
| 6 | Web playground (interpreter → wasm32) — **v0.1 shipped** | High | Medium |
| 7 | `lu bindgen` C-header importer — **foundation shipped** | Very high | Medium–high |
| 8 | `wasm32-wasi` / `wasm32-web` target | High | Medium |
| 9 | Git-based package manager (`lu.toml`) | High once libraries exist | Medium |
| 10 | Flagship demo (`luphysics`) | High visibility | Medium |
| 11 | `lu doc` + benchmark observatory | High credibility | Medium |
| 12 | Autodiff (`ludiff`, forward-mode duals first) | High technical value | High |

### 1–2. C ABI: `extern` + `export` (milestone M8)

Both directions, landed together with generated artifacts:

```lu
extern fn llabs(x: i64): i64          // symbol from already-linked libs
extern "m" fn cbrt(x: f64): f64       // the string names the library

export fn dot(x: [f64], y: [f64], n: i64): f64 {
  return sum(i in 0..n) x[i] * y[i]
}
```

```text
lu build --lib kernels.lu
    → libkernels.a / libkernels.dylib|so (--shared)
    → kernels.h        # generated C header
    → kernels.json     # machine-readable ABI manifest
```

Import unlocks BLAS/LAPACK/libm/SDL — real programs, instantly. Export is the
more viral direction: nobody rewrites their app, but everyone will try a
2×-faster kernel. The JSON manifest is the foundation for automatic bindings
(`pylulang`, future `lu bindgen` output verification).

M8 boundary subset: `i64`, `f64`, `bool` (0/1 as `int64_t`), enums (i64 tag),
`str` as `(const char*, int64_t)` parameters, `[i64]`/`[f64]` as
`(T* data, int64_t n)`. Signatures capped at 6 integer-class + 8 float-class
components (a language rule that keeps every argument in registers on both
SysV x86-64 and AArch64). The first follow-up, boundary-only `c_ptr[T]` opaque
handles, now works in all four tiers and in generated headers. Remaining
follow-ups: `f32` at the boundary, `c_slice[T]`, `@c_layout` records, `str`
returns, callbacks, and zero-copy array export handles.

### 3. `pylulang`

The single best adoption bridge after the C ABI. Start simple — no Python
compiler, no decorator magic:

```python
from pylulang import compile

module = compile(open("kernels.lu").read())
result = module.dot(numpy_x, numpy_y)   # zero-copy via (ptr, len)
```

Zero-copy NumPy arrays through the boundary types. The narrative: *write your
hot loop in a tiny value-semantic language, call it from your notebook, keep
everything else.* Scientists will try lulang because it accelerates a function
inside an existing Python program, not because it is interesting. A
`@lulang.jit` decorator / restricted translator can come much later.

### 4. Editor tooling

Tree-sitter grammar first — it alone gets highlighting in Neovim, Zed, Helix,
and on GitHub. Then `lu lsp` reusing the existing checker for diagnostics,
hover types, go-to-definition, completion, operator-definition navigation, and
property-test lenses. `lu fmt` already ships; format-on-save wires straight
in. Unicode operators make this non-optional: without an editor story —
including input (ASCII aliases that display/expand to `·`, `‖`, snippets) —
the syntax reads as a barrier instead of a feature. Deliverable: one VS Code
extension bundling grammar + LSP + operator input + "run property" commands
with inline counterexample display.

### 5. `lu-numerics`

In a numerics language, the stdlib of math types *is* the ecosystem — seed it
first-party. Modules: `linalg`, `stats`, `geometry` (Vec3/Quat graduate from
the corpus), `random`, `signal`, `optimize`, `special`. Start with what
showcases semantic advantages: dot/norm/matvec, small fixed-size matrices,
quaternions, reductions, polynomial evaluation, convolution, integration, root
finding, Monte Carlo kernels. **Every function ships with (1) properties,
(2) benchmarks, (3) a C++/NumPy/Julia comparison, (4) generated docs.**
First-class `property` blocks with shrinking are genuinely differentiating —
they should appear throughout the ecosystem, not stay a compiler demo.

### 6. Web playground

The first public version at `lulang.lulzx.space` includes a local browser
interpreter, editable source, and examples for functions, reductions, arrays,
and value semantics. It has no server-side execution and requires no install.
Next: compile the reference interpreter to wasm32 (the CFG evaluator compiles
where Cranelift won't), then add property tests, lowered IR, generated LLVM,
and shareable permalinks. Best later examples are visual and surprising:
quaternion slerp, N-body, Mandelbrot, particle systems, property
counterexamples, and numerical-instability demos.

### 7. `lu bindgen`

Exporting lulang is useful; **calling the existing world is dramatically more
useful**. `lu bindgen fftw3.h -o fftw3.lu` over a deliberately small C subset
(functions, primitives, pointers, fixed-layout structs, enums, constants,
opaque handles; callbacks later). Demonstration targets in order: BLAS, FFTW,
SQLite, raylib, libpng, SuiteSparse. BLAS/FFTW reinforce the numerics
identity; raylib produces visible demos.

The first slice ships a dependency-free C lexer/parser, typedef resolution,
numeric macros, sequential enums, function prototypes, register-cap checking,
and checker-valid `extern` generation. The second adds boundary-only
`c_ptr[T]`, opaque C structs, and end-to-end pointer calls in the interpreter,
JIT, AOT, and self-hosted compiler. Unsupported pointees degrade to the
explicit untyped handle `c_ptr[()]`; no C layout is inferred. Narrower
integers, `float`, C `bool`, callbacks, and by-value aggregates remain
diagnostics instead of being unsafely widened. A macOS `math.h` preflight
currently produces 41 checker-valid imports. The remaining work for the full
promised subset is now split cleanly: bindgen emits validated `@c_layout`
record declarations plus header/manifest layout metadata, while by-value
aggregate calls stay disabled until target ABI classification lands.
Conversion shims for C-width scalars follow, then callbacks.

### 8. WASM target

The second backend target after the C ABI — not GPU. `lu build --target
wasm32-wasi` (and `wasm32-web`). Enables playground execution, browser
kernels, serverless, JS embedding, portable benchmark artifacts, sandboxed
plugins. Because every tier already consumes one validated CFG IR, another
backend is architecturally cheap here. Treat WASM as distribution leverage;
don't promise native SIMD parity immediately.

### 9. Package manager — deliberately minimal

No registry until ~20–30 meaningful packages exist. Git-pinned source
dependencies fit the whole-program compilation model:

```toml
[package]
name = "orbit"
version = "0.1.0"

[dependencies]
numerics = { git = "https://github.com/lulang/lu-numerics", rev = "..." }
```

Commands: `lu init | add | build | test | bench | doc`. Content-addressed
lockfiles, immutable commit pins, reproducible builds, whole-program
compilation after resolution. (Prerequisite: a module/import story beyond the
stdlib — currently deferred in SPEC §deferred.)

### 10. Flagship showcase: `luphysics`

Infrastructure alone doesn't create an ecosystem; applications prove why the
language exists. Strongest flagship: a small rigid-body/particle physics
engine — vectors, quaternions, collision kernels, integration, constraints,
property tests for invariants, C ABI embedding, raylib visualizer. The
language's teaser is already quaternion operators and `slerp`; this is that
teaser grown up. Other candidates as the ecosystem matures: `luspice`
(circuit simulation), `lurocket` (orbital mechanics), `luquant` (Monte Carlo
pricing), `luimage` (kernels with visible output).

### 11. `lu doc` + benchmark observatory

Docs where properties are executable claims: each function page shows
signature, description, examples, its properties *and their execution
status*, benchmark history, and the generated C ABI signature. Separately, a
public benchmark observatory (continuous: lulang JIT/AOT/selfhost vs
C++/Rust/Julia/NumPy) that is more than a leaderboard — per benchmark it
exposes source in every language, generated IR, optimization toggles (the
ablation flags already exist: `LU_MATH/LU_IFCONV/LU_LICM/LU_SIMD/LU_LAYOUT`),
chosen layout, and the semantic assumptions responsible for the number. That
page is the marketing; it's what gets linked.

### 12. `ludiff`

Forward-mode automatic differentiation as *library code* — dual numbers as
records with user-defined operators, no compiler support needed. Reverse mode
later. AD fits the language beautifully but is high-effort; it waits.

## Explicit non-goals (for now)

Garbage collector, classes/OO, async runtime, macro system, LLVM replacement,
native GUI, large package registry, Python-syntax compatibility, GPU backend,
web frameworks. They expand scope without strengthening the numerical thesis.
GPU in particular: lulang needs users, embeddability, and libraries more than
a fifth execution tier.

## The near-term push: "lulang Embedded"

One packaged milestone arc built from items 1–4:

1. `extern` declarations and `export fn` (M8).
2. Boundary ABI types with the checker-enforced subset.
3. Generated `.h` + `.json` per `lu build --lib`.
4. `pylulang` loading compiled libraries, zero-copy NumPy vectors.
5. One striking example — an N-body or quaternion kernel beating the
   conventional implementation from a notebook.
6. VS Code syntax + diagnostics.
7. A page showing the source, the generated header, and the benchmark.

The adoption loop this creates:

> discover benchmark → try playground → `pip install` → accelerate one NumPy
> function → embed the library elsewhere → publish a reusable lulang package.

The C ABI is not merely another feature; it is the bridge that turns lulang
from a compiler experiment into usable infrastructure.
