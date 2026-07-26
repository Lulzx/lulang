# Known issues (2026-07-26)

State of the compiler regressions and design constraints found while
pre-flighting M8 and the shared SIMD middle-end. Fixed entries retain their
repros so a reintroduction is recognizable. One entry is open: issue 8,
nondeterministic LLVM emission, which predates the Cranelift AOT tier and costs
byte-reproducible `lu build` output without affecting program behavior.

## 1. FIXED — JIT assumed topological block order after IR inlining

**Symptom:** `lu run corpus/bench_slerp.lu` → `error: IR value %15
unavailable`; `selfhost/build.sh --bootstrap` died the same way at stage 1.
Interp and AOT were unaffected.

**Cause:** `inline_calls` (`src/backend/optimization.rs`) appends the
continuation block and the inlined callee's blocks at the end of
`Function.blocks`, so block indices stop being control-flow-ordered (entry
jumping to b5, loop header b1 using `%6` defined in b4). But
`Gen::gen_ir_body` (`src/jit.rs`) walks blocks **by index** and fills its
value table in that order, so a value defined in a later-indexed dominator
was "unavailable". The interpreter walks the CFG dynamically and LLVM IR has
no textual-order requirement, which is why only the JIT broke.

**Fix (landed):** `normalize_block_order` in
`src/backend/optimization.rs`, called at the end of `inline_calls` — renumbers
blocks into reverse postorder from the entry (a definition's block dominates
its uses, and dominators precede what they dominate in RPO, so index-order
emission is valid again). Unreachable blocks are dropped. Verified: repros
pass, all four `cargo test` suites green, interp/JIT/AOT agree on
`bench_slerp.lu`.

## 2. FIXED — JIT miscompiled outlined functions' string constants

**Repro** (fast, deterministic failure with nondeterministic garbage):

```sh
echo 'main { print("hi") }' > /tmp/tiny.lu
target/release/lu run    selfhost/interp.lu /tmp/tiny.lu   # garbage + "PARSE FAILED"
target/release/lu interp selfhost/interp.lu /tmp/tiny.lu   # prints "hi" (reference OK)
target/release/lu build  selfhost/interp.lu && ./interp /tmp/tiny.lu  # prints "hi" (AOT OK)
```

The bootstrap fails the same way: stage 1 (`lu run selfhost/codegen.lu ...`)
emits a `.ll` whose first bytes are raw memory (looks like a pointer value +
zeros) followed by fragments of real output (`" 47 "`, `"defi"`,
`"@.str."`), i.e. codegen.lu's own parser fails on its own source with
corrupted diagnostic strings.

**Cause:** `Constant::Bytes` emission embedded `bytes.as_ptr()` from the
optimized `ir::Function`. Each optimized function is a temporary clone that is
dropped immediately after compilation, leaving generated code with dangling
addresses. Later compiler allocations reused that memory for names such as
`$inlined66_$tmp1`, which is why those names appeared in corrupted token text.
The main function often hid the bug because its optimized clone remains alive
through execution.

**Fix (landed, in `src/jit.rs`):** JIT-owned boxed copies of string
constants now stay alive until generated code finishes executing. A recursive
outlined-function regression in `tests/conformance.rs` failed deterministically
before the fix (thirteen NUL bytes instead of `stable string`) and now passes.
The original tiny-file repro and `selfhost/parser.lu` are byte-correct, and
`cargo test --release` passes all 20 tests.

## 3. FIXED — eager array copying exhausted memory in bootstrap stage 1

With issue 2 fixed, bootstrap stage 1 proceeds past parsing but is killed by
the OS while `lu run selfhost/codegen.lu selfhost/codegen.lu` emits the
compiler. A measured run reached about 9.3 GB resident memory before SIGKILL.

**Cause:** every language store eagerly cloned array components, and IR
inlining represents call parameters/results with synthetic stores. Passing
the selfhost compiler's large `P` and `G` records through inlined calls
therefore copied their backing arrays repeatedly. Allocation tracing at the
first GiB measured 1,013 MiB of array clones, 15 MiB of initial arrays, and
effectively no string allocation.

**Fix (landed):** the JIT runtime now keeps array ownership counts in a
side table without changing the compiler-owned array layout. Language stores
retain shared storage, mutations call `lu_arr_cow` and update the owning local
(including arrays nested in records), and inliner-generated parameter/result
stores are explicitly marked as call-scoped borrows. Fresh SSA allocations
start with zero persistent owners. The full `selfhost/build.sh --bootstrap`
now completes, stage 1 matches stage 2, and stages 2/3 are byte-identical.

## 4. FIXED — SIMD `sum` treated an inlined return slot as invariant

**Symptom:** `lu run corpus/alcubierre.lu` printed `total: 0`; the reference
interpreter and both AOT compilers printed `25.587776819835558`.
`LU_SIMD=off` restored the correct JIT result.

**Cause:** reduction vectorization allowed any non-induction `f64` local as a
loop invariant. After `rho` was inlined inside `sum`, the callee return slot
was an ordinary local but was stored on every iteration. SIMD splatted its
pre-loop zero value and skipped the scalar loop body.

**Fix:** the shared middle-end SIMD plan now proves that a loaded local has no
stores or `inout` writes in the natural loop before treating it as invariant.
JIT and LLVM AOT consume that proof directly; the self-host mirrors the same
pure-expression rules.
`simd_reductions_do_not_treat_inlined_return_slots_as_invariants` is the small
four-tier regression. `tools/verify_corpus.py` additionally runs the full
benchmark inputs across JIT, host AOT, and selfhost AOT and scaled forms
through the reference interpreter.

## 5. FIXED — selfhost persistent array values aliased

**Symptom:** the observatory's selfhost dot binary was much faster than host
AOT, but a direct value-semantics check printed `2 2` instead of `1 2`:

```lu
main {
  var a = arr(1, 1)
  var b = a
  b[0] = 2
  print(a[0], b[0])
}
```

**Cause:** `selfhost/codegen.lu` stored array pointers directly for `let`,
`var`, and whole-variable assignment. The generated program therefore shared
mutable backing storage between language values. The dot benchmark happened
to be read-only, so its numerical-answer check could not expose the violation.
Host LLVM had the opposite problem: it cloned immutable array parameters on
every call even though the checker makes them read-only.

**Fix:** the selfhost emitter now recursively locates owning array components
inside flattened records and clones them at persistent binding and assignment
boundaries. Parameters borrow: ordinary parameters are immutable and `inout`
parameters are exclusive. Host LLVM now follows the same calling convention,
matching the JIT. A compiled regression covers direct arrays, record-contained
arrays, and rebinding. Bootstrap again reaches a stage-1/2/3 byte-identical
fixpoint. Fresh observatory medians put host and selfhost dot AOT at 16.102 ms
and 15.746 ms respectively, replacing the unsound 64.053/13.677 comparison.

## 6. FIXED — packed f32 SIMD required an array-layout migration

Scalar array components used to occupy uniform 8-byte storage slots, including
`f32`. That kept record flattening, SoA plane offsets, the C runtime, and both
bootstrapped emitters on one simple addressing contract, but adjacent `f32`
language elements were not adjacent 4-byte machine values, so loading
`<4 x float>` from that storage would have mixed values with slot padding.
The shared SIMD plan therefore accepted f64 and exact wrapping i64 reductions
while leaving f32 scalar, and
`keeps_f32_reductions_scalar_until_arrays_are_packed` guarded against an unsafe
partial implementation.

**Fix (landed):** the coordinated packed-layout migration. Element storage now
uses packed component widths (`Component::bytes` in `src/backend/layout.rs`:
4 for `f32`, 8 for `i64`/`f64`/pointers, 16 for the explicit vector
components), with each SoA plane individually 8-byte aligned. The array header
grew from 8 to 16 bytes and caches the logical length, so bounds checks and
slice coercions no longer recompute it with an `sdiv` by the element stride.
SIMD load/store in both backends addresses by byte offset with per-component
plane spans, shared through `src/backend/simd.rs`.

The guard test is replaced by
`packed_f32_simd_reductions_handle_four_lane_vectors_and_scalar_tails`
(explicit `f32x4` values and intrinsics alongside `f64x2`/`i64x2`),
`proves_packed_f32_array_reductions_for_simd` in
`src/backend/optimization.rs`, and a selfhost_sync assertion that host and
selfhost emit packed `f32x4` loads byte-for-byte. WASM coverage sums a packed
`f32` array. Verified: `cargo test --release` (87 tests),
`selfhost/build.sh --bootstrap` at a stage-1/2/3 byte-identical fixpoint, and
`tools/verify_corpus.py` four-tier agreement across the corpus.

Note for anyone touching `src/lu_runtime.c`: `selfhost/build.sh` caches the
compiled runtime object at `$TMPDIR/lu_selfhost_runtime.o` and does not notice
that the source changed. Delete it before bootstrapping after a runtime edit.

## 7. FIXED — interpreter deep-copied the array on every element store

**Symptom:** the reference interpreter was quadratic in array length. Element
assignment loops that the other three tiers finish in milliseconds ran for
minutes, and the full benchmark inputs were not interpretable at all.

**Repro** (each row is 2× the elements of the row above; time grows 4×):

```sh
target/release/lu interp corpus/dot.lu   # n = 100k: 105.3 s (JIT: 12 ms)
```

| n | before | after |
|---|---|---|
| 12 500 | 1.40 s | 0.01 s |
| 25 000 | 5.97 s | 0.01 s |
| 50 000 | 23.72 s | 0.02 s |
| 100 000 (`corpus/dot.lu`) | 105.3 s | 0.04 s |

**Cause:** `a[i] = x` lowers to a `Load` of the array followed by `SetIndex`
(the compiled backends need the loaded pointer; the interpreter updates through
`root` and never reads `base`). The `Load` result stayed in the SSA value table
for the rest of the function, as did the `arr()` result already stored to the
local, so `Rc::make_mut` in `set_index` always saw a shared `Rc` —
`strong_count` measured 3 on the first store and 2 on every store after — and
copied the entire `Vec<Value>` per element written. The value table had no
notion of liveness: nothing was ever released.

**Fix (landed, in `src/interp.rs`):** a per-function `Liveness` analysis
(upward-exposed uses, backward dataflow to a fixpoint over the CFG) records
which values take their last read at each instruction; `execute` releases those
slots, and the arms that mutate — `Store`, `SetIndex`, `SetField`, and calls —
release *before* the update so the owning local is the sole reference and the
write happens in place. Restricting `gen` to upward-exposed uses is what lets a
value defined and consumed inside a loop body die there rather than being held
live around the back edge. Liveness is computed once per function in
`Interp::new`, so recursive calls (self-hosted interpreter towers) pay only a
lookup.

The guard is `tests/interp_perf.rs`, which interprets 8 000 and 32 000 stores
and fails if 4× the work costs more than 8× the time (the regression measures
15.7×). Verified: `cargo test --release` (all 19 suites, 88 tests),
`tools/verify_corpus.py` four-tier agreement, and a cross-tier value-semantics
check (copy-on-assign, callee copies, record-held arrays, aliasing after a
write loop) where interpreter, JIT, and AOT print identical results.

Now that interpretation is linear, the full benchmark inputs run through the
reference tier in seconds (`bench_dot` 6.9 s, `bench_qnorm` 18.6 s,
`bench_slerp` 8.1 s, `alcubierre` 5.2 s). `tools/verify_corpus.py` still
interprets mechanically scaled inputs to keep the correctness gate quick; that
scaling is now a speed choice rather than a necessity.

## 8. OPEN — the Rust LLVM emitter is not deterministic

**Symptom:** the same source, the same binary, the same environment, three
different outputs:

```sh
for i in 1 2 3; do
  target/release/lu build --emit-llvm -o /tmp/d$i.ll selfhost/codegen.lu
done
md5 /tmp/d1.ll /tmp/d2.ll /tmp/d3.ll   # three different hashes
```

The diff is small (~124 lines on a 9.6 MB module) and semantically empty —
two temporaries swap numbers and their uses follow:

```
28503c28503
<   %t4892 = load ptr, ptr %t65
---
>   %t4892 = load ptr, ptr %t64
```

**Cause:** not yet located. The shape (adjacent temporaries permuting) points
at iteration over an unordered collection during emission — `src/llvm.rs`
carries several `HashMap`/`HashSet`s, as does the shared `analyze_cfg`.

**Scope:** found while confirming that `LU_INLINE` cannot reach the LLVM tier.
Reproduced unchanged at commit `43ae1a2`, so it predates the Cranelift AOT
work. Program behavior is unaffected — `tools/verify_corpus.py` agrees across
all four tiers on every run — and the self-hosted bootstrap fixpoint is
unaffected because it compares output from `selfhost/codegen.lu`, which is
deterministic (the same file emitted through the JIT at inline budgets 256 and
3000 is byte-identical). What it costs is byte-reproducible host `lu build`
output, which the project otherwise takes seriously enough to gate the
bootstrap on.

**Suggested fix:** switch the emission-order-visible maps to `BTreeMap`/
`IndexMap`, or seed a deterministic hasher, then add a regression that emits
the same module twice and compares bytes.

## Incident note: lost uncommitted jit.rs delta

During diagnosis, `git checkout src/jit.rs` was run to revert a temporary
debug edit and instead discarded the **uncommitted** ~124-line working-tree
delta to `src/jit.rs` (part of the workspace/f32/fmt restructure; everything
else from that restructure is intact). Post-loss verification: the workspace
compiles clean, all test suites pass, the corpus and both regressions behave
identically — HEAD's jit.rs already contains the f32 handling, `pure_imports`
and LICM wiring, so the lost lines are not covered by any current test.
Recovery options if the content mattered: an open editor buffer holding
`src/jit.rs`, or the implementing agent's (codex) session log/diff if it
authored those lines.
