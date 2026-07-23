# ludiff

Forward-mode automatic differentiation implemented entirely as lulang library
code. `Dual` is an ordinary record containing a primal value and one tangent;
the chain rule comes from user-defined `⊕`, `⊖`, `⊗`, and `⊘` operators plus
dual-aware elementary functions. The compiler has no differentiation pass and
no knowledge of dual numbers.

```lu
let x = variable(2.0)
let square = x ⊗ x
let result = dual_sin(square) ⊕ (square ⊗ x)
print(primal(result), tangent(result))
```

Run the example, derivative laws, benchmarks, and executable API docs:

```bash
lu run
lu test --runs 1000
lu bench --runs 7
lu doc --runs 100
```

`polynomial_derivative(f64): f64` is exported as a scalar C function. This is
intentional: `Dual` keeps compiler-owned record layout, while the stable
boundary ABI exposes only its numerical result. Reverse mode and a tape are
later work; this package stays small enough to show that forward AD needs no
compiler feature.
