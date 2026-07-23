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

typedef struct bindgen_box bindgen_box;

bindgen_box *bindgen_box_new(int64_t value);
int64_t bindgen_box_read(const bindgen_box *box);
void bindgen_box_free(bindgen_box *box);
