/* reverse-complement — same algorithm as revcomp.lu, single-threaded.
   Takes the input as a file argument rather than on stdin, matching the
   lulang program (the language has no stdin builtin). */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define WIDTH 60

static void emit_revcomp(char *seq, long lo, long hi, const unsigned char *table) {
  long i = lo, j = hi - 1;
  while (i < j) {
    unsigned char a = table[(unsigned char)seq[i]];
    unsigned char b = table[(unsigned char)seq[j]];
    seq[i] = (char)b;
    seq[j] = (char)a;
    i++;
    j--;
  }
  if (i == j) seq[i] = (char)table[(unsigned char)seq[i]];

  for (long start = lo; start < hi; start += WIDTH) {
    long stop = start + WIDTH;
    if (stop > hi) stop = hi;
    fwrite(seq + start, 1, (size_t)(stop - start), stdout);
    putchar('\n');
  }
}

int main(int argc, char **argv) {
  if (argc < 2) { printf("usage: revcomp <fasta-file>\n"); return 0; }
  FILE *f = fopen(argv[1], "rb");
  if (!f) { perror("open"); return 1; }
  fseek(f, 0, SEEK_END);
  long n = ftell(f);
  fseek(f, 0, SEEK_SET);
  char *text = malloc((size_t)n + 1);
  if (fread(text, 1, (size_t)n, f) != (size_t)n) { perror("read"); return 1; }
  fclose(f);

  unsigned char table[256];
  for (int i = 0; i < 256; i++) table[i] = (unsigned char)i;
  const char *from = "ABCDGHKMNRSTVWYabcdghkmnrstvwy";
  const char *to = "TVGHCDMKNYSABWRTVGHCDMKNYSABWR";
  for (int i = 0; from[i]; i++) table[(unsigned char)from[i]] = (unsigned char)to[i];

  char *seq = malloc((size_t)n);
  long count = 0, seq_start = 0, i = 0;
  while (i < n) {
    if (text[i] == '>') {
      if (count > seq_start) emit_revcomp(seq, seq_start, count, table);
      long j = i;
      while (j < n && text[j] != '\n') j++;
      fwrite(text + i, 1, (size_t)(j - i), stdout);
      putchar('\n');
      i = j + 1;
      seq_start = count;
    } else {
      long j = i;
      while (j < n && text[j] != '\n') seq[count++] = text[j++];
      i = j + 1;
    }
  }
  if (count > seq_start) emit_revcomp(seq, seq_start, count, table);
  return 0;
}
