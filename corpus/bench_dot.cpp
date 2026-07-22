// C++ twin of bench_dot.lu — idiomatic style, heap vectors.
#include <cstdio>
#include <vector>

double dot(const std::vector<double>& a, const std::vector<double>& b, long n) {
  double s = 0.0;
  for (long i = 0; i < n; i++) s += a[i] * b[i];
  return s;
}

int main() {
  const long n = 2000000;
  std::vector<double> a(n), b(n);
  for (long i = 0; i < n; i++) {
    a[i] = i * 0.000001;
    b[i] = i * 0.000002;
  }
  double acc = 0.0;
  for (int r = 0; r < 200; r++) acc += dot(a, b, n);
  printf("acc: %.17g\n", acc);
}
