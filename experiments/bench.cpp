// lulang experiment: does "AE beats C++" fall out of semantic defaults?
// Kernels: dot (reduction/reassociation), nbody (fma/rsqrt), slerp AoS vs SoA (layout).
// Compile the SAME file with different flags; the runner compares binaries.
#include <cmath>
#include <cstdio>
#include <cstdint>
#include <chrono>
#include <vector>

static inline double now_ms() {
  using namespace std::chrono;
  return duration<double, std::milli>(steady_clock::now().time_since_epoch()).count();
}

// xorshift so every variant gets identical inputs, no <random> needed
static uint64_t rng_state = 88172645463325252ull;
static inline double frand() {
  rng_state ^= rng_state << 13; rng_state ^= rng_state >> 7; rng_state ^= rng_state << 17;
  return (double)(rng_state >> 11) * (1.0 / 9007199254740992.0);
}

// ---------- dot: 2M-element reduction. Reassociation-gated vectorization. ----------
static double kernel_dot(const double* __restrict a, const double* __restrict b, int n) {
  double s = 0.0;
  for (int i = 0; i < n; i++) s += a[i] * b[i];
  return s;
}

// ---------- nbody: one acceleration step, n=1500 ----------
struct Bodies { std::vector<double> x, y, z, ax, ay, az; };
static double kernel_nbody(Bodies& bd, int n) {
  double* __restrict x = bd.x.data(); double* __restrict y = bd.y.data();
  double* __restrict z = bd.z.data();
  double* __restrict ax = bd.ax.data(); double* __restrict ay = bd.ay.data();
  double* __restrict az = bd.az.data();
  for (int i = 0; i < n; i++) { ax[i] = ay[i] = az[i] = 0.0; }
  for (int i = 0; i < n; i++) {
    double axi = 0, ayi = 0, azi = 0;
    double xi = x[i], yi = y[i], zi = z[i];
    for (int j = 0; j < n; j++) {
      double dx = x[j] - xi, dy = y[j] - yi, dz = z[j] - zi;
      double r2 = dx * dx + dy * dy + dz * dz + 1e-9;
      double inv = 1.0 / std::sqrt(r2);
      double inv3 = inv * inv * inv;
      axi += dx * inv3; ayi += dy * inv3; azi += dz * inv3;
    }
    ax[i] = axi; ay[i] = ayi; az[i] = azi;
  }
  double s = 0;
  for (int i = 0; i < n; i++) s += ax[i] + ay[i] + az[i];
  return s;
}

// ---------- slerp: quaternion interpolation over 200k pairs ----------
struct Quat { double w, x, y, z; };

static inline Quat slerp1(Quat a, Quat b, double t) {
  double d = a.w * b.w + a.x * b.x + a.y * b.y + a.z * b.z;
  if (d < 0) { d = -d; b.w = -b.w; b.x = -b.x; b.y = -b.y; b.z = -b.z; }
  double wa, wb;
  if (d > 0.9995) { wa = 1.0 - t; wb = t; }
  else {
    double th = std::acos(d), s = std::sin(th);
    wa = std::sin((1.0 - t) * th) / s; wb = std::sin(t * th) / s;
  }
  Quat r{wa * a.w + wb * b.w, wa * a.x + wb * b.x, wa * a.y + wb * b.y, wa * a.z + wb * b.z};
  double n = 1.0 / std::sqrt(r.w * r.w + r.x * r.x + r.y * r.y + r.z * r.z);
  return {r.w * n, r.x * n, r.y * n, r.z * n};
}

// idiomatic layout: array of structs
static double kernel_slerp_aos(const std::vector<Quat>& A, const std::vector<Quat>& B, int n) {
  double s = 0;
  for (int i = 0; i < n; i++) {
    Quat q = slerp1(A[i], B[i], 0.37);
    s += q.w + q.x + q.y + q.z;
  }
  return s;
}

// compiler-chosen layout: struct of arrays (what a layout-free language could emit)
struct QuatsSoA { std::vector<double> w, x, y, z; };
static double kernel_slerp_soa(const QuatsSoA& A, const QuatsSoA& B, int n) {
  const double* __restrict aw = A.w.data(); const double* __restrict ax = A.x.data();
  const double* __restrict ay = A.y.data(); const double* __restrict az = A.z.data();
  const double* __restrict bw = B.w.data(); const double* __restrict bx = B.x.data();
  const double* __restrict by = B.y.data(); const double* __restrict bz = B.z.data();
  const double t = 0.37;
  double s = 0;
  for (int i = 0; i < n; i++) {
    double d = aw[i] * bw[i] + ax[i] * bx[i] + ay[i] * by[i] + az[i] * bz[i];
    double sign = d < 0 ? -1.0 : 1.0; d = d * sign;
    double wa, wb;
    if (d > 0.9995) { wa = 1.0 - t; wb = t; }
    else {
      double th = std::acos(d), sn = std::sin(th);
      wa = std::sin((1.0 - t) * th) / sn; wb = std::sin(t * th) / sn;
    }
    wb *= sign;
    double rw = wa * aw[i] + wb * bw[i], rx = wa * ax[i] + wb * bx[i];
    double ry = wa * ay[i] + wb * by[i], rz = wa * az[i] + wb * bz[i];
    double inv = 1.0 / std::sqrt(rw * rw + rx * rx + ry * ry + rz * rz);
    s += (rw + rx + ry + rz) * inv;
  }
  return s;
}

template <typename F>
static double best_ms(F&& f, int reps) {
  double best = 1e30;
  for (int r = 0; r < reps; r++) {
    double t0 = now_ms();
    volatile double sink = f();
    (void)sink;
    double dt = now_ms() - t0;
    if (dt < best) best = dt;
  }
  return best;
}

int main() {
  const int ND = 2'000'000, NB = 1500, NQ = 200'000;

  std::vector<double> da(ND), db(ND);
  for (int i = 0; i < ND; i++) { da[i] = frand() - 0.5; db[i] = frand() - 0.5; }

  Bodies bd; bd.x.resize(NB); bd.y.resize(NB); bd.z.resize(NB);
  bd.ax.resize(NB); bd.ay.resize(NB); bd.az.resize(NB);
  for (int i = 0; i < NB; i++) { bd.x[i] = frand(); bd.y[i] = frand(); bd.z[i] = frand(); }

  std::vector<Quat> qa(NQ), qb(NQ);
  QuatsSoA sa, sb;
  sa.w.resize(NQ); sa.x.resize(NQ); sa.y.resize(NQ); sa.z.resize(NQ);
  sb.w.resize(NQ); sb.x.resize(NQ); sb.y.resize(NQ); sb.z.resize(NQ);
  for (int i = 0; i < NQ; i++) {
    Quat q1{frand() - .5, frand() - .5, frand() - .5, frand() - .5};
    Quat q2{frand() - .5, frand() - .5, frand() - .5, frand() - .5};
    double n1 = 1 / std::sqrt(q1.w * q1.w + q1.x * q1.x + q1.y * q1.y + q1.z * q1.z);
    double n2 = 1 / std::sqrt(q2.w * q2.w + q2.x * q2.x + q2.y * q2.y + q2.z * q2.z);
    qa[i] = {q1.w * n1, q1.x * n1, q1.y * n1, q1.z * n1};
    qb[i] = {q2.w * n2, q2.x * n2, q2.y * n2, q2.z * n2};
    sa.w[i] = qa[i].w; sa.x[i] = qa[i].x; sa.y[i] = qa[i].y; sa.z[i] = qa[i].z;
    sb.w[i] = qb[i].w; sb.x[i] = qb[i].x; sb.y[i] = qb[i].y; sb.z[i] = qb[i].z;
  }

  const int R = 30;
  printf("dot,%.3f\n",       best_ms([&] { return kernel_dot(da.data(), db.data(), ND); }, R));
  printf("nbody,%.3f\n",     best_ms([&] { return kernel_nbody(bd, NB); }, R));
  printf("slerp_aos,%.3f\n", best_ms([&] { return kernel_slerp_aos(qa, qb, NQ); }, R));
  printf("slerp_soa,%.3f\n", best_ms([&] { return kernel_slerp_soa(sa, sb, NQ); }, R));
  return 0;
}
