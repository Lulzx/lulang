// C++ twin of alcubierre.lu — idiomatic, plain -O3 semantics.
#include <cmath>
#include <cstdio>

static double rho(double x, double y, double z, double xs) {
  double rx = x - xs;
  double r2 = rx * rx + y * y + z * z;
  double rs = std::sqrt(r2);
  double u = r2 / 4.0;
  double w = 1.0 + u * u * u;
  double dfdr = -6.0 * u * u * rs / 4.0 / (w * w);
  return 0.25 * dfdr * dfdr * (y * y + z * z) / r2;
}

int main() {
  const int n = 96;
  const double dx = 16.0 / n;
  double total = 0.0;
  for (int it = 0; it < 6; it++) {
    double xs = -4.0 + it * 0.5;
    for (int ix = 0; ix < n; ix++) {
      double x = -8.0 + (ix + 0.5) * dx;
      for (int iy = 0; iy < n; iy++) {
        double y = -8.0 + (iy + 0.5) * dx;
        double acc = 0.0;
        for (int iz = 0; iz < n; iz++) {
          double z = -8.0 + (iz + 0.5) * dx;
          acc += rho(x, y, z, xs);
        }
        total += acc;
      }
    }
  }
  printf("total: %.17g\n", total * dx * dx * dx);
}
