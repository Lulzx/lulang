# lulang

lulang is a small numerics-oriented programming language with four execution
tiers: a reference interpreter, a Cranelift JIT, an LLVM AOT compiler, and a
self-hosted AOT compiler written in lulang itself. It exists to test one
claim — that the performance reported for [Rysana's unreleased AE
language](ae-research.md) follows from language semantics rather than from
compiler tricks. The relevant semantics are approximate floating point by
contract, value semantics without aliasing, compiler-owned data layout, and
whole-program compilation.

Measured result: 2.08× geomean over idiomatic C++ `-O3` for AOT output, from
defaults alone. Details in [experiments/RESULTS.md](experiments/RESULTS.md).

```
type Quat { w: f64, x: f64, y: f64, z: f64 }

operator* (a: Quat) · (b: Quat): f64 {
  return a.w * b.w + a.x * b.x + a.y * b.y + a.z * b.z
}

operator ‖(q: Quat)‖: f64 { return sqrt(q · q) }

property slerp_stays_unit(a: Quat, b: Quat, t: f64) {
  ‖slerp(normalize(a), normalize(b), t)‖ ~= 1.0
}
```

The language has records, enums, `match`, user-defined Unicode operators
(infix by precedence anchor, circumfix pairs), an order-free `sum` reduction
primitive, and property-based tests with counterexample shrinking.
[SPEC.md](SPEC.md) is the frozen v0.1 specification.

## Building

```
cargo build --release
```

Requires a Rust toolchain, `clang` for AOT output, and optionally
[Zig](https://ziglang.org/) for wasm targets and Python 3 for the tools.

## Usage

```
lu run     prog.lu              execute via the JIT
lu interp  prog.lu              execute via the reference interpreter
lu build   prog.lu              AOT compile through LLVM
lu build --fast prog.lu         AOT compile through Cranelift (dev loop)
lu test --runs 1000 prog.lu     run property tests
lu check   prog.lu              typecheck only
lu fmt [--check] prog.lu        canonical layout and Unicode operators
lu bench [--runs N] [prog.lu]   interpreter/JIT/AOT whole-process timings
lu doc [-o dir] [prog.lu]       generate documentation

lu build --target wasm32-wasi prog.lu
lu build --target wasm32-web  prog.lu
lu build --lib [--shared] -o kernel prog.lu
lu build --emit-llvm -o kernel.ll prog.lu

lu abi check old.json new.json  compare generated ABI manifests
lu sdk  rust|cpp|julia|node|go|swift|r -o out kernel.json
lu bindgen --lib m -o math.lu /usr/include/math.h
lu lsp                          language server
lu init | lu add | lu fetch     package commands
```

Self-hosting and verification:

```
lu run selfhost/interp.lu prog.lu     lulang interpreter written in lulang
selfhost/build.sh prog.lu             compile with the lulang compiler
selfhost/build.sh --bootstrap         3-stage self-compilation, checks the fixpoint
python3 tools/verify_corpus.py        four-tier differential gate
experiments/alcubierre.sh             replicate AE's "alcubierre" benchmark
```

## Implementation

Every mode runs the same front end: lex, parse into a flat arena AST, check,
then lower to a typed CFG. Execution APIs accept only `LoweredProgram`, so
unchecked parser output cannot reach an interpreter or a code generator. The
reference interpreter and the property engine execute the CFG directly;
Cranelift and LLVM emit its blocks and instructions. The source declaration
view is used only for record layout and ABI names. Shared component layout,
the flattened calling convention, and optimization analysis live in
`src/backend/`, separate from the two emission modules. `tests/conformance.rs`
generates programs and diffs interpreter, JIT, AOT, and self-hosted output.

The Cranelift code generator drives two tiers from one implementation: the
in-memory JIT behind `lu run`, and object emission behind `lu build --fast`.
They differ only in how a program's edges are resolved — the JIT bakes host
addresses for string literals and resolves externs in process, while object
output emits data symbols and leaves externs to the linker. `--fast` skips
LLVM entirely and builds the self-hosted compiler 5.8× faster
(1.73 s → 0.30 s), at the cost of Cranelift's code quality rather than
`clang -O3`'s: the benchmark kernels run 1.06–2.8× slower than a `lu build`
binary. It is the dev-loop build; the measured numbers below come from
`lu build`.

The middle end produces a target-independent legality proof and expression
plan for order-free `sum` reductions; the JIT and LLVM consume it as
four-accumulator vector loops with scalar tails. f64 uses f64x2, f32 uses
f32x4, and wrapping integer arithmetic uses exact i64x2 lanes without passing
through floating point. wasm builds enable SIMD128. Also present: branch-free
inline sin/cos/acos kernels (musl polynomials emitted as Cranelift IR),
if-conversion of speculation-safe `if`s, LICM including pure math libcalls,
and SoA field planes for record arrays. Each is ablatable — `LU_MATH=call`,
`LU_IFCONV=off`, `LU_LICM=off`, `LU_SIMD=off`, `LU_LAYOUT=aos`, `LU_INLINE=n`.

The Cranelift tiers inline before code generation, bounded by a per-function
budget of 256 IR instructions. Measurement set that number: inlining pays for
the user-operator chain in `bench_slerp` (21% if disabled outright) and is
fully paid off by ~128 instructions, while every instruction past that is
compile time the JIT re-pays on every run. The LLVM tier does not use this
inliner at all; it emits one LLVM function per lulang function and lets
`clang -O3` decide.

Arrays are values in every tier. Persistent stores retain independent values;
immutable parameters borrow and `inout` parameters are exclusive. The runtime
uses copy-on-write, and compiled tiers clone owning components at persistent
value boundaries, including arrays nested in records. Array storage packs
components to their natural widths — `f32` occupies 4 bytes — with each SoA
plane separately 8-byte aligned, and the 16-byte array header caches the
logical length so bounds checks and slice coercions need no division by the
element stride. `f32` is a distinct IEEE-754 binary32 type throughout.

## Self-hosting

The compiler is rewritten in lulang as a ladder, each rung run by the tier
below it: `selfhost/lexer.lu`, `parser.lu`, `checker.lu`, then `interp.lu`
(the full language, able to run its own source; interpreter towers reach depth
3), then `codegen.lu`, an LLVM AOT backend. codegen.lu's front end is a byte
copy of interp.lu up to its evaluator marker; its back end mirrors
`src/llvm.rs` — same flattened multi-component values, fast-flagged FP, SoA
record arrays, hoisted bounds checks, and C runtime ABI.

`selfhost/build.sh --bootstrap` compiles codegen.lu with the interpreter, uses
the result to compile codegen.lu again, and repeats. Stage-2 and stage-3 IR
must be byte-identical; stage 1 is checked against them too. Self-compilation
takes 1.6 s through the JIT and 0.21 s compiled (best-of-5; the JIT figure was
3.5 s before the inline budget was measured down to 256). The verified stage-2
binary is installed as `target/release/luc`.

A fixpoint only proves self-consistency, so it is not the correctness
argument. The independently written Rust tiers are the oracle: every corpus
program is run through the reference interpreter, the JIT, host AOT,
interp.lu on the host, and luc, and all five outputs must agree. The only
tolerated drift is the last float digit of fast-math reductions, with host AOT
as reference.

## C ABI

`extern` imports and `export fn` cross a deliberately narrow boundary. Ordinary
lulang records and arrays keep compiler-controlled layout; no internal ABI is
promised.

The boundary subset is `i64`, `f32`, `f64`, `bool` (0/1 as `int64_t`), enums
(i64 tag), `str` as `(const char *, int64_t)` parameters and
`const char *fn(..., int64_t *out_len)` returns, and `[i64]`/`[f64]` as
`(T *data, int64_t n)`. Signatures are capped at 6 integer-class and 8
float-class components, which keeps every argument in registers on both SysV
x86-64 and AArch64. `c_ptr[T]` handles cross, are stored, passed and compared,
but cannot be dereferenced. `c_slice[T]` is a read-only borrowed view;
`c_mut_slice[T]` is an exclusive writable one, parameter-only, and cannot
alias a sibling argument — so exported kernels read and write C and NumPy
buffers with no array copy. Exact record layout is opt-in via `@c_layout`;
flat one- or two-field homogeneous 64-bit records pass directly by value.
`c_fn[(...) -> T]` transports C callbacks in every tier.

`lu build --lib` writes the library, a C header, and a JSON ABI manifest.
`lu abi check` compares two manifests and fails on removed exports, changed
types or layouts, changed enum tags, and library renames; additions and
parameter renames pass. `lu sdk` turns a manifest into Rust, C++, Julia, Node,
Go, Swift, or R wrappers; the release suite compiles and runs the Rust, C++,
Go, and Swift ones, runs Julia and R when installed, and syntax-checks Node.

`lu bindgen` reads C headers and writes checker-valid `extern` declarations:
constants, sequential enums, typedef-resolved parameters, and functions with
an exact boundary representation. Narrower integers, C `bool`, by-value
structs, bitfields, and flat aggregate returns go through a generated adapter
written beside the bindings as `*.bindgen.c` (`--no-shims` emits only
declarations needing none). Variadics produce explicit wrappers for zero to
three `i64`/`f64` arguments, named by pattern (`log_v_i64_f64`); untyped `...`
is not pretended to be type-safe. By-value unions, callbacks mixed with
shim-only parameters, nested aggregate-result adapters, and wider variadic
patterns are explicit diagnostics. A macOS `math.h` preflight yields 41 direct
imports.

## Python

```python
import pylulang
module = pylulang.compile(open("corpus/kernel_saxpy.lu").read())
module.saxpy(2.0, [1.0, 2.0, 3.0], [10.0, 20.0, 30.0], 3)
```

`pylulang` is pure Python: it compiles the source and drives the generated ABI
manifest. Contiguous writable NumPy `float64`/`int64` arrays and compatible
buffers pass straight to the C shim. `str` results become `bytes` with their
exact length, keeping embedded NULs. Install with
`python3 -m pip install python/pylulang`.

## WebAssembly

`--target wasm32-wasi` produces a command module for a preview1 host.
`--target wasm32-web` produces a reactor module and a dependency-free loader:

```javascript
import { instantiateLulang } from "./slerp.js";
const program = await instantiateLulang("./slerp.wasm", console.log);
program.run();
```

Both consume the same validated CFG and runtime as native AOT. Native dynamic
`extern` declarations are rejected rather than left as unresolved imports.

## Packages

Registry-free and source-based. `lu add name --git URL --rev REV` resolves the
revision to an immutable commit and tree, writes `lu.lock`, and stores the
checkout by commit ID in a content-addressed cache (`LULANG_CACHE` overrides
the location); later builds follow the lock even if the branch moves.
Dependencies provide `src/lib.lu`, roots provide `src/main.lu`. Each file is
parsed once into its own arena; `use name` imports a checked namespace and
`use name as local` renames it. The module linker remaps arena and symbol IDs
with collision-proof internal names before a single whole-program typecheck
and optimization pass.

## Libraries

- [`lib/lu-numerics`](lib/lu-numerics) — 26 kernels across vectors,
  statistics, integration, dense linear algebra, signal processing, random and
  Monte Carlo work, optimization, geometry, and special functions. Every export
  has an executable law, a benchmark entry, a generated page, and a
  C++/NumPy/Julia reference.
- [`lib/luphysics`](lib/luphysics) — N-body integration, rigid-circle
  impulses, conservation laws, native and WASI builds, an exported SoA kernel,
  and an optional raylib visualizer.
- [`lib/luimage`](lib/luimage) — Mandelbrot rendering into a C host's pixel
  buffer through `c_mut_slice[f64]`; `./run_preview.sh` writes a viewable PGM.
- [`lib/ludiff`](lib/ludiff) — forward-mode automatic differentiation as
  ordinary library code: a two-field `Dual` record, user-defined `⊕ ⊖ ⊗ ⊘`,
  derivative rules, and nine laws including a finite-difference check. There is
  no differentiation pass in the compiler.
- [`lib/lutelegram`](lib/lutelegram) — Telegram Bot API client generated from
  the official documentation page: 362 types, 26 unions, 185 methods at the Bot
  API 10.2 snapshot. Only HTTPS is foreign, through a small libcurl bridge.

## Tooling

`lu lsp` is a dependency-free language server with live diagnostics,
formatting, symbols, typed hover and completion, go-to-definition for
functions and operators, and property lenses that publish shrunk
counterexamples on the declaration. The VS Code extension in `editors/vscode`
bundles the same features, format-on-save, and Unicode input snippets; the
tree-sitter grammar is in `editors/tree-sitter-lulang`.

`lu doc` emits one page per function with adjacent `///` prose, examples,
property status, benchmark history, the exported C signature, the ABI
manifest, source, and generated LLVM. Package docs execute the laws in
`tests/*.lu`, so a documented claim is an executed one. `benchmarks/run_observatory.py`
regenerates the [observatory](https://lulang.lulzx.space/observatory), which
links every measurement to its lulang, C++, Rust, Julia, NumPy and JavaScript
source with machine provenance and the semantic assumptions behind the number.

## Documents

| File | Contents |
|---|---|
| [ae-research.md](ae-research.md) | What is publicly known about AE, with sources |
| [DESIGN.md](DESIGN.md) | Reverse-engineering AE's architecture |
| [SPEC.md](SPEC.md) | The frozen lulang v0.1 specification |
| [experiments/RESULTS.md](experiments/RESULTS.md) | Measurements validating the semantics thesis |
| [ROADMAP.md](ROADMAP.md) | Ecosystem plan |
| [M8-PLAN.md](M8-PLAN.md) | C ABI milestone plan; superseded, kept as history |
| [KNOWN-ISSUES.md](KNOWN-ISSUES.md) | Fixed regressions, with repros |
| [ROADMAP-AUDIT.md](ROADMAP-AUDIT.md) | Requirement-by-requirement verification evidence |

Online interpreter: <https://lulang.lulzx.space>.

## Status

M0 spec and corpus, M1 front end and interpreter, M2 JIT, M3 LLVM AOT, M4
property engine, M5 middle end, M6 self-hosted interpreter, M7 bootstrapping
compiler, and M8 C ABI are complete. Measured: 2.08× geomean over idiomatic
C++ for AOT; JIT slerp 1.7× and dot 1.3×; a record-array kernel 1.4× over C++
`-O3` in both compiled tiers.
