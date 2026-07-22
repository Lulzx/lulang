# lulang — reverse-engineering AE and building our own

*Companion to [ae-research.md](ae-research.md). Written 2026-07-22.*

The goal: take everything AE has shown publicly, deduce the most plausible technical
design behind the claims, and use that as the blueprint for a language we build from
scratch (working name: **lulang**).

---

## Part 1 — What the evidence constrains

Every public claim narrows the design space. Working through them:

### Claim: AOT output 1.8–2.2× faster than "equivalent" C++/Rust (numeric corpus)

This is the load-bearing claim, and the key insight is that AE almost certainly uses
**LLVM as its backend** ("the underlying backend" whose overhead is excluded). So AE's
generated code goes through the *same* optimizer as clang/rustc. You cannot beat C++ by
2× with a better instruction selector — you beat it by having **language semantics that
make optimizations legal which C++/Rust must conservatively forbid**:

1. **Approximate floating point by default.** The `≈` operator in the slerp snippet is
   the smoking gun: AE treats FP as approximate *in the language contract*. If the
   semantics say floats are "approximately associative", then reassociation, FMA
   contraction, and vectorized reductions are always legal — i.e. `-ffast-math`
   permanently on, *by definition rather than by flag*. Idiomatic C++ (`-O3`, no
   fast-math, since fast-math is non-default and scary) loses 1.5–3× on exactly the
   numeric-kernel workloads AE benchmarks. This alone could explain most of the gap.
2. **Value semantics, no exposed pointers/aliasing.** If nothing can alias, every load
   can be hoisted and every loop vectorized without runtime alias checks. Rust gets
   part of this via borrows, but idiomatic Rust still pays bounds checks and rustc's
   noalias info is imperfect. A language with *no* address-of at all beats both.
3. **Data-layout freedom.** C++/Rust guarantee struct layout and address identity, so
   they can never transform array-of-structs into struct-of-arrays. A value-semantics
   language guarantees neither, so the compiler can re-layout aggregates for
   vectorization (the Halide/Mojo trick, applied language-wide).
4. **Whole-program compilation by default.** No ABI boundaries, no separate
   translation units — everything monomorphized, specialized, and inlinable, like
   Julia's specialization model. C++ only approaches this with careful LTO.
5. **Properties feeding the optimizer.** A `property` that's checked (by testing or
   proof) is a fact the compiler may assume — e.g. an index-in-bounds property deletes
   bounds checks; a unit-norm property enables algebraic simplification. This would
   make the verification story and the performance story *the same feature*.

The thread quote confirms the thesis: "you could hypothetically write your C++ really
well and match AE's speeds. But you won't." AE isn't claiming to beat optimal C++ —
it's claiming its *idiomatic* code lowers to what *hand-tuned* C++ would be. Defaults
are the product.

### Claim: JIT ~5–9× faster than Bun

Least surprising claim: a statically-typed language JIT vs a dynamic-JS JIT
(even a great one) is routinely 5–10×. Architecture implication: one typed IR, two
backends — a fast low-optimization codegen tier for `ae jit` (instant startup, think
copy-and-patch compilation or Cranelift-class codegen) and LLVM `-O3` for AOT. The
25ms JIT vs 4.4ms AOT gap in the alcubierre image is consistent with a cheap tier-0
compile plus unoptimized-but-native code.

### Claim: compiles ~10× faster than C++ on large projects; "1000× less build overhead outside the underlying backend"

"Outside the underlying backend" is an admission: LLVM dominates their compile time,
and *everything else is nearly free*. That's achievable if you delete C++'s actual
compile-time costs, none of which are codegen:

- No textual `#include` — a real module system (each file parsed once, ever).
- No C++-template-style metaprogramming blowup in the common path.
- Data-oriented compiler internals: flat arena-allocated ASTs/IR, index-based (not
  pointer-based) nodes, cache-friendly passes — the Zig/Carbon frontend playbook.
- Parallel per-module frontend + codegen, likely with incremental caching.

A frontend measured in milliseconds where C++ frontends take seconds *is* "1000× less
overhead". The remaining 10× on full builds is LLVM invoked once per module with no
redundant work.

### Claim: self-hosted (bootstrap was C++), ~1 month between claims escalating from parity to 2×

Implications: the language is small enough to rewrite its own compiler quickly, and
systems-capable enough to write a compiler in. The parity→2× jump right after
self-hosting suggests the optimization-enabling semantics (fast-math contracts,
layout freedom, property-assumptions) landed as *compiler features* recently — the
language semantics were designed for them in advance, and they switched them on.

### The surface evidence: `sketches/slerp.ae`

- `property` as a top-level declaration → built-in property-based testing (QuickCheck
  as a language keyword), plausibly doubling as optimizer-visible contracts.
- `‖x‖` and `≈` → math notation as real syntax; a language that wants numeric code to
  *read like the paper it came from* (Lean/Julia lineage).
- `f64`, `Quat`, postfix `name: Type` → Rust-flavored types, numerics-first stdlib.
- `sketches/` → lightweight script-like files; the toolchain runs loose files (JIT
  mode is the dev loop, AOT is the ship path).

### Synthesis — the AE hypothesis in one paragraph

> AE is most plausibly a **statically-typed, value-semantics, numerics-first language**
> with **approximate floating-point semantics in the core contract**, **no exposed
> aliasing**, and **compiler-owned data layout**, compiled **whole-program** through a
> **Zig-style near-zero-cost frontend** into **one typed IR** with **two backends**:
> a fast tier for `ae jit` and LLVM `-O3 -march=native` for AOT — plus **built-in
> property-based testing** whose verified facts the optimizer may assume. Nearest
> relatives: Jai (compile speed philosophy), Zig (frontend engineering), Julia
> (specialization + math), Mojo (AI/ML + beats-C++ positioning), Dafny/liquid types
> (contracts erasing runtime checks).

None of this requires magic. Each mechanism is published, proven tech; the novelty is
committing to the aggressive defaults *as the language definition* and refusing the
compatibility constraints (ABI, IEEE strictness, address identity) that stop C++ and
Rust from doing the same.

### Revision 2 — after the @Rysana teaser and our experiments (2026-07-22)

New evidence tightened the picture in four ways:

1. **The notation layer is a language feature, not built-ins.** The teaser shows
   `operator* (a: Vec3) · (b: Vec3): f64`, circumfix `operator ‖(v)‖`, and
   `operator |(v)⟩` composing into `⟨a| · |b⟩`. So AE has **user-defined mixfix
   operators with precedence-by-analogy** (`operator*` = "binds like `*`"), and the
   `‖·‖`/`≈` from the slerp sketch are *library code*. This is Agda/Lean-class
   notation grafted onto a TS/Rust-looking core. Parser implication: a Pratt parser
   with a dynamically extensible operator table — cheap to build, cheap to run, and
   it makes "reads like the paper" a stdlib property.
2. **Our benchmarks validated the semantics thesis** (see
   [experiments/RESULTS.md](experiments/RESULTS.md)): fast-math + restrict + SoA on
   clang gives a **~1.9× geomean over idiomatic C++ `-O3`** — inside AE's claimed
   1.8–2.2× — with the reduction kernel alone at 3.9×. And ~75% of C++ compile time
   measured as frontend/header overhead, matching the 10×-compile claim's shape.
3. **Missing ingredient found: vectorized transcendentals.** Our slerp kernel only
   improved 1.18–1.33× because scalar `acos`/`sin` libcalls dominate. To hit 2×
   *across a corpus* AE almost certainly ships its own SLEEF-class vector math
   runtime. Added to our roadmap (M5).
4. **Product thesis: AI-agent infrastructure.** "A radically faster language you can
   call AI from… your agents should be quick" + Rysana's Inversion API ⇒ AE likely
   has first-class async LLM/tool-call integration (typed structured-output calls
   fitting their existing product). The perf story is the moat; the AI runtime is
   the product.

Also from the teaser's syntax: `type Ket { v: Vec3 }` records, positional literals
`Vec3 { 1, 2, 3 }`, bare `main { … }` entry block, single-line `return` bodies —
all now reflected in the lulang spec.

### Revision 3 — the deep architecture model (re-reading the numbers)

Going back over every number with fresh eyes changes the model in important ways.

#### The self-host table is the single most informative artifact

| Metric | C++ bootstrap | Self-host |
|---|---|---|
| self-compile (sec) | 29.63 | 3.64 |
| JIT run all tests (ms) | 396 | 407 |
| AOT run all tests (ms) | 115 | 63 |

Three separate deductions hide in these six numbers:

1. **Row 1 is a language-vs-language compile-speed datum, not compiler throughput.**
   "Compiling itself almost 10x faster than the equivalent C++ toolchain" read
   literally: building the *C++ bootstrap toolchain* from source (29.63s of clang/g++
   compiling C++) vs building the *self-hosted toolchain* from source (3.64s of AE
   compiling a comparable AE codebase). Same program family, two languages, 8.1× —
   that's the compile-speed claim measured on their largest real codebase.
2. **Row 3 is the smoking gun for a custom middle-end.** The self-hosted compiler's
   AOT *output runs 1.8× faster* than the bootstrap's output (115→63ms on the same
   test suite). If both compilers merely lowered naively and handed everything to
   LLVM `-O3`, output quality would be identical. Therefore the optimizations that
   produce AE's 2× **live in AE's own mid-level optimizer** — which exists only in
   the self-hosted compiler, was written in AE, and landed in early July. The C++
   bootstrap was a minimal seed: parse → naive lower → LLVM. This also cleanly
   explains the claim timeline: "was previously roughly 1:1" (bootstrap, naive
   lowering, LLVM alone ≈ clang) → "1.8–2.2×" the week self-hosting landed. The
   semantics were designed years ahead; the pass pipeline that *exploits* them
   shipped with the rewrite.
3. **Row 2 says the JIT skips the middle-end.** JIT test time is unchanged
   (396≈407ms) across compilers with hugely different optimizers — so `ae jit` is a
   single naive-lowering tier in both. JIT vs AOT on the same suite is 407/63 ≈
   6.5×, and on alcubierre 25.09/4.43 ≈ 5.7× (including compile time) — exactly the
   profile of an unoptimized-but-native single tier (Cranelift-class / `-O0` /
   copy-and-patch codegen). Static types are why it's *immediately* fast: no
   speculation, no deopt guards, no warmup — the thing JS engines can never skip.

So the pipeline is:

```
source ──frontend (ms)──▶ typed IR ──┬── naive lower ─▶ fast codegen ─▶ ae jit
                                     └── AE middle-end ─▶ LLVM -O3 ─▶ ae binary
                                          (the actual product)
```

Where the middle-end plausibly spends its effort — each transform legal only because
of the semantics: AoS→SoA layout selection, reduction reassociation + vectorization
at typed-IR level, lowering transcendentals to their own SIMD math kernels (our slerp
experiment showed libm-bound code is where flag-flipping fails), aggressive copy
elision / destination-passing, and whole-program monomorphization. Given the AI/ML
pedigree, this layer may literally be **MLIR dialects** (linalg/vector), making AE
"Mojo-shaped" under a TS/Agda-flavored surface — but the conclusion holds either way:
**the moat is a semantics-aware middle-end above LLVM, not LLVM itself.**

#### The memory-model gap, and why it points at mutable value semantics

A self-hosted compiler plus ported "parsers/servers/codecs" need strings, growable
arrays, hashmaps, and IO. Pure copy-on-assignment value semantics can't power those;
tracing GC or pervasive refcounting would show up as lost benchmarks. The one
published design that delivers no-aliasing *and* C++-class performance *and* systems
usability is **mutable value semantics** (Hylo/Val lineage): `inout`-style parameters
under a law of exclusivity, no first-class references, static destruction, arenas
underneath. Prediction: AE has an `inout` (or equivalent) that hasn't appeared in a
screenshot yet.

Corroborating oddity: Rysana **ported** their C++ foundational libraries rather than
binding them. If AE had comfortable C FFI they'd have wrapped first and rewritten
lazily. Porting-first says either FFI is immature — or, more interestingly, FFI
boundaries would *break the middle-end* (no layout transforms or inlining across an
opaque C++ call), so whole-program purity is load-bearing for the numbers.

#### Compile-time arithmetic supports "whole-program, every time"

432ms to AOT-compile a small benchmark is *large* for one tiny module through LLVM —
unless every build monomorphizes and recompiles the reachable stdlib plus links
(~100–200ms of ld). That's consistent with whole-program compilation as the only
mode, and with "1000× less overhead outside the backend": their frontend is ms-scale,
LLVM+linker is everything else. C++ loses 10× on large projects because header
re-parsing scales superlinearly while AE modules parse once.

#### The reframe: a language designed to be written *by* AI

"Your agents should be quick, both in their brain and on your machine" is not flavor
text. Consider AE as the substrate for LLM-generated code: a 25ms no-warmup JIT is a
tool-execution loop; static types + whole-program checking turn agent mistakes into
compile errors instead of runtime surprises; `property` declarations are
machine-checkable specs an agent can be asked to satisfy (generate until properties
pass — a verification loop that needs no human); dense mathematical notation cuts
token counts. The perf numbers are the marketing; *verifiable-by-construction agent
code* may be the actual product Rysana ships on top.

#### Falsifiable predictions (how we'll know if this model is right)

1. AE has `inout`/MVS mutation, no GC, no pervasive refcounts.
2. An `exact`/strict-FP escape hatch exists (they can't do numerics credibly without one).
3. The toolchain ships its own SIMD transcendental library.
4. JIT output quality stays ~5–6× behind AOT (single tier, no middle-end).
5. C FFI is absent or explicitly perf-fenced at launch.
6. Operators must be declared/imported before use (single-pass parsing constraint).
7. An AI/agent runtime (typed LLM calls, property-checked codegen) ships within months
   of the language.

#### Corrections this forces on the lulang plan

- **Our experiment's flag-flipping was a simulation, not the architecture.** Real
  lulang must *own the middle-end*: SoA selection, reduction vectorization, and
  math-kernel lowering as IR passes — clang flags can't be the product. M5 promotes
  from "experiments" to the core deliverable.
- The JIT path should deliberately skip the middle-end (matching row 2) — it buys
  simplicity and instant startup, and AE proves users accept 5–6× vs AOT in dev mode.
- `inout` mutation moves from "maybe v0.2" to a committed v0.2 feature — the compiler
  can't self-host, and servers can't exist, without it.

---

## Part 2 — lulang: language design

Build the same thesis, honestly. Scope ruthlessly: a numeric-first core that can win
the same benchmarks, not a general-purpose language on day one.

### Core semantic commitments (the ones that buy performance)

1. **Value semantics only.** No pointers, no references, no address-of in v1.
   Assignment copies (semantically); the compiler elides copies freely because
   aliasing is impossible.
2. **Approximate FP by default.** `f64`/`f32` arithmetic is defined as
   "IEEE-shaped but reassociable" (fast-math semantics). An `exact` context/type is
   the opt-out for the rare code that needs bit-reproducibility.
3. **No layout guarantees.** Records and arrays have no defined memory layout; the
   compiler may re-layout (SoA), pad, split, or scalarize.
4. **Whole-program compilation.** Monomorphize everything; no stable ABI in v1.
5. **Properties are contracts.** `property` declarations are fuzzed by the test
   runner; a passing property may be assumed by the optimizer under an explicit
   `--trust-properties` flag (honest about the soundness tradeoff, unlike hiding it).

### Surface syntax sketch

```
// arith.lu
fn dot(a: [f64; N], b: [f64; N]) -> f64 {
  sum(i in 0..N) a[i] * b[i]        // reduction primitive: always vectorizable
}

type Quat = { w: f64, x: f64, y: f64, z: f64 }

fn slerp(a: Quat, b: Quat, t: f64) -> Quat { ... }

property slerp_stays_unit(a: Quat, b: Quat, t: f64) {
  ‖slerp(a, b, t)‖ ≈ 1.0
}
```

- Postfix types, Rust-style scalars (`i64`, `f64`, `bool`), fixed-size arrays,
  records, generics over sizes later.
- Unicode operators with mandatory ASCII spellings: `≈` = `~=`, `‖x‖` = `norm(x)`.
  Editor/formatter canonicalizes ASCII → Unicode so typing stays easy.
- `≈` uses a relative-epsilon comparison scaled by operand magnitude and type.
- Explicit reduction/map primitives (`sum`, `prod`, `map`) rather than hoping the
  autovectorizer recognizes loops — guaranteed-vectorizable by construction.
- Modules: one file = one module, `use` imports, no headers, no preprocessor.

### Two execution modes, one IR

- `lu run file.lu` — JIT: parse → typecheck → lower → **Cranelift** codegen. Target:
  cold start under ~50ms for small programs (AE shows 25ms).
- `lu build file.lu` — AOT: same IR → **LLVM** with fast-math flags +
  `-march=native` equivalent → native binary.

### Property engine

- `lu test` runs every `property`: generate random typed inputs (with shrinking),
  check the body's boolean value. Generators derived from types (`f64` → mixed
  normal/subnormal/edge values, records → product generators).
- Later: `assume`/`ensure` on functions; verified facts flow into the IR as
  `llvm.assume`-style hints.

---

## Part 3 — Implementation architecture

**Host language: Rust** (recommended — Cranelift is native to it, LLVM via `inkwell`,
excellent parsing/arena crates, and we get memory safety in the compiler for free).
Alternatives considered: Zig (better compile-speed culture, weaker backend-library
story), C++ (only if we want to feel what we're competing with).

Pipeline (data-oriented from day one — compile speed is a feature, not an
afterthought):

```
source → lexer → parser → flat AST (arena, u32 indices)
       → name resolution → type inference (monomorphic HM subset)
       → typed SSA IR
       → [Cranelift]  jit run          (lu run)
       → [LLVM -O3 + fast-math + native]  binary   (lu build)
```

- Flat, index-based AST/IR (no `Box`/pointer trees). Single arena per module.
- Errors carry spans; diagnostics matter even in a prototype.
- Benchmark harness in-repo from M1: our own "alcubierre" — nbody, mandelbrot,
  matmul, slerp/quaternion corpus, each with a hand-written C++ (`g++ -O3
  -march=native`, both with and without `-ffast-math` — publish both numbers) and a
  TypeScript/Bun equivalent. Honest baselines are the whole point.

---

## Part 4 — Roadmap

- **M0 — Spec + corpus.** Freeze the v1 grammar and semantics (this doc → SPEC.md).
  Write the benchmark corpus in C++/TS first so targets exist before the language does.
- **M1 — Front half.** Lexer, parser, flat AST, typechecker for the numeric core
  (scalars, records, fixed arrays, `fn`, `sum`/`map`, control flow). Tree-walking
  interpreter to lock semantics with golden tests.
- **M2 — JIT.** SSA IR + Cranelift backend; `lu run` beats Bun on the corpus.
- **M3 — AOT.** LLVM backend with fast-math + native codegen; `lu build` reaches
  parity-or-better with `g++ -O3` (no fast-math) on the corpus, and we understand
  exactly which semantic freedom each win came from.
- **M4 — Properties.** `property` keyword, generator/shrinker engine, `lu test`;
  ship `≈` and `‖·‖`.
- **M5 — Layout & assumption experiments.** SoA transformation for record arrays,
  property-derived `assume` hints, measure each optimization's contribution.
- **M6 — Stretch: begin self-hosting** the lexer/parser in lulang once the language
  can express them (needs strings/enums/pattern matching — v2 features).

### What we deliberately do differently from AE

- **Publish everything**: corpus, methodology, flags, losses as well as wins.
- **Both C++ baselines** (with/without fast-math) so our headline numbers can't hide
  the semantics trick — we *explain* the trick instead, because the trick is the
  actual contribution.
