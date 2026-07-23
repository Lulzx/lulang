# Known issues (2026-07-23)

State of the JIT regressions found while pre-flighting M8 against the
workspace-restructure tree. All three are fixed and the selfhost bootstrap is
back at its stage-1/2/3 byte-identical fixpoint.

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

**Fix:** `cfg_vector_value` now proves that a loaded local has no stores or
`inout` writes in the natural loop before treating it as invariant.
`simd_reductions_do_not_treat_inlined_return_slots_as_invariants` is the small
four-tier regression. `tools/verify_corpus.py` additionally runs the full
benchmark inputs across JIT, host AOT, and selfhost AOT and scaled forms
through the reference interpreter.

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
