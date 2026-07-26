/* fasta — same algorithm and buffering strategy as fasta.lu, single-threaded. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define IM 139968
#define IA 3877
#define IC 29573
#define WIDTH 60
#define BUFLINES 1024

static const char *ALU =
    "GGCCGGGCGCGGTGGCTCACGCCTGTAATCCCAGCACTTTGGGAGGCCGAGGCGGGCGGA"
    "TCACCTGAGGTCAGGAGTTCGAGACCAGCCTGGCCAACATGGTGAAACCCCGTCTCTACT"
    "AAAAATACAAAAATTAGCCGGGCGTGGTGGCGCGCGCCTGTAATCCCAGCTACTCGGGAG"
    "GCTGAGGCAGGAGAATCGCTTGAACCCGGGAGGCGGAGGTTGCAGTGAGCCGAGATCGCG"
    "CCACTGCACTCCAGCCTGGGCGACAGAGCGAGACTCCGTCTCAAAAA";

static long seed = 42;

/* emit whole lines from buf, return leftover count moved to the front */
static long flush_lines(char *buf, long n) {
  long start = 0;
  while (n - start >= WIDTH) {
    fwrite(buf + start, 1, WIDTH, stdout);
    putchar('\n');
    start += WIDTH;
  }
  long rest = n - start;
  memmove(buf, buf + start, (size_t)rest);
  return rest;
}

static void repeat_fasta(const char *alu, long total) {
  long alu_len = (long)strlen(alu);
  long cap = WIDTH * BUFLINES;
  char *buf = malloc((size_t)cap);
  long pending = 0, pos = 0, written = 0;
  while (written < total) {
    buf[pending++] = alu[pos++];
    if (pos == alu_len) pos = 0;
    written++;
    if (pending == cap) pending = flush_lines(buf, pending);
  }
  if (pending > 0) {
    fwrite(buf, 1, (size_t)pending, stdout);
    putchar('\n');
  }
  free(buf);
}

static void random_fasta(const char *chars, const double *cum, int count,
                         long total) {
  long cap = WIDTH * BUFLINES;
  char *buf = malloc((size_t)cap);
  long pending = 0, written = 0;
  while (written < total) {
    seed = (seed * IA + IC) % IM;
    double r = (double)seed / IM;
    int k = 0;
    while (k < count - 1 && r >= cum[k]) k++;
    buf[pending++] = chars[k];
    written++;
    if (pending == cap) pending = flush_lines(buf, pending);
  }
  if (pending > 0) {
    fwrite(buf, 1, (size_t)pending, stdout);
    putchar('\n');
  }
  free(buf);
}

int main(int argc, char **argv) {
  long n = argc > 1 ? atol(argv[1]) : 1000;

  const char *iub_chars = "acgtBDHKMNRSVWY";
  double iub_probs[15] = {0.27, 0.12, 0.12, 0.27, 0.02, 0.02, 0.02, 0.02,
                          0.02, 0.02, 0.02, 0.02, 0.02, 0.02, 0.02};
  double iub_cum[15];
  double acc = 0.0;
  for (int i = 0; i < 15; i++) { acc += iub_probs[i]; iub_cum[i] = acc; }

  const char *homo_chars = "acgt";
  double homo_probs[4] = {0.3029549426680, 0.1979883004921, 0.1975473066391,
                          0.3015094502008};
  double homo_cum[4];
  acc = 0.0;
  for (int i = 0; i < 4; i++) { acc += homo_probs[i]; homo_cum[i] = acc; }

  printf(">ONE Homo sapiens alu\n");
  repeat_fasta(ALU, n * 2);
  printf(">TWO IUB ambiguity codes\n");
  random_fasta(iub_chars, iub_cum, 15, n * 3);
  printf(">THREE Homo sapiens frequency\n");
  random_fasta(homo_chars, homo_cum, 4, n * 5);
  return 0;
}
