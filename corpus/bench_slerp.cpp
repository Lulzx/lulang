// C++ twin of bench_slerp.lu — idiomatic value-type quaternions.
#include <cmath>
#include <cstdio>

struct Quat { double w, x, y, z; };

static double dotq(Quat a, Quat b) { return a.w * b.w + a.x * b.x + a.y * b.y + a.z * b.z; }
static double norm(Quat q) { return std::sqrt(dotq(q, q)); }
static Quat scale(Quat q, double s) { return {q.w * s, q.x * s, q.y * s, q.z * s}; }
static Quat addq(Quat a, Quat b) { return {a.w + b.w, a.x + b.x, a.y + b.y, a.z + b.z}; }
static Quat normalize(Quat q) { return scale(q, 1.0 / norm(q)); }

static Quat slerp(Quat a, Quat b, double t) {
  double d = dotq(a, b);
  Quat bb = b;
  if (d < 0) { d = -d; bb = scale(b, -1.0); }
  double wa = 1.0 - t, wb = t;
  if (d < 0.9995) {
    double th = std::acos(d), s = std::sin(th);
    wa = std::sin((1.0 - t) * th) / s;
    wb = std::sin(t * th) / s;
  }
  return normalize(addq(scale(a, wa), scale(bb, wb)));
}

int main() {
  Quat a = normalize({1, 2, 3, 4});
  Quat b = normalize({4, 3, 2, 1});
  const long n = 2000000;
  double acc = 0.0;
  for (long i = 0; i < n; i++) {
    double t = (double)i / (double)n;
    acc += norm(slerp(a, b, t));
  }
  printf("acc: %.17g\n", acc);
}
