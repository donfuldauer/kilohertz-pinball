/* Route (a) spike: a hand-written runtime for LLVM 18's Wasm setjmp/longjmp
 * lowering (-mllvm -wasm-enable-sjlj with clang-18 from Ubuntu 24.04): LLVM 18
 * calls saveSetjmp/testSetjmp/getTempRet0 and mallocs a label table in every
 * function that calls setjmp (Lua: luaD_rawrunprotected, i.e. every protected
 * call); LLVM >= 19 replaced this with __wasm_setjmp/__wasm_setjmp_test and a
 * stack buffer (no malloc). __wasm_longjmp and the __c_longjmp tag come from
 * wasi-libc's libsetjmp (its throw payload {env, val} is the same in both ABIs).
 * Mirrors emscripten's pre-LLVM-19 emscripten_setjmp.c. */
#include <stdint.h>
#include <stdlib.h>

typedef struct TableEntry { uintptr_t id; uint32_t label; } TableEntry;
static uint32_t temp_ret0;
static uintptr_t setjmp_id;
unsigned long khz_sjlj18_mallocs;  /* counted: hidden C allocations */

uint32_t getTempRet0(void) { return temp_ret0; }
static void setTempRet0(uint32_t v) { temp_ret0 = v; }

TableEntry *saveSetjmp(uintptr_t *env, uint32_t label, TableEntry *table, uint32_t size) {
  uint32_t i = 0;
  setjmp_id++;
  *env = setjmp_id;
  while (i < size) {
    if (table[i].id == 0) {
      table[i].id = setjmp_id;
      table[i].label = label;
      table[i + 1].id = 0;
      setTempRet0(size);
      return table;
    }
    i++;
  }
  size *= 2;
  khz_sjlj18_mallocs++;
  table = (TableEntry *)realloc(table, sizeof(TableEntry) * (size + 1));
  table = saveSetjmp(env, label, table, size);
  setTempRet0(size);
  return table;
}

uint32_t testSetjmp(uintptr_t id, TableEntry *table, uint32_t size) {
  uint32_t i = 0;
  while (i < size) {
    uintptr_t curr = table[i].id;
    if (curr == 0) break;
    if (curr == id) return table[i].label;
    i++;
  }
  return 0;
}
