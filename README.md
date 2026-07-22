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

## Usage

```
cargo build --release
./target/release/lu run  corpus/slerp.lu   # execute main
./target/release/lu test corpus/slerp.lu   # run property-based tests
./target/release/lu build corpus/slerp.lu  # AOT-compile via LLVM
./target/release/lu run selfhost/lexer.lu  # the lulang lexer, written in lulang
./target/release/lu run selfhost/parser.lu # the lulang parser, written in lulang
./target/release/lu run selfhost/checker.lu # the lulang typechecker, written in lulang
./target/release/lu run selfhost/interp.lu # lulang running lulang: full front end + evaluator
./target/release/lu run selfhost/interp.lu prog.lu                     # run a file
./target/release/lu run selfhost/interp.lu selfhost/interp.lu prog.lu # interpreter tower
```

## Status

| Milestone | State |
|---|---|
| M0 — spec + benchmark corpus | done |
| M1 — lexer, parser, typechecker, interpreter | done |
| M2 — Cranelift JIT (`lu run`): inlining, 4-acc `sum`, hoisted bounds checks, opt_level=speed | done — 2.6× over Bun on dot; slerp needs pure-call LICM |
| M3 — LLVM AOT (`lu build`): fast-flagged IR via clang | **done — 2.08× geomean over idiomatic C++, inside AE's claimed band** |
| M4 — property engine with counterexample shrinking | done |
| M5 — middle-end: inline math kernels, if-conversion + LICM, SIMD `sum`, SoA record arrays | **done — JIT slerp 1.7×, dot 1.3×; record-array kernel beats idiomatic C++ `-O3` by 1.4× in both tiers** |
| M6 — self-hosting: full v0.1+v0.2 surface + lulang lexer, parser, typechecker, and interpreter in lulang, able to run **its own source** | **done — [selfhost/interp.lu](selfhost/interp.lu) handles records, enums, `match`, `sum`, user `operator`/`property` declarations, Unicode glyphs, string escapes, and file input; interpreter towers reach depth 3 (`--heap` scaling, 2.9 s AOT); the whole ladder *and* the slerp teaser corpus run on it byte-identically (`lu run selfhost/interp.lu corpus/slerp.lu`). All tiers print floats identically (shortest round-trip, plain notation)** |

The M5 middle-end lives in the JIT tier (the AOT tier gets the equivalent from
LLVM, plus the same SoA layout): branch-free inline sin/cos/acos kernels (musl
polynomials as pure Cranelift IR), if-conversion of speculation-safe `if`s into
selects, a CLIF-level LICM pass, f64x2 vectorization of `sum`, and SoA field
planes for record arrays. Each is ablatable (`LU_MATH=call`, `LU_IFCONV=off`,
`LU_LICM=off`, `LU_SIMD=off`, `LU_LAYOUT=aos`) — measurements in
[experiments/RESULTS.md](experiments/RESULTS.md).

Known v0.1 deviations from spec: arrays are reference-backed in the interpreter
(aliasing is observable through them — will be fixed when the IR lands); functions
must end in an explicit `return` unless they return the block's final expression.
