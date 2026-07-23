#include <stdint.h>

int64_t bindgen_add(int64_t left, int64_t right);
double bindgen_scale(double value, double factor);

typedef struct bindgen_box bindgen_box;

bindgen_box *bindgen_box_new(int64_t value);
int64_t bindgen_box_read(const bindgen_box *box);
void bindgen_box_free(bindgen_box *box);
