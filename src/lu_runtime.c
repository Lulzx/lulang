// Native runtime linked into lu-built binaries.
#include <pthread.h>
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

/* program arguments (argv after the binary name) and the str-returning
   builtin protocol: str-producing calls return the pointer and stash the
   length for an immediately following lu_last_len() call. */
static int g_argc = 0;
static char **g_argv = 0;
static long long g_last_len = 0;

void lu_set_args(int argc, char **argv) {
  g_argc = argc;
  g_argv = argv;
}

long long lu_nargs(void) { return g_argc > 1 ? g_argc - 1 : 0; }

const char *lu_arg(long long i) {
  if (i < 0 || i + 1 >= g_argc) {
    g_last_len = 0;
    return "";
  }
  g_last_len = (long long)strlen(g_argv[i + 1]);
  return g_argv[i + 1];
}

const char *lu_read_file(const char *p, long long n) {
  char path[4096];
  if (n >= (long long)sizeof(path)) n = sizeof(path) - 1;
  memcpy(path, p, (size_t)n);
  path[n] = 0;
  FILE *f = fopen(path, "rb");
  if (!f) {
    fprintf(stderr, "error: cannot read %s\n", path);
    exit(1);
  }
  fseek(f, 0, SEEK_END);
  long long sz = ftell(f);
  fseek(f, 0, SEEK_SET);
  char *buf = malloc((size_t)sz + 1);
  if (!buf || fread(buf, 1, (size_t)sz, f) != (size_t)sz) {
    fprintf(stderr, "error: cannot read %s\n", path);
    exit(1);
  }
  fclose(f);
  g_last_len = sz;
  return buf;
}

long long lu_last_len(void) { return g_last_len; }

/* Compiled programs enter through lu_entry; main runs it on a 512 MiB stack
   so deep recursion (e.g. self-hosted interpreter towers) doesn't overflow. */
extern int lu_entry(void);

static void *entry_thunk(void *unused) {
  (void)unused;
  return (void *)(long)lu_entry();
}

int main(int argc, char **argv) {
  lu_set_args(argc, argv);
  pthread_attr_t attr;
  pthread_attr_init(&attr);
  pthread_attr_setstacksize(&attr, 512ull << 20);
  pthread_t t;
  if (pthread_create(&t, &attr, entry_thunk, 0) != 0) {
    fprintf(stderr, "error: cannot start program thread\n");
    return 1;
  }
  void *ret = 0;
  pthread_join(t, &ret);
  fflush(stdout);
  return (int)(long)ret;
}
