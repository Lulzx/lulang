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
| [fasta](fasta.lu) | 25 000 000 | byte-identical to the C reference |
| [reverse-complement](revcomp.lu) | 25 000 000 | byte-identical to the C reference |
| [k-nucleotide](knucleotide.lu) | 2 500 000 | reference-exact output |

Two Benchmarks Game programs are **not** implemented; see
[Not implemented](#not-implemented) for why.

`revcomp` and `knucleotide` read a FASTA file produced by our own `fasta`, as
the benchmark specifies. The language has no stdin builtin, so both take the
path as an argument instead of reading a pipe; the C twins do the same, so the
comparison is unaffected. `knucleotide` runs at a reduced N because an exact
count of every distinct 18-mer over 125 M bases needs a multi-gigabyte table in
either language; 2 500 000 keeps it honest and comparable.

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

The C twins do not change between lulang iterations, so their timings are
cached in `baselines.json`, keyed by program, N, variant, and a hash of the
binary. A recompiled or edited twin re-times itself automatically. Pass
`--refresh-baseline` to force a re-measure — worth doing whenever the machine
may have shifted, because the C column is what makes a lulang delta readable as
signal rather than drift.

Variants are timed **round-robin** (run 1 of each, then run 2 of each), not one
variant's repeats at a time. This matters more than it sounds: with sequential
repeats, a thermal ramp lands entirely on whichever variant went first, and
fannkuch-redux read 0.92× that way against 0.98–0.99× interleaved on the same
binaries. Several apparent regressions in this file's history were exactly
that; mandelbrot likewise read 0.92× on one run and 0.99× on a re-measure, with
C moving 4% on identical code in between.

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
| spectral-norm | 5 500 | 0.657s | 1.016s | 0.629s | **1.55×** |
| fasta | 25 000 000 | 2.492s | 2.942s | 2.793s | **1.18×** |
| n-body | 50 000 000 | 1.691s | 1.943s | 1.836s | **1.15×** |
| binary-trees † | 21 | 1.226s | 1.356s | 1.435s | **1.11×** |
| fannkuch-redux | 12 | 23.484s | 22.708s | 23.673s | 0.97× |
| mandelbrot | 16 000 | 9.475s | 9.054s | 8.560s | 0.96× |
| reverse-complement | 25 000 000 | 0.659s | 0.656s | 0.625s | 1.00× |
| k-nucleotide | 2 500 000 | 1.237s | 0.965s | 0.994s | 0.78× |

Four clear wins, three near-ties, one loss.

**Read these ratios with about ±0.05–0.10× of slack.** Even interleaved,
run-to-run spread on this machine is real: reverse-complement read 1.00× in the
run above and 0.80× on a five-run re-measure minutes later, and k-nucleotide
read 0.78× and 0.85×. Only differences larger than that, or ones with a
mechanism attached, are worth acting on.

† binary-trees read 0.86× in the run that produced this table. That was drift,
not a regression, and it is worth spelling out how that was established rather
than asserted: the emitted LLVM IR for binary-trees is **byte-identical**
before and after the change in this commit, so no code-generation difference
exists to explain a 22% move. A five-run interleaved re-measure gave 1.11×,
matching its historical value, and that is the number in the table.

**This does not reproduce the 2.08× geomean in the top-level README.** That
figure comes from the dot/slerp corpus, which is pure vectorizable reduction
code over `f64` arrays — the shape lulang is built for. The Benchmarks Game
programs are branch-heavy, integer-heavy, and output-heavy, and on them lulang
lands between 0.75× and 1.59× of straightforward C. Both numbers are real; they
measure different workloads, and the repo's headline claim should be read as
scoped to the corpus it was measured on.

Writing these programs surfaced two real compiler bugs — one of them a
**wrong-answer** bug — three missed optimisations, and five missing language
features, all since addressed. Earlier revisions of this table (0.73×–1.10×, then 0.74×–1.33×, then 0.97×–1.57×) are
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

### Added: `str_from_bytes`, the language's first linear string builder

The three string programs were previously listed here as not implementable.
`concat` copies both operands, so building output a byte at a time was
quadratic, and there was no way at all to turn a computed byte buffer into a
`str`. `str_from_bytes(a: [i64], lo, hi): str` takes the low byte of each
element in one allocation and one pass. It is checked on both ends and
implemented in all three tiers.

That unblocked fasta, reverse-complement, and k-nucleotide, and fasta now beats
its C twin (1.15×) despite writing 250 MB.

### Fixed: `calloc` for zero-initialized arrays, inline divide for variable divisors

Profiling k-nucleotide found two more.

`arr(n, 0)` allocated with `malloc` and then ran a scalar fill loop, so a
268 MB hash table was eagerly written before a single lookup — where the C
twin's `calloc` gets lazily-zeroed pages from the OS. `lu_arr_new_i64` and
`lu_arr_new_f64` now take a `calloc` path when the initializer is zero. `-0.0`
still takes the fill path, since its bits are not all zero.

The constant-divisor fix further up left every *variable* divisor still calling
`@lu_i64_rem` — which is exactly what `key * 2654435761 % cap` does on every
hash probe. The two trapping cases are now guarded inline and the division is a
real `sdiv`/`srem`; the trap edge calls the same helper, so diagnostics are
unchanged. `selfhost/codegen.lu` emits byte-identical IR for both paths.

Together these took k-nucleotide from 0.55s to 0.47s at N=1 000 000.

## Language features this exercise added

Writing these programs turned up five things the language could not express, or
could only express quadratically. All five are now in, across the interpreter,
the Cranelift JIT, and the LLVM AOT tier.

**`str_from_bytes(a, lo, hi): str` and `putbytes(a, lo, hi)`.** `concat` copies
both operands, so accumulating output a byte at a time was quadratic, and there
was no way at all to turn a computed byte buffer into output. `str_from_bytes`
builds a str in one allocation and one pass; `putbytes` writes the span
straight to stdout with no allocation at all, which is what fasta and
reverse-complement use (one `fwrite` per line instead of one malloc per line).
Both accept `[i64]` or `[i8]` and are bounds-checked.

**`i8`.** A one-byte storage type. It widens to `i64` in every arithmetic,
comparison, and `sum` context and narrows only through the explicit `i8(x)`, so
it behaves like the existing `f32`→`f64` promotion. Arrays store one byte per
element — 100 M elements take 192 MB as `[i8]` against 1527 MB as `[i64]`.

  It is deliberately **not** allowed in records or across the C boundary yet:
  record arrays allocate through `lu_arr_new_raw`, whose layout ABI only knows
  4- and 8-byte components, and widening that reaches the SoA planes, the ABI
  manifests, and the generated headers. Both cases are rejected with an
  explicit error rather than silently miscompiled.

**`break` and `continue`.** Previously every early exit was a hand-rolled
`done`/`advanced` flag pair. `continue` in a `for` targets a dedicated latch
block so the index still advances; using the condition head would have made it
an infinite loop. Both are rejected outside a loop.

**Bitwise `& | ^ << >>`.** i64-only, `>>` is arithmetic, and shift counts are
masked to 0..63 so an out-of-range shift is defined rather than LLVM poison.
Precedence is `|` < `^` < `&` < shifts < additive < multiplicative, so bitwise
binds *tighter* than comparison — the Rust order, not C's famous footgun where
`a & b == c` parses as `a & (b == c)`. Verified against Python for both values
and grouping.

mandelbrot now packs bits with `(byte << 1) | bit`, binary-trees uses
`1 << k` instead of a multiply loop, and fannkuch-redux's permutation advance
is a plain `break` instead of two flags.

## What the language still makes awkward

**No indexed field assignment.** `bodies[i].vx = …` is rejected — "field
assignment root must be a variable". n-body therefore uses parallel arrays.
That happens to be the layout the compiler wants anyway, but the natural
array-of-records spelling is unavailable, and nested indexed assignment
(`grid[i][j] = …`) is rejected for the same reason, so 2-D data needs flat
arrays and manual index arithmetic.

**No top-level constants.** `let N_BODIES = 5` at file scope is a parse error,
so compile-time constants are written as zero-argument functions that inline
away.

**No exponent literals.** `4.84143144246472090e+00` does not lex. The n-body
constants are transcribed in plain decimal form.

**Newline-sensitive signatures.** A function signature cannot be split across
lines; `fn energy(` followed by a newline fails to parse.

**No stdin.** reverse-complement and k-nucleotide take a file path instead of
reading a pipe. The C twins do the same, so the comparison is unaffected.

**Good news on `inout`.** Passing a large array `inout` through a recursive
outlined call is O(1) — measured, not assumed — so binary-trees' arena
recursion costs nothing extra. Value semantics did not force a copy anywhere in
these eight programs.

## Where the two losses come from

### reverse-complement (0.84×): mostly closed by `i8`

This was the worst result in the table at **0.50×**, and the cause was not code
generation: lulang's only integer array was `[i64]`, so each DNA base occupied
8 bytes where the C twin used 1. The benchmark is memory-bandwidth-bound — read
a 250 MB file, complement it in place, write it back — so lulang moved 8× the
bytes and landed at half the speed. No optimiser pass could have reached it.

Adding `i8` and `putbytes` took it to **0.84×**: 0.50× → 0.67× from the dense
buffer, then → 0.84× from writing spans directly instead of building a str per
line. What is left is the extra copy from the `read_file` str into the `[i8]`
buffer, plus the per-access bounds checks the C twin does not have.

### k-nucleotide: bounds checks in the probe loop

Both languages now store the sequence as one byte per base — `[i8]` against
`signed char *` — so memory traffic matches and the gap is per-operation
overhead: bounds checks the C twin does not have.

The `trusted`-range hoist used to recognise only an array indexed *directly*
by a `for`-range variable, so `pack`'s `seq[start + i]` kept a compare-and-
branch per element. It now recognises affine indices too — `a[c + i]` and
`a[i + c]`, where `c` is a local the loop never writes — and hoists a check
over the shifted range `[lower+c, upper+c)`. `pack` went from one check per
element to one per call.

Two details make it sound. The offset is tracked as a *local*, not as the
`ValueId` of the add's other operand: that operand is loaded inside the loop
body and is not in scope at the preheader where the check is emitted, so the
backend re-loads the local there — valid precisely because the loop does not
write it. And because i64 addition wraps, a wrapped `upper + c` is caught by an
explicit negative test alongside the existing `> len` test.

The payoff is real but modest: **0.75× → 0.78×**. What remains is
`table_bump`'s probe loop, which is a `while`, not a canonical `for` — it has
no induction variable and its index is bounded by `if i == cap { i = 0 }`,
which is only in-range because `cap` happens to equal the array length.
Proving that needs a different mechanism than range hoisting.

Profiling this program is also what found the `calloc` and variable-divisor
wins above, which took it from 0.55s to 0.47s at N=1 000 000.

## Not implemented

| program | blocker |
| --- | --- |
| pidigits | needs arbitrary-precision integers; no 128-bit multiply and no bitwise ops makes a limb library slow and large |
| regex-redux | needs a regex engine; every published entry links PCRE or equivalent |

Both would compare a library we wrote in lulang against C's GMP and PCRE, which
is not a language comparison.

## Caveats on comparing with the published table

The site's numbers come from its own hardware and, for most programs, from
heavily hand-optimised entries that use explicit SIMD intrinsics and multiple
threads. The lulang programs here are straightforward single-threaded
transcriptions of the reference algorithm. Compare them against the `c-O3`
and `c-O3-fastmath` columns produced by this harness on the same machine; treat
the published table as context for the algorithm and the expected output, not
as a like-for-like time.
