// S9: the S1/S2/S4 replay-parity checks against the COMBINED module, in Node (no GPU).
// usage: node parity_node.mjs <dist> [scenario-substring|all] [lua,wasm] [pkgdir]
// For each scenario and rules backend: load the bundle (lua: <t>.pinbundle with S1's .scr
// stream checked per script step by S2's checker; wasm: <t>_wasm.pinbundle through wasmi +
// S4's instrumentation), replay S1's recorded scenario, compare every tick with the native
// reference (lua: S1 native state_hash+ext_hash; wasm: S4 native wasmtime engine path
// state+ext+command-queue hash). One PASS/FAIL line per run; exit 1 on any FAIL.
import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';
import path from 'node:path';

const dist = path.resolve(process.argv[2]);
const filt = process.argv[3] || 'all';
const kinds = (process.argv[4] || 'lua,wasm').split(',');
const pkg = path.resolve(process.argv[5] || path.join(dist, 'pkg'));
const glue = await import(pathToFileURL(path.join(pkg, 'pin_web.js')).href);
glue.initSync({ module: readFileSync(path.join(pkg, 'pin_web_bg.wasm')) });
const T = ['apex_circuit', 'sky_deck', 'feature_lab'];
let scen = T.flatMap((t) => ['bot60', 'pat60', 'mb8'].map((k) => `${t}_${k}`)).concat(['apex_circuit_bot600']);
if (filt !== 'all') scen = scen.filter((s) => s.includes(filt));
const rd = (p) => new Uint8Array(readFileSync(path.join(dist, p)));
const tableOf = (s) => T.find((t) => s.startsWith(t));
let fails = 0;
const eng = new glue.WebEngine();
for (const s of scen) {
  for (const k of kinds) {
    const t = tableOf(s);
    const bundle = rd(k === 'wasm' ? `bundles/${t}_wasm.pinbundle` : `bundles/${t}.pinbundle`);
    const scr = k === 'lua' ? rd(`test/rec/${s}.scr`) : undefined;
    const t0 = performance.now();
    try {
      eng.load_replay(bundle, rd(`test/rec/${s}.scn`), scr, rd(`test/refs/${s}.${k}.ref`));
      while (!eng.replay_done()) eng.step(10000);
    } catch (e) {
      fails += 1;
      console.log(`FAIL ${s} ${k} error=${e}`);
      continue;
    }
    const ms = performance.now() - t0;
    const st = JSON.parse(eng.status());
    const r = st.replay;
    const ticksOk = r.compared === r.ticks + 1 && r.beyond === 0;
    const hashOk = r.state_bad === 0 && r.ext_bad === 0 && r.cmd_bad === 0;
    let streamOk = true;
    let extra = '';
    if (k === 'lua') {
      const l = st.lua_stream;
      streamOk = l.steps > 0 && l.exhausted === 0 && l.tick_mismatch + l.draw_mismatch + l.digest_mismatch + l.queue_mismatch === 0;
      extra = `lua_steps=${l.steps} lua_queue_bytes=${l.queue_bytes} lua_first_mismatch=${l.first_mismatch}`;
    } else {
      const w = st.wasm_rules;
      extra = `callbacks=${w.callbacks} max_fuel=${w.max_fuel} fuel_digest=${w.fuel_digest}`;
    }
    const ok = ticksOk && hashOk && streamOk && !st.faulted;
    if (!ok) fails += 1;
    console.log(`${ok ? 'PASS' : 'FAIL'} ${s} ${k} ticks=${st.tick} compared=${r.compared} cols=${r.ref_cols} ` +
      `state_bad=${r.state_bad} ext_bad=${r.ext_bad} cmd_bad=${r.cmd_bad} first_bad=${r.first_bad} ` +
      `final_state=${st.state_hash} final_ext=${st.ext_hash} stream=${st.stream} max_balls=${st.max_balls} ` +
      `faulted=${st.faulted} ${extra} wall_ms=${ms.toFixed(0)}`);
  }
}
console.log(`== parity_node: ${fails} failing line(s)`);
process.exit(fails ? 1 : 0);
