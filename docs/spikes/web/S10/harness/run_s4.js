// S10: shell port of S4/node/run_web.mjs (+ run_stack.mjs, S8/node/s4/run_s4.mjs) for the
// S4 engine wasm (wasmi + instrumentation; --target web ES-module glue, unchanged,
// loaded with a dynamic import). Run after shellenv.js.
// usage: <shell> shellenv.js run_s4.js [--] <pkg_dir> <khz_rules.js> <cmd> <args...>
//   play <design> <bundle> <scn> <out.hashes> [every]
//   adv <design> <prod.wasm> | case <design> <name> <prod.wasm>
//   stack <design> [reps] | bench <design> <bundle> <scn> | worst <design> [reps] [case]
//   loadtime <design> <rules.wasm> [reps]
//   capplay <cap> <design> <bundle> <scn> <out.hashes>   (S8 copy only: s8_set_tick_cap)
//   s8adv <cap> <design> <mix> <edges> [reps]               (S8 copy only)
(function () {
  const [pkg, glue, cmd, ...a] = __args;
  load(glue);
  const tImp0 = nowMs();
  import(pkg + '/s4_rules.js').then((mod) => {
    const raw = mod.initSync({ module: readBinary(pkg + '/s4_rules_bg.wasm') });
    const loadMs = nowMs() - tImp0;
    globalThis.khzRules.sim = raw;
    mod.install_panic_hook();
    const rd = (p) => readBinary(p);
    const rulesOf = (bundlePath) => rd(bundlePath.replace(/_wasm\.pinbundle$/, '') + '/rules.wasm');
    try {
      switch (cmd) {
        case 'play': {
          const r = mod.play(a[0], rd(a[1]), rd(a[2]), Number(a[4] || 1));
          const nl = r.indexOf('\n');
          writeText(a[3], r.slice(nl + 1));
          print(r.slice(0, nl) + ' mem_bytes=' + raw.memory.buffer.byteLength + ' load_ms=' + loadMs.toFixed(1));
          break;
        }
        case 'capplay': {
          mod.s8_set_tick_cap(Number(a[0]));
          const r = mod.play(a[1], rd(a[2]), rd(a[3]), 1);
          const nl = r.indexOf('\n');
          writeText(a[4], r.slice(nl + 1));
          print(r.slice(0, nl));
          break;
        }
        case 's8adv':
          mod.s8_set_tick_cap(Number(a[0]));
          out(mod.s8adv(a[1], a[2], Number(a[3]), Number(a[4] || 3)));
          break;
        case 'adv': out(mod.adversarial(a[0], rd(a[1]))); break;
        case 'case': print(mod.adv_case(a[0], a[1], rd(a[2]))); break;
        case 'stack': out(mod.stack_probe(a[0], Number(a[1] || 2))); break;
        case 'bench': print(mod.bench(a[0], rd(a[1]), rd(a[2]))); break;
        case 'worst': out(mod.worst(a[0], Number(a[1] || 5), a[2] || '')); break;
        case 'loadtime': print(mod.load_time(a[0], rd(a[1]), Number(a[2] || 20)) + ' ' + JSON.stringify(globalThis.khzRules.stats)); break;
        case 'load': print('load_ms=' + loadMs.toFixed(1)); break;
        default: print('bad command ' + cmd);
      }
    } catch (e) {
      print('ERROR ' + (e && e.constructor ? e.constructor.name : typeof e) + ': ' + (e && e.message));
    }
  }).catch((e) => print('ERROR import ' + e + ' ' + (e && e.stack)));
})();
