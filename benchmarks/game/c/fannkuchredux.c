/* fannkuch-redux — same algorithm as fannkuchredux.lu, single-threaded. */
#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
  int n = argc > 1 ? atoi(argv[1]) : 7;
  int *perm1 = malloc((n + 1) * sizeof(int));
  int *perm = malloc((n + 1) * sizeof(int));
  int *count = malloc((n + 1) * sizeof(int));
  for (int i = 0; i < n; i++) perm1[i] = i;
  for (int i = 0; i <= n; i++) count[i] = 0;

  long maxflips = 0, checksum = 0, permcount = 0;
  int r = n, done = 0;

  while (!done) {
    while (r != 1) { count[r - 1] = r; r--; }

    for (int i = 0; i < n; i++) perm[i] = perm1[i];

    long flips = 0;
    int k = perm[0];
    while (k != 0) {
      int i = 0, j = k;
      while (i < j) { int t = perm[i]; perm[i] = perm[j]; perm[j] = t; i++; j--; }
      flips++;
      k = perm[0];
    }

    if (flips > maxflips) maxflips = flips;
    checksum += (permcount % 2 == 0) ? flips : -flips;
    permcount++;

    int advanced = 0;
    while (!advanced && !done) {
      if (r == n) { done = 1; }
      else {
        int perm0 = perm1[0];
        int i = 0;
        while (i < r) { perm1[i] = perm1[i + 1]; i++; }
        perm1[r] = perm0;
        count[r]--;
        if (count[r] > 0) advanced = 1; else r++;
      }
    }
  }

  printf("%ld\n", checksum);
  printf("Pfannkuchen(%d) = %ld\n", n, maxflips);
  return 0;
}
