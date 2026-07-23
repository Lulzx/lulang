"""NumPy reference formulas for every public lu-numerics kernel."""
import math
import numpy as np

def dot(x, y, n): return float(np.dot(x[:n], y[:n]))
def norm2(x, n): return float(np.linalg.norm(x[:n]))
def axpy(a, x, y, n): y[:n] = a * x[:n] + y[:n]
def scale(a, x, n): x[:n] *= a
def mean(x, n): return float(np.mean(x[:n]))
def variance(x, n): return float(np.var(x[:n]))
def mse(x, y, n): return float(np.mean((x[:n] - y[:n]) ** 2))
def trapz_uniform(y, dx, n): return float(np.trapezoid(y[:n], dx=dx))
def trapz_xy(x, y, n): return float(np.trapezoid(y[:n], x[:n]))
def polynomial_eval(c, x, n): return float(np.polynomial.polynomial.polyval(x, c[:n]))
def gemv_inplace(matrix, x, rows, columns): x[:rows] = np.asarray(matrix).reshape(rows, columns) @ x[:columns].copy()
def matmul_square_inplace(a, b, n): a[:] = (a.reshape(n, n).copy() @ b.reshape(n, n)).ravel()
def convolution_inplace(signal, kernel, output_n, kernel_n): signal[:output_n] = np.convolve(signal.copy(), kernel[:kernel_n])[:output_n]
def moving_average_inplace(signal, window, n):
    source = signal.copy()
    for i in range(n): signal[i] = np.mean(source[max(0, i-window+1):i+1])
def lcg_step(seed):
    state = abs(int(seed)) % 2147483647
    return (state * 48271 + 1) % 2147483647
def monte_carlo_pi(samples, seed):
    inside = 0
    for _ in range(samples):
        seed = lcg_step(seed); x = seed % 1000000 / 1000000
        seed = lcg_step(seed); y = seed % 1000000 / 1000000
        inside += x*x + y*y <= 1
    return 4 * inside / samples if samples > 0 else 0
def bisect_sqrt(value, iterations):
    if value <= 0: return 0
    low, high = 0, max(1, value)
    for _ in range(iterations):
        middle = (low + high) / 2
        if middle * middle > value: high = middle
        else: low = middle
    return (low + high) / 2
def sinc(x): return 1 if x == 0 else math.sin(x) / x
def normal_pdf(x): return 0.3989422804014327 * 2.718281828459045 ** (-0.5*x*x)
def sigmoid(x): return 1 / (1 + 2.718281828459045 ** -x)
def clamp(x, low, high): return min(high, max(low, x))
def lerp(a, b, t): return a + (b-a)*t
def distance3(ax, ay, az, bx, by, bz): return math.sqrt((bx-ax)**2 + (by-ay)**2 + (bz-az)**2)
def determinant2(a, b, c, d): return a*d-b*c
def factorial(n): return math.factorial(n)
def binomial(n, k): return math.comb(n, k) if 0 <= k <= n else 0
