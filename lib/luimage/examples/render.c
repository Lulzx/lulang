#include "luimage.h"

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
    const int64_t width = 160;
    const int64_t height = 100;
    const int64_t count = width * height;
    if (argc != 2) {
        fprintf(stderr, "usage: render output.pgm\n");
        return 2;
    }
    double *pixels = calloc((size_t)count, sizeof(double));
    if (pixels == NULL) return 3;
    render_mandelbrot(pixels, count, width, height, 96);

    FILE *output = fopen(argv[1], "wb");
    if (output == NULL) {
        free(pixels);
        return 4;
    }
    fprintf(output, "P5\n%lld %lld\n255\n",
            (long long)width, (long long)height);
    for (int64_t pixel = 0; pixel < count; ++pixel) {
        double value = pixels[pixel];
        if (value < 0.0) value = 0.0;
        if (value > 1.0) value = 1.0;
        fputc((int)(value * 255.0 + 0.5), output);
    }
    fclose(output);
    printf("16000 %.6f\n", image_checksum(pixels, count));
    free(pixels);
    return 0;
}
