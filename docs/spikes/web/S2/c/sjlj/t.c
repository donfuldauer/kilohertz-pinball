typedef struct { long long b[8]; } jmp_buf[1];
int setjmp(jmp_buf);
void longjmp(jmp_buf, int) __attribute__((noreturn));
extern void work(jmp_buf *b);
int run(void) {
  jmp_buf b;
  int r = setjmp(b);
  if (r == 0) { work(&b); return 0; }
  return r;
}
void thrower(jmp_buf *b) { longjmp(*b, 7); }
