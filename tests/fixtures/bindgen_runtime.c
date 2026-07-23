#include "bindgen_runtime.h"

int64_t bindgen_add(int64_t left, int64_t right) {
  return left + right;
}

double bindgen_scale(double value, double factor) {
  return value * factor;
}
