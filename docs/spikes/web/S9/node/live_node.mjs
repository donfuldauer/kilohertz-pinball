// S9: live-mode stepping in Node (no GPU): start press, then chunks of 50 ticks, timed.
import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';
import path from 'node:path';
const dist = path.resolve(process.argv[2]); const table = process.argv[3] || 'apex_circuit'; const ticks = Number(process.argv[4] || 20000);
const g = await import(pathToFileURL(path.join(dist, 'pkg', 'pin_web.js')).href);
g.initSync({ module: readFileSync(path.join(dist, 'pkg', 'pin_web_bg.wasm')) });
const e = new g.WebEngine();
console.log(e.load_bundle(new Uint8Array(readFileSync(path.join(dist, 'bundles', `${table}.pinbundle`))), 0x4b485a));
e.input('start', true);
let worst = 0; let worstAt = 0; const t0 = performance.now();
for (let t = 0; t < ticks; t += 50) {
  if (t === 150) e.input('start', false);
  if (t === 2000) e.input('launch', true);
  if (t === 2300) e.input('launch', false);
  const a = performance.now(); e.step(50); const d = performance.now() - a;
  if (d > worst) { worst = d; worstAt = t; }
  if (d > 50) { console.log(`slow chunk at tick ${t}: ${d.toFixed(1)} ms`, e.status()); break; }
}
console.log(`total ${(performance.now() - t0).toFixed(0)} ms worst chunk ${worst.toFixed(2)} ms at ${worstAt}`, e.status());
