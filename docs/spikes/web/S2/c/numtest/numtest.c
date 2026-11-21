/* Native self-test of lkhznum.c against glibc (spike S2).
 * khz_strx2number vs strtod on random hexadecimal numerals;
 * khz_number2strx vs snprintf on random subnormals and normals with random
 * flags/width/precision. Prints mismatch counts (expected 0). */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include "lua.h"
lua_Number khz_strx2number (const char *s, char **endptr);
int khz_number2strx (char *buff, int sz, const char *fmt, lua_Number x);
static uint64_t st = 0x853c49e6748fea9bULL;
static uint64_t rnd(void) { st ^= st << 13; st ^= st >> 7; st ^= st << 17; return st; }
int main(int argc, char **argv) {
  long n = argc > 1 ? atol(argv[1]) : 1000000;
  long bad_parse = 0, bad_end = 0, bad_fmt = 0, nsub = 0;
  char s[256], a[512], b[512], fmt[32];
  for (long i = 0; i < n; i++) {
    /* random hex numeral */
    int k = 0, nd = 1 + rnd() % 40, dotpos = rnd() % (nd + 2);
    if (rnd() % 4 == 0) s[k++] = '-';
    s[k++] = '0'; s[k++] = (rnd() & 1) ? 'x' : 'X';
    if (rnd() % 8 == 0) for (int z = rnd() % 30; z > 0; z--) s[k++] = '0';
    for (int d = 0; d < nd; d++) {
      if (d == dotpos) s[k++] = '.';
      int v = (rnd() % 5 == 0) ? (rnd() % 2 ? 0 : 15) : (int)(rnd() % 16);
      s[k++] = "0123456789abcdef"[v];
    }
    if (rnd() % 6) k += sprintf(s + k, "p%d", (int)(rnd() % 2300) - 1150);
    s[k] = 0;
    char *e1, *e2;
    double x1 = strtod(s, &e1), x2 = khz_strx2number(s, &e2);
    if (memcmp(&x1, &x2, 8) != 0) { if (bad_parse < 5) printf("parse %s glibc=%a khz=%a\n", s, x1, x2); bad_parse++; }
    if (e1 != e2) bad_end++;
    /* random format of a random subnormal (2/3) or normal (1/3) */
    uint64_t bits = rnd();
    if (i % 3) { bits &= 0x800fffffffffffffULL; if ((bits & 0x000fffffffffffffULL) == 0) bits |= 1; nsub++; }
    double x; memcpy(&x, &bits, 8);
    if (x != x) continue;
    int f = 0; fmt[f++] = '%';
    if (rnd() % 4 == 0) fmt[f++] = '-';
    if (rnd() % 4 == 0) fmt[f++] = '+';
    if (rnd() % 4 == 0) fmt[f++] = ' ';
    if (rnd() % 4 == 0) fmt[f++] = '#';
    if (rnd() % 4 == 0) fmt[f++] = '0';
    if (rnd() % 2) f += sprintf(fmt + f, "%d", (int)(rnd() % 40));
    if (rnd() % 2) f += sprintf(fmt + f, ".%d", (int)(rnd() % 20));
    fmt[f++] = (rnd() & 1) ? 'a' : 'A'; fmt[f] = 0;
    int la = snprintf(a, sizeof a, fmt, x);
    int lb = khz_number2strx(b, sizeof b, fmt, x);
    if (la != lb || strcmp(a, b) != 0) { if (bad_fmt < 5) printf("fmt %s glibc=[%s] khz=[%s]\n", fmt, a, b); bad_fmt++; }
  }
  printf("cases=%ld parse_mismatch=%ld endptr_mismatch=%ld format_mismatch=%ld (subnormals=%ld)\n", n, bad_parse, bad_end, bad_fmt, nsub);
  return (bad_parse || bad_end || bad_fmt) ? 1 : 0;
}
