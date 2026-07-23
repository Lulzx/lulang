#include <raylib.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

void lp_init_window(int64_t width, int64_t height,
                    const char *title, int64_t title_length) {
  char *copy = malloc((size_t)title_length + 1);
  memcpy(copy, title, (size_t)title_length);
  copy[title_length] = 0;
  InitWindow((int)width, (int)height, copy);
  free(copy);
  SetTargetFPS(60);
}

int64_t lp_window_should_close(void) {
  return WindowShouldClose() ? 1 : 0;
}

void lp_begin_frame(void) {
  BeginDrawing();
  ClearBackground((Color){12, 17, 27, 255});
}

void lp_draw_circle(double x, double y, double radius, int64_t rgba) {
  Color color = {
      (unsigned char)((rgba >> 24) & 255),
      (unsigned char)((rgba >> 16) & 255),
      (unsigned char)((rgba >> 8) & 255),
      (unsigned char)(rgba & 255),
  };
  DrawCircleV((Vector2){(float)x, (float)y}, (float)radius, color);
}

void lp_end_frame(void) {
  EndDrawing();
}

void lp_close_window(void) {
  CloseWindow();
}
