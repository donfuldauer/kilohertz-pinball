/* S8 probe (a): sizeof/offsetof of every Lua 5.4.9 structure that is heap-allocated
** through luaM_ / luaC_newobj, plus the size/width-dependent limits.
** Built natively (x86_64, LP64) and for wasm32-wasip1 (ILP32, wasi-libc); outputs diffed. */
#include <stdio.h>
#include <stddef.h>
#include <limits.h>
#include <float.h>
#define LUA_CORE
#include "lprefix.h"
#include "lua.h"
#include "lauxlib.h"
#include "lobject.h"
#include "lstate.h"
#include "lfunc.h"
#include "lstring.h"
#include "ltable.h"
#include "lparser.h"
#include "llex.h"
#include "lzio.h"
#include "lgc.h"
#include "lmem.h"
typedef struct UBox { void *box; size_t bsize; } UBox;               /* lauxlib.c */
typedef struct LXs { lu_byte extra_[LUA_EXTRASPACE]; lua_State l; } LXs; /* lstate.c LX */
typedef struct LGs { LXs l; global_State g; } LGs;                       /* lstate.c LG */
#define P(name, v) printf("%s\t%llu\n", name, (unsigned long long)(v))
#define S(t) P("sizeof(" #t ")", sizeof(t))
#define O(t, f) P("offsetof(" #t "," #f ")", offsetof(t, f))
int main(void) {
  S(void*); S(size_t); S(int); S(long); S(long long); S(lua_Integer); S(lua_Number); S(l_mem); S(lu_mem);
  S(Instruction); S(lua_KContext); S(ptrdiff_t); S(sig_atomic_t); S(Value); S(TValue); S(StackValue); S(StkIdRel);
  S(GCObject); S(TString); O(TString, contents); S(Udata); O(Udata, uv); S(Udata0); O(Udata0, bindata);
  S(UValue); S(UBox); S(Table); S(Node); S(UpVal); S(CClosure); O(CClosure, upvalue); S(LClosure); O(LClosure, upvals);
  S(Proto); S(LocVar); S(Upvaldesc); S(AbsLineInfo); S(CallInfo); S(lua_State); S(global_State); S(LXs); S(LGs);
  S(stringtable); S(Vardesc); S(Labeldesc); S(Dyndata); S(FuncState); S(LexState); S(Mbuffer); S(luaL_Buffer);
  S(lua_Debug); S(TString*); S(Proto*);
  P("LUAI_MAXSTACK", LUAI_MAXSTACK); P("LUAI_MAXCCALLS", LUAI_MAXCCALLS); P("LUAL_BUFFERSIZE", LUAL_BUFFERSIZE);
  P("LUA_EXTRASPACE", LUA_EXTRASPACE); P("LUAI_MAXSHORTLEN", LUAI_MAXSHORTLEN); P("LUA_IDSIZE", LUA_IDSIZE);
  P("MAX_SIZET", MAX_SIZET); P("MAX_SIZE", MAX_SIZE); P("MAX_LMEM", MAX_LMEM); P("MAX_INT", MAX_INT);
  P("LUA_MAXINTEGER", LUA_MAXINTEGER); P("LUA_MINSTACK", LUA_MINSTACK); P("BASIC_STACK_SIZE", BASIC_STACK_SIZE);
  P("EXTRA_STACK", EXTRA_STACK); P("MINSTRTABSIZE", MINSTRTABSIZE); P("STRCACHE_N", STRCACHE_N); P("STRCACHE_M", STRCACHE_M);
  P("LUAI_GCPAUSE", LUAI_GCPAUSE); P("LUAI_GCMUL", LUAI_GCMUL); P("LUAI_GCSTEPSIZE", LUAI_GCSTEPSIZE);
  P("log2maxs(l_mem)", log2maxs(l_mem)); P("MAXUPVAL", MAXUPVAL); P("LUA_MINBUFFER", LUA_MINBUFFER);
  P("MAXABITS(int bits-1)", (int)(sizeof(int) * CHAR_BIT - 1)); P("MAXASIZE", luaM_limitN(1u << (sizeof(int)*CHAR_BIT-1), TValue));
  P("MAXHSIZE", luaM_limitN(1u << (sizeof(int)*CHAR_BIT-2), Node)); P("MAXSTRTB", cast_int(luaM_limitN(MAX_INT, TString*)));
  P("DBL_MAX_10_EXP", DBL_MAX_10_EXP); P("DBL_MANT_DIG", DBL_MANT_DIG); P("FLT_EVAL_METHOD", FLT_EVAL_METHOD);
  P("UINT_MAX", UINT_MAX); P("ULONG_MAX", ULONG_MAX); P("CHAR_MIN(signed char?)", (unsigned long long)(CHAR_MIN != 0));
  P("SIZE_MAX", SIZE_MAX); P("LUAI_MAXALIGN size", sizeof(union { LUAI_MAXALIGN; }));
  return 0;
}
