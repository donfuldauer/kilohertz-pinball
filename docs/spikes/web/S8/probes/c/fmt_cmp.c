/* S8 probe (d)/(e): the C library conversions Lua uses for numbers, glibc
** (x86_64) vs wasi-libc (musl, wasm32): snprintf with the formats Lua's
** tostring and string.format pass through, and strtod on decimal strings.
** Prints one FNV-1a digest per format plus the first differing sample, so the
** two outputs can be compared with cmp. Inputs: deterministic PCG stream. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
static uint64_t st = 0x853c49e6748fea9bULL;
static uint32_t pcg(void) { uint64_t o = st; st = o * 6364136223846793005ULL + 1442695040888963407ULL;
  uint32_t x = (uint32_t)(((o >> 18) ^ o) >> 27); uint32_t r = (uint32_t)(o >> 59); return (x >> r) | (x << ((-r) & 31)); }
static uint64_t r64(void) { return ((uint64_t)pcg() << 32) | pcg(); }
static double sample(int i) {
  union { uint64_t u; double d; } v;
  switch (i % 4) {
    case 0: v.u = r64(); return v.d;                                   /* any bit pattern */
    case 1: v.u = (r64() & 0x800fffffffffffffULL) | ((uint64_t)(1023 - 40 + pcg() % 80) << 52); return v.d; /* |x| ~ 1e-12..1e12 */
    case 2: return (double)(int64_t)(r64() >> (pcg() % 64)) / (double)(1 + pcg() % 1000); /* ratios */
    default: return (double)(pcg() % 2000000) / 1000.0 + ((pcg() & 1) ? 0.0005 : 0.0); /* decimal ties */
  }
}
static uint64_t fnv(uint64_t h, const char *s) { for (; *s; s++) { h ^= (unsigned char)*s; h *= 0x100000001b3ULL; } return h ^ 0xff; }
int main(int argc, char **argv) {
  int n = argc > 1 ? atoi(argv[1]) : 1000000;
  static const char *fm[] = {"%.14g", "%.17g", "%.0f", "%.3f", "%.20e", "%e", "%g", "%.3g", "%#.6g", "%10.4f", "%-24.10e", "%+.5g", "% .2e", "%.99f", "%.40e"};
  int nf = sizeof(fm) / sizeof(fm[0]), k, i;
  char buf[512], first[1200];
  for (k = 0; k < nf; k++) {
    uint64_t h = 0xcbf29ce484222325ULL; st = 0x853c49e6748fea9bULL + (uint64_t)k;
    first[0] = 0;
    for (i = 0; i < n; i++) { double d = sample(i); snprintf(buf, sizeof buf, fm[k], d); h = fnv(h, buf); if (i == 12345) snprintf(first, sizeof first, "%s", buf); }
    printf("fmt %-10s n=%d digest=%016llx sample=%s\n", fm[k], n, (unsigned long long)h, first);
  }
  /* strtod: round trip of %.17g, and random decimal strings with 1..40 digits and exponents */
  { uint64_t h = 0xcbf29ce484222325ULL; st = 99; int bad = 0, nan = 0;
    for (i = 0; i < n; i++) { double d = sample(i), e; union { double d; uint64_t u; } a, b; snprintf(buf, sizeof buf, "%.17g", d); e = strtod(buf, NULL); a.d = d; b.d = e;
      if (d == d && a.u != b.u) bad++;
      if (d != d) { nan++; continue; }  /* Lua never passes "nan"/"inf" to strtod (l_str2d rejects 'n'/'N') */
      snprintf(buf, sizeof buf, "%016llx", (unsigned long long)b.u); h = fnv(h, buf); }
    printf("strtod_roundtrip n=%d digest=%016llx mismatches=%d nan_inputs_skipped=%d\n", n, (unsigned long long)h, bad, nan); }
  { uint64_t h = 0xcbf29ce484222325ULL; st = 7;
    for (i = 0; i < n; i++) { int nd = 1 + pcg() % 40, j, p = 0; char s[96]; union { double d; uint64_t u; } v;
      if (pcg() & 1) s[p++] = '-';
      for (j = 0; j < nd; j++) { s[p++] = (char)('0' + pcg() % 10); if (j == (int)(pcg() % (nd + 1))) s[p++] = '.'; }
      p += sprintf(s + p, "e%d", (int)(pcg() % 700) - 350); s[p] = 0;
      v.d = strtod(s, NULL); snprintf(buf, sizeof buf, "%016llx", (unsigned long long)v.u); h = fnv(h, buf); }
    printf("strtod_random n=%d digest=%016llx\n", n, (unsigned long long)h); }
  return 0;
}
