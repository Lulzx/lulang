#include "kernel_saxpy.h"
#include <stdio.h>

int main(void) {
  double x[] = {1.0, 2.0, 3.0};
  double y[] = {10.0, 20.0, 30.0};
  double total = saxpy(2.0, x, 3, y, 3, 3);
  printf("%.0f %.0f %.0f %.0f\n", total, y[0], y[1], y[2]);
  return 0;
}
