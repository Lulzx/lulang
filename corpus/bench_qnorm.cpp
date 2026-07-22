// C++ twin of bench_qnorm.lu — idiomatic style: AoS struct array, heap vector.
#include <cstdio>
#include <vector>

struct Quat {
  double w, x, y, z;
};

double qq(const std::vector<Quat>& qs, long n) {
  double s = 0.0;
  for (long i = 0; i < n; i++)
    s += qs[i].w * qs[i].w + qs[i].x * qs[i].x + qs[i].y * qs[i].y + qs[i].z * qs[i].z;
  return s;
}

int main() {
  const long n = 2000000;
  std::vector<Quat> qs(n);
  for (long i = 0; i < n; i++) {
    double f = i * 0.000001;
    qs[i] = Quat{1.0 + f, 2.0 - f, f * 3.0, 0.5};
  }
  double acc = 0.0;
  for (int r = 0; r < 20; r++) acc += qq(qs, n);
  printf("acc: %.17g\n", acc);
}
