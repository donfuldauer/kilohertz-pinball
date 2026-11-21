// Drives the wasm-bindgen (--target nodejs) build of s2_harness.
// usage: node run_node.cjs <pkg_dir> <cmd> <args...>
//   play|play-timed <bundle> <scn> <scr> <native_hashes|->
//   sandbox [filter] | sandbox-describe | format <n> | parse <n> | libm <fn> <n> | rust-libm <fn> <n>
//   ctype | sp-after-errors <n> | case-stack | smoke <src> <ticks> | layout
const fs = require('node:fs');
const [pkg, cmd, ...a] = process.argv.slice(2);
const m = require(require("node:path").resolve(pkg, "s2_harness.js"));
const rd = (p) => new Uint8Array(fs.readFileSync(p));
if (process.env.S2_PANIC_HOOK === '1') m.s2_panic_hook();
function guard(f) {
  try {
    return f();
  } catch (e) {
    console.log(`TRAP: ${e.constructor.name}: ${e.message}`);
    process.exitCode = 134;
    return undefined;
  }
}
const out = (s) => { if (s !== undefined) process.stdout.write(s.endsWith('\n') ? s : s + '\n'); };
switch (cmd) {
  case 'play':
  case 'play-timed': {
    const h = a[3] && a[3] !== '-' ? rd(a[3]) : new Uint8Array(0);
    out(guard(() => m.lua_play(rd(a[0]), rd(a[1]), rd(a[2]), h, cmd === 'play-timed')));
    break;
  }
  case 'sandbox': out(guard(() => m.sandbox(a[0] ?? ''))); break;
  case 'sandbox-describe': out(m.sandbox_describe()); break;
  case 'budget-edge': out(guard(() => m.budget_edge())); break;
  case 'format': out(guard(() => m.format_dump(Number(a[0] ?? 10000)))); break;
  case 'parse': out(guard(() => m.parse_dump(Number(a[0] ?? 10000)))); break;
  case 'libm': out(guard(() => m.libm_dump(a[0], Number(a[1] ?? 100000)))); break;
  case 'rust-libm': out(guard(() => m.rust_libm_dump(a[0], Number(a[1] ?? 100000)))); break;
  case 'ctype': out(guard(() => m.ctype_dump())); break;
  case 'sp-after-errors': out(guard(() => m.sp_after_errors(Number(a[0] ?? 100)))); break;
  case 'case-stack': out(guard(() => m.case_stack())); break;
  case 'smoke': out(guard(() => m.lua_smoke(a[0], Number(a[1] ?? 1)))); break;
  case 'layout': out(m.s2_stack_layout()); break;
  case 'play-inject': case 'play-remote': break;
  default: console.log('bad command'); process.exitCode = 2;
}
if (cmd === 'play-inject') {
  // play-inject <bundle> <scn> <scr> <native_hashes> <inject.lua>
  out(guard(() => m.lua_play_inject(rd(a[0]), rd(a[1]), rd(a[2]), rd(a[3]), fs.readFileSync(a[4], 'utf8'))));
}
if (cmd === 'play-remote') {
  // Route (f): play-remote <lua_module.wasm> <bundle> <scn> <scr> <native_hashes>  (S2_TIMING=1 for timing)
  const L = new WebAssembly.Instance(new WebAssembly.Module(fs.readFileSync(a[0])), {}).exports;
  const emem = m.s2_memory();
  globalThis.s2_remote_new = (ptr, sl, nl) => {
    const lb = L.l_buf() >>> 0;
    new Uint8Array(L.memory.buffer).set(new Uint8Array(emem.buffer, ptr >>> 0, sl + nl), lb);
    return L.l_new(sl, nl);
  };
  globalThis.s2_remote_call = (k, sw, c, ptr, len) => {
    try {
      const lb = L.l_buf() >>> 0;
      new Uint8Array(L.memory.buffer).set(new Uint8Array(emem.buffer, ptr >>> 0, len), lb);
      const n = L.l_call(k, sw, c);
      new Uint8Array(emem.buffer).set(new Uint8Array(L.memory.buffer, lb, n), ptr >>> 0);
      return n;
    } catch (e) {
      console.error(`lua instance trapped: ${e}`);
      return 0;
    }
  };
  out(guard(() => m.lua_play_remote(rd(a[1]), rd(a[2]), rd(a[3]), rd(a[4]), process.env.S2_TIMING === '1')));
}
