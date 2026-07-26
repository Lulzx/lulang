/* spectral-norm — same algorithm as spectralnorm.lu, single-threaded. */
#include <stdio.h>
#include <stdlib.h>
#include <math.h>

static double eval_a(long i, long j) {
  return 1.0 / (double)((i + j) * (i + j + 1) / 2 + i + 1);
}

static void mul_av(long n, const double *v, double *out) {
  for (long i = 0; i < n; i++) {
    double s = 0.0;
    for (long j = 0; j < n; j++) s += eval_a(i, j) * v[j];
    out[i] = s;
  }
}

static void mul_atv(long n, const double *v, double *out) {
  for (long i = 0; i < n; i++) {
    double s = 0.0;
    for (long j = 0; j < n; j++) s += eval_a(j, i) * v[j];
    out[i] = s;
  }
}

static void mul_atav(long n, const double *v, double *out, double *tmp) {
  mul_av(n, v, tmp);
  mul_atv(n, tmp, out);
}

int main(int argc, char **argv) {
  long n = argc > 1 ? atol(argv[1]) : 100;
  double *u = malloc(n * sizeof(double));
  double *v = malloc(n * sizeof(double));
  double *tmp = malloc(n * sizeof(double));
  for (long i = 0; i < n; i++) u[i] = 1.0;

  for (int r = 0; r < 10; r++) {
    mul_atav(n, u, v, tmp);
    mul_atav(n, v, u, tmp);
  }

  double vbv = 0.0, vv = 0.0;
  for (long i = 0; i < n; i++) { vbv += u[i] * v[i]; vv += v[i] * v[i]; }
  printf("%.9f\n", sqrt(vbv / vv));
  return 0;
}
