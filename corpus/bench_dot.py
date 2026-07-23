"""NumPy twin of bench_dot.lu — contiguous float64 vectors and numpy.dot."""

import numpy as np

n = 2_000_000
a = np.arange(n, dtype=np.float64) * 0.000001
b = np.arange(n, dtype=np.float64) * 0.000002
acc = 0.0
for _ in range(20):
    acc += float(np.dot(a, b))
print(f"acc: {acc:.17g}")
