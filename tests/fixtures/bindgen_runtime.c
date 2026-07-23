#include "bindgen_runtime.h"
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
