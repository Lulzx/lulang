# Experiment 1 — do AE's numbers fall out of semantic defaults?

*2026-07-22, Apple M4 Pro, Apple clang 21, Bun 1.3.14. Kernels: `bench.cpp` /
`bench.ts`. Best-of-30 per kernel, ms, lower is better.*

## Hypothesis under test

AE's "faster than C++" is mostly (1) fast-math semantics by default, (2) no aliasing,
(3) layout freedom — i.e. things a language *contract* can grant that C++ flags/idioms
don't. If true, `clang++ -O3 -ffast-math` (+ restrict + SoA) should approximate
"AE AOT", plain `-O3` is "idiomatic C++", and Bun is the TS baseline.

## Runtime results

| Kernel | C++ `-O3` | C++ `-O3 -ffast-math` | speedup | Bun (TS) | "AE-sim" vs Bun |
|---|---|---|---|---|---|
| dot (2M reduction) | 1.177 | **0.301** | **3.9×** | 1.489 | 4.9× |
| nbody (1500²) | 2.127 | **1.276** | **1.67×** | 3.393 | 2.7× |
| slerp AoS (200k) | 5.126 | 4.335 | 1.18× | 6.453 (obj) | 1.5× |
| slerp SoA (200k) | 5.239 | **3.841** | 1.33× vs base AoS | — | 1.7× |

Geomean of "semantic defaults" speedup vs idiomatic C++ `-O3`: **~1.9×** across
dot/nbody/slerp-SoA — squarely inside AE's claimed 1.8–2.2×.

### Reading per kernel

- **dot, 3.9×**: reassociation legalizes vectorized reduction — the single biggest
  lever, and pure semantics (idiomatic C++ can't have it without `-ffast-math`
  because IEEE addition isn't associative). The `≈`-operator-implies-approx-FP
  hypothesis looks right.
- **nbody, 1.67×**: fast-math enables rsqrt approximations + better FMA/vector
  codegen. Almost exactly AE's alcubierre native ratio (6.96/4.43 = 1.57×).
- **slerp, only 1.18–1.33×**: dominated by scalar `acos`/`sin` libcalls, which
  `-ffast-math` can't vectorize without a vector math library. **This is the gap in
  the simulation**: a language shipping its own vectorized transcendentals
  (SLEEF-class, 4-8 lanes per call) would win big here where C++-with-libm can't.
  Likely a real AE ingredient. SoA layout adds a further ~13% over AoS.
- **Bun**: with typed arrays JSC is only ~2.7× behind "AE-sim" (geomean); the
  idiomatic-objects slerp shows allocation cost. AE's ~5× JIT claim vs js is
  plausible for idiomatic (object-heavy, allocating) TS code rather than
  typed-array-tuned TS — consistent with their "apples to apples idiomatic" framing.

## Compile-time results (`bench.cpp`, ~230 lines)

| Measurement | Time |
|---|---|
| Full `-O3 -ffast-math` compile (warm) | ~0.35 s |
| Frontend only (`-fsyntax-only`) | ~0.26 s |
| Empty no-header file, frontend | ~0.07 s |

**~75% of compile time is the C++ frontend**, and most of *that* is parsing
`<vector>/<chrono>/<cstdio>` headers — not our 230 lines, and not optimization
(~0.1s). A language with real modules and a near-zero frontend pays only the ~0.1s
backend cost → ~3.5× faster on this tiny case, growing with project size as header
re-parsing compounds. AE's "60% on small benchmarks, ~10× on large projects, 1000×
less overhead outside the backend" matches this shape exactly.

## Verdict

The hypothesis survives contact with measurement:

1. **~1.9× geomean over idiomatic C++ from semantics alone** (fast-math + restrict +
   SoA) — matching AE's claimed 1.8–2.2× band without any compiler novelty.
2. **C++ compile time is frontend/header-dominated** — the 10×-compile claim needs no
   backend magic, just not-being-C++ in the frontend.
3. **Identified missing ingredient**: vectorized transcendental math library — the
   slerp gap says AE likely ships one; lulang should plan for SLEEF or equivalent
   (M5 of the roadmap).
4. Bun is closer than AE's marketing on typed-array code; the 5–9× JIT claims imply
   idiomatic/allocating JS baselines.

## Follow-ups

- Add a SLEEF-backed slerp variant to quantify the transcendental win.
- Rust baseline (`-C target-cpu=native`, with/without fast-math intrinsics) to test
  the "beats Rust too" claim.
- A `main`-to-exit wall-clock harness (their alcubierre measures whole-process,
  including startup — our numbers are kernel-only).
