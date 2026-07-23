# Benchmark observatory

`observatory.tsv` is the source of truth for the cross-language table emitted
by `lu doc`. Every row links to all implementations and states the semantic or
layout assumptions that affect the comparison. Missing runtimes are left
blank, never estimated.

Regenerate it on the current machine with:

```bash
python3 benchmarks/run_observatory.py --runs 7 --bootstrap
```

The runner:

1. builds the release host compiler and, with `--bootstrap`, proves and installs
   the three-stage self-hosted compiler;
2. compiles the same dot and quaternion-slerp workloads with lulang host AOT,
   lulang selfhost, C++ `-O3`, C++ fast-math, and Rust;
3. measures those binaries plus lulang JIT, Julia, NumPy, and JavaScript when
   their runtimes are installed;
4. rejects results whose printed numerical answers disagree beyond the
   approximate-floating-point tolerance;
5. writes median whole-process time and an `environment.json` provenance
   record.

The programs deliberately keep initialization and output inside every process.
That makes the result reproducible and honest about startup, but it is not a
microbenchmark of kernel cycles alone. NumPy slerp uses its natural vectorized
batch representation; the observatory calls that layout difference out rather
than presenting it as a like-for-like scalar loop.
