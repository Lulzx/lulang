# lulang

[Website and online interpreter](https://lulang.lulzx.space)

A numerics-first programming language built from scratch to test a thesis: that the
extraordinary performance claims of [Rysana's unreleased AE
language](ae-research.md) fall out of **language semantics**, not compiler magic —
approximate floating point by contract, value semantics with no aliasing, compiler-
owned data layout, and whole-program compilation.

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

User-defined Unicode operators (infix by precedence-anchor, circumfix pairs),
records, an order-free `sum` reduction primitive, and first-class property-based
testing.

## Documents

| File | What it is |
|---|---|
| [ae-research.md](ae-research.md) | Everything publicly known about AE, with sources |
| [DESIGN.md](DESIGN.md) | Reverse-engineering AE's architecture; three revisions deep |
| [SPEC.md](SPEC.md) | The frozen lulang v0.1 language specification |
| [experiments/RESULTS.md](experiments/RESULTS.md) | Measurements validating the semantics thesis (~1.9× over idiomatic C++ from defaults alone) |
| [ROADMAP.md](ROADMAP.md) | Ecosystem growth plan: C ABI boundary, pylulang, bindgen, tooling, playground, showcase apps |
| [M8-PLAN.md](M8-PLAN.md) | Implementation plan for the C ABI milestone (extern/export, per-tier, slices, test plan) |
| [KNOWN-ISSUES.md](KNOWN-ISSUES.md) | Fixed JIT correctness and bootstrap-memory regressions |
| [ROADMAP-AUDIT.md](ROADMAP-AUDIT.md) | Requirement-by-requirement implementation and verification evidence |

## Milestone status

| Milestone | Status |
|---|---|
| M8 C imports (`extern`) | Complete across host interpreter, JIT, AOT, and self-hosted interpreter |
| M8 C exports (`export fn`) | Static/shared libraries, C headers, JSON manifests, and array copy-in/out complete |
| M8 C ABI / FFI | Complete; C and `ctypes` integration tests plus four-tier import conformance |

## Usage

```
cargo build --release
./target/release/lu run  corpus/slerp.lu   # execute main
./target/release/lu test --runs 1000 corpus/slerp.lu # property tests, configurable runs
./target/release/lu build corpus/slerp.lu  # AOT-compile via LLVM
./target/release/lu build --target wasm32-wasi corpus/slerp.lu
./target/release/lu build --target wasm32-web corpus/slerp.lu
./target/release/lu build --lib -o kernel corpus/kernel_saxpy.lu
./target/release/lu build --lib --shared -o kernel corpus/kernel_saxpy.lu
./target/release/lu abi check released/kernel.json kernel.json
./target/release/lu sdk rust -o kernel.rs kernel.json
./target/release/lu sdk go -o kernel-go kernel.json
./target/release/lu bindgen --lib m -o math.lu /usr/include/math.h
./target/release/lu bench --runs 7 corpus/bench_dot.lu
./target/release/lu doc --runs 100 -o target/doc corpus/kernel_saxpy.lu
./target/release/lu build --emit-llvm -o kernel.ll corpus/kernel_saxpy.lu
./target/release/lu fmt corpus/slerp.lu    # canonical Unicode operators + layout
./target/release/lu fmt --check corpus/slerp.lu
./target/release/lu run selfhost/lexer.lu  # the lulang lexer, written in lulang
./target/release/lu run selfhost/parser.lu # the lulang parser, written in lulang
./target/release/lu run selfhost/checker.lu # the lulang typechecker, written in lulang
./target/release/lu run selfhost/interp.lu # lulang running lulang: full front end + evaluator
./target/release/lu run selfhost/interp.lu prog.lu                     # run a file
./target/release/lu run selfhost/interp.lu selfhost/interp.lu prog.lu # interpreter tower
./experiments/alcubierre.sh          # replicate AE's "alcubierre" benchmark table
./selfhost/build.sh prog.lu          # AOT-compile prog.lu with the compiler written in lulang
./selfhost/build.sh --bootstrap      # 3-stage self-compilation; verifies the IR fixpoint
python3 tools/verify_corpus.py       # four-tier corpus differential gate
```

### Python kernels

The pure-Python `pylulang` package compiles source through the generated ABI
manifest and exposes each `export fn` as a callable:

```python
import pylulang

module = pylulang.compile(open("corpus/kernel_saxpy.lu").read())
x = [1.0, 2.0, 3.0]
y = [10.0, 20.0, 30.0]
total = module.saxpy(2.0, x, y, 3)  # y is copied back: [12, 24, 36]
# A lulang `str` result is returned as exact bytes, including embedded NULs.
```

Writable contiguous NumPy `float64`/`int64` arrays and compatible Python
buffers are passed directly to the generated C shim. Install the local package
with `python3 -m pip install python/pylulang`. Exported `str` results become
Python `bytes`, preserving their explicit length rather than decoding or
stopping at a NUL byte.

### C header imports

`lu bindgen` reads C headers and writes lulang `extern` declarations:

```bash
lu bindgen --lib m -o math.lu /usr/include/math.h
lu check math.lu
```

The importer emits constants, sequential enums, typedef-resolved parameters,
and functions whose C types have an exact lulang boundary representation.
Raw pointers and opaque structs are emitted as boundary-only `c_ptr[T]`
handles: they may cross an `extern` boundary, be stored, passed, and compared,
but cannot be dereferenced or used to expose C layout. Declarations involving
narrower C integers, C `bool`, and non-portable by-value struct parameters use
a generated C adapter shared library; C `float` maps directly to `f32`.
Flat `@c_layout` records containing one or two homogeneous 64-bit fields
(integer/pointer or `f64`) pass directly by value on SysV x86-64 and AArch64.
The public lulang wrapper keeps the useful logical type while a remaining
private adapter crosses only the stable scalar ABI.
Callback typedefs and inline function-pointer parameters become typed
`c_fn[(...) -> T]` values. C unions remain opaque but are available behind
typed `c_ptr[Union]` handles. Bitfield records use logical Lulang records and
adapter-side field initialization/access. Portable aggregate returns pass
directly; other flat scalar struct returns are captured once in an
adapter-owned temporary, copied field-by-field into a Lulang record, and
released before the wrapper returns.

With adapters enabled, a C variadic function produces explicit wrappers for
zero through three `i64`/`f64` varargs, named with their exact pattern—for
example `log_v_i64_f64`. The caller must select the wrapper matching the C
function's promoted argument contract. This deliberately avoids pretending
that an untyped `...` is type-safe.

Borrowed `c_slice[i64]` and `c_slice[f64]` parameters cross as
`(const T *data, int64_t length)`. They are read-only, cannot escape by
returning, and let exported kernels consume C and NumPy buffers without an
array copy. Borrowed `c_mut_slice[i64]` and `c_mut_slice[f64]` cross as
`(T *data, int64_t length)` and write caller-owned buffers directly. Mutable
slices are parameter-only, require a mutable variable at lulang call sites,
and are exclusive: they cannot alias any sibling argument. Compiler-owned
arrays retain value semantics when borrowed; exported C and NumPy buffers are
never wrapped in or copied through the ordinary array runtime.

### ABI compatibility

`lu abi check <old.json> <new.json>` compares generated ABI manifests for CI
and release gating. Added exports, records, and enum values are compatible;
parameter renames are reported without failing. Removed exports, changed
parameter or return types, changed record layouts, changed existing enum tags,
and library renames are breaking and produce a failing exit status. Both
inputs must use a manifest version understood by the compiler.

### Generated host SDKs

An ABI manifest can be turned into host wrappers or complete package
directories:

```text
lu sdk rust  -o kernels.rs  kernels.json
lu sdk cpp   -o kernels.hpp kernels.json
lu sdk julia -o Kernels.jl  kernels.json
lu sdk node  -o kernels-node  kernels.json
lu sdk go    -o kernels-go    kernels.json
lu sdk swift -o kernels-swift kernels.json
lu sdk r     -o kernels-r     kernels.json
```

The Rust SDK exposes slices and mutable slices as `&[T]` and `&mut [T]`, the
C++ SDK uses `std::span`/`std::string_view`, and the Julia SDK pins arrays
across `ccall`. All three copy returned length-delimited strings into
host-owned storage, preserve embedded NUL bytes, convert ABI booleans, and
generate exact `@c_layout` record definitions where the host language needs
them. Exported `[i64]`/`[f64]` results become zero-copy owning handles: Rust
and C++ release them with RAII, Julia supplies a finalizer and `close`, and
`pylulang.OwnedArray` supplies sequence access plus a context manager. Rust
and C++ wrappers are compiled and executed against a generated static library
in the release suite; Julia is executed too when its runtime is available.
The standard `@c_layout { status: i64, value: i64 }` result convention maps
status zero to success and nonzero codes to each host SDK's error path.
Typed `c_fn[(...) -> T]` parameters and returns become exact C function
pointer typedefs, Rust `extern "C" fn` values, C++ callback aliases, Julia
`Ptr{Cvoid}`, and retained Python callables.

The Node target emits an npm package using `ffi-napi`, including typed
records, callbacks, buffer views, owning-result wrappers, and error
translation. Go emits a cgo module with zero-copy slices and owning views;
Swift emits a SwiftPM package with a system-library target and native
convenience wrappers; R emits an installable source package with registered
`.Call` adapters. The release suite executes real Go and Swift callers,
syntax-checks Node without downloading dependencies, and runs Julia/R when
their runtimes are installed.

Returned strings use `const char *fn(..., int64_t *out_len)`: the hidden
length pointer is the final C argument. Imports copy the returned bytes into
lulang-owned storage before the foreign call completes. Exports return
library-lifetime storage and write the exact byte count, so strings are
length-delimited and may contain embedded NUL bytes.
This works in the interpreter, JIT, LLVM AOT, and self-hosted compiler without
making compiler-owned record layout part of the C ABI. The reproducible
adapter source is written beside the bindings as `*.bindgen.c`; use
`--no-shims` to emit only declarations that need no adapter. By-value unions,
callbacks mixed with otherwise shim-only parameters, nested aggregate-result
adapters, and variadic patterns wider than three values remain explicit
diagnostics.

Bindgen recognizes conventional adjacent `const T *data, int64_t length` and
`T *data, int64_t length` pairs for 64-bit scalar elements and emits
`c_slice[T]` and `c_mut_slice[T]` respectively. Pairing requires a conventional
length name (`n`, `len`, `length`, `count`, or a data-name suffix), avoiding
speculative conversion of unrelated pointer and integer parameters.

### Editor tooling

`lu lsp` starts the dependency-free Language Server with live diagnostics,
formatting, symbols, typed hover/completion, function and operator
go-to-definition, and executable property lenses. A failed lens publishes the
shrunk counterexample on the property declaration. Set `LULANG_LSP` only when
the Python server is installed outside the usual source/share paths. The VS
Code extension in `editors/vscode` also provides these features directly,
enables format-on-save, and includes `dot`, `norm`, and `approx` Unicode input
snippets. The tree-sitter grammar and highlight query live in
`editors/tree-sitter-lulang`.

### WebAssembly targets

With [Zig](https://ziglang.org/) on `PATH`, `lu build --target wasm32-wasi`
produces a command module for a preview1 WASI host. The `wasm32-web` target
produces a reactor module plus a small dependency-free JavaScript loader:

```javascript
import { instantiateLulang } from "./slerp.js";

const program = await instantiateLulang("./slerp.wasm", console.log);
program.run();
```

Both targets consume the same validated CFG and runtime as native AOT.
Native dynamic `extern` declarations are rejected for wasm builds rather than
becoming unresolved imports.

### Git packages

Packages are deliberately registry-free and source-based:

```bash
mkdir orbit && cd orbit
lu init orbit
lu add numerics --git https://github.com/example/lu-numerics --rev v0.1.0
lu run
lu test --runs 1000
lu build
```

`lu add` resolves the requested Git revision to an immutable commit and tree,
writes `lu.lock`, and stores the checkout by commit ID in the content-addressed
cache. Later builds use the lock even if a branch or tag moves. Dependencies
provide `src/lib.lu`; the root provides `src/main.lu`, and `use name` must
refer to a declared dependency. Each source file is parsed once into an
independent module arena. Imported functions, records, and enums can be
qualified (`numerics.dot`, `geometry.Vec3`); qualification prevents dependency
names from colliding with root declarations. Imports may be locally renamed
with `use numerics as numbers`. The module linker remaps arena and symbol IDs
and uses collision-proof internal names before one whole-program typecheck and
optimization pipeline. Set
`LULANG_CACHE` to override the cache location.

### Flagship: luphysics

[`lib/luphysics`](lib/luphysics) is the end-to-end showcase: value-semantic
vectors and bodies, softened N-body integration, rigid-circle impulses,
executable conservation laws, native/WASI builds, an exported SoA integration
kernel with a generated C header, and an optional raylib visualizer. Run
`lu run` or `lu test --runs 1000` from that directory.

The [Embedded notebook](examples/lulang_embedded.ipynb) compiles the
quaternion-slerp export with `pylulang`, checks it against NumPy, and measures
both implementations. Run it without a Jupyter dependency with
`python3 examples/run_embedded_notebook.py`.

### Visible C embedding: luimage

[`lib/luimage`](lib/luimage) renders a Mandelbrot image through a real C host.
The host allocates the pixels, Lulang fills them through
`c_mut_slice[f64]`, and the resulting `target/mandelbrot.pgm` is directly
viewable. Run `./run_preview.sh` from that directory. The package also ships
exposure, inversion, and luminance kernels, executable image laws, generated
documentation, and interpreter/JIT/AOT/C integration coverage.

### First-party numerics

[`lib/lu-numerics`](lib/lu-numerics) is a package of 26 documented kernels
across vectors, statistics, integration, dense linear algebra, signal
processing, random/Monte Carlo work, optimization, geometry, and special
functions. Every export is tied to an executable law, benchmark entry,
generated function page, and C++/NumPy/Julia reference source.

### Autodiff: ludiff

[`lib/ludiff`](lib/ludiff) implements forward-mode automatic differentiation
as ordinary library code: a two-field `Dual` record, user-defined `⊕`, `⊖`,
`⊗`, and `⊘` operators, elementary derivative rules, and nine executable laws
including a finite-difference check. No compiler differentiation pass exists.
The exported example returns a scalar derivative through the stable C ABI
without exposing `Dual` record layout.

### Telegram Bot API: lutelegram

[`lib/lutelegram`](lib/lutelegram) is a document-generated Telegram Bot API
client. Its reproducible pipeline parses the official Bot API page into a
checked-in JSON schema, then generates all LuLang object wrappers, union
specifications, field accessors, method parameter records, typed responses,
and documentation comments. The current Bot API 10.2 snapshot covers 362
types, 26 unions, and 185 methods. A small libcurl bridge supplies HTTPS while
the request encoding and response navigation remain LuLang code.

### Executable documentation and benchmarks

`lu bench [--runs N] [file.lu]` measures whole-process interpreter, JIT, and
AOT execution and appends the result to `benchmarks/history.csv`. With no file,
it resolves the current `lu.toml` package.

`lu doc [--runs N] [-o directory] [file.lu]` generates a static site containing
one page per function, adjacent `///` prose, example calls, related property
statuses, local benchmark history, exported C signatures, the ABI manifest,
source, and generated LLVM. Package docs include laws from `tests/*.lu`, so the
status shown beside an API is an executed claim rather than copied prose.

The generated benchmark observatory links every measurement to its lulang,
C++, Rust, Julia, NumPy, and JavaScript source, publishes the selfhost result
and measurement environment when available, and names semantic/layout
differences. Regenerate the checked-in matrix with
`python3 benchmarks/run_observatory.py --runs 7 --bootstrap`; a scheduled
workflow produces the same source-linked artifact.

## Architecture

One front end, four back ends, packaged as the `lu_syntax`, `lu_check`, `lu_ir`,
`lu_jit`, `lu_llvm`, `lu_test`, and `lu` workspace crates. Every execution mode
runs lex → parse → typecheck over a flat arena AST, then dispatches:

```
                         prog.lu
                            │
              ┌─────────────▼──────────────┐
              │        FRONT END           │
              │  lexer.rs  → tokens        │
              │  parser.rs → flat AST      │   (arena tables, ExprId indices;
              │  check.rs  → validation    │    match desugared at parse)
              │  ir.rs     → lowered CFG   │   (typed values/locals, resolved
              │                           │    calls/fields, explicit branches)
              └─────────────┬──────────────┘
                            │
      ┌──────────────┬──────┴───────┬──────────────┐
      ▼              ▼              ▼              ▼
 ┌──────────┐  ┌───────────┐  ┌───────────┐  ┌──────────┐
 │lu interp │  │  lu run   │  │ lu build  │  │ lu test  │
 │          │  │           │  │           │  │          │
 │ CFG      │  │ Cranelift │  │ textual   │  │ property │
 │ executor │  │ JIT       │  │ LLVM IR   │  │ engine + │
 │ eval     │  │           │  │   │       │  │ shrinker │
 │          │  │ inlining  │  │   ▼       │  └──────────┘
 │reference │  │ shared    │  │ explicit  │
 │semantics │  │ SIMD plan │  │ SIMD IR   │
 └──────────┘  │ if-conv   │  │   ▼       │
               │ SoA arrays│  │ native +  │
               │ math krnls│  │lu_runtime.c
               └───────────┘  └───────────┘
```

Execution APIs accept only `LoweredProgram`; unchecked parser output cannot
reach an interpreter or code generator. The reference interpreter and property
engine execute its CFG directly. Cranelift and LLVM emit the same CFG's blocks
and typed instructions directly; the source declaration view is used only for
record layout and ABI names. Lowering/validation lives in `ir.rs`; shared
component layout, flattened ABI, and optimization analysis live under
`src/backend/`, separate from the Cranelift and LLVM emission modules.
`tests/conformance.rs` generates small programs and diffs reference
interpreter, JIT, AOT, and self-hosted-interpreter output automatically.

The same architecture is rewritten in lulang itself as a ladder — each rung
written in lulang, run by the tier below:

```
   rung                          surface
   ────────────────────────────────────────────────────────
   lexer.lu      ──┐             tokens only
   parser.lu       │ early       + flat AST, types dropped
   checker.lu      │ rungs       + types kept, core subset
                 ──┘
   interp.lu       full language: lex+parse+check+eval
                   │  can run its own source (tower, depth 3)
                   ▼
   codegen.lu      AOT compiler = shared front end + IR emitter
                   ┌──────────────────┬─────────────────────┐
                   │ front end        │ back end            │
                   │ BYTE COPY of     │ mirrors src/llvm.rs │
                   │ interp.lu        │ same ABI, fastmath, │
                   │ up to its        │ SoA, bounds hoisting│
                   │ evaluator marker │                     │
                   └──────────────────┴─────────────────────┘
```

`selfhost/build.sh --bootstrap` closes the loop: codegen.lu compiles itself
(interpreted), the result compiles itself, and again — stage-2 and stage-3 IR
must be byte-identical:

```
 stage 1          stage 2               stage 3
 ───────          ───────               ───────
 lu run
 codegen.lu ──ll──▶ cg1 (native)
 (codegen.lu        │
  compiles          │ compiles codegen.lu
  itself,           ▼
  interpreted)     cg2.ll ──clang──▶ cg2 (native)
                    │                 │ compiles codegen.lu
                    │                 ▼
                    │                cg3.ll
                    │                 │
                    └────── cmp ──────┘
                     byte-identical?  ── yes ──▶ install cg2
                                                 as target/release/luc
```

Day-to-day compilation then goes through the installed self-hosted compiler:

```
 prog.lu ──▶ luc ──▶ prog.ll ──▶ clang -O3 ──▶ a.out ◀── linked with lu_runtime.c
                    (textual                            (print, arrays,
                     LLVM IR,                            read_file/write_file,
                     fast flags)                         str = ptr+len protocol)
```

Correctness rests on a verification lattice, not the fixpoint alone (a fixpoint
only proves self-consistency — the independently written Rust tiers are the
oracle that catches a bug codegen.lu would faithfully preserve in itself):

```
                        prog.lu
        ┌──────────┬───────┼────────────┬─────────────┐
        ▼          ▼       ▼            ▼             ▼
    lu interp   lu run  lu build   interp.lu on   luc (selfhost
     (tree)     (JIT)   (host AOT)  the host       AOT)
        │          │       │            │             │
        └──────────┴───────┴─────┬──────┴─────────────┘
                                 ▼
                        diff — all identical
              (sole tolerated drift: last float digit on
               fast-math reductions; host AOT is reference)
```

## Status

| Milestone | State |
|---|---|
| M0 — spec + benchmark corpus | done |
| M1 — lexer, parser, typechecker, interpreter | done |
| M2 — Cranelift JIT (`lu run`): inlining, 4-acc `sum`, hoisted bounds checks, opt_level=speed | done — 2.6× over Bun on dot; pure-call LICM hoists invariant math libcalls (slerp's `acos(d)`/`sin(th)` under `LU_MATH=call`: 2.1×, at parity with the inline kernels) |
| M3 — LLVM AOT (`lu build`): fast-flagged IR via clang | **done — 2.08× geomean over idiomatic C++, inside AE's claimed band** |
| M4 — property engine with counterexample shrinking | done |
| M5 — middle-end: inline math kernels, if-conversion + LICM, SIMD `sum`, SoA record arrays | **done — JIT slerp 1.7×, dot 1.3×; record-array kernel beats idiomatic C++ `-O3` by 1.4× in both tiers** |
| M6 — self-hosting: full v0.1+v0.2 surface + lulang lexer, parser, typechecker, and interpreter in lulang, able to run **its own source** | **done — [selfhost/interp.lu](selfhost/interp.lu) handles records, enums, `match`, `sum`, user `operator`/`property` declarations, Unicode glyphs, string escapes, and file input; interpreter towers reach depth 3 (`--heap` scaling, 2.9 s AOT); the whole ladder *and* the slerp teaser corpus run on it byte-identically (`lu run selfhost/interp.lu corpus/slerp.lu`). All tiers print floats identically (shortest round-trip, plain notation)** |
| M7 — bootstrapping compiler: LLVM AOT backend in lulang ([selfhost/codegen.lu](selfhost/codegen.lu)) | **done — the front end shared with interp.lu plus an IR emitter mirroring `src/llvm.rs` (flattened multi-component values, fast-flagged FP, SoA record arrays, hoisted bounds checks, same C runtime ABI). Compiles the whole ladder and the teaser corpus byte-identically to `lu build`, compiles interp.lu into a native interpreter that reruns the ladder, and **compiles itself to a fixpoint**: stage-1 (interpreted), stage-2, and stage-3 binaries emit byte-identical IR (`selfhost/build.sh --bootstrap`; self-compilation drops from 6.5 s interpreted to 60 ms compiled)** |

The M5 middle-end supplies a target-independent legality proof and expression
plan for order-free `sum` reductions. Cranelift JIT and LLVM AOT consume that
plan as explicit four-accumulator vector loops with scalar tails: f64 uses
f64x2, while wrapping integer arithmetic uses exact i64x2 lanes without
passing through floating point. The self-host mirrors both expression subsets,
and wasm builds enable SIMD128. f32 reductions remain scalar because the
current internal array representation reserves an 8-byte slot per component;
packed f32 lanes require an array-layout migration first. The middle-end also
provides branch-free inline sin/cos/acos kernels (musl
polynomials as pure Cranelift IR), if-conversion of speculation-safe `if`s,
LICM, and SoA field planes for record arrays. Each is ablatable
(`LU_MATH=call`, `LU_IFCONV=off`, `LU_LICM=off`, `LU_SIMD=off`,
`LU_LAYOUT=aos`) — measurements in
[experiments/RESULTS.md](experiments/RESULTS.md).

The v0.1 surface is fully represented in the checked CFG. Arrays use value
semantics in every tier: persistent stores retain independent values, while
immutable parameters borrow and `inout` parameters have exclusive access.
The runtime uses copy-on-write where applicable and compiled tiers clone
owning components at persistent value boundaries, including arrays nested in
records. Functions may return the final expression of their body. `f32` is a
distinct IEEE-754 binary32 type throughout the host interpreter, JIT, and AOT
pipelines.
