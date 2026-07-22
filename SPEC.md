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
- **Whole-program compilation.** One compilation unit per invocation; everything
  monomorphized and inlinable.
- Statically typed, with local type inference (`let` needs no annotation; function
  signatures are fully annotated).

## 2. Files & entry

- Extension: `.lu`. One file = one module. `use math` imports (stdlib only in v0.1).
- Entry point: a bare `main { … }` block.

## 3. Types

```
scalar  := i64 | f64 | bool
array   := [T; N]          // fixed length, N a literal or const generic (v0.2)
record  := type Name { field: T, ... }
```

Records declare with `type Name { … }`; literals are positional `Vec3 { 1, 2, 3 }`
or named `Ket { v: v }`. Field access `a.x`. Array literals `[1.0, 2.0, 3.0]`,
indexing `a[i]` (bounds-checked in JIT/debug; checks removable by properties later).

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
modules beyond stdlib, closures, heap collections (growable arrays), FFI, `exact`
FP mode, property-driven optimizer assumptions, AI/LLM-call runtime, self-hosting.

**Committed for v0.2** (per DESIGN.md Revision 3): `inout` parameters (mutable value
semantics, law of exclusivity) — required for growable collections, and eventually
self-hosting; and the first middle-end passes (SoA layout selection, reduction
vectorization) as lulang IR transforms rather than clang flags.

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
lulang — and [selfhost/parser.lu](selfhost/parser.lu) — a recursive-descent
Pratt parser for the core language, building a flat index-based AST out of
record arrays (the same architecture as the Rust compiler) and printing a
deterministic pre-order dump. Both produce byte-identical output under
`lu interp`, `lu run`, and `lu build`.
