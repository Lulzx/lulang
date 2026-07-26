/* mandelbrot — same algorithm as mandelbrot.lu, single-threaded, scalar. */
#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
  int w = argc > 1 ? atoi(argv[1]) : 200;
  int h = w;
  printf("P4\n%d %d\n", w, h);
  for (int y = 0; y < h; y++) {
    double Ci = 2.0 * y / h - 1.0;
    int byte = 0, nbits = 0;
    for (int x = 0; x < w; x++) {
      double Cr = 2.0 * x / w - 1.5;
      double Zr = 0.0, Zi = 0.0;
      int i = 0, escaped = 0;
      while (i < 50 && !escaped) {
        double Zr2 = Zr * Zr, Zi2 = Zi * Zi;
        if (Zr2 + Zi2 > 4.0) { escaped = 1; }
        else { Zi = 2.0 * Zr * Zi + Ci; Zr = Zr2 - Zi2 + Cr; i++; }
      }
      byte = byte * 2 + (escaped ? 0 : 1);
      if (++nbits == 8) { putchar(byte); byte = 0; nbits = 0; }
    }
    if (nbits > 0) putchar(byte << (8 - nbits));
  }
  return 0;
}
