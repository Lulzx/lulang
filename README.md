# lulang

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
./target/release/lu build --lib -o kernel corpus/kernel_saxpy.lu
./target/release/lu build --lib --shared -o kernel corpus/kernel_saxpy.lu
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
```

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
 │reference │  │ SIMD sum  │  │ clang -O3 │
 │semantics │  │ LICM      │  │   │       │
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

The M5 middle-end lives in the JIT tier (the AOT tier gets the equivalent from
LLVM, plus the same SoA layout): branch-free inline sin/cos/acos kernels (musl
polynomials as pure Cranelift IR), if-conversion of speculation-safe `if`s into
selects, a CLIF-level LICM pass, f64x2 vectorization of `sum`, and SoA field
planes for record arrays. Each is ablatable (`LU_MATH=call`, `LU_IFCONV=off`,
`LU_LICM=off`, `LU_SIMD=off`, `LU_LAYOUT=aos`) — measurements in
[experiments/RESULTS.md](experiments/RESULTS.md).

The v0.1 surface is fully represented in the checked CFG. Arrays use value
semantics in every host tier (copy-on-write in the reference interpreter and
explicit compiler-owned copies in compiled tiers), including arrays nested in
records. Functions may return the final expression of their body. `f32` is a
distinct IEEE-754 binary32 type throughout the host interpreter, JIT, and AOT
pipelines.
