# luphysics

A compact value-semantic physics showcase for lulang:

- 2D vectors, particles, semi-implicit Euler integration, softened N-body
  gravity, and equal/unequal-mass circle impulses;
- executable laws for stationary bodies, zero timesteps, elastic momentum,
  and elastic kinetic energy;
- an exported SoA `integrate_axis` kernel whose generated C header can be
  embedded in Python, C, C++, or Rust without exposing record layout.

Run it from this directory:

```bash
lu run
lu test --runs 1000
lu build --target wasm32-wasi
lu build --lib --shared -o luphysics src/lib.lu
```

If raylib is installed, `./run_raylib.sh` builds the small boundary adapter and
runs an interactive three-body visualizer. The adapter deliberately converts
raylib's `float`, `Color`, `Vector2`, and NUL-terminated title at the C
boundary; none of those layouts leak into lulang records.

The implementation deliberately uses records as compiler-owned values and
arrays as the numerical storage boundary. No internal `Body` or `Vec2` layout
is promised to C; only the exported array/scalar kernel crosses the stable
boundary ABI.
