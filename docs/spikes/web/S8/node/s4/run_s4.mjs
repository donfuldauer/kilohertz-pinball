// S8: Node driver for the S8 copy of S4's engine wasm (rules.wasm designs).
// usage: S4_PKG=<pkg dir> node run_s4.mjs <cap|0> play <design> <bundle> <scn> <out.hashes>
//        S4_PKG=<pkg dir> node run_s4.mjs <cap|0> s8adv <design> <mix> <edges> [reps]
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
const [cap, cmd, ...a] = process.argv.slice(2);
mod.s8_set_tick_cap(Number(cap));
if (cmd === 'play') {
  const r = mod.play(a[0], fs.readFileSync(a[1]), fs.readFileSync(a[2]), 1);
  const i = r.indexOf('\n');
  fs.writeFileSync(a[3], r.slice(i + 1));
  console.log(r.slice(0, i));
} else if (cmd === 's8adv') {
  process.stdout.write(mod.s8adv(a[0], a[1], Number(a[2]), Number(a[3] || 3)));
}
