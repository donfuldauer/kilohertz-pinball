// S4 Node driver for the engine wasm (wasm-bindgen --target web output).
// usage: S4_PKG=<pkg dir> node run_web.mjs <cmd> args...
//   play <design> <bundle> <scn> <out.hashes> [every]
//   bench <design> <bundle> <scn>
//   alloc <design> <bundle> <scn>
//   adv <design> <prod.wasm>
//   case <design> <name> <prod.wasm>
//   worst <design> [reps] [case]
//   loadtime <design> <rules.wasm> [reps]
//   zip <file> [web]
//   mode=cache before <cmd>: async-precompile rules modules (JIT designs)
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
const require = createRequire(import.meta.url);
require('./khz_rules.js');

const pkg = process.env.S4_PKG;
const mod = await import(path.join(pkg, 's4_rules.js'));
const raw = mod.initSync({ module: fs.readFileSync(path.join(pkg, 's4_rules_bg.wasm')) });
globalThis.khzRules.sim = raw;
mod.install_panic_hook();

let args = process.argv.slice(2);
if (args[0] === 'mode=cache') { globalThis.khzRules.mode = 'cache'; args = args.slice(1); }
const [cmd, ...a] = args;
const rd = (p) => fs.readFileSync(p);

async function precompile(design, src, depth) {
  // The async route: instrument in the engine module, compile with
  // WebAssembly.compile (off the main thread in browsers), cache.
  if (globalThis.khzRules.mode !== 'cache') return;
  const bytes = mod.instrument_jit(src, depth);
  await globalThis.khzRules.precompile(bytes);
}

function rulesOf(bundlePath) {
  // The bundle's rules.wasm sits next to the zip as <name>/rules.wasm.
  const dir = bundlePath.replace(/_wasm\.pinbundle$/, '');
  return rd(path.join(dir, 'rules.wasm'));
}

try {
  switch (cmd) {
    case 'play': {
      if (a[0].startsWith('jit')) await precompile(a[0], rulesOf(a[1]), a[0] === 'jit-depth');
      const out = mod.play(a[0], rd(a[1]), rd(a[2]), Number(a[4] || 1));
      const nl = out.indexOf('\n');
      fs.writeFileSync(a[3], out.slice(nl + 1));
      console.log(out.slice(0, nl), 'mem_bytes=' + raw.memory.buffer.byteLength);
      break;
    }
    case 'bench': {
      if (a[0].startsWith('jit')) await precompile(a[0], rulesOf(a[1]), a[0] === 'jit-depth');
      console.log(mod.bench(a[0], rd(a[1]), rd(a[2])));
      break;
    }
    case 'alloc': console.log(mod.alloc_check(a[0], rd(a[1]), rd(a[2]))); break;
    case 'adv': process.stdout.write(mod.adversarial(a[0], rd(a[1]))); break;
    case 'case': console.log(mod.adv_case(a[0], a[1], rd(a[2]))); break;
    case 'worst': process.stdout.write(mod.worst(a[0], Number(a[1] || 5), a[2] || '')); break;
    case 'loadtime': {
      if (a[0].startsWith('jit')) await precompile(a[0], rd(a[1]), a[0] === 'jit-depth');
      console.log(mod.load_time(a[0], rd(a[1]), Number(a[2] || 20)), 'mode=' + globalThis.khzRules.mode, JSON.stringify(globalThis.khzRules.stats));
      break;
    }
    case 'zip': {
      const before = raw.memory.buffer.byteLength;
      const r = mod.zip_check(rd(a[0]), a[1] === 'web');
      console.log(r, `wasm_mem_before=${before} wasm_mem_after=${raw.memory.buffer.byteLength}`);
      break;
    }
    default: console.error('unknown command'); process.exit(2);
  }
} catch (e) {
  console.error('ERROR', e && e.stack || e);
  process.exit(1);
}
