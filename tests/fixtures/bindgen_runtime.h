#include <stdint.h>

int64_t bindgen_add(int64_t left, int64_t right);
double bindgen_scale(double value, double factor);
float bindgen_half(float value);
int32_t bindgen_increment_i32(int32_t value);
_Bool bindgen_is_positive(int32_t value);

typedef struct bindgen_pair {
  double x;
  double y;
} bindgen_pair;

double bindgen_pair_sum(bindgen_pair value);

typedef struct bindgen_mixed {
  int32_t count;
  float scale;
} bindgen_mixed;

double bindgen_mixed_value(bindgen_mixed value);
bindgen_mixed bindgen_make_mixed(int32_t count, float scale);

typedef struct bindgen_box bindgen_box;

bindgen_box *bindgen_box_new(int64_t value);
int64_t bindgen_box_read(const bindgen_box *box);
void bindgen_box_free(bindgen_box *box);

double bindgen_slice_sum(const double *values, int64_t values_len);
double bindgen_slice_bump(double *values, int64_t values_len);

typedef int64_t (*bindgen_callback)(int64_t);
int64_t bindgen_apply(bindgen_callback callback, int64_t value);
bindgen_callback bindgen_incrementer(void);

bindgen_pair bindgen_make_pair(int64_t x, int64_t y);

typedef struct bindgen_flags {
    unsigned int mode : 3;
    int enabled : 1;
} bindgen_flags;
int64_t bindgen_flags_score(bindgen_flags value);
bindgen_flags bindgen_make_flags(unsigned int mode, int enabled);

typedef union bindgen_value {
    int64_t integer;
    double real;
} bindgen_value;
bindgen_value *bindgen_value_new(int64_t value);
int64_t bindgen_value_read(const bindgen_value *value);
void bindgen_value_free(bindgen_value *value);

int64_t bindgen_variadic_sum(int64_t count, ...);
