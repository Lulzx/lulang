# Benchmarks Game programs in lulang

Implementations of programs from [The Computer Language Benchmarks
Game](https://benchmarksgame-team.pages.debian.net/benchmarksgame/index.html),
written in lulang, plus same-algorithm C twins so the comparison measures the
language rather than a different algorithm.

## What is here

| program | official N | status |
| --- | --- | --- |
| [n-body](nbody.lu) | 50 000 000 | reference-exact output |
| [spectral-norm](spectralnorm.lu) | 5 500 | reference-exact output |
| [fannkuch-redux](fannkuchredux.lu) | 12 | reference-exact output |
| [mandelbrot](mandelbrot.lu) | 16 000 | byte-identical to the C reference |
| [binary-trees](binarytrees.lu) | 21 | reference-exact output |

The other five Benchmarks Game programs are **not** implemented; see
[Not implemented](#not-implemented) for why.

## Running

```bash
cargo build --release
python3 benchmarks/game/run_game.py --runs 5          # official N
python3 benchmarks/game/run_game.py --quick --runs 3  # small-N smoke check
```

The harness builds every variant, hashes each program's stdout, and refuses to
report a time for any variant whose output disagrees with the others. Results
land in `results.json` along with a machine record.

Variants measured:

- `lulang-aot` — `lu build` (LLVM, `-O3 -ffast-math -mcpu=native`)
- `lulang-jit` — `lu run` (Cranelift, includes compile time in the process)
- `c-O3` — `clang -O3 -march=native`
- `c-O3-fastmath` — same plus `-ffast-math`

`c-O3-fastmath` is the honest baseline for the floating-point programs, because
lulang's approximate-FP contract is a *language* rule, not a compiler flag: the
compiler is always permitted to reassociate and contract. Comparing lulang
against plain `-O3` C would credit lulang for a licence the C program was never
given.

## Verification

Every program's output was checked against the published reference results
before any timing was taken:

- **n-body** N=1000 → `-0.169075164` / `-0.169087605`; N=50 000 000 →
  `-0.169075164` / `-0.169059907`.
- **spectral-norm** N=100 → `1.274219991`; N=5 500 → `1.274224153`.
- **fannkuch-redux** N=7 → `228` / `16`; N=10 → `73196` / `38`; N=12 →
  `3968050` / `65`.
- **mandelbrot** — stdout md5 identical to the C reference at N=200, 201
  (exercises the odd-width padding path), 1 000 and 16 000.
- **binary-trees** N=10 and N=21 → reference checksums, including the tab
  layout of the reference `printf` format.

Reassociation did not move any published digit, which is worth stating plainly:
these programs are numerically well-conditioned enough that lulang's
approximate-FP contract costs nothing observable here.

## Measured results

Apple M-series, `clang -O3 -march=native`, median of 3 whole-process runs at
the official N. Every variant produced byte-identical output; nothing below is
a mismatch. Full record in `results.json`.

| program | N | lulang AOT | C -O3 | C -O3 -ffast-math | lulang vs C -O3 |
| --- | --- | --- | --- | --- | --- |
| n-body | 50 000 000 | 1.688s | 1.987s | 1.841s | **1.18×** |
| spectral-norm | 5 500 | 0.665s | 1.026s | 0.620s | **1.54×** |
| fannkuch-redux | 12 | 23.050s | 22.509s | 22.906s | 0.98× |
| mandelbrot | 16 000 | 8.566s | 8.960s | 8.135s | 1.05× |
| binary-trees | 21 | 1.191s | 1.313s | 1.306s | 1.10× |

(Ratios above 1.00× mean lulang is faster.)

**This does not reproduce the 2.08× geomean in the top-level README.** That
figure comes from the dot/slerp corpus, which is pure vectorizable reduction
code over `f64` arrays — the shape lulang is built for. The Benchmarks Game
programs are branch-heavy, integer-heavy, and output-heavy, and on them lulang
lands between 0.98× and 1.54× of straightforward C. Both numbers are real; they
measure different workloads, and the repo's headline claim should be read as
scoped to the corpus it was measured on.

Writing these programs surfaced two real compiler bugs — one of them a
**wrong-answer** bug — plus one missed optimisation, all since fixed. Earlier
revisions of this table (0.73×–1.10×, then 0.74×–1.33×, then 0.97×–1.57×) are
preserved in the sections below so the effect of each change is visible.

### Fixed: integer division was an out-of-line call

Writing spectral-norm surfaced a real codegen bug, since fixed.

Its inner term is `1.0 / ((i+j)*(i+j+1)/2 + i + 1)`. Every integer `/` and `%`
used to lower to a call into the C runtime, even for a literal divisor:

```llvm
%t11 = call i64 @lu_i64_div(i64 %t10, i64 2)      ; before
%t11 = sdiv i64 %t10, 2                            ; after
```

`lu_i64_div` implements the trap-on-zero and `i64::MIN / -1` rules from
SPEC §3.1, and the AOT driver links the runtime without `-flto`. So the divide
was a non-inlinable call into another translation unit: a call per element,
no strength reduction of the constant divisor, and no vectorization of any loop
containing one.

A literal divisor that is neither `0` nor `-1` can trip neither trap, so in
that case `emit_checked_int_div` (`src/llvm.rs`) and `checked_int_div`
(`src/jit.rs`) now emit a plain `sdiv`/`srem`. Variable divisors, `0`, and `-1`
still route through the helper and still trap.

Effect on spectral-norm: **1.665s → 0.835s**, from 0.73× of C `-O3` to 1.33×.
The mechanism was confirmed before the fix by substituting float `* 0.5` for
the `/2` (same answer, 0.85s) — which predicted the post-fix number almost
exactly. The `sum` primitive vectorized fine all along; the divide was what
stopped it.

**The other four programs did not benefit, and their movement between the two
runs is machine variance, not the fix.** n-body's only divides are the two
`put_f9` calls that format the answer — its hot loop contains none — and the C
baselines moved by a similar proportion in the same direction across the two
runs. Treat only the spectral-norm delta as causal.

### Fixed: i64 ordering comparisons went through f64

Chasing fannkuch-redux's remaining gap found a **correctness** bug. Its inner
swap loop is `while i < j`, and that condition compiled to:

```llvm
%t138 = sitofp i64 %t136 to double
%t139 = sitofp i64 %t137 to double
%t140 = fcmp fast olt double %t138, %t139
```

Two integers, compared as floats. `emit_ir_binary` in `src/llvm.rs` only
special-cased `Eq`/`Ne` for integer operands and sent **every ordering
relation** through `to_f64`; `src/interp.rs` did the same. So any `i64` past
2^53 lost its low bits:

```
9007199254740993 >  9007199254740992   ->  false   (should be true)
9223372036854775807 > 9223372036854775806 -> false (should be true)
```

Worse, the three tiers disagreed: the Cranelift JIT (`src/jit.rs`) always
dispatched on a `both_int` predicate and got these right, while the
interpreter and the LLVM AOT path did not. That breaks the invariant that all
three tiers print identical output — and it is the kind of bug that stays
invisible until a program uses large integers, since `==` and `!=` were
correct and only the orderings were wrong.

Both tiers now mirror the JIT's `both_int` dispatch and emit signed integer
comparisons. `~=` is explicitly excluded and keeps its relative-epsilon float
semantics on integer operands.

Effect on fannkuch-redux: **34.197s → 23.232s**, from 0.74× of C `-O3` to
0.97×. All 18 `sitofp` instructions are gone from its IR. Measured
back-to-back at N=11 to rule out machine drift: lulang 2.18s → 1.73s while the
C baseline held at 1.85s → 1.74s.

### Fixed (small win): array lengths now load as `!invariant.load`

Bounds checks were the next thing to look at. The cost was not the check
itself but *aliasing*: an array's length sits at offset 0 of the same
allocation as its elements (which start at offset 16), so LLVM had to assume a
store through the element pointer might clobber the length. fannkuch-redux's
swap loop paid for that every iteration:

```asm
ldr  x1, [x22]              ; reload length
cmp  x10, x1 / b.hs …       ; check i
cmp  x9,  x1 / b.hs …       ; check j
ldr  x2, [x23, x10, lsl #3]
ldr  x1, [x23, x9,  lsl #3]
str  x1, [x23, x10, lsl #3] ; <- element store clobbers LLVM's knowledge
ldr  x1, [x22]              ; reload length AGAIN
cmp  x9, x1 / b.hs …        ; re-check j, redundantly
str  x2, [x23, x9,  lsl #3]
```

`arr_alloc` writes the header once at construction and `lu_arr_clone` memcpys
into a fresh allocation — no path ever rewrites a live array's length. Marking
the length load `!invariant.load` states exactly that. The loop becomes two
checks against a register-held length, with the reloads gone:

```asm
cmp  x1, x7 / b.eq …
cmp  x9, x1 / b.hs …
ldr / ldr / str / str
```

There was already a `trusted` mechanism hoisting the check out of `for`-range
loops; this covers the `while`-loop case it could not reach.

**The payoff is small and narrow.** Measured by interleaved A/B (same binary
pair, alternating runs, so machine drift cancels):

| program | checks reload | invariant load | change |
| --- | --- | --- | --- |
| n-body | 1.886s | 1.727s | **8.5%** |
| mandelbrot | 10.783s | 10.561s | 2.1% |
| fannkuch-redux | 1.814s | 1.797s | 0.9% |
| binary-trees | 1.202s | 1.226s | −2.0% |

Only n-body is a real win — its hot loop indexes seven arrays repeatedly, so
hoisting seven lengths out matters. The irony is that fannkuch-redux, whose
swap loop motivated the whole investigation, gained ~1%: removing 2 of its 14
loop instructions barely moved it, because at N=12 most permutations need few
flips and the time is in the copy and permutation-advance loops instead. The
last two rows are noise.

So: bounds checks are **not** a significant cost in these five programs. The
change is kept because it is sound and free, not because it rescued anything.

The self-hosted compiler (`selfhost/codegen.lu`) emits the same metadata — the
`selfhost_sync` test compares host and self-hosted IR byte-for-byte and caught
the drift immediately.

## What the language made awkward

These are real findings from writing the programs, not complaints — each one is
a place where the benchmark had to be shaped around a missing feature.

**No bitwise operators.** `mandelbrot` packs one bit per pixel and
`binary-trees` needs `1 << k`. Both are written arithmetically: `byte * 2 + bit`
to shift left, and a multiply loop for `1 << k`. The mandelbrot inner loop is
unaffected (the packing is not hot), but `shift_left` is a loop where every
other language has one instruction.

**No indexed field assignment.** `bodies[i].vx = …` is rejected — "field
assignment root must be a variable". n-body therefore uses parallel arrays.
That happens to be the layout the compiler wants anyway, so the program is not
worse for it, but the natural array-of-records spelling is unavailable. Nested
indexed assignment (`grid[i][j] = …`) is rejected for the same reason, so 2-D
data needs flat arrays and manual index arithmetic.

**No `break` or `continue`.** fannkuch-redux's permutation advance is a loop
with an early exit in every other implementation; here it carries explicit
`advanced` / `done` flags. binary-trees and mandelbrot needed the same
treatment for their early-exit loops.

**No top-level constants.** `let N_BODIES = 5` at file scope is a parse error,
so compile-time constants are written as zero-argument functions that inline
away.

**No exponent literals.** `4.84143144246472090e+00` does not lex. The n-body
constants are transcribed in plain decimal form.

**Newline-sensitive signatures.** A function signature cannot be split across
lines; `fn energy(` followed by a newline fails to parse.

**No string builder.** `concat` copies both operands, so accumulating output
byte-by-byte is quadratic. mandelbrot writes each byte with `puts(chr(b))`
instead, which is fine because stdout is C-buffered, but a program that must
build a large string in memory has no linear way to do it. This is the single
biggest gap, and it is what rules out three of the five missing programs.

**Good news on `inout`.** Passing a large array `inout` through a recursive
outlined call is O(1) — measured, not assumed — so binary-trees' arena
recursion costs nothing extra. Value semantics did not force a copy anywhere in
these five programs.

## Not implemented

| program | blocker |
| --- | --- |
| fasta | 25 MB of generated output with no linear string builder |
| reverse-complement | same, plus in-place block reversal of a 25 MB buffer |
| k-nucleotide | needs a hash map; would be a hand-rolled open-addressed table over `[i64]`, measuring our table rather than the language |
| pidigits | needs arbitrary-precision integers; no 128-bit multiply and no bitwise ops makes a limb library slow and large |
| regex-redux | needs a regex engine; every published entry links PCRE or equivalent |

The last three would compare a library we wrote in lulang against C's GMP and
PCRE, which is not a language comparison. The first two are blocked on the
string-builder gap above and would become straightforward if lulang gained a
growable byte buffer.

## Caveats on comparing with the published table

The site's numbers come from its own hardware and, for most programs, from
heavily hand-optimised entries that use explicit SIMD intrinsics and multiple
threads. The lulang programs here are straightforward single-threaded
transcriptions of the reference algorithm. Compare them against the `c-O3`
and `c-O3-fastmath` columns produced by this harness on the same machine; treat
the published table as context for the algorithm and the expected output, not
as a like-for-like time.
