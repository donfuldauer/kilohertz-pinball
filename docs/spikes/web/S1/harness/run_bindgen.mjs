// Drives the wasm-bindgen (--target nodejs) build of s1_harness.
// usage: node run_bindgen.mjs <pkg_dir> <cmd> <args...>
//   play <bundle> <scn> <scr> <out_hashes>
//   bench <bundle> <scn> <scr> [batch] [warmup]
//   alloc <bundle> <scn> <scr>
//   dump <bundle> <scn> <scr> <tick>
//   probe <name> | recurse-big <n> | recurse-small <n>
//   highwater <bundle> <scn> <scr> | panic-in-step <bundle> [--hook]
//   panic-then-call <bundle>
import { createRequire } from 'node:module';
import { readFileSync, writeFileSync } from 'node:fs';
const require = createRequire(import.meta.url);
const [pkg, cmd, ...a] = process.argv.slice(2);
const m = require(`${pkg}/s1_harness.js`);
const rd = (p) => new Uint8Array(readFileSync(p));
if (process.env.S1_PANIC_HOOK === '1') m.install_panic_hook();
function guard(f) {
  try {
    return f();
  } catch (e) {
    console.log(`TRAP: ${e.constructor.name}: ${e.message}`);
    process.exitCode = 134;
    return undefined;
  }
}
switch (cmd) {
  case 'play': {
    const t0 = performance.now();
    const out = guard(() => m.play(rd(a[0]), rd(a[1]), rd(a[2])));
    if (out) {
      const nl = out.indexOf(10);
      writeFileSync(a[3], out.subarray(nl + 1));
      console.log(`${Buffer.from(out.subarray(0, nl)).toString()} wall_ms=${(performance.now() - t0).toFixed(0)}`);
    }
    break;
  }
  case 'bench':
    console.log(guard(() => m.bench(rd(a[0]), rd(a[1]), rd(a[2]), Number(a[3] ?? 16), Number(a[4] ?? 5000))));
    break;
  case 'alloc':
    console.log(guard(() => m.alloc_check(rd(a[0]), rd(a[1]), rd(a[2]))));
    break;
  case 'dump':
    process.stdout.write(guard(() => m.dump(rd(a[0]), rd(a[1]), rd(a[2]), Number(a[3]))) ?? '');
    break;
  case 'probe':
    console.log(`${a[0]}: ${guard(() => m.probe(a[0]))}`);
    break;
  case 'recurse-big':
    console.log(`recurse_big(${a[0]}) = ${guard(() => m.recurse_big(Number(a[0])))}`);
    break;
  case 'recurse-small':
    console.log(`recurse_small(${a[0]}) = ${guard(() => m.recurse_small(Number(a[0])))}`);
    break;
  case 'stack-phases':
    console.log(guard(() => m.stack_phases(rd(a[0]), rd(a[1]), rd(a[2]))));
    break;
  case 'highwater':
    console.log(guard(() => m.highwater(rd(a[0]), rd(a[1]), rd(a[2]))));
    break;
  case 'panic-in-step':
    if (a[1] === '--hook') m.install_panic_hook();
    console.log(guard(() => m.panic_in_step(rd(a[0]))));
    break;
  case 'panic-then-call': {
    // After a trap: is the instance still usable?
    console.log(`first call: ${guard(() => m.panic_in_step(rd(a[0])))}`);
    console.log(`next call probe(mutex): ${guard(() => m.probe('mutex'))}`);
    const traps = [];
    for (let i = 0; i < 2000; i++) {
      try { m.panic_in_step(rd(a[0])); } catch (e) { traps.push(e.message); }
    }
    console.log(`2000 more panicking calls: ${traps.length} traps; last: ${traps.at(-1)}`);
    console.log(`after them probe(stack_layout): ${guard(() => m.probe('stack_layout'))}`);
    break;
  }
  default:
    console.log('bad command');
    process.exitCode = 2;
}
