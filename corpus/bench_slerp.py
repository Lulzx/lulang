"""NumPy twin of bench_slerp.lu — vectorized float64 quaternion batches."""

import numpy as np

a = np.array([1.0, 2.0, 3.0, 4.0], dtype=np.float64)
b = np.array([4.0, 3.0, 2.0, 1.0], dtype=np.float64)
a /= np.linalg.norm(a)
b /= np.linalg.norm(b)
n = 2_000_000
t = np.arange(n, dtype=np.float64) / n
d = float(a @ b)
if d < 0.0:
    d = -d
    b = -b
wa = 1.0 - t
wb = t
if d < 0.9995:
    theta = np.arccos(d)
    sine = np.sin(theta)
    wa = np.sin((1.0 - t) * theta) / sine
    wb = np.sin(t * theta) / sine
q = wa[:, None] * a + wb[:, None] * b
q /= np.linalg.norm(q, axis=1)[:, None]
acc = float(np.linalg.norm(q, axis=1).sum())
print(f"acc: {acc:.17g}")
