#include "bindgen_runtime.h"
#include <stdarg.h>
#include <stdlib.h>

struct bindgen_box {
  int64_t value;
};

int64_t bindgen_add(int64_t left, int64_t right) {
  return left + right;
}

double bindgen_scale(double value, double factor) {
  return value * factor;
}

float bindgen_half(float value) {
  return value * 0.5f;
}

int32_t bindgen_increment_i32(int32_t value) {
  return value + 1;
}

_Bool bindgen_is_positive(int32_t value) {
  return value > 0;
}

double bindgen_pair_sum(bindgen_pair value) {
  return value.x + value.y;
}

double bindgen_mixed_value(bindgen_mixed value) {
  return (double)value.count * (double)value.scale;
}

bindgen_mixed bindgen_make_mixed(int32_t count, float scale) {
  return (bindgen_mixed){ .count = count, .scale = scale };
}

bindgen_box *bindgen_box_new(int64_t value) {
  bindgen_box *box = malloc(sizeof(*box));
  if (box != NULL) {
    box->value = value;
  }
  return box;
}

int64_t bindgen_box_read(const bindgen_box *box) {
  return box == NULL ? -1 : box->value;
}

void bindgen_box_free(bindgen_box *box) {
    free(box);
}

double bindgen_slice_sum(const double *values, int64_t values_len) {
    double total = 0.0;
    for (int64_t i = 0; i < values_len; i++) total += values[i];
    return total;
}

double bindgen_slice_bump(double *values, int64_t values_len) {
    for (int64_t i = 0; i < values_len; i++) values[i] += 1.0;
    return values[0];
}

int64_t bindgen_apply(bindgen_callback callback, int64_t value) {
  return callback(value);
}

static int64_t bindgen_increment_callback(int64_t value) {
  return value + 1;
}

bindgen_callback bindgen_incrementer(void) {
  return bindgen_increment_callback;
}

bindgen_pair bindgen_make_pair(int64_t x, int64_t y) {
  return (bindgen_pair){ .x = (double)x, .y = (double)y };
}

int64_t bindgen_flags_score(bindgen_flags value) {
  return (int64_t)value.mode + (value.enabled ? 10 : 0);
}

bindgen_flags bindgen_make_flags(unsigned int mode, int enabled) {
  return (bindgen_flags){ .mode = mode, .enabled = enabled };
}

bindgen_value *bindgen_value_new(int64_t value) {
  bindgen_value *result = malloc(sizeof(*result));
  result->integer = value;
  return result;
}

int64_t bindgen_value_read(const bindgen_value *value) {
  return value->integer;
}

void bindgen_value_free(bindgen_value *value) {
  free(value);
}

int64_t bindgen_variadic_sum(int64_t count, ...) {
  int64_t total = 0;
  va_list values;
  va_start(values, count);
  for (int64_t i = 0; i < count; i++) {
    total += va_arg(values, int64_t);
  }
  va_end(values);
  return total;
}
