// Native runtime linked into lu-built binaries.
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

void lu_print_f64(double v) { printf("%.17g", v); }
void lu_print_i64(long long v) { printf("%lld", v); }
void lu_print_bool(long long v) { printf(v ? "true" : "false"); }
void lu_print_str(const char *p, long long n) { fwrite(p, 1, (size_t)n, stdout); }
void lu_print_sep(void) { putchar(' '); }
void lu_print_nl(void) { putchar('\n'); }

static char *arr_alloc(long long n) {
  char *p = malloc(8 + (size_t)n * 8);
  if (!p) {
    fprintf(stderr, "error: out of memory allocating array of %lld elements\n", n);
    exit(1);
  }
  *(long long *)p = n;
  return p;
}

char *lu_arr_new_f64(long long n, double init) {
  char *p = arr_alloc(n);
  double *d = (double *)(p + 8);
  for (long long i = 0; i < n; i++) d[i] = init;
  return p;
}

char *lu_arr_new_i64(long long n, long long init) {
  char *p = arr_alloc(n);
  long long *d = (long long *)(p + 8);
  for (long long i = 0; i < n; i++) d[i] = init;
  return p;
}

/* Uninitialized array of n 8-byte slots; the compiler emits the fill loop
   (record arrays are laid out SoA — a compiler decision, not a runtime one). */
char *lu_arr_new_raw(long long n) { return arr_alloc(n); }

long long lu_str_eq(const char *ap, long long al, const char *bp, long long bl) {
  if (al != bl) return 0;
  for (long long i = 0; i < al; i++)
    if (ap[i] != bp[i]) return 0;
  return 1;
}

void lu_oob(long long idx, long long len) {
  fprintf(stderr, "error: index %lld out of bounds (length %lld)\n", idx, len);
  exit(1);
}
