# luimage

`luimage` is a visible-output C-ABI showcase: dependency-free Mandelbrot,
exposure, inversion, and luminance kernels operate directly on borrowed
caller-owned image planes.

From this directory:

```bash
lu run
lu test --runs 100
lu bench --runs 7
lu doc --runs 50
./run_preview.sh
```

The preview command builds a static Lulang library, compiles the small C host
in `examples/render.c`, and writes `target/mandelbrot.pgm`. The PGM file opens
directly in Preview, GIMP, ImageMagick, and most image viewers. Its pixel plane
is allocated by C and filled without a boundary copy through
`c_mut_slice[f64]`; the checksum reads the same storage through
`c_slice[f64]`.

The package laws check normalization, inversion, real-axis symmetry, and
zero-exposure behavior. `lu doc` executes those laws and emits API pages,
LLVM, a generated C header/manifest, and benchmark metadata.
