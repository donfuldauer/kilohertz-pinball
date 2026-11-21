/* S10 micro-test reference: the Wasm semantics of every op in ops.txt, computed
 * natively on x86_64 (SSE2 scalar IEEE arithmetic, round-to-nearest-even, no
 * contraction: build with -O0 -ffp-contract=off). Bitwise ops (abs, neg, copysign,
 * moves, pmin/pmax, lane moves, bitselect) are exact including NaN payloads; for
 * arithmetic ops a NaN result is compared by class only (the Wasm spec leaves the
 * sign and, for non-canonical inputs, the payload nondeterministic).
 * usage: ref <dir>   (reads dir/inputs.bin and dir/ops.txt, writes dir/ref.bin)
 */
#include <fenv.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define A32O 0x10000
#define B32O 0x20000
#define A64O 0x30000
#define B64O 0x50000
#define I32O 0x70000
#define I64O 0x80000
#define OUTO 0x100000

static uint8_t mem[OUTO];
static int N;

static float fA(int i) { float f; memcpy(&f, mem + A32O + 4 * i, 4); return f; }
static float fB(int i) { float f; memcpy(&f, mem + B32O + 4 * i, 4); return f; }
static double dA(int i) { double d; memcpy(&d, mem + A64O + 8 * i, 8); return d; }
static double dB(int i) { double d; memcpy(&d, mem + B64O + 8 * i, 8); return d; }
static uint32_t uA32(int i) { uint32_t u; memcpy(&u, mem + A32O + 4 * i, 4); return u; }
static uint32_t uB32(int i) { uint32_t u; memcpy(&u, mem + B32O + 4 * i, 4); return u; }
static uint64_t uA64(int i) { uint64_t u; memcpy(&u, mem + A64O + 8 * i, 8); return u; }
static uint64_t uB64(int i) { uint64_t u; memcpy(&u, mem + B64O + 8 * i, 8); return u; }
static int32_t I32(int i) { int32_t v; memcpy(&v, mem + I32O + 4 * i, 4); return v; }
static int64_t I64(int i) { int64_t v; memcpy(&v, mem + I64O + 8 * i, 8); return v; }
static uint32_t fu(float f) { uint32_t u; memcpy(&u, &f, 4); return u; }
static uint64_t du(double d) { uint64_t u; memcpy(&u, &d, 8); return u; }
static float uf(uint32_t u) { float f; memcpy(&f, &u, 4); return f; }
static double ud(uint64_t u) { double d; memcpy(&d, &u, 8); return d; }

static float wminf(float a, float b) {
  if (isnan(a) || isnan(b)) return NAN;
  if (a == 0 && b == 0) return (signbit(a) || signbit(b)) ? -0.0f : 0.0f;
  return a < b ? a : b;
}
static float wmaxf(float a, float b) {
  if (isnan(a) || isnan(b)) return NAN;
  if (a == 0 && b == 0) return (signbit(a) && signbit(b)) ? -0.0f : 0.0f;
  return a > b ? a : b;
}
static double wmin(double a, double b) {
  if (isnan(a) || isnan(b)) return NAN;
  if (a == 0 && b == 0) return (signbit(a) || signbit(b)) ? -0.0 : 0.0;
  return a < b ? a : b;
}
static double wmax(double a, double b) {
  if (isnan(a) || isnan(b)) return NAN;
  if (a == 0 && b == 0) return (signbit(a) && signbit(b)) ? -0.0 : 0.0;
  return a > b ? a : b;
}
static int32_t sat_i32(double x) {
  if (isnan(x)) return 0;
  if (x >= 2147483648.0) return INT32_MAX;
  if (x <= -2147483649.0) return INT32_MIN;
  return (int32_t)trunc(x);
}
static uint32_t sat_u32(double x) {
  if (isnan(x)) return 0;
  if (x >= 4294967296.0) return UINT32_MAX;
  if (x <= -1.0) return 0;
  return (uint32_t)trunc(x);
}
static int64_t sat_i64(double x) {
  if (isnan(x)) return 0;
  if (x >= 9223372036854775808.0) return INT64_MAX;
  if (x < -9223372036854775808.0) return INT64_MIN;
  return (int64_t)trunc(x);
}
static uint64_t sat_u64(double x) {
  if (isnan(x)) return 0;
  if (x >= 18446744073709551616.0) return UINT64_MAX;
  if (x <= -1.0) return 0;
  return (uint64_t)trunc(x);
}

#define PUT32(k, v) do { uint32_t _u = (v); memcpy(o + 4 * (k), &_u, 4); } while (0)
#define PUT64(k, v) do { uint64_t _u = (v); memcpy(o + 8 * (k), &_u, 8); } while (0)

static int eq(const char *a, const char *b) { return strcmp(a, b) == 0; }

static void f32bin(const char *op, int i, uint8_t *o) {
  float a = fA(i), b = fB(i), r;
  if (eq(op, "add")) r = a + b;
  else if (eq(op, "sub")) r = a - b;
  else if (eq(op, "mul")) r = a * b;
  else if (eq(op, "div")) r = a / b;
  else if (eq(op, "min")) r = wminf(a, b);
  else if (eq(op, "max")) r = wmaxf(a, b);
  else if (eq(op, "copysign")) { PUT32(0, (uA32(i) & 0x7fffffffu) | (uB32(i) & 0x80000000u)); return; }
  else { fprintf(stderr, "f32bin %s\n", op); exit(2); }
  PUT32(0, fu(r));
}

static int lane_op_f32(const char *op, float a, float b, uint32_t ua, uint32_t ub, uint32_t *out) {
  /* returns 1 if handled */
  if (eq(op, "add")) *out = fu(a + b);
  else if (eq(op, "sub")) *out = fu(a - b);
  else if (eq(op, "mul")) *out = fu(a * b);
  else if (eq(op, "div")) *out = fu(a / b);
  else if (eq(op, "min")) *out = fu(wminf(a, b));
  else if (eq(op, "max")) *out = fu(wmaxf(a, b));
  else if (eq(op, "pmin")) *out = (b < a) ? ub : ua;
  else if (eq(op, "pmax")) *out = (a < b) ? ub : ua;
  else if (eq(op, "eq")) *out = a == b ? 0xffffffffu : 0;
  else if (eq(op, "ne")) *out = a != b ? 0xffffffffu : 0;
  else if (eq(op, "lt")) *out = a < b ? 0xffffffffu : 0;
  else if (eq(op, "gt")) *out = a > b ? 0xffffffffu : 0;
  else if (eq(op, "le")) *out = a <= b ? 0xffffffffu : 0;
  else if (eq(op, "ge")) *out = a >= b ? 0xffffffffu : 0;
  else if (eq(op, "abs")) *out = ua & 0x7fffffffu;
  else if (eq(op, "neg")) *out = ua ^ 0x80000000u;
  else if (eq(op, "sqrt")) *out = fu(sqrtf(a));
  else if (eq(op, "ceil")) *out = fu(ceilf(a));
  else if (eq(op, "floor")) *out = fu(floorf(a));
  else if (eq(op, "trunc")) *out = fu(truncf(a));
  else if (eq(op, "nearest")) *out = fu(nearbyintf(a));
  else return 0;
  return 1;
}
static int lane_op_f64(const char *op, double a, double b, uint64_t ua, uint64_t ub, uint64_t *out) {
  if (eq(op, "add")) *out = du(a + b);
  else if (eq(op, "sub")) *out = du(a - b);
  else if (eq(op, "mul")) *out = du(a * b);
  else if (eq(op, "div")) *out = du(a / b);
  else if (eq(op, "min")) *out = du(wmin(a, b));
  else if (eq(op, "max")) *out = du(wmax(a, b));
  else if (eq(op, "pmin")) *out = (b < a) ? ub : ua;
  else if (eq(op, "pmax")) *out = (a < b) ? ub : ua;
  else if (eq(op, "eq")) *out = a == b ? ~0ull : 0;
  else if (eq(op, "ne")) *out = a != b ? ~0ull : 0;
  else if (eq(op, "lt")) *out = a < b ? ~0ull : 0;
  else if (eq(op, "gt")) *out = a > b ? ~0ull : 0;
  else if (eq(op, "le")) *out = a <= b ? ~0ull : 0;
  else if (eq(op, "ge")) *out = a >= b ? ~0ull : 0;
  else if (eq(op, "abs")) *out = ua & 0x7fffffffffffffffull;
  else if (eq(op, "neg")) *out = ua ^ 0x8000000000000000ull;
  else if (eq(op, "sqrt")) *out = du(sqrt(a));
  else if (eq(op, "ceil")) *out = du(ceil(a));
  else if (eq(op, "floor")) *out = du(floor(a));
  else if (eq(op, "trunc")) *out = du(trunc(a));
  else if (eq(op, "nearest")) *out = du(nearbyint(a));
  else return 0;
  return 1;
}

static void compute(const char *name, int i, uint8_t *o) {
  char ty[32], op[64];
  const char *dot = strchr(name, '.');
  size_t tl = (size_t)(dot - name);
  memcpy(ty, name, tl); ty[tl] = 0;
  strcpy(op, dot + 1);
  memset(o, 0, 16);
  if (eq(ty, "f32") || eq(ty, "f64")) {
    int is32 = eq(ty, "f32");
    if (eq(op, "load_store")) { if (is32) PUT32(0, uA32(i)); else PUT64(0, uA64(i)); return; }
    if (eq(op, "reinterpret_roundtrip")) { if (is32) PUT32(0, uA32(i)); else PUT64(0, uA64(i)); return; }
    if (eq(op, "demote_f64")) { PUT32(0, fu((float)dA(i))); return; }
    if (eq(op, "promote_f32")) { PUT64(0, du((double)fA(i))); return; }
    if (!strncmp(op, "convert_", 8)) {
      if (is32) {
        if (eq(op, "convert_i32_s")) PUT32(0, fu((float)I32(i)));
        else if (eq(op, "convert_i32_u")) PUT32(0, fu((float)(uint32_t)I32(i)));
        else if (eq(op, "convert_i64_s")) PUT32(0, fu((float)I64(i)));
        else if (eq(op, "convert_i64_u")) PUT32(0, fu((float)(uint64_t)I64(i)));
      } else {
        if (eq(op, "convert_i32_s")) PUT64(0, du((double)I32(i)));
        else if (eq(op, "convert_i32_u")) PUT64(0, du((double)(uint32_t)I32(i)));
        else if (eq(op, "convert_i64_s")) PUT64(0, du((double)I64(i)));
        else if (eq(op, "convert_i64_u")) PUT64(0, du((double)(uint64_t)I64(i)));
      }
      return;
    }
    if (eq(op, "copysign")) {
      if (is32) PUT32(0, (uA32(i) & 0x7fffffffu) | (uB32(i) & 0x80000000u));
      else PUT64(0, (uA64(i) & 0x7fffffffffffffffull) | (uB64(i) & 0x8000000000000000ull));
      return;
    }
    if (is32) {
      uint32_t r;
      if (!lane_op_f32(op, fA(i), fB(i), uA32(i), uB32(i), &r)) { fprintf(stderr, "op %s\n", name); exit(2); }
      if (!strcmp(op, "eq") || !strcmp(op, "ne") || !strcmp(op, "lt") || !strcmp(op, "gt") || !strcmp(op, "le") || !strcmp(op, "ge")) r &= 1;
      PUT32(0, r);
    } else {
      uint64_t r;
      if (!lane_op_f64(op, dA(i), dB(i), uA64(i), uB64(i), &r)) { fprintf(stderr, "op %s\n", name); exit(2); }
      if (!strcmp(op, "eq") || !strcmp(op, "ne") || !strcmp(op, "lt") || !strcmp(op, "gt") || !strcmp(op, "le") || !strcmp(op, "ge")) { PUT32(0, (uint32_t)(r & 1)); return; }
      PUT64(0, r);
    }
    return;
  }
  if (eq(ty, "i32") || eq(ty, "i64")) {
    int is32 = eq(ty, "i32");
    if (eq(op, "trunc_sat_f32_s")) { if (is32) PUT32(0, (uint32_t)sat_i32(fA(i))); else PUT64(0, (uint64_t)sat_i64(fA(i))); }
    else if (eq(op, "trunc_sat_f32_u")) { if (is32) PUT32(0, sat_u32(fA(i))); else PUT64(0, sat_u64(fA(i))); }
    else if (eq(op, "trunc_sat_f64_s")) { if (is32) PUT32(0, (uint32_t)sat_i32(dA(i))); else PUT64(0, (uint64_t)sat_i64(dA(i))); }
    else if (eq(op, "trunc_sat_f64_u")) { if (is32) PUT32(0, sat_u32(dA(i))); else PUT64(0, sat_u64(dA(i))); }
    else if (eq(op, "trunc_f64_s_guarded")) {
      double a = dA(i);
      PUT64(0, (a >= -9223372036854775808.0 && a < 9223372036854775808.0) ? (uint64_t)(int64_t)a : 0x5a5a5a5a5a5a5a5aull);
    } else if (eq(op, "trunc_f32_s_guarded")) {
      float a = fA(i);
      PUT32(0, (a >= -2147483648.0f && a < 2147483648.0f) ? (uint32_t)(int32_t)a : 0x5a5a5a5au);
    } else { fprintf(stderr, "op %s\n", name); exit(2); }
    return;
  }
  if (eq(ty, "f32x4")) {
    if (eq(op, "demote_f64x2_zero")) { PUT32(0, fu((float)dA(i))); PUT32(1, fu((float)dA(i + 1))); return; }
    if (eq(op, "convert_i32x4_s")) { for (int k = 0; k < 4; k++) PUT32(k, fu((float)(int32_t)uA32(i + k))); return; }
    if (eq(op, "convert_i32x4_u")) { for (int k = 0; k < 4; k++) PUT32(k, fu((float)uA32(i + k))); return; }
    if (eq(op, "splat_extract_replace")) { uint32_t x = uA32(i + 1); PUT32(0, x); PUT32(1, x); PUT32(2, uB32(i)); PUT32(3, x); return; }
    for (int k = 0; k < 4; k++) {
      uint32_t r;
      if (!lane_op_f32(op, fA(i + k), fB(i + k), uA32(i + k), uB32(i + k), &r)) { fprintf(stderr, "op %s\n", name); exit(2); }
      PUT32(k, r);
    }
    return;
  }
  if (eq(ty, "f64x2")) {
    if (eq(op, "promote_low_f32x4")) { PUT64(0, du((double)fA(i))); PUT64(1, du((double)fA(i + 1))); return; }
    if (eq(op, "splat_extract_replace")) { PUT64(0, uB64(i)); PUT64(1, uA64(i + 1)); return; }
    for (int k = 0; k < 2; k++) {
      uint64_t r;
      if (!lane_op_f64(op, dA(i + k), dB(i + k), uA64(i + k), uB64(i + k), &r)) { fprintf(stderr, "op %s\n", name); exit(2); }
      PUT64(k, r);
    }
    return;
  }
  if (eq(ty, "i32x4")) {
    for (int k = 0; k < 4; k++) {
      if (eq(op, "trunc_sat_f32x4_s")) PUT32(k, (uint32_t)sat_i32(fA(i + k)));
      else if (eq(op, "trunc_sat_f32x4_u")) PUT32(k, sat_u32(fA(i + k)));
      else { fprintf(stderr, "op %s\n", name); exit(2); }
    }
    return;
  }
  if (eq(ty, "v128")) {
    if (eq(op, "bitselect_f32")) { for (int k = 0; k < 4; k++) PUT32(k, fA(i + k) < fB(i + k) ? uA32(i + k) : uB32(i + k)); return; }
  }
  fprintf(stderr, "unknown op %s\n", name);
  exit(2);
}

int main(int argc, char **argv) {
  if (argc < 2) return 2;
  char p[4096];
  fesetround(FE_TONEAREST);
  snprintf(p, sizeof p, "%s/inputs.bin", argv[1]);
  FILE *f = fopen(p, "rb");
  char magic[8];
  if (!f || fread(magic, 1, 8, f) != 8 || fread(&N, 4, 1, f) != 1) return 3;
  if (fread(mem + A32O, 1, OUTO - A32O, f) != OUTO - A32O) return 4;
  fclose(f);
  snprintf(p, sizeof p, "%s/ops.txt", argv[1]);
  FILE *ops = fopen(p, "r");
  snprintf(p, sizeof p, "%s/ref.bin", argv[1]);
  FILE *out = fopen(p, "wb");
  char line[256];
  uint8_t *buf = malloc((size_t)N * 16);
  int nops = 0;
  while (fgets(line, sizeof line, ops)) {
    char name[128], kind[16];
    int w;
    if (sscanf(line, "%127s %15s %d", name, kind, &w) != 3) continue;
    for (int i = 0; i < N; i++) compute(name, i, buf + (size_t)i * 16);
    fwrite(buf, 1, (size_t)N * 16, out);
    nops++;
  }
  fclose(out);
  printf("ref: %d ops x %d records\n", nops, N);
  return 0;
}
