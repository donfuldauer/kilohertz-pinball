// S10: shell port of S1/harness/run_bindgen.mjs for JSC and SpiderMonkey.
// Same glue (pkg_dir/s1_harness.js, --target nodejs) and wasm bytes as the V8 runs;
// only the loader differs (shellenv.js loadCjs). Run after shellenv.js.
// usage: <shell> shellenv.js run_s1.js [--] <pkg_dir> <cmd> <args...>
//   play <bundle> <scn> <scr> <out_hashes>
//   bench <bundle> <scn> <scr> [batch] [warmup]
//   dump <bundle> <scn> <scr> <tick> | probe <name> | recurse-big <n> | recurse-small <n>
//   stack-phases <bundle> <scn> <scr> | alloc <bundle> <scn> <scr>
(function () {
  const [pkg, cmd, ...a] = __args;
  const tLoad0 = nowMs();
  const m = loadCjs(pkg + '/s1_harness.js');
  const loadMs = nowMs() - tLoad0;
  const rd = (p) => readBinary(p);
  function guard(f) {
    try {
      return f();
    } catch (e) {
      print(`TRAP: ${e && e.constructor ? e.constructor.name : typeof e}: ${e && e.message}`);
      return undefined;
    }
  }
  switch (cmd) {
    case 'play': {
      const t0 = nowMs();
      const out = guard(() => m.play(rd(a[0]), rd(a[1]), rd(a[2])));
      if (out) {
        const nl = out.indexOf(10);
        writeBytes(a[3], out.subarray(nl + 1));
        print(`${__utf8Decode(out.subarray(0, nl))} wall_ms=${(nowMs() - t0).toFixed(0)} load_ms=${loadMs.toFixed(1)}`);
      }
      break;
    }
    case 'bench':
      print(guard(() => m.bench(rd(a[0]), rd(a[1]), rd(a[2]), Number(a[3] ?? 16), Number(a[4] ?? 5000))));
      break;
    case 'alloc':
      print(guard(() => m.alloc_check(rd(a[0]), rd(a[1]), rd(a[2]))));
      break;
    case 'dump':
      out(guard(() => m.dump(rd(a[0]), rd(a[1]), rd(a[2]), Number(a[3]))) ?? '');
      break;
    case 'probe':
      print(`${a[0]}: ${guard(() => m.probe(a[0]))}`);
      break;
    case 'recurse-big':
      print(`recurse_big(${a[0]}) = ${guard(() => m.recurse_big(Number(a[0])))}`);
      break;
    case 'recurse-small':
      print(`recurse_small(${a[0]}) = ${guard(() => m.recurse_small(Number(a[0])))}`);
      break;
    case 'stack-phases':
      print(guard(() => m.stack_phases(rd(a[0]), rd(a[1]), rd(a[2]))));
      break;
    case 'load':
      print(`load_ms=${loadMs.toFixed(1)}`);
      break;
    default:
      print('bad command ' + cmd);
  }
})();
