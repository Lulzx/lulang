/* k-nucleotide — same algorithm as knucleotide.lu, single-threaded.
   Same open-addressed table over parallel arrays, same 2-bit packing, same
   insertion sort, so the comparison is language-to-language.
   Takes the input as a file argument, matching the lulang program. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static long code_of(int b) {
  if (b == 'a' || b == 'A') return 0;
  if (b == 'c' || b == 'C') return 1;
  if (b == 'g' || b == 'G') return 2;
  return 3;
}

static char base_of(long c) {
  if (c == 0) return 'A';
  if (c == 1) return 'C';
  if (c == 2) return 'G';
  return 'T';
}

static long table_capacity(long k, long n) {
  long space = 1;
  int capped = 0;
  for (long i = 0; i < k && !capped; i++) {
    space *= 4;
    if (space > n) capped = 1;
  }
  long want = n;
  if (space < want) want = space;
  long cap = 16;
  while (cap < want * 2) cap *= 2;
  return cap;
}

static long mix(long key, long cap) {
  long h = (long)((unsigned long)key * 2654435761UL) % cap;
  if (h < 0) h += cap;
  return h;
}

static void table_bump(long *slot_key, long *slot_count, long cap, long key) {
  long i = mix(key, cap);
  while (slot_count[i] != 0 && slot_key[i] != key) {
    i++;
    if (i == cap) i = 0;
  }
  slot_key[i] = key;
  slot_count[i]++;
}

static long table_get(const long *slot_key, const long *slot_count, long cap, long key) {
  long i = mix(key, cap);
  while (slot_count[i] != 0) {
    if (slot_key[i] == key) return slot_count[i];
    i++;
    if (i == cap) i = 0;
  }
  return 0;
}

static long pack(const signed char *seq, long start, long k) {
  long key = 0;
  for (long i = 0; i < k; i++) key = key * 4 + seq[start + i];
  return key;
}

static void put_pct(long count, long total) {
  long scaled = (long)((double)count * 100000.0 / (double)total + 0.5);
  printf("%ld.%03ld", scaled / 1000, scaled % 1000);
}

static void report_frequencies(const signed char *seq, long n, long k) {
  long cap = table_capacity(k, n);
  long *slot_key = calloc((size_t)cap, sizeof(long));
  long *slot_count = calloc((size_t)cap, sizeof(long));
  for (long i = 0; i + k <= n; i++) table_bump(slot_key, slot_count, cap, pack(seq, i, k));

  long *keys = malloc((size_t)cap * sizeof(long));
  long *counts = malloc((size_t)cap * sizeof(long));
  long m = 0;
  for (long s = 0; s < cap; s++) {
    if (slot_count[s] != 0) { keys[m] = slot_key[s]; counts[m] = slot_count[s]; m++; }
  }

  for (long a = 1; a < m; a++) {
    long ck = keys[a], cc = counts[a];
    long b = a - 1;
    while (b >= 0 && (counts[b] < cc || (counts[b] == cc && keys[b] > ck))) {
      keys[b + 1] = keys[b];
      counts[b + 1] = counts[b];
      b--;
    }
    keys[b + 1] = ck;
    counts[b + 1] = cc;
  }

  long total = n - k + 1;
  char buf[64];
  for (long r = 0; r < m; r++) {
    long rest = keys[r];
    for (long i = k - 1; i >= 0; i--) { buf[i] = base_of(rest % 4); rest /= 4; }
    buf[k] = 0;
    printf("%s ", buf);
    put_pct(counts[r], total);
    putchar('\n');
  }
  putchar('\n');
  free(slot_key); free(slot_count); free(keys); free(counts);
}

static void report_count(const signed char *seq, long n, const char *fragment) {
  long k = (long)strlen(fragment);
  long cap = table_capacity(k, n);
  long *slot_key = calloc((size_t)cap, sizeof(long));
  long *slot_count = calloc((size_t)cap, sizeof(long));
  for (long i = 0; i + k <= n; i++) table_bump(slot_key, slot_count, cap, pack(seq, i, k));
  long key = 0;
  for (long j = 0; j < k; j++) key = key * 4 + code_of(fragment[j]);
  printf("%ld\t%s\n", table_get(slot_key, slot_count, cap, key), fragment);
  free(slot_key); free(slot_count);
}

int main(int argc, char **argv) {
  if (argc < 2) { printf("usage: knucleotide <fasta-file>\n"); return 0; }
  FILE *f = fopen(argv[1], "rb");
  if (!f) { perror("open"); return 1; }
  fseek(f, 0, SEEK_END);
  long n = ftell(f);
  fseek(f, 0, SEEK_SET);
  char *text = malloc((size_t)n + 1);
  if (fread(text, 1, (size_t)n, f) != (size_t)n) { perror("read"); return 1; }
  fclose(f);

  long i = 0;
  int found = 0;
  while (i < n && !found) {
    if (text[i] == '>' && i + 6 <= n && memcmp(text + i, ">THREE", 6) == 0) {
      found = 1;
      while (i < n && text[i] != '\n') i++;
      i++;
    } else {
      while (i < n && text[i] != '\n') i++;
      i++;
    }
  }

  signed char *seq = malloc((size_t)n);
  long count = 0;
  for (; i < n; i++) {
    if (text[i] != '\n') seq[count++] = (signed char)code_of(text[i]);
  }

  report_frequencies(seq, count, 1);
  report_frequencies(seq, count, 2);
  report_count(seq, count, "GGT");
  report_count(seq, count, "GGTA");
  report_count(seq, count, "GGTATT");
  report_count(seq, count, "GGTATTTTAATT");
  report_count(seq, count, "GGTATTTTAATTTATAGT");
  return 0;
}
