// S1 browser run: per-tick hash parity against the native files and tick
// timing with performance.now(). Results land in window.__s1.
import init, * as m from './pkg/s1_harness.js';
const log = (s) => { document.getElementById('log').textContent += s + '\n'; };
const get = async (p) => new Uint8Array(await (await fetch(p)).arrayBuffer());
const text = async (p) => await (await fetch(p)).text();
const params = new URLSearchParams(location.search);
const scenarios = (params.get('scn') || 'apex_circuit_bot60,sky_deck_bot60,feature_lab_bot60').split(',');
const doBench = params.get('bench') !== '0';
const out = { userAgent: navigator.userAgent, crossOriginIsolated: self.crossOriginIsolated,
  sharedArrayBuffer: typeof SharedArrayBuffer, simd: false, results: {} };
try {
  // Timer resolution: smallest non-zero step of performance.now().
  let minStep = Infinity, prev = performance.now();
  for (let i = 0; i < 200000; i++) { const t = performance.now(); if (t > prev) { minStep = Math.min(minStep, t - prev); prev = t; } }
  out.performanceNowMinStepUs = minStep * 1000;
  // Main-thread timer granularity: nested setTimeout(0) and a MessageChannel loop.
  const chain = (n, sched) => new Promise((res) => { const t0 = performance.now(); let i = 0;
    const step = () => { if (++i >= n) res((performance.now() - t0) / n); else sched(step); }; sched(step); });
  out.setTimeout0_mean_ms = await chain(200, (f) => setTimeout(f, 0));
  const mc = new MessageChannel(); let pending = null; mc.port1.onmessage = () => { const p = pending; pending = null; p(); };
  out.messageChannel_mean_ms = await chain(2000, (f) => { pending = f; mc.port2.postMessage(0); });
  out.simd = WebAssembly.validate(new Uint8Array([0,97,115,109,1,0,0,0,1,5,1,96,0,1,123,3,2,1,0,10,10,1,8,0,65,0,253,15,253,98,11]));
  await init();
  for (const s of scenarios) {
    const table = s.replace(/_(bot|pat|mb)\d+$/, '');
    const [b, scn, scr] = await Promise.all([get(`data/${table}.pinbundle`), get(`data/${s}.scn`), get(`data/${s}.scr`)]);
    const ref = await text(`data/${s}.native.hashes`);
    const t0 = performance.now();
    const res = m.play(b, scn, scr);
    const playMs = performance.now() - t0;
    const all = new TextDecoder().decode(res);
    const nl = all.indexOf('\n');
    const summary = all.slice(0, nl), hashes = all.slice(nl + 1);
    const a = ref.trimEnd().split('\n'), c = hashes.trimEnd().split('\n');
    let first = null;
    for (let i = 0; i < Math.max(a.length, c.length); i++) if (a[i] !== c[i]) { first = { line: i, native: a[i], browser: c[i] }; break; }
    const r = { summary, playMs, lines: c.length, identical: first === null, firstDivergence: first };
    if (doBench) r.bench = JSON.parse(m.bench(b, scn, scr, 16, 5000));
    out.results[s] = r;
    log(`${s}: identical=${r.identical} ${summary}`);
  }
} catch (e) {
  out.error = String(e && e.stack || e);
  log(out.error);
}
window.__s1 = out;
