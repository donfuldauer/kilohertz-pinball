// Stack probe in Node: S4_PKG=<pkg> node run_stack.mjs <design> [reps]
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
const require = createRequire(import.meta.url);
require('./khz_rules.js');
const pkg = process.env.S4_PKG;
const mod = await import(path.join(pkg, 's4_rules.js'));
const raw = mod.initSync({ module: fs.readFileSync(path.join(pkg, 's4_rules_bg.wasm')) });
globalThis.khzRules.sim = raw;
const [design, reps] = process.argv.slice(2);
process.stdout.write(mod.stack_probe(design, Number(reps || 2)));
