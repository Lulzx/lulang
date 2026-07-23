// C++ reference formulas for every public lu-numerics kernel.
#include <algorithm>
#include <cmath>
#include <cstdint>
#include <vector>

double dot(const std::vector<double>& x, const std::vector<double>& y, long n) {
  double result = 0;
  for (long i = 0; i < n; ++i) result += x[i] * y[i];
  return result;
}
double norm2(const std::vector<double>& x, long n) { return std::sqrt(dot(x, x, n)); }
void axpy(double a, const std::vector<double>& x, std::vector<double>& y, long n) {
  for (long i = 0; i < n; ++i) y[i] = a * x[i] + y[i];
}
void scale(double a, std::vector<double>& x, long n) {
  for (long i = 0; i < n; ++i) x[i] *= a;
}
double mean(const std::vector<double>& x, long n) {
  double result = 0;
  for (long i = 0; i < n; ++i) result += x[i];
  return result / n;
}
double variance(const std::vector<double>& x, long n) {
  double center = mean(x, n), result = 0;
  for (long i = 0; i < n; ++i) result += (x[i] - center) * (x[i] - center);
  return result / n;
}
double mse(const std::vector<double>& x, const std::vector<double>& y, long n) {
  double result = 0;
  for (long i = 0; i < n; ++i) result += (x[i] - y[i]) * (x[i] - y[i]);
  return result / n;
}
double trapz_uniform(const std::vector<double>& y, double dx, long n) {
  if (n < 2) return 0;
  double result = 0.5 * (y[0] + y[n - 1]);
  for (long i = 1; i + 1 < n; ++i) result += y[i];
  return dx * result;
}
double trapz_xy(const std::vector<double>& x, const std::vector<double>& y, long n) {
  double result = 0;
  for (long i = 0; i + 1 < n; ++i)
    result += 0.5 * (x[i + 1] - x[i]) * (y[i] + y[i + 1]);
  return result;
}
double polynomial_eval(const std::vector<double>& c, double x, long n) {
  double result = 0;
  while (n > 0) result = result * x + c[--n];
  return result;
}
void gemv_inplace(const std::vector<double>& matrix, std::vector<double>& x, long rows, long columns) {
  auto input = x;
  for (long row = 0; row < rows; ++row) {
    x[row] = 0;
    for (long column = 0; column < columns; ++column)
      x[row] += matrix[row * columns + column] * input[column];
  }
}
void matmul_square_inplace(std::vector<double>& a, const std::vector<double>& b, long n) {
  auto input = a;
  for (long row = 0; row < n; ++row)
    for (long column = 0; column < n; ++column) {
      a[row * n + column] = 0;
      for (long k = 0; k < n; ++k)
        a[row * n + column] += input[row * n + k] * b[k * n + column];
    }
}
void convolution_inplace(std::vector<double>& signal, const std::vector<double>& kernel, long output_n, long kernel_n) {
  auto input = signal;
  for (long i = 0; i < output_n; ++i) {
    signal[i] = 0;
    for (long tap = 0; tap < kernel_n; ++tap)
      if (i >= tap) signal[i] += input[i - tap] * kernel[tap];
  }
}
void moving_average_inplace(std::vector<double>& signal, long window, long n) {
  auto input = signal;
  for (long i = 0; i < n; ++i) {
    long begin = std::max(0L, i - window + 1);
    signal[i] = 0;
    for (long j = begin; j <= i; ++j) signal[i] += input[j];
    signal[i] /= i - begin + 1;
  }
}
std::int64_t lcg_step(std::int64_t seed) {
  std::int64_t state = seed % 2147483647;
  if (state < 0) state = -state;
  return (state * 48271 + 1) % 2147483647;
}
double monte_carlo_pi(long samples, std::int64_t seed) {
  if (samples <= 0) return 0;
  long inside = 0;
  for (long i = 0; i < samples; ++i) {
    seed = lcg_step(seed); double x = double(seed % 1000000) / 1000000;
    seed = lcg_step(seed); double y = double(seed % 1000000) / 1000000;
    inside += x * x + y * y <= 1;
  }
  return 4.0 * inside / samples;
}
double bisect_sqrt(double value, long iterations) {
  if (value <= 0) return 0;
  double low = 0, high = std::max(1.0, value);
  while (iterations-- > 0) {
    double middle = 0.5 * (low + high);
    (middle * middle > value ? high : low) = middle;
  }
  return 0.5 * (low + high);
}
double sinc(double x) { return x == 0 ? 1 : std::sin(x) / x; }
double normal_pdf(double x) { return 0.3989422804014327 * std::pow(2.718281828459045, -0.5 * x * x); }
double sigmoid(double x) { return 1 / (1 + std::pow(2.718281828459045, -x)); }
double clamp(double x, double low, double high) { return std::min(high, std::max(low, x)); }
double lerp(double a, double b, double t) { return a + (b - a) * t; }
double distance3(double ax, double ay, double az, double bx, double by, double bz) {
  return std::sqrt((bx-ax)*(bx-ax) + (by-ay)*(by-ay) + (bz-az)*(bz-az));
}
double determinant2(double a, double b, double c, double d) { return a*d-b*c; }
std::int64_t factorial(std::int64_t n) {
  std::int64_t result = 1;
  for (std::int64_t i = 2; i <= n; ++i) result *= i;
  return result;
}
std::int64_t binomial(std::int64_t n, std::int64_t k) {
  if (k < 0 || k > n) return 0;
  k = std::min(k, n-k);
  std::int64_t result = 1;
  for (std::int64_t i = 1; i <= k; ++i) result = result * (n-k+i) / i;
  return result;
}
