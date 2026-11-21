// S10: shell port of S8/node/run.cjs + S8/harness/run_node.cjs (the S2/S8 Lua module,
// v2 = KHZ 29-37, wasm-bindgen --target nodejs glue, unchanged). Run after shellenv.js.
// usage: <shell> shellenv.js run_s8.js [--] <pkg_dir> <cmd> <args...>
//   s8 <cmd> <args...>          = S8/node/run.cjs (args starting with '@' are files)
//   sandbox [filter] | sandbox-describe | budget-edge | case-stack | layout | sp-after-errors <n>
//   format <n> | parse <n> | libm <fn> <n> | rust-libm <fn> <n> | ctype   (= S2/S8 harness/run_node.cjs)
//   play|play-timed <bundle> <scn> <scr> <native_hashes|->
(function () {
  const [pkg, cmd, ...a] = __args;
  const tLoad0 = nowMs();
  const m = loadCjs(pkg + '/s2_harness.js');
  const loadMs = nowMs() - tLoad0;
  const rd = (p) => readBinary(p);
  const enc = new TextEncoder();
  function guard(f) {
    try {
      return f();
    } catch (e) {
      print(`TRAP: ${e && e.constructor ? e.constructor.name : typeof e}: ${e && e.message}`);
      return undefined;
    }
  }
  const emit = (s) => { if (s !== undefined) out(s.endsWith('\n') ? s : s + '\n'); };
  switch (cmd) {
    case 's8': {
      const [c, ...rest] = a;
      const args = rest.map((x) => (x.startsWith('@') ? rd(x.slice(1)) : enc.encode(x)));
      while (args.length < 6) args.push(new Uint8Array(0));
      const r = guard(() => m.s8_call(c, ...args.slice(0, 6)));
      if (r !== undefined) out(r);
      break;
    }
    case 'play':
    case 'play-timed': {
      const h = a[3] && a[3] !== '-' ? rd(a[3]) : new Uint8Array(0);
      emit(guard(() => m.lua_play(rd(a[0]), rd(a[1]), rd(a[2]), h, cmd === 'play-timed')));
      break;
    }
    case 'sandbox': emit(guard(() => m.sandbox(a[0] ?? ''))); break;
    case 'sandbox-describe': emit(m.sandbox_describe()); break;
    case 'budget-edge': emit(guard(() => m.budget_edge())); break;
    case 'case-stack': emit(guard(() => m.case_stack())); break;
    case 'layout': emit(m.s2_stack_layout()); break;
    case 'format': emit(guard(() => m.format_dump(Number(a[0] ?? 10000)))); break;
    case 'parse': emit(guard(() => m.parse_dump(Number(a[0] ?? 10000)))); break;
    case 'libm': emit(guard(() => m.libm_dump(a[0], Number(a[1] ?? 100000)))); break;
    case 'rust-libm': emit(guard(() => m.rust_libm_dump(a[0], Number(a[1] ?? 100000)))); break;
    case 'ctype': emit(guard(() => m.ctype_dump())); break;
    case 'sp-after-errors': emit(guard(() => m.sp_after_errors(Number(a[0] ?? 100)))); break;
    case 'load': print(`load_ms=${loadMs.toFixed(1)}`); break;
    default: print('bad command ' + cmd);
  }
})();
