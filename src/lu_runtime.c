// Native runtime linked into lu-built binaries.
#if !defined(__wasm__)
#include <pthread.h>
#include <dlfcn.h>
#endif
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* Shortest round-trip decimal, plain notation — matches Rust's f64 Display
   (the JIT/interp tiers print through it), so all tiers agree byte-for-byte:
   fewest digits that parse back exactly, never scientific notation. */
void lu_print_f64(double v) {
  if (v != v) {
    printf("NaN");
    return;
  }
  if (v > 1.7976931348623157e308) {
    printf("inf");
    return;
  }
  if (v < -1.7976931348623157e308) {
    printf("-inf");
    return;
  }
  char buf[64];
  int p;
  for (p = 0; p < 17; p++) {
    snprintf(buf, sizeof buf, "%.*e", p, v);
    if (strtod(buf, 0) == v) break;
  }
  const char *s = buf;
  if (*s == '-') {
    putchar('-');
    s++;
  }
  char digits[32];
  long long nd = 0;
  digits[nd++] = *s++;
  if (*s == '.') {
    s++;
    while (*s && *s != 'e') digits[nd++] = *s++;
  }
  while (*s && *s != 'e') s++;
  long long e10 = strtoll(s + 1, 0, 10);
  while (nd > 1 && digits[nd - 1] == '0') nd--;
  if (e10 >= nd - 1) { /* integer, possibly with trailing zeros */
    fwrite(digits, 1, (size_t)nd, stdout);
    for (long long i = 0; i < e10 - (nd - 1); i++) putchar('0');
  } else if (e10 >= 0) { /* decimal point inside the digits */
    fwrite(digits, 1, (size_t)e10 + 1, stdout);
    putchar('.');
    fwrite(digits + e10 + 1, 1, (size_t)(nd - e10 - 1), stdout);
  } else { /* leading 0.000... */
    printf("0.");
    for (long long i = 0; i < -e10 - 1; i++) putchar('0');
    fwrite(digits, 1, (size_t)nd, stdout);
  }
}
void lu_print_i64(long long v) { printf("%lld", v); }
void lu_print_bool(long long v) { printf(v ? "true" : "false"); }
void lu_print_str(const char *p, long long n) { fwrite(p, 1, (size_t)n, stdout); }
void lu_print_sep(void) { putchar(' '); }
void lu_print_nl(void) { putchar('\n'); }

static char *arr_alloc(long long n, long long stride) {
  if (n < 0 || stride <= 0 ||
      (unsigned long long)n > (SIZE_MAX - 8) / 8 / (unsigned long long)stride) {
    fprintf(stderr, "error: invalid array length %lld with stride %lld\n", n, stride);
    exit(1);
  }
  long long slots = n * stride;
  char *p = malloc(8 + (size_t)slots * 8);
  if (!p) {
    fprintf(stderr, "error: out of memory allocating array of %lld elements\n", n);
    exit(1);
  }
  *(long long *)p = slots;
  return p;
}

char *lu_arr_new_f64(long long n, double init) {
  char *p = arr_alloc(n, 1);
  double *d = (double *)(p + 8);
  for (long long i = 0; i < n; i++) d[i] = init;
  return p;
}

char *lu_arr_new_i64(long long n, long long init) {
  char *p = arr_alloc(n, 1);
  long long *d = (long long *)(p + 8);
  for (long long i = 0; i < n; i++) d[i] = init;
  return p;
}

/* Uninitialized array of n 8-byte slots; the compiler emits the fill loop
   (record arrays are laid out SoA — a compiler decision, not a runtime one). */
char *lu_arr_new_raw(long long n, long long stride) { return arr_alloc(n, stride); }

char *lu_arr_clone(const char *source) {
  if (!source) return 0;
  long long slots = *(const long long *)source;
  if (slots < 0 || (unsigned long long)slots > (SIZE_MAX - 8) / 8) {
    fprintf(stderr, "error: array allocation size overflow\n");
    exit(1);
  }
  size_t bytes = 8 + (size_t)slots * 8;
  char *copy = malloc(bytes);
  if (!copy) {
    fprintf(stderr, "error: out of memory cloning array\n");
    exit(1);
  }
  memcpy(copy, source, bytes);
  return copy;
}

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

static long long checked_int_div(long long lhs, long long rhs, int remainder) {
  if (rhs == 0) {
    fprintf(stderr, "error: integer division by zero\n");
    exit(1);
  }
  if (lhs == INT64_MIN && rhs == -1) {
    fprintf(stderr, "error: integer division overflow: %lld / %lld\n", lhs, rhs);
    exit(1);
  }
  return remainder ? lhs % rhs : lhs / rhs;
}

long long lu_i64_div(long long lhs, long long rhs) {
  return checked_int_div(lhs, rhs, 0);
}

long long lu_i64_rem(long long lhs, long long rhs) {
  return checked_int_div(lhs, rhs, 1);
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

void lu_write_file(const char *p, long long n, const char *data, long long dn) {
  char path[4096];
  if (n >= (long long)sizeof(path)) n = sizeof(path) - 1;
  memcpy(path, p, (size_t)n);
  path[n] = 0;
  FILE *f = fopen(path, "wb");
  if (!f || fwrite(data, 1, (size_t)dn, f) != (size_t)dn) {
    fprintf(stderr, "error: cannot write %s\n", path);
    exit(1);
  }
  fclose(f);
}

long long lu_last_len(void) { return g_last_len; }

const char *lu_chr(long long c) {
  char *p = malloc(1);
  p[0] = (char)c;
  g_last_len = 1;
  return p;
}

const char *lu_concat(const char *ap, long long al, const char *bp, long long bl) {
  char *p = malloc((size_t)(al + bl) + 1);
  memcpy(p, ap, (size_t)al);
  memcpy(p + al, bp, (size_t)bl);
  g_last_len = al + bl;
  return p;
}

/* Packed dynamic-call bridge used by the self-hosted interpreter.  The
   language-level bridge stays inside the ordinary 6-GPR/8-FPR boundary cap:
   one packed i64 control array and one packed f64 data array. */
static void *lu_ffi_prepared;

#if !defined(__wasm__)
long long lu_ffi_prepare(const char *lib, long long ll,
                         const char *symbol, long long sl) {
  char *library = malloc((size_t)ll + 1);
  char *name = malloc((size_t)sl + 1);
  memcpy(library, lib, (size_t)ll);
  memcpy(name, symbol, (size_t)sl);
  library[ll] = 0;
  name[sl] = 0;
  void *handle = RTLD_DEFAULT;
  if (ll != 0) {
    char candidate[1024];
    if (strchr(library, '/') || strstr(library, ".so") ||
        strstr(library, ".dylib")) {
      snprintf(candidate, sizeof candidate, "%s", library);
    } else {
#ifdef __APPLE__
      snprintf(candidate, sizeof candidate, "lib%s.dylib", library);
#else
      snprintf(candidate, sizeof candidate, "lib%s.so", library);
#endif
    }
    handle = dlopen(candidate, RTLD_LAZY);
#ifndef __APPLE__
    if (!handle && !strchr(library, '/') && !strstr(library, ".so"))
      {
        snprintf(candidate, sizeof candidate, "lib%s.so.6", library);
        handle = dlopen(candidate, RTLD_LAZY);
      }
#endif
    if (!handle) {
      fprintf(stderr, "runtime error: cannot load FFI library `%s`: %s\n",
              library, dlerror());
      free(library);
      free(name);
      return 0;
    }
  }
  dlerror();
  lu_ffi_prepared = dlsym(handle, name);
  const char *error = dlerror();
  if (error) {
    fprintf(stderr, "runtime error: cannot resolve FFI symbol `%s`: %s\n",
            name, error);
    lu_ffi_prepared = 0;
  }
  free(library);
  free(name);
  return lu_ffi_prepared != 0;
}

typedef long long (*lu_ffi_i_fn)(
    long long, long long, long long, long long, long long, long long,
    double, double, double, double, double, double, double, double);
typedef double (*lu_ffi_f_fn)(
    long long, long long, long long, long long, long long, long long,
    double, double, double, double, double, double, double, double);

static int lu_ffi_unpack(long long *control, long long control_len,
                         double *floats, long long float_len,
                         long long ints[6], double fp[8],
                         unsigned char *strings[6], int *string_count) {
  if (!control || control_len < 1 || float_len < 0) return 0;
  long long nargs = control[0], ni = 0, nf = 0;
  if (nargs < 0 || 1 + nargs * 3 > control_len) return 0;
  for (long long argument = 0; argument < nargs; argument++) {
    long long descriptor = 1 + argument * 3;
    long long kind = control[descriptor];
    long long value = control[descriptor + 1];
    long long length = control[descriptor + 2];
    if (kind == 0) {
      if (ni >= 6) return 0;
      ints[ni++] = value;
    } else if (kind == 1) {
      if (nf >= 8 || value < 0 || value >= float_len) return 0;
      fp[nf++] = floats[value];
    } else if (kind == 2) {
      if (ni + 2 > 6 || value < 0 || length < 0 ||
          value + length > control_len || *string_count >= 6) return 0;
      unsigned char *bytes = malloc((size_t)length);
      for (long long i = 0; i < length; i++)
        bytes[i] = (unsigned char)control[value + i];
      strings[(*string_count)++] = bytes;
      ints[ni++] = (long long)(intptr_t)bytes;
      ints[ni++] = length;
    } else if (kind == 3) {
      if (ni + 2 > 6 || value < 0 || length < 0 ||
          value + length > control_len) return 0;
      ints[ni++] = (long long)(intptr_t)(control + value);
      ints[ni++] = length;
    } else if (kind == 4) {
      if (ni + 2 > 6 || value < 0 || length < 0 ||
          value + length > float_len) return 0;
      ints[ni++] = (long long)(intptr_t)(floats + value);
      ints[ni++] = length;
    } else {
      return 0;
    }
  }
  return 1;
}

long long lu_ffi_call_i(long long *control, long long control_len,
                        double *floats, long long float_len) {
  long long ints[6] = {0};
  double fp[8] = {0};
  unsigned char *strings[6] = {0};
  int string_count = 0;
  if (!lu_ffi_prepared ||
      !lu_ffi_unpack(control, control_len, floats, float_len, ints, fp,
                     strings, &string_count)) {
    fprintf(stderr, "runtime error: invalid packed FFI call\n");
    return 0;
  }
  long long result = ((lu_ffi_i_fn)lu_ffi_prepared)(
      ints[0], ints[1], ints[2], ints[3], ints[4], ints[5],
      fp[0], fp[1], fp[2], fp[3], fp[4], fp[5], fp[6], fp[7]);
  for (int i = 0; i < string_count; i++) free(strings[i]);
  return result;
}

double lu_ffi_call_f(long long *control, long long control_len,
                     double *floats, long long float_len) {
  long long ints[6] = {0};
  double fp[8] = {0};
  unsigned char *strings[6] = {0};
  int string_count = 0;
  if (!lu_ffi_prepared ||
      !lu_ffi_unpack(control, control_len, floats, float_len, ints, fp,
                     strings, &string_count)) {
    fprintf(stderr, "runtime error: invalid packed FFI call\n");
    return 0.0;
  }
  double result = ((lu_ffi_f_fn)lu_ffi_prepared)(
      ints[0], ints[1], ints[2], ints[3], ints[4], ints[5],
      fp[0], fp[1], fp[2], fp[3], fp[4], fp[5], fp[6], fp[7]);
  for (int i = 0; i < string_count; i++) free(strings[i]);
  return result;
}
#else
long long lu_ffi_prepare(const char *lib, long long ll,
                         const char *symbol, long long sl) {
  (void)lib; (void)ll; (void)symbol; (void)sl;
  fprintf(stderr, "runtime error: dynamic FFI is unavailable on wasm32\n");
  return 0;
}

long long lu_ffi_call_i(long long *control, long long control_len,
                        double *floats, long long float_len) {
  (void)control; (void)control_len; (void)floats; (void)float_len;
  return 0;
}

double lu_ffi_call_f(long long *control, long long control_len,
                     double *floats, long long float_len) {
  (void)control; (void)control_len; (void)floats; (void)float_len;
  return 0.0;
}
#endif

/* Compiled programs enter through lu_entry; main runs it on a 512 MiB stack
   so deep recursion (e.g. self-hosted interpreter towers) doesn't overflow. */
#ifndef LU_LIB
extern int lu_entry(void);

#if defined(LU_WEB)
int lu_web_run(void) {
  int result = lu_entry();
  fflush(stdout);
  return result;
}
#elif defined(__wasm__)
int main(int argc, char **argv) {
  lu_set_args(argc, argv);
  int result = lu_entry();
  fflush(stdout);
  return result;
}
#else
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
#endif
#endif
