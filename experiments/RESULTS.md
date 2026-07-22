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

---

# Experiment 2 — lulang JIT (M2) vs Bun, whole process

*2026-07-22, hyperfine, Apple M4 Pro. lu = Cranelift JIT with codegen inlining,
4-accumulator `sum`, and loop-hoisted bounds checks.*

| Workload | lu run | bun | ratio |
|---|---|---|---|
| dot 2M×20 (startup-dominated) | 26.5 ms | 39.9 ms | **1.50× lu** |
| slerp 2M (startup-dominated) | 36.9 ms | 32.1 ms | 0.87× |
| dot 2M×200 (steady state) | 213.5 ms | 270.9 ms | **1.27× lu** |
| slerp 20M (steady state) | 341.3 ms | 157.0 ms | 0.46× |

Findings:

1. **Bounds-check hoisting was worth 2.9×** on dot (75.7→26.5 ms): one range check
   per loop instead of a length-load + compare + branch per access. Safety
   preserved (out-of-range loops still trap with a clean error).
2. **Codegen inlining fixed the operator-chain tax**: before it, slerp lost to Bun
   even at small n because every `·`/`‖·‖`/`scale` was a real call.
3. **The slerp steady-state loss is missing LICM over pure calls**: `acos(d)` and
   `sin(th)` are loop-invariant but our backend re-executes them every iteration,
   while JSC hoists them (it knows Math.sin is pure). Next middle-end pass:
   purity-annotated math builtins + loop-invariant code motion.
4. Startup: lu ≈ 8 ms (parse+check+Cranelift), Bun ≈ 25 ms — the dev-loop win AE
   markets is real and cheap to get.

The ≥3× spec gate vs Bun is **not yet met**; consistent with the AE architecture
model, the naive tier buys rough parity and each middle-end pass buys a multiple.

---

# Experiment 3 — lulang AOT (M3) vs C++, Bun, and its own JIT

*2026-07-22, hyperfine, Apple M4 Pro. `lu build` emits fast-flagged LLVM IR
(+ `memory(none)` math decls, hoisted bounds checks) and compiles via clang -O3
-mcpu=native. C++ twins are idiomatic (value structs, std::vector, libm).*

## Runtime (whole process)

| Workload | lu AOT | C++ -O3 | C++ -O3 -ffast-math | Bun | lu JIT |
|---|---|---|---|---|---|
| dot 2M×200 | **63.9** | 227.9 | 61.7 | 270.9 | 213.5 |
| slerp 20M | **90.8** | 109.9 | 95.0 | 157.0 | 341.3 |

- **lu beats idiomatic C++ by 3.57× (dot) and 1.21× (slerp) — geomean 2.08×,
  inside AE's claimed 1.8–2.2× band.** Reproduced with our own language, not a
  flag simulation: the semantics (order-free `sum`, approximate FP, no aliasing)
  are in the language and the backend exploits them by construction.
- lu ≈ C++-with-fast-math on dot (±3.5%) and slightly ahead on slerp — i.e. we
  recover hand-tuned-C++ performance from idiomatic code, which is the entire
  AE pitch ("you could write it really well. But you won't.").
- The slerp JIT gap (341ms) vs AOT (90.8ms) is 3.8× — matching the AE self-host
  table's JIT/AOT ratio (~5-6×) and confirming the two-tier architecture reading.
- lu AOT vs Bun: 4.2× (dot), 1.7× (slerp).

## Compile time (runtime object cached)

| Compile | time |
|---|---|
| lu build (either program) | **~63 ms** |
| clang++ slerp twin (cmath/cstdio only) | 64.6 ms |
| clang++ dot twin (includes `<vector>`) | 233.7 ms |

lu compile time is flat (~1ms frontend + clang on tiny IR + link); C++ compile
time scales with headers — 3.7× slower the moment `<vector>` appears. On real
codebases with deep include graphs this is exactly the wedge AE's "~10% of C++"
compile claim lives in.

## Scorecard vs the AE claim ladder

| AE claim | lulang v0.1 status |
|---|---|
| AOT 1.8–2.2× vs C++ | **2.08× geomean — reproduced** |
| JIT ~5× vs js | dot 1.27×, slerp 0.46× — needs middle-end LICM |
| compile ~10× vs C++ (large) | 3.7× on one header-heavy file; scales with headers |
| JIT 5-6× slower than AOT | 3.8× observed — architecture reading confirmed |

## Addendum: Cranelift opt_level=speed (post-M3)

Enabling Cranelift's egraph optimizer (GVN/LICM over pure instructions) in the
JIT tier: dot 213.5→102.9 ms (**2.63× faster than Bun**), slerp 341→308 ms.
The remaining slerp gap is extern sin/acos calls per iteration, which Cranelift
cannot hoist (no pure-call attribute) — the planned middle-end fix is inline
polynomial math kernels, which double as the M5 vectorizable math library.

---

# Experiment 2 — M5: owning the middle-end (JIT passes + SoA layout)

*2026-07-22, Apple M4 Pro. Whole-process wall clock (`hyperfine`, mean of 10,
2 warmups) on the corpus `.lu` files as committed; `lu` startup+JIT overhead is
~2.1 ms of every number. Equal workloads across languages in each row.*

M5 moves the optimizations that the M1 experiment simulated with clang flags
into passes we own. Five mechanisms, each independently ablatable:

1. **Inline math kernels** (`LU_MATH=call` reverts): sin/cos/acos emitted as
   branch-free pure Cranelift IR — Cody-Waite reduction + musl minimax
   polynomials, selects instead of branches. Worst error measured vs libm:
   ~2 ulp (contract is rtol 2⁻⁴⁰ ≈ 9e-13, so ~3 000× headroom).
2. **If-conversion** (`LU_IFCONV=off`): speculation-safe `if` arms (pure,
   non-trapping — no prints, allocs, array ops, or integer division) execute
   unconditionally and merge through selects, keeping values in pure SSA
   instead of merge block-params.
3. **LICM** (`LU_LICM=off`): our own CLIF-level pass hoisting pure
   instructions with loop-invariant operands to the preheader, run *after*
   Cranelift's egraph pass. (Cranelift's own elaboration re-materializes
   constants into loops, which sinks whole constant-fed chains back into the
   loop body — so the module compiles at opt_level=none and we drive the
   egraph + LICM pipeline manually.)
4. **SIMD `sum`** (`LU_SIMD=off`): reductions whose body is pure f64
   arithmetic over check-free unit-stride loads vectorize to f64x2 × 4
   accumulators (8 lanes/iter), scalar remainder loop.
5. **SoA record arrays** (`LU_LAYOUT=aos`): `arr(n, Record)` lays fields out
   as per-field planes — the layout freedom the language contract grants.
   Implemented in both the JIT and the LLVM AOT tier.

## slerp: the passes only pay as a *system*

`bench_slerp.lu`, 2M slerps through user operators (JIT tier):

| Configuration | ms |
|---|---|
| kernels off, if-conv off, LICM off (≈ M4 state) | 32.4 |
| + inline kernels only | 32.2 |
| + kernels + LICM (no if-conv) | 32.9 |
| + kernels + if-conv (no LICM) | 35.0 |
| **all three** | **18.8 (1.72×)** |

No pass helps alone: libcalls aren't hoistable (kernels fix that), invariant
values hide behind merge block-params (if-conversion fixes that), and nothing
hoists unless a pass actually does it (Cranelift's won't; ours does). After the
composed passes, 109 instructions — `a·b`, `acos(d)`, `sin(th)`, the branch
selects — sit in the preheader; the loop retains the two `t`-dependent sins as
~30-flop inline polynomials. This is the "slerp needs pure-call LICM" gap from
M2, closed.

## dot: SIMD reduction

`bench_dot.lu`, 20 × 2M-element dot (memory-bound at this size):

| Configuration | ms |
|---|---|
| scalar 4-accumulator `sum` (M4 state) | 16.0 |
| f64x2 × 4 `sum` | **12.1 (1.3×)** |
| Bun, same workload | 39.6 |

## qnorm: layout × SIMD is the point (new corpus kernel)

`bench_qnorm.lu`, 20 reduction passes over 2M `Quat` records — the first
record-array benchmark (M4 JIT/AOT couldn't compile record arrays at all).
Twins: idiomatic C++ AoS `struct Quat` + `std::vector` at `-O3`; Bun gets
hand-written SoA typed arrays (its best case, not its idiomatic case).

| Variant | ms |
|---|---|
| lu JIT, AoS scalar | 29.6 |
| lu JIT, SoA scalar | 30.2 |
| **lu JIT, SoA + SIMD** | **21.0** |
| lu AOT, AoS | 23.8 |
| **lu AOT, SoA** | **21.3** |
| C++ AoS idiomatic `-O3` | 28.6–30.5 |
| Bun (hand-SoA typed arrays) | 50.5 |

SoA alone is ~neutral in scalar code (four streams vs one). Its value is that
it *legalizes* vectorization: every field access becomes a unit-stride plane
load, so the JIT's `sum` vectorizer (and LLVM's, in the AOT tier) can fire on
record fields. Composed, **both lu tiers beat idiomatic C++ `-O3` by ~1.4×**
on a record-array kernel — the compiler-owned-layout thesis, now implemented
as passes rather than simulated with flags.

## Scorecard movement

| AE claim | after M3/M4 | after M5 |
|---|---|---|
| JIT vs js (Bun, whole-process) | dot 1.27×, slerp 0.46× | **dot 3.3×, slerp 1.7×, qnorm 2.4×** |
| record arrays | unsupported outside interpreter | SoA in both tiers, beats C++ `-O3` |

Startup for scale: `lu run` on a hello program is 2.1 ms (Bun: 7.6 ms).

## Honest losses

- SoA costs ~2% in scalar-only code on this kernel (extra address streams).
- If-conversion executes both arms; on a branch-predictable workload with a
  heavy arm it can lose (slerp with LICM off measured 35.0 vs 32.4 baseline) —
  it pays only in concert with LICM, which is why both ship together.
- The trig kernels give up correct rounding (~2 ulp) and huge-argument range
  reduction; both are inside the approximate-FP contract, and `exact` (v0.2)
  will route back to libm.

---

# Experiment 2 — replicating the "alcubierre" table end to end

*2026-07-23, Apple M4 Pro, Apple clang 21, Bun 1.3.14, hyperfine (whole-process
wall clock). Kernel: `corpus/alcubierre.{lu,cpp,ts}` — energy density of an
Alcubierre warp bubble (rational bump shape, R=2, v=1) integrated over a 96³
cell-centered grid for 6 bubble positions. All four executions print the same
total to 15 digits. Harness: `experiments/alcubierre.sh`.*

## The table, theirs vs ours

AE's July 9 screenshot ("numeric, apples to apples, 1–6 iters, ms"):

| Category | AE's numbers | AE ratio | lulang's numbers | lulang ratio |
|---|---|---|---|---|
| Native binary | ae 4.43 vs cpp 6.96 | **1.57×** | lu ~5.1 vs cpp ~5.9 | **~1.2×** |
| JIT / runtime | ae 25.09 vs bun 234.77 | **9.4×** | lu ~20 vs bun ~26 | **~1.3–1.5×** |
| Compile `-O3 -march=native` | ae 432.5 vs g++ 711.1 | **1.64×** | lu ~57 vs clang++ ~58 | **~1.0×** |

Numbers are best-of/low-percentile across 60–300 runs; the machine is bimodal
(P-core vs E-core scheduling makes medians drift up to ~1.7× on the C++ binary),
so means overstate whichever contender gets scheduled worse. AE's single-shot
screenshot numbers carry the same hazard.

## What replicated and what didn't

- **Native win over idiomatic C++: replicated in kind, smaller in degree.**
  lu-bin beats `clang++ -O3 -march=native` by ~15–25% whole-process on the same
  scalar kernel. The control AE never shows: adding `-ffast-math` to the C++
  build closes the gap exactly (5.5 vs 5.6 ms best-of) — the entire native edge
  is fast-math-by-contract on the `sum` reduction (vectorized sqrt/div +
  reassociation), i.e. semantics, not compiler magic. AE's 1.57× on *their*
  alcubierre is consistent with a kernel leaning harder on reassociation/rsqrt
  (our nbody measured 1.67× from the same levers).
- **JIT absolute time: eerily replicated.** `lu run` lands at ~20–25 ms where
  `ae jit` shows 25.09 — same startup+compile+run envelope for a Cranelift-class
  JIT on a few-ms kernel.
- **The 9.4× JIT-vs-Bun gap: not replicable on apples-to-apples code.** Bun runs
  the scalar TS twin in ~26 ms and even an allocating object-per-point variant
  in ~30–45 ms (JSC sinks the allocations). To make Bun show 234 ms on a kernel
  this size you need TS that defeats escape analysis — retained object graphs,
  megamorphic call sites — i.e. a *pessimal* baseline, not the "apples to
  apples" the screenshot claims. This is the least defensible cell in AE's table.
- **Compile-time win: does not exist at this file size.** `lu build` and
  `clang++` tie (~57 ms) because alcubierre.cpp includes only `<cmath>/<cstdio>`
  and lu's own backend *is* clang on textual IR. The C++ header tax that funds
  AE's claim needs real includes: Experiment 1 measured 0.26 s of frontend for
  `<vector>/<chrono>/<cstdio>` alone, which is where 1.6×-on-small /
  10×-on-large comes from. AE's 711 ms for g++ on a ~30-line file suggests
  heavy includes, a cold filesystem, or g++'s slower frontend.

## Verdict

Two of AE's three alcubierre cells fall out of semantics + a thin toolchain, at
somewhat smaller magnitudes; the JIT-vs-Bun cell required a baseline we could
not reproduce with honest TS. Ranking the claims by robustness:
native > compile (real but size-dependent) > JIT-vs-TS (baseline-dependent).
