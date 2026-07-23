# lu-numerics

The first-party numerical kernel package for lulang. Its v0.1 API covers:

- vectors, descriptive statistics, polynomial evaluation, and trapezoidal
  integration;
- dense matrix-vector and square matrix multiplication;
- convolution and moving-average signal kernels;
- deterministic random sampling and a Monte Carlo pi kernel;
- bisection, geometry, interpolation, probability-density, sigmoid, sinc, and
  integer combinatorics.

Run the package from this directory:

```bash
lu run
lu test --runs 1000
lu bench --runs 7
lu doc --runs 100
lu build --lib --shared -o lu_numerics src/lib.lu
```

Every exported function has adjacent `///` documentation, is reached by an
executable law in `tests/laws.lu`, appears in
`benchmarks/functions.tsv`, and has a source-matched C++/NumPy/Julia row in
`comparisons/functions.tsv`. The integration suite proves all function pages
have prose and passing properties, compiles the C++ reference, calls the
generated library from Python, and runs the package through the interpreter,
JIT, LLVM AOT, and self-hosted interpreter.

Array-transform exports use boundary buffers. A C or Python caller observes
copy-out; an ordinary lulang caller still gets value semantics and no alias to
its input. Scalar dimensions keep signatures within the stable C ABI register
cap.

The original `vector.lu`, `statistics.lu`, `integrate.lu`, and `linalg.lu`
files remain small standalone build examples. The package API and executable
documentation are sourced from `src/lib.lu`.
