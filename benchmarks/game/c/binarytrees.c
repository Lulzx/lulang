/* binary-trees — same arena strategy as binarytrees.lu, single-threaded.
   Nodes are indices into two parallel child arrays; index 0 is null. */
#include <stdio.h>
#include <stdlib.h>

static long shift_left(long v, long k) { return v << k; }
static long tree_nodes(long depth) { return (1L << (depth + 1)) - 1; }

static long bottom_up_tree(long depth, long *left, long *right, long *next) {
  long node = (*next)++;
  if (depth > 0) {
    long l = bottom_up_tree(depth - 1, left, right, next);
    long r = bottom_up_tree(depth - 1, left, right, next);
    left[node] = l;
    right[node] = r;
  } else {
    left[node] = 0;
    right[node] = 0;
  }
  return node;
}

static long check_tree(long node, const long *left, const long *right) {
  long l = left[node];
  if (l == 0) return 1;
  return 1 + check_tree(l, left, right) + check_tree(right[node], left, right);
}

int main(int argc, char **argv) {
  long n = argc > 1 ? atol(argv[1]) : 10;
  const long min_depth = 4;
  long max_depth = min_depth + 2;
  if (n > max_depth) max_depth = n;
  long stretch_depth = max_depth + 1;

  long s_cap = tree_nodes(stretch_depth) + 1;
  long *s_left = calloc(s_cap, sizeof(long));
  long *s_right = calloc(s_cap, sizeof(long));
  long s_next = 1;

  long l_cap = tree_nodes(max_depth) + 1;
  long *l_left = calloc(l_cap, sizeof(long));
  long *l_right = calloc(l_cap, sizeof(long));
  long l_next = 1;

  long stretch = bottom_up_tree(stretch_depth, s_left, s_right, &s_next);
  printf("stretch tree of depth %ld\t check: %ld\n", stretch_depth,
         check_tree(stretch, s_left, s_right));

  long long_lived = bottom_up_tree(max_depth, l_left, l_right, &l_next);

  for (long depth = min_depth; depth <= max_depth; depth += 2) {
    long iterations = shift_left(1, max_depth - depth + min_depth);
    long check = 0;
    for (long i = 0; i < iterations; i++) {
      s_next = 1;
      long t = bottom_up_tree(depth, s_left, s_right, &s_next);
      check += check_tree(t, s_left, s_right);
    }
    printf("%ld\t trees of depth %ld\t check: %ld\n", iterations, depth, check);
  }

  printf("long lived tree of depth %ld\t check: %ld\n", max_depth,
         check_tree(long_lived, l_left, l_right));
  return 0;
}
