# AE — Rysana's unreleased programming language

*Research notes, compiled 2026-07-22.*

## TL;DR

**AE** is a programming language being developed by **John (@jrysana)**, founder/CEO of
**Rysana, Inc.** (rysana.com, GitHub org `rysana-ai`). It is currently **unreleased and
closed-source** — everything known about it comes from teaser posts on X: benchmark
screenshots and a single code snippet. No repo, docs, or spec are public as of July 2026.

## Who's behind it

- **John (@jrysana)** — X bio describes Rysana's focus as "1M t/s LLMs, 10x faster PL"
  (PL = programming language, i.e. AE). Account joined March 2023, ~10.5k followers,
  Canada signup country (per Grok profile lookup, 2026-07-22); posts benchmarks and
  small feature demos regularly.
- **Rysana, Inc.** — tagline "General Intelligence at the Speed of Light". Public
  products: *Inversion* (structured language models / typed data generation API),
  *Translate*, and open-source TypeScript tools (*bundown* — Markdown runtime built on
  Bun; *rysana-ai* TS library; *react-shaders*). All public repos are TypeScript;
  nothing named `ae` is public.
- Community profiling of jrysana's public work notes a pattern of ambitious design
  documents (Polymath markup language, "Full" distributed-systems language) with little
  published implementation — worth keeping in mind when weighing AE's claims.

## Source tweets

### July 7, 2026 — "pareto-superior" claim

> "Self-hosted AE programs running 1.8-2.2x faster AOT than equivalent C++/Rust end to
> end now across example corpus - was previously roughly 1:1.
>
> Adding this to being ~5x faster than Bun, >100x faster than Python, and compiling AOT
> ~10x faster than C++, AE is now pareto-superior."
> — [@jrysana, July 7, 2026](https://x.com/jrysana/status/2074441824941363310)

From the same thread, on the "you could just write C++ well" objection:

> "Granted, you could *hypothetically* write your C/C++/Rust/Assembly 'really well' and
> match AE's speeds. But you won't. Nobody does - that's why C++ has always been faster
> than Rust, C, etc. and all serious code over the past 30 years has been C++. Now AE
> will replace it."

The claim ladder as of July 2026, per the author: AOT runtime 1.8–2.2× vs C++/Rust
(up from parity earlier in development), ~5× vs Bun in JIT, >100× vs Python, ~10×
faster AOT compilation than C++. The stated thesis is that AE wins not by beating
theoretically optimal C++, but by making well-optimized code the *default* output for
idiomatic programs — a "fast by construction" argument, with "pareto-superior" meaning
no axis (runtime, compile time, ergonomics) on which you'd pick another language.

### July 8, 2026 — self-hosting milestone

> "AE is now fully self-hosted, compiling itself almost 10x faster than the equivalent
> C++ toolchain, with over 1000x less build overhead outside of the underlying backend.
> It's also, to be clear, still as before producing code that runs much faster than JS
> in JIT / C++ in AOT."
> — [@jrysana, July 8, 2026](https://x.com/jrysana/status/2074797864362955083)

From the same thread, on design pedigree:

> "AE has by far the best design of any language I've ever come to know - it's been
> carefully curated as such through years of experience not only driving large TS/Python
> frontend/full-stack orgs but also high-perf, C/C++ systems incl. parsers/servers/codecs
> but especially AI/ML"

Attached image — "AE toolchain" table comparing the original C++ bootstrap compiler
against the self-hosted (AE-written) compiler:

| Metric | C++ bootstrap | Self-host |
|---|---|---|
| self-compile (sec) | 29.63 | **3.64** |
| JIT run all tests (ms) | **396** | 407 |
| AOT run all tests (ms) | 115 | **63** |

Reading the table: the self-hosted compiler compiles the compiler ~8.1× faster than the
C++ bootstrap did (the "almost 10x" claim), its AOT-compiled output runs the test suite
~1.8× faster, and JIT test throughput is essentially unchanged (407 vs 396 ms, ~3%
slower — within noise).

Notable implications:
- **The compiler is self-hosted** (written in AE itself) as of early July 2026 — the
  language is real and mature enough to compile its own toolchain.
- "Underlying backend" suggests AE still leans on an existing codegen backend
  (LLVM or similar) beneath its own frontend/toolchain; the "1000x less build overhead"
  claim explicitly excludes it.
- Design influences claimed: TypeScript/Python ergonomics + C/C++ systems experience
  (parsers, servers, codecs) + AI/ML workloads.

### July 9, 2026 — benchmark teaser

> "Almost every time we get a new test on a new use case, AE is immediately as if by
> magic faster in both modes than any other language and compiles much faster than C++.
> For ages programming has been chained to old, bad ideas. Turns [sic] those chains are
> heavy."
> — [@jrysana, July 9, 2026](https://x.com/jrysana/status/2075192443436171638)

"Both modes" implies AE has **two execution modes: AOT native compilation and a JIT**,
which the attached benchmark confirms.

## The "alcubierre" benchmark (attached image)

A numeric benchmark named "alcubierre" (after the warp-drive metric), labeled
"numeric, apples to apples", comparing AE vs C++ vs TypeScript, 1–6 iters, ms,
lower is better:

| Category | Contender | Time (ms) |
|---|---|---|
| Native binary | `./ae-bin` | **4.43** |
| Native binary | `./cpp-bin` | 6.96 |
| JIT / runtime | `ae jit` | **25.09** |
| JIT / runtime | `bun run` (TS) | 234.77 |
| Compile time (`-O3 -march=native`) | `ae` | **432.50** |
| Compile time | `g++` | 711.07 |

From the same thread, clarifying the compile-time comparison:

> "By the way, this is a really rough compile time comparison. For larger projects like
> the language's own toolchain and our large previously-C++ foundational libraries,
> compile time is around 10% of C++."

I.e. the ~60% figure in the image is the small-benchmark case; on large codebases the
claim is **~10× faster compilation than C++** — consistent with the July 7 "compiling
AOT ~10x faster than C++" and July 8 self-compile numbers. The mention of "our large
previously-C++ foundational libraries" also implies Rysana has **already ported
substantial internal C++ code to AE**.

Headline claims from the image:
- **>5.5× runtime vs TypeScript** (JIT mode vs Bun; actually ~9.4× on the shown numbers)
- **>15% runtime vs C++** (native; shown numbers are ~36% faster)
- **>60% compile vs C++** (shown numbers are ~39% faster / ~1.64×; the ">60%" framing
  presumably means ae's compile takes ~60% of g++'s time)

## Official teaser from @Rysana (July 7, 2026)

> "Radically faster AI needs a radically faster programming language that you can call
> it from. Your agents should be quick, both in their brain and on your machine.
> Everything you love about TypeScript, C++, and more. None of the decades-old
> mistakes. Coming soon from Rysana:"
> — [@Rysana, July 7, 2026](https://x.com/Rysana/status/2074456277552361775)

Positioning confirmed: AE is a product ("coming soon"), aimed at **AI/agent
infrastructure** — a fast language you call AI *from*. Attached image (transcribed):

```
operator* (a: Vec3) · (b: Vec3): f64 { return dot_prod(a, b) }
operator ‖(v: Vec3)‖: f64 { return sqrt(v · v) }
type Ket { v: Vec3 }
type Bra { v: Vec3 }
operator |(v: Vec3)⟩: Ket { return Ket { v: v } }

main {
  let a = Vec3 { 1, 2, 3 }
  let b = Vec3 { 4, 5, 6 }
  print("norm:", ‖a × b‖)
  print("braket:", ⟨a| · |b⟩)
}
```

Side panels: **5× JIT** (vs js/py/etc.), **2× AOT** (vs c++/etc.), **9× compile**
(vs c++/etc.).

This is the richest syntax sample yet. It reveals:

- **User-defined operators, including Unicode glyphs**: `·` (dot product) is defined
  by the user, not the language. `operator*` likely means "an operator at `*`'s
  precedence level" — precedence-by-analogy instead of numeric precedence tables.
- **Circumfix / mixfix operators** (Agda-style): `‖(v: Vec3)‖` defines the *bracketing
  pair* `‖…‖` as an operator; `|(v: Vec3)⟩` defines quantum ket notation `|v⟩`
  returning a `Ket` record. `⟨a| · |b⟩` composes user-defined mixfix operators into
  Dirac bra-ket notation. So the `‖…‖ ≈` in the earlier slerp snippet was probably
  *library* syntax, not built-in.
- **Record types**: `type Ket { v: Vec3 }` — no `struct` keyword, no `=`.
- **Record literals**: positional `Vec3 { 1, 2, 3 }` and named `Ket { v: v }`.
- **`let` bindings**, **`main { }`** as a bare entry block (no `fn main()`), and a
  variadic `print`.
- Everything is expression-brief: single-line function-style operator bodies with
  `return`.

Design lineage this implies: TS/Rust surface + **Agda/Lean-class user-defined mixfix
notation** as a headline feature — "your code reads like the math paper" is a core
pitch, aimed at AI/ML/scientific users.

## First glimpse of syntax (`sketches/slerp.ae`)

An earlier code screenshot of a file named `sketches/slerp.ae`:

```
property slerp_stays_unit(a: Quat, b: Quat, t: f64) {
  ‖slerp(a, b, t)‖ ≈ 1.0
}
```

What this one snippet reveals:

- **File extension:** `.ae`.
- **First-class property-based testing:** `property` is a top-level keyword — the
  declaration reads as "for all quaternions a, b and scalar t, the slerp result stays
  unit-length". Verification-style specs appear baked into the language rather than
  bolted on via a testing library.
- **Unicode math operators in surface syntax:**
  - `‖x‖` — double-bar norm/magnitude delimiters, straight from math notation.
  - `≈` — an approximately-equal operator (presumably epsilon-based float comparison),
    exactly what you want for floating-point property tests instead of `abs(x-y) < 1e-9`
    boilerplate.
- **Type syntax:** postfix annotations `name: Type` (TS/Rust style), Rust-style scalar
  names (`f64`), and built-in or library math types like `Quat` (quaternion) —
  consistent with the claimed AI/ML and numerics focus.
- The `sketches/` path and expression-as-body style suggest lightweight,
  script-like files with implicit assertion of a bare boolean expression.

## What can be inferred about AE

- Two execution modes: native AOT binaries and a JIT (`ae jit`), from a single language.
- Positioned as a systems/numeric language competing with C++ on runtime performance
  while compiling faster, and with TypeScript/Bun on developer-facing JIT execution.
- Fits Rysana's stated mission ("10x faster PL") — likely intended as infrastructure
  for their high-throughput LLM work ("1M tokens/sec").
- The benchmark harness compares against `g++` and `bun`, suggesting a Unix/CLI-first
  toolchain (`ae` compile command, `ae jit` run command).

## Caveats

- **All performance claims are self-reported** by the author via screenshots; the
  benchmark source, workload, and methodology are not public, so nothing is
  independently verifiable.
- Microbenchmarks at 1–6 iterations and ~4–25 ms scales are highly sensitive to
  process startup, allocation strategy, and compiler flags; "faster than C++" claims
  at that scale should be treated as marketing until code is released.
- Web searches surface **no** HN/Reddit discussion, docs page, or repo for AE.
  `rysana.com/ae` doesn't serve meaningful content; `rysana.com/docs` covers only the
  Inversion API.

## Sources

- Tweet: https://x.com/jrysana/status/2075192443436171638 (retrieved via fxtwitter, July 9, 2026)
- Benchmark image attached to the tweet (transcribed above)
- https://rysana.com and https://rysana.com/docs
- https://github.com/jrysana and https://github.com/rysana-ai
- https://www.github.gg/jrysana (third-party profile analysis)
