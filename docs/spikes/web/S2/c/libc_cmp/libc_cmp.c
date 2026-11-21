/* glibc vs wasi-libc, directly (no Lua): strtod on every line of a file
 * (exact bits) and printf "%a" "%.3a" "%A" of N PCG-chosen subnormals plus the
 * same formats of N normals. Build natively and with wasi-sdk for wasm32-wasip1;
 * diff the outputs. usage: libc_cmp <strings.txt> <n_subnormals> */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
int main(int argc, char **argv) {
  char line[512];
  FILE *f = fopen(argv[1], "r");
  if (!f) { perror("open"); return 2; }
  while (fgets(line, sizeof line, f)) {
    line[strcspn(line, "\n")] = 0;
    char *end; double x = strtod(line, &end);
    uint64_t b; memcpy(&b, &x, 8);
    printf("strtod %016llx %ld\n", (unsigned long long)b, (long)(end - line));
  }
  fclose(f);
  long n = argc > 2 ? atol(argv[2]) : 10000;
  uint64_t s = 0x853c49e6748fea9bULL;
  for (long i = 0; i < 2 * n; i++) {
    s ^= s << 13; s ^= s >> 7; s ^= s << 17;
    uint64_t b = s;
    if (i < n) { b &= 0x800fffffffffffffULL; if (!(b & 0x000fffffffffffffULL)) b |= 1; }
    double x; memcpy(&x, &b, 8);
    if (x != x) continue;
    printf("%s %a %.3a %A\n", i < n ? "sub" : "nrm", x, x, x);
  }
  return 0;
}
