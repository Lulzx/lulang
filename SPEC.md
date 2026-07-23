# lulang v0.1 specification

*The buildable core, frozen 2026-07-22. Everything here is chosen to (a) reproduce
AE's performance thesis, (b) be implementable by one team in weeks, not years.
See [DESIGN.md](DESIGN.md) for rationale, [ae-research.md](ae-research.md) for
evidence.*

## 1. Model

- **Value semantics only.** Variables hold values; assignment and argument passing
  are semantically copies (elided freely — aliasing is unobservable). No pointers,
  no references, no address-of.
- **Approximate floating point.** `f64`/`f32` arithmetic is reassociable and
  contractible (fast-math semantics) by language definition. `≈` (`~=`) is the
  blessed comparison. Bit-exact reproducibility is out of scope for v0.1.
- **No layout guarantees.** The compiler may re-layout records and arrays (AoS→SoA,
  padding, scalarization). There is no `sizeof`, no FFI in v0.1.
- **Whole-program optimization.** Source files are parsed as modules and linked
  before lowering; the resulting program remains fully inlinable.
- Statically typed, with local type inference (`let` needs no annotation; function
  signatures are fully annotated).

## 2. Files & entry

- Extension: `.lu`. A standalone file is the root module. In a `lu.toml`
  package, `use name` imports a declared Git dependency as a namespace.
  `use name as local` gives that namespace a local alias.
  Dependency declarations are addressed as `name.member`; each source file is
  parsed once into its own arena and modules are linked before checking and
  whole-program lowering. Unqualified imported names remain accepted when
  exactly one imported module defines them, for source compatibility.
- Entry point: a bare `main { … }` block.

## 3. Types

```
scalar  := i64 | f32 | f64 | bool
array   := [T; N]          // fixed length, N a literal or const generic (v0.2)
record  := type Name { field: T, ... }
```

Records declare with `type Name { … }`; literals are positional `Vec3 { 1, 2, 3 }`
or named `Ket { v: v }`. Field access `a.x`. Array literals `[1.0, 2.0, 3.0]`,
indexing `a[i]` (bounds-checked in JIT/debug; checks removable by properties later).

### 3.1 Edge semantics

- **Integer overflow.** `i64` unary negation, addition, subtraction,
  multiplication, and integer `sum` use two's-complement wrapping modulo 2^64.
  Integer division and remainder trap on a zero divisor and on
  `i64::MIN / -1` (or `i64::MIN % -1`); they never invoke backend undefined
  behavior.
- **Allocation.** Array lengths are logical element counts and must be
  non-negative. A negative length, a byte-size/stride overflow, or allocation
  failure terminates execution with a diagnostic and a non-zero status. No
  partially initialized array becomes observable.
- **Strings.** Runtime `str` values are byte sequences. Source string literals
  contribute their UTF-8 bytes, but `len`, indexing, and `substr` operate on
  bytes and may produce non-UTF-8 values. Equality and output preserve those
  bytes exactly; decoding is a tool/UI concern, not a language operation.
- **Evaluation order.** Function arguments, binary operands, array elements,
  record fields, and call receivers evaluate left-to-right exactly once.
  `and` and `or` short-circuit; the untaken operand is not evaluated. Assignment
  evaluates its right-hand side before mutating the target.

## 4. Declarations

```
fn dot_prod(a: Vec3, b: Vec3): f64 { return a.x*b.x + a.y*b.y + a.z*b.z }

type Vec3 { x: f64, y: f64, z: f64 }

operator* (a: Vec3) · (b: Vec3): f64 { return dot_prod(a, b) }   // infix, binds like *
operator+ (a: Vec3) ⊕ (b: Vec3): Vec3 { ... }                    // infix, binds like +
operator ‖(v: Vec3)‖: f64 { return sqrt(v · v) }                 // circumfix

property norm_nonneg(v: Vec3) { ‖v‖ >= 0.0 }

main { ... }
```

### Operator system (the AE headline feature, scoped for v0.1)

- **Infix**: `operator<anchor> (a: T) <glyph> (b: U): R { body }` where `<anchor>` ∈
  `* + == |>` — the new operator gets the anchor's precedence and associativity.
  Glyph = any single Unicode symbol character not already core syntax.
- **Circumfix**: `operator <open>(v: T)<close>: R { body }` — a matched delimiter
  pair acting as a unary operator (`‖v‖`, `|v⟩`, `⟨v|`).
- Operators are ordinary functions after parsing; resolution is by operand types
  (no overloading beyond that in v0.1).
- Every Unicode operator must have an ASCII-callable form (its function name);
  the formatter canonicalizes.
- Built-in: arithmetic `+ - * / %`, comparison `== != < <= > >=`, `~=` / `≈`
  (relative-epsilon FP compare), logical `and or not`, range `a..b`.

## 5. Statements & expressions

```
let x = expr                 // immutable binding
var x = expr                 // mutable
x = expr                     // assignment to var
if cond { } else { }
for i in 0..n { }
return expr
sum(i in 0..n) expr          // reduction primitive: order-free, always vectorizable
print(args...)
```

Blocks are expressions (last expression is the value); `property` bodies are a single
boolean expression.

## 6. Standard library (v0.1)

`math`: `sqrt sin cos acos atan2 abs min max floor pow` — **implemented over a
vectorized math kernel** (SLEEF-style) so `sum`/`for` loops over transcendentals
vectorize; scalar fallback for singleton calls. `Vec3`/`Quat` with `·`, `×`, `‖·‖`,
`slerp` as *library code* written in lulang itself, exercising the operator system.

## 7. Semantics of `≈`

`a ≈ b` ⇔ `|a-b| <= atol + rtol * max(|a|,|b|)` with `rtol = 2^-40`, `atol = 2^-100`
(overridable later). Defined for scalars; lifts componentwise to records/arrays via
`all`.

## 8. Toolchain

```
lu run  file.lu     # JIT: parse → check → SSA IR → Cranelift → execute   (dev loop)
lu build file.lu    # AOT: same IR → LLVM .ll text → clang -O3 -ffast-math -mcpu=native
lu test file.lu     # run every `property`: typed generators + shrinking
lu fmt  file.lu     # canonicalize (ASCII → Unicode operators, layout)
```

AOT pragmatism: v0.1 emits **textual LLVM IR** and shells out to `clang` — zero
linker/codegen code of our own, full `-O3` quality, and honest about the "underlying
backend" exactly as AE is. Native `inkwell` integration can come later if emit time
matters.

Performance gate (from experiments/RESULTS.md): on the alcubierre-style corpus,
`lu build` output must beat `clang++ -O3` (no fast-math) and match
`clang++ -O3 -ffast-math` within 10%; `lu run` must beat `bun run` by ≥3×.

## 9. Implementation layout (Rust workspace)

The checked middle end lowers source into a control-flow graph before any
execution tier runs. Each function owns numeric local and value tables plus
basic blocks of typed instructions. Calls contain resolved function IDs,
record/enum operations contain declaration and field/tag IDs, and every block
ends in `jump`, `branch`, `return`, or `unreachable`. Short-circuit operators,
`for`, `while`, and `sum` are therefore control flow rather than backend
special cases. Mutable bindings are explicit slots; expression evaluation is
the instruction order within a block. The IR validator checks references,
types, call signatures, inout destinations, branch targets, and returns.

```
crates/
  lu_syntax   lexer + Pratt parser w/ extensible operator table → flat arena AST
  lu_check    name resolution, type inference/checking, operator resolution
  lu_ir       typed SSA IR + lowering (sum → vector-friendly loop form)
  lu_jit      Cranelift backend (lu run)
  lu_llvm     .ll emitter + clang driver (lu build)
  lu_test     property runner: generators, shrinking
  lu (bin)    CLI
corpus/       dot.lu nbody.lu slerp.lu + C++/TS twins (from experiments/)
```

Compile-speed discipline from day one: index-based flat AST (no pointer trees),
single arena per module, spans everywhere, target <10ms frontend for 1k-line files.

## 10. Explicitly deferred (v0.2+)

Strings beyond literals in `print`, enums/matching, generics beyond array sizes,
namespaced/separately compiled modules, closures, heap collections (growable arrays), FFI, `exact`
FP mode, property-driven optimizer assumptions, AI/LLM-call runtime, self-hosting.

**Committed for v0.2** (per DESIGN.md Revision 3): `inout` parameters (mutable value
semantics, law of exclusivity) — required for growable collections, and eventually
self-hosting; and the first middle-end passes (SoA layout selection, reduction
vectorization) as lulang IR transforms rather than clang flags.

---

## 11. C boundary pointers and slices

`c_ptr[T]` is an opaque, boundary-only C pointer. It may be received from or
returned to an `extern` function, stored in a local, passed unchanged, returned
from an exported function, and compared for equality with the same pointer
type. `c_ptr[()]` is the deliberately untyped handle used when the pointee is
unknown.

There is no pointer literal, address-of operator, dereference, arithmetic, or
conversion to an integer. `T` documents and checks the boundary contract; it
does not give lulang access to C layout. The JIT and AOT represent `c_ptr[T]`
as a native pointer. The interpreters carry the same pointer bits through the
FFI bridge without inspecting the pointee.

`c_slice[i64]` and `c_slice[f64]` are borrowed, read-only boundary views.
They cross C as `(const T *data, int64_t length)`, may be indexed and passed
to `len`, and cannot be mutated or returned. The borrow lasts only for the
call. Converting an ordinary scalar array to a slice passes its existing data
buffer without copying; an exported function receives the caller's C buffer
directly. This does not expose the layout of ordinary arrays or records.

`c_mut_slice[i64]` and `c_mut_slice[f64]` are borrowed, mutable boundary
views. They cross C as `(T *data, int64_t length)`, may be indexed, passed to
`len`, and updated through indexed assignment. Like `c_slice`, they cannot be
stored in records or returned. A mutable-slice argument must be a mutable
variable, and it may not overlap any other argument for the duration of the
call. The checker rejects direct and nested aliases it can express in lulang;
foreign callers are responsible for honoring the same exclusivity contract.
Converting a compiler-owned array preserves value semantics: shared JIT
storage is made unique before the borrow, while AOT and self-hosted tiers
already clone at value-copy boundaries. An exported function mutates the
caller's C or NumPy buffer directly without allocating, copying, or invoking
the compiler-owned array runtime.

`c_fn[(P1, P2) -> R]` is a typed C function pointer. It may cross an
`extern` or `export` boundary, be stored in a local, and be returned or passed
unchanged. Its parameter and result types are checked structurally, use the
same boundary lowering and register limits as an ordinary C ABI function, and
may themselves include another callback. `c_fn[() -> ()]` denotes a callback
with no arguments and no result. Lulang does not invoke a function pointer
directly; foreign C code invokes it. An exported scalar Lulang function has a
compatible address and can therefore be installed as a callback. Generated C
headers emit exact function-pointer typedefs, and the Rust, C++, Julia, and
Python SDKs preserve the signature. Python wrappers retain callback owners
for the lifetime of a returned callable.

String parameters cross as `(const char *data, int64_t length)`. A function
returning `str` has the C signature `const char *fn(..., int64_t *out_len)`;
the length pointer is an additional final integer-class argument and counts
toward the six-integer register cap. The bytes are length-delimited and may
contain NUL. An imported return is copied into lulang-owned storage before the
foreign call completes, so the source pointer need only remain valid through
the call. An exported return points to library-lifetime storage and writes the
exact byte length through `out_len`.

An exported `[i64]` or `[f64]` return becomes an opaque owning C handle,
`lu_owned_i64 *` or `lu_owned_f64 *`. The generated header exposes typed
`data`, `len`, and `release` functions. The export wrapper transfers the
fresh array allocation into a separate boundary handle without copying
elements; its data pointer remains valid until `release`. This is export-only:
an `extern` cannot return an ordinary array, and no compiler-owned array
header or record layout is public. Generated Rust/C++ wrappers release with
RAII, Julia uses a finalizer plus `close`, and `pylulang.OwnedArray` is a
context manager.

A standard portable error result is an `@c_layout` record with exactly
`status: i64` followed by `value: i64`. Status zero (`LU_STATUS_OK` in
generated C headers) means success; any nonzero value is an application error
code and the value field is ignored. The exact two-register record works in
both ABI directions and every execution tier. Generated Rust returns
`Result<i64, LuError>`, C++ returns `Result<int64_t>`, Julia throws
`LulangError`, and `pylulang` raises `LulangError`. Error codes remain
domain-defined and stable ABI changes are checked through the record manifest.

`@c_layout type Name { ... }` opts a record into stable C field order and
layout metadata. Its fields are restricted to exact-width boundary scalars,
`c_ptr[T]`, and nested `@c_layout` records; empty records, layout cycles,
strings, and arrays are rejected. The annotation does not change ordinary
lulang records, which retain compiler-owned layout. C headers and ABI
manifests describe annotated records. By-value aggregate calls remain rejected
except for a deliberately portable register subset: a flat annotated record
with one or two fields, either all `f64` or all 64-bit integer/pointer-class
fields, may be a parameter or return value. It is passed as a real C value
aggregate in LLVM imports and export wrappers and as the equivalent return or
argument registers in both interpreters and the JIT. Mixed-class, `f32`,
nested, and wider aggregates still require an adapter. Using any annotated
record behind `c_ptr[Name]` remains supported.

Generated bindgen adapters may expose a record-valued lulang parameter without
passing that record through the boundary. The generated lulang wrapper
flattens its logical fields into supported scalar arguments, and generated C
code reconstructs the C value before the real call. Narrow integer and C
`_Bool` conversions follow the same rule; C `float` maps directly to boundary
`f32`. Flat scalar record returns that are not in the direct portable subset
use a temporary C-owned result handle plus typed field accessors; the Lulang
wrapper constructs the logical result and releases the temporary before
returning. Bitfields use these same parameter and result adapters. Unions are
opaque typed pointer targets and never claim record layout. Callback
declarations map to `c_fn`, while variadic declarations produce explicitly
typed zero-to-three-argument `i64`/`f64` wrapper patterns whose selection is
part of the caller's C contract. This is an adapter contract,
not a relaxation of the boundary type set or a promise about internal record
layout.

`lu build --target wasm32-wasi` emits a WASI command module.
`--target wasm32-web` emits a reactor module and JavaScript host loader.
WebAssembly uses the same checked program and LLVM lowering as native AOT, but
dynamic native `extern` declarations are rejected. The target does not weaken
the C-boundary rules or imply native SIMD parity.

---

# v0.2 additions (M6 — the self-hosting surface)

Implemented in all three tiers (interpreter, Cranelift JIT, LLVM AOT):

- **`enum`** — C-like sum tags: `enum Kind { Ident, Int, Eof }`. Values are
  written `Kind.Ident`, compare with `==`/`!=`, convert to their tag with
  `int(k)`, and may be stored in fields and arrays. Runtime representation is
  an `i64` tag.
- **`match`** — statement form over enum values with block arms and optional
  `else`. Without `else` the match must be exhaustive (checked at parse time,
  where the declaration is in scope — a single-pass constraint, by design).
  Desugars to an `==` chain, so pure arms if-convert in the JIT like any
  other branch.
- **`inout` parameters** — `fn step(inout lx: Lexer)`. Copy-in/copy-out value
  semantics: the callee works on its own copy; on return the final value is
  written back to the caller's variable. Arguments must be mutable variables
  of the exact type; operators and properties cannot take `inout`. In the
  outlined-call ABI the copy-out travels as extra return values; inlined
  calls write the SSA values back directly. No aliasing is ever created.
  The checker enforces this at call sites: an `inout` argument's variable
  may not be passed `inout` twice in one call, and no sibling argument may
  contain a call that mutates it through its own `inout` parameter (copy-in
  snapshots the variable at its argument position, so such a sibling write
  would be silently lost). Read-only sibling uses remain legal:
  `addto(g, g * 2)` is fine, `take(g, bump(g))` is rejected.
- **Field assignment** — `lx.pos = e`, including nested paths, on mutable
  record variables. Pure value semantics (copy-on-write in the interpreter,
  per-component SSA/alloca writes in the back ends).
- **`while`** — `while cond { … }`. The first unbounded loop; `for` remains
  the bounded range loop.
- **Strings** — `s[i]` yields the byte at `i` as `i64` (bounds-checked),
  `len(s)` the byte length, `substr(s, lo, hi)` a checked view (no copy in
  the compiled tiers), `==`/`!=` compare contents. Byte-char literals `'a'`,
  `'\n'`, `'\''` are `i64` literals.
- **`fn` with no return annotation** returns unit (previously an accidental
  `bool` default; `property` bodies remain predicates).

- **Short-circuit `and`/`or`** — the right operand does not evaluate when the
  left decides. (The interpreter always did this; the compiled tiers
  previously evaluated both sides, which aborted on guard idioms like
  `i < len(s) and s[i] == c`. Now uniform across tiers.)
- **Escapes** — `\r` and `\0` now unescape correctly in string and char
  literals (previously fell through as the literal letter).

**inout ABI note.** Inlined calls write the copy-out back as SSA values. An
outlined call (recursion) passes one hidden out-pointer per `inout` parameter
and the callee stores the final value through it before returning — copy-out
values can exceed what return registers hold (e.g. `inout p: Parser`).

First artifacts: [selfhost/lexer.lu](selfhost/lexer.lu) — the lulang lexer in
lulang — [selfhost/parser.lu](selfhost/parser.lu) — a recursive-descent
Pratt parser for the core language, building a flat index-based AST out of
record arrays (the same architecture as the Rust compiler) and printing a
deterministic pre-order dump — and [selfhost/checker.lu](selfhost/checker.lu)
— a typechecker over that flat AST mirroring `src/check.rs`: types are i64
codes (`i64/f64/bool/str/()` = 0–4, `[T]` = T+8), scopes are a linear symbol
stack, and the rules match the Rust checker (int→float widening, bool
conditions, exact-type `var` for `inout` args, fixed builtin signatures). Its
driver checks 2 well-typed and 10 ill-typed programs, reporting the first
error. Finally [selfhost/interp.lu](selfhost/interp.lu) closes the loop and
**runs its own source**: the full pipeline (lexer with char literals, parser
with `type`/`enum` declarations and record literals, quiet checker with
enum/record types) plus a tree-walking evaluator — tagged value records, a
bump-allocated heap of value slots for arrays and record blocks (records keep
value semantics: blocks copy on var-bind/assign/array-store, immutable
bindings share), a linear env stack with per-call frame pointers,
short-circuit `and`/`or`, `inout` write-back, and int→float coercion at the
points the checker allows it. Programs executed by interp.lu print
byte-identical output to the same programs under `lu run`, and
`lu run selfhost/interp.lu selfhost/interp.lu fib.lu` runs a two-level
interpreter tower — interp.lu interpreting its own 1,750-line source, which
then interprets fib — verified to print the same answer in all three tiers
(0.9 s AOT, 2.4 s JIT, 160 s host interpreter). Towers go deeper with more
value-heap: `--heap N` sets slots per guest source byte (default 256, each
level multiplies the cost ~10×), and flags pass through levels, so

```
./interp --heap 4000 selfhost/interp.lu --heap 190 selfhost/interp.lu fib5.lu
```

runs a **three-level** tower — lulang on lulang on lulang on native — in
2.9 s (AOT, ~11 GB value heap). All
four artifacts produce byte-identical output under `lu interp`, `lu run`,
and `lu build`.

**Program input.** `nargs(): i64`, `arg(i: i64): str`, and
`read_file(path: str): str` expose CLI arguments (everything after the source
file) and file contents in all three tiers;
`write_file(path: str, contents: str)` writes a file (creating or replacing
it), so a self-hosted toolchain can emit its output without shell redirection; `puti/putf/putb/puts/putsp/putnl`
are newline-free print primitives (the evaluator uses them to reproduce host
`print` formatting exactly); `chr(b: i64): str` and
`concat(a: str, b: str): str` construct strings (the self-hosted lexer uses
them to decode escape sequences). The self-hosted interpreter shifts `arg` by
one for the program it runs — the unix interpreter convention that makes
unmodified towers possible.

**Ladder self-application.** interp.lu also supports `sum` and `match`
(desugared at parse time into an `==` if-chain with exhaustiveness checking,
exactly like the host parser), decodes string escapes, and parses float
literals via exact-mantissa/single-division (correctly rounded like the
host's, up to ~15 significant digits). It covers the full v0.1 surface too:
`operator` declarations (infix with anchor-copied precedence, circumfix with
open/close glyphs — both desugared at parse time into calls of
`operator<glyph>` functions, the host's own scheme), `property` declarations
(bool-checked, never called by `run`), positional record literals, and
multi-byte UTF-8 operator glyphs lexed as single symbol tokens. With that,
the whole ladder *and the AE teaser corpus* run on it byte-identically:
`lu run selfhost/interp.lu <file>` prints exactly what the native tiers
print for lexer.lu, parser.lu, checker.lu, corpus/dot.lu, and
corpus/slerp.lu (`·`, `‖·‖`, and all).

**Float printing.** All tiers print f64 as the shortest decimal that parses
back exactly, in plain notation (never scientific) — Rust's `Display`
semantics. The interp/JIT tiers print through Rust; the C runtime implements
the same contract (probe `%.*e` precisions until `strtod` round-trips, then
re-render without the exponent), so AOT output is byte-identical too.

Self-hosting has now surfaced and fixed three compiler gaps: float `%`
(JIT calls `lu_fmod`, AOT emits `frem`, matching Rust's libm `%`), an
unbounded inline policy (the JIT inlined every call to depth 8 with no size
limit, which exploded exponentially on the evaluator's large mutually
recursive functions — inlining now has a 3,000-statement per-function
budget), and thin native stacks (all tiers now run programs on a 512 MiB
thread; AOT binaries enter through `lu_entry` on a pthread the C runtime
spawns).
