# M8 Implementation Plan: C ABI / FFI (import + export)

Execution plan for the first [ROADMAP.md](ROADMAP.md) milestone. Written for
an implementing agent; all file paths are real edit sites verified against the
current tree (Cargo workspace where `crates/*` are `#[path]` shims into
`src/`, so `src/*.rs` remain the sources of truth).

## Pre-flight (blocking)

Before starting M8, the tree must pass the standing verification bar:
`cargo test --release`, the corpus under all tiers, and
`selfhost/build.sh --bootstrap` (stage-2/stage-3 byte-identical IR). As of
2026-07-23 all pre-flight regressions in
[KNOWN-ISSUES.md](KNOWN-ISSUES.md) are fixed, the release tests pass, and the
bootstrap has regained its stage-1/2/3 byte-identical fixpoint.

## Syntax

```lu
extern fn llabs(x: i64): i64          // symbol from already-linked libs
extern "m" fn cbrt(x: f64): f64       // the string names the library
export fn saxpy(a: f64, x: [f64], y: [f64], n: i64): f64 { ... }
```

- `extern` and `export` stay plain identifiers (`Tok::Ident`) — no lexer
  change, which keeps the selfhost lexer region untouched.
- The string after `extern` names the **library** (`-lm` for AOT, dlopen for
  JIT/interp), not a calling convention. Absolute paths accepted verbatim.
- Checker errors: `inout` in an FFI signature; extern name colliding with a
  user fn, builtin, or the `lu_` prefix; `extern` anywhere but top level.

## Type mapping (checker-enforced boundary subset)

| lulang | C | components | import arg | import ret | export arg | export ret |
|---|---|---|---|---|---|---|
| `i64` | `int64_t` | I64 | yes | yes | yes | yes |
| `f64` | `double` | F64 | yes | yes | yes | yes |
| `bool` | `int64_t` (0/1) | I64 | yes | yes | yes | yes |
| `enum E` | `int64_t` tag | I64 | yes | yes | yes | yes |
| `()` | `void` | — | — | yes | — | yes |
| `str` | arg: `const char* p, int64_t n`; ret: `const char *` + final `int64_t *out_len` | Ptr+I64 arg; Ptr ret + hidden I64 arg | yes | yes | yes | yes |
| `[f64]`/`[i64]` | `T* data, int64_t n` | Ptr | yes (data ptr = handle+8) | no | yes (copy-in/out wrapper) | no |
| `c_slice[f64]`/`c_slice[i64]` | `const T* data, int64_t n` | Ptr+I64 | yes | no | yes (borrowed, no copy) | no |
| `f32` | `float` | F32 | yes | yes | yes | yes |
| portable `@c_layout` record | C struct value | 1–2 homogeneous I64/Ptr or F64 | yes | no | yes | no |
| other records, nested arrays | — | — | no | no | no | no |

- Direct `f32` is a post-M8 compatible extension. The dependency-free
  interpreter trampoline carries raw F32 bits in the low half of each FP
  register; JIT and LLVM use native `float` signatures. Selfhost preserves
  the distinct type and generated headers/manifests spell it as `float`/`f32`.
- `c_slice[T]` is a post-M8 borrowed-view extension for 64-bit scalar
  elements. It is read-only, cannot be returned, and preserves the caller's
  `(const T*, length)` without constructing an ordinary lulang array.
- String returns are a post-M8 length-delimited extension. The hidden
  `int64_t *out_len` is appended to the C parameter list and consumes one
  integer register. Imports copy the returned bytes immediately; exports
  return library-lifetime storage. Embedded NUL bytes are preserved.
- Direct `@c_layout` parameters are a post-M8 compatible extension limited to
  flat records with one or two 64-bit fields in one register class. This is
  the subset whose register placement is identical on SysV x86-64 and
  AArch64; mixed, `f32`, nested, wider, and returned aggregates retain
  adapters or explicit diagnostics.
- **Register-class cap (language rule):** ≤6 integer-class + ≤8 float-class
  components per FFI signature. This keeps every argument in registers on
  SysV x86-64 (6 GPR/8 XMM) and AArch64 (8/8), which is what makes the
  dependency-free trampoline correct. Count with the existing
  `layout::components()` (`src/backend/layout.rs`).

## Shared front end + IR

- `src/ast.rs`: `ExternDecl { name, lib: Option<String>, params, ret }`,
  `Program.externs`, `FnDecl.exported: bool`.
- `src/parser.rs`: `extern` arm in the top-level loop (optional `Tok::Str`
  lib, then a body-less fn signature — reuse `parse_params`/type parsing);
  accept `export` prefix on `fn`.
- `src/check.rs`: register extern signatures into `Checker::sigs` (same loop
  that registers fns, ~line 94) so call sites check for free; add the
  boundary-subset + 6/8-cap validation over `p.externs` and exported fns.
- `src/ir.rs`: `Callee::Extern(u32)`; `ExternDef { name, lib, params, ret }`;
  `LoweredProgram.externs`; `Function.exported`. Lowering resolves call names
  user fns → externs → builtins. `validate()` checks extern arity/types and
  all-`None` inout, mirroring the `Callee::Function` arm.

## Import, per tier

New `src/ffi.rs` (~120 lines, no external crates), included via `#[path]` in
both `crates/lu_jit/src/lib.rs` and `crates/lu_test/src/lib.rs` exactly like
`src/runtime.rs` already is:

- raw `dlopen`/`dlsym` externs (darwin/linux), `RTLD_DEFAULT` when `lib` is
  `None`, `lib{name}.dylib|.so` resolution with handle caching, good errors.
- Universal trampoline: `call_ret_i64(fnptr, [i64;6], [f64;8]) -> i64` and
  `call_ret_f64(...) -> f64` via transmute to
  `extern "C" fn(i64×6, f64×8) -> _`. Integer and float register classes are
  assigned independently on both target ABIs, and the 6/8 cap guarantees no
  stack arguments, so garbage in unused slots is safe for non-variadic
  callees. `()` returns go through `call_ret_i64` and discard. If the
  trampoline misbehaves on a future target, swap the internals for the
  `libffi` crate without touching any tier.

- **AOT** (`src/llvm.rs`): emit one `declare` per extern after the builtin
  declare block (~lines 126-146); `Callee::Extern` call arm next to the
  builtin arm (arrays: `getelementptr i8, ptr %h, i64 8` for the data
  pointer); collect `{lib}` and append `-l<lib>` to the clang invocation;
  honor `LU_LINK_FLAGS` for `-L` paths.
- **JIT** (`src/jit.rs`): `jb.symbol(&e.name, ffi::resolve(&e.lib, &e.name)?)`
  before `JITModule::new` (same mechanism as the `lu_*` symbols); declare with
  `Linkage::Import` using `layout::components()` (Ptr→I64); do **not** mark
  pure; `Callee::Extern` call arm mirrors the builtin path.
- **Interpreter** (`src/interp.rs`): implement from day one — the interpreter
  is the reference tier and refusal would fork the differential harness.
  `Callee::Extern` arm beside `Callee::Builtin`: marshal Int/Bool/Enum → int
  slots, Float → float slots, Str → (ptr,len) int slots; arrays copy-in/out
  between `Rc<RefCell<Vec<Value>>>` and a flat temp buffer (preserves
  C-visible mutation semantics identically to the flat tiers). Cache resolved
  pointers per `ExternDef`.

## Export: `lu build --lib`

Only AOT materializes exports; elsewhere `export` is a no-op annotation, so
conformance is unaffected.

- `src/llvm.rs`: user fns stay `define internal`; per exported fn emit a
  `define dso_local` C-ABI shim that copy-ins `[T]` params via
  `lu_arr_new_raw` (stride 1 so slots == len — assert this), calls the
  internal fn, copies back out. Scalar components already match 1:1.
- New `src/cheader.rs`: pure `emit_header(&LoweredProgram) -> String` —
  `#include <stdint.h>`, enum constants, prototypes with parameter names and
  lulang-signature doc comments. Also emit `NAME.json` (machine-readable ABI
  manifest: name/params/ret per export) — the `pylulang` foundation.
- `src/lu_runtime.c`: wrap `main`/`entry_thunk`/the `lu_entry` call in
  `#ifndef LU_LIB`; `--lib` compiles the runtime with `-DLU_LIB`; keep
  `lu_set_args` exported. Lift the "no `main` block" requirement in lib mode.
- `src/main.rs`: `lu build --lib [--shared] [-o name] file.lu` → `libNAME.a`
  or `.dylib`/`.so` + `NAME.h` + `NAME.json`. Add flag arms to the existing
  option loop (the one handling `--runs`/`--check`), gated on
  `mode == "build"`; update `usage()`.

## Self-hosted compiler (order is forced by the sync rule)

The front end (lexer/parser/checker) is shared verbatim between
`selfhost/interp.lu` and `selfhost/codegen.lu` — edit interp.lu's shared
region and byte-copy it into codegen.lu (don't hardcode line numbers).

1. Add extern/export parsing + subset checks to interp.lu's shared region
   (flat parallel tables for externs; an `fexported` flag).
2. Byte-copy into codegen.lu; refresh the standalone `lexer.lu`/`parser.lu`/
   `checker.lu` mirrors.
3. codegen.lu emits declares/wrappers **byte-identically** to src/llvm.rs —
   fix the emission order in llvm.rs first, then mirror. Surface link flags
   as a `; link: -lm` comment line consumed by `selfhost/build.sh` (stdout
   stays a valid .ll).
4. Extern *execution* in interp.lu (Slice C): add `lu_dlopen`, `lu_dlsym`,
   `lu_ffi_call_i`, `lu_ffi_call_f` (the same trampoline, in C) to
   `src/lu_runtime.c`, and have interp.lu call them **via the new extern
   feature itself**. Until then interp.lu errors cleanly on extern calls.
5. After every slice: `selfhost/build.sh --bootstrap` must hold its
   stage-2/stage-3 fixpoint (codegen.lu itself uses no externs). Delete the
   cached `$TMPDIR/lu_selfhost_runtime.o` after touching lu_runtime.c.

## Slices (each independently landable)

- **A — import, host tiers (complete)**: front end + IR + ffi.rs + all three Rust tiers
  + selfhost front-end parsing + codegen.lu declares; selfhost interp gives a
  clean "not yet supported" error; conformance runs with a temporary
  `skip_selfhost` mask on extern-executing cases.
- **B — export (complete)**: `--lib` CLI, wrappers, header + manifest, `-DLU_LIB`
  runtime guard, C and ctypes integration tests, codegen.lu wrapper
  byte-diff.
- **C — close the gap (complete)**: interp.lu extern execution, delete the skip mask,
  full four-tier FFI conformance.

## Test plan

- `tests/conformance.rs` (complete): positive cases with deterministic, portable symbols
  (`extern fn llabs(x: i64): i64`; `extern "m" fn cbrt(x: f64): f64`; an
  array-mutation case against a known `lu_*` runtime symbol declared as a
  plain extern; `cbrtf(float) -> float`). Negative cases: record in signature,
  >6 int-class args, `[f32]`, inout, unresolvable symbol.
- `tests/ffi_export.rs` (complete): build a `--lib` fixture, compile a ~20-line C
  harness against the generated header with clang, run, diff stdout; a
  second harness via `python3 -c "import ctypes; ..."` guarded on python3
  presence.
- Corpus (complete): `corpus/ffi_cbrt.lu` (import) and
  `corpus/kernel_saxpy.lu` (export,
  with `saxpy.c`/`saxpy.py` harnesses) — doubles as the README adoption demo.
- Selfhost (complete): bootstrap fixpoint after every slice; byte-diff
  `lu build`'s .ll
  vs codegen.lu's .ll for an extern-using and an export-using corpus program;
  test that interp.lu's shared region == codegen.lu's. These invariants are
  executable in `tests/selfhost_sync.rs`.
- Close-out (complete): README M8 status row and the landed M8 commit.

## Risks

1. Trampoline correctness on future targets — isolated behind `ffi.rs`,
   swappable for libffi.
2. Byte-identity of declare/wrapper ordering between llvm.rs and codegen.lu —
   fix llvm.rs order first; land the byte-diff test in the same PR.
3. `lu_arr_new_raw`'s header stores *slots*, not element count — export
   wrappers use stride 1 so slots == len; assert in the emitter.
