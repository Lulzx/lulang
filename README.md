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
```

## Status

| Milestone | State |
|---|---|
| M0 — spec + benchmark corpus | done |
| M1 — lexer, parser, typechecker, interpreter | done |
| M2 — Cranelift JIT (`lu run`): inlining, 4-acc `sum`, hoisted bounds checks, opt_level=speed | done — 2.6× over Bun on dot; slerp needs pure-call LICM |
| M3 — LLVM AOT (`lu build`): fast-flagged IR via clang | **done — 2.08× geomean over idiomatic C++, inside AE's claimed band** |
| M4 — property engine with counterexample shrinking | done |
| M5 — middle-end: inline math kernels (JIT LICM + vectorization), SoA layout | planned |

Known v0.1 deviations from spec: arrays are reference-backed in the interpreter
(aliasing is observable through them — will be fixed when the IR lands); functions
must end in an explicit `return` unless they return the block's final expression.
