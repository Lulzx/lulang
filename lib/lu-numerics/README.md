# lu-numerics

First-party numerical kernels written in portable lulang. Each source is an
independent library unit:

```sh
lu build --lib --shared -o vector lib/lu-numerics/vector.lu
```

| Source | Exported kernels |
|---|---|
| `vector.lu` | `dot`, `norm2`, `axpy`, `scale` |
| `statistics.lu` | `mean`, `variance`, `mse` |
| `integrate.lu` | `trapz_uniform`, `trapz_xy` |
| `linalg.lu` | `gemv_inplace`, `matmul_inplace` (row-major boundary arrays) |

Array parameters are boundary buffers. Read-only kernels leave them unchanged;
transform kernels copy results back through the generated ABI shim. Dimensions
are explicit so callers can use slices of larger buffers. The dense kernels
overwrite their first array; `matmul_inplace` takes `[rows, inner, columns]`
as its shape argument to remain inside the portable six-integer-register ABI.
