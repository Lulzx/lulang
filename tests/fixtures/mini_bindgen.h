#ifndef MINI_BINDGEN_H
#define MINI_BINDGEN_H

#include <stdint.h>

#define MINI_LIMIT 0x10
#define MINI_RATE 0.125

typedef long mini_index;

typedef enum mini_state {
  MINI_IDLE = 0,
  MINI_BUSY = 1
} mini_state;

typedef struct mini_vector {
  double x;
  double y;
} mini_vector;

double hypot(double x, double y);
int64_t clamp_index(mini_index value, int64_t low, int64_t high);

/* These declarations are parsed, diagnosed, and deliberately not emitted. */
float narrow_float(float value);
void *allocate_bytes(int64_t size);
int64_t consume_vector(mini_vector value);

#endif
