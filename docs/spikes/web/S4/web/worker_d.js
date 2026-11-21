// Design D probe: a rules module in its own Worker, called synchronously from
// a "sim" Worker over SharedArrayBuffer + Atomics.wait (main thread may not
// wait), with worker.terminate() as the watchdog. Host calls go back to the
// sim worker over the same SAB. Needs crossOriginIsolated (COOP/COEP).
const IMPORTS = ['pulse_coil', 'hold_coil', 'set_magnet', 'set_zipper', 'set_diverter', 'spawn_ball', 'set_lamp',
  'set_gi_tint', 'set_score', 'show_scene', 'play_sound', 'persist_set', 'persist_get', 'switch', 'tick',
  'balls_in_play', 'random', 'id_of'];
const rulesSrc = `
const IMPORTS = ${JSON.stringify(IMPORTS)};
let ctl, f64, inst;
function hostCall(k, args) {
  for (let i = 0; i < args.length; i++) f64[2 + i] = Number(args[i]);
  ctl[1] = k; Atomics.store(ctl, 0, 2); Atomics.notify(ctl, 0);
  while (Atomics.load(ctl, 0) === 2) Atomics.wait(ctl, 0, 2);
  return f64[1];
}
onmessage = async (e) => {
  if (e.data.init) {
    ctl = new Int32Array(e.data.sab); f64 = new Float64Array(e.data.sab, 64);
    const pinball = {};
    IMPORTS.forEach((n, k) => {
      pinball[n] = (...a) => { const r = hostCall(k, a); return (n === 'tick' || n === 'persist_get' || n === 'random') ? BigInt(Math.trunc(r)) : r; };
    });
    const m = await WebAssembly.compile(e.data.bytes);
    inst = await WebAssembly.instantiate(m, { pinball });
    postMessage('ready');
    for (;;) {
      Atomics.wait(ctl, 0, 0);
      if (Atomics.load(ctl, 0) !== 1) continue;
      const entry = ctl[1];
      let st = 0;
      try { if (entry === 2) inst.exports.on_switch_edge(ctl[2], ctl[3]); else (entry ? inst.exports.on_tick : inst.exports.on_init)(); }
      catch (err) { st = 1; }
      ctl[1] = st; Atomics.store(ctl, 0, 4); Atomics.notify(ctl, 0);
      while (Atomics.load(ctl, 0) === 4) Atomics.wait(ctl, 0, 4);
    }
  }
};`;
const simSrc = `
let ctl, f64, tick = 0, hostCalls = 0;
const names = {};
function call(entry, a, b, timeoutMs) {
  ctl[1] = entry; ctl[2] = a; ctl[3] = b;
  Atomics.store(ctl, 0, 1); Atomics.notify(ctl, 0);
  const t0 = performance.now();
  for (;;) {
    const s = Atomics.load(ctl, 0);
    if (s === 4) { Atomics.store(ctl, 0, 0); Atomics.notify(ctl, 0); return 'ok'; }
    if (s === 2) {
      hostCalls++;
      const k = ctl[1];
      // host stubs: tick() -> tick, id_of -> 0, persist_get -> 0, others -> 0
      f64[1] = k === 14 ? tick : 0;
      Atomics.store(ctl, 0, 3); Atomics.notify(ctl, 0);
      continue;
    }
    const left = timeoutMs - (performance.now() - t0);
    if (left <= 0) return 'timeout';
    Atomics.wait(ctl, 0, s, left);
  }
}
onmessage = (e) => {
  const d = e.data;
  if (d.init) { ctl = new Int32Array(d.sab); f64 = new Float64Array(d.sab, 64); postMessage('ready'); return; }
  if (d.bench) {
    const samples = [];
    let r = call(0, 0, 0, 1000);
    for (let i = 0; i < d.n; i++) {
      tick = i + 1;
      const t0 = performance.now();
      r = call(1, 0, 0, d.timeoutMs);
      samples.push(performance.now() - t0);
      if (r !== 'ok') break;
    }
    postMessage({ samples, r, hostCalls });
  }
  if (d.hang) {
    const t0 = performance.now();
    const r = call(1, 0, 0, d.timeoutMs);
    postMessage({ r, waitedMs: performance.now() - t0 });
  }
};`;
const blobUrl = (s) => URL.createObjectURL(new Blob([s], { type: 'text/javascript' }));
function stats(v) { v = v.slice().sort((a, b) => a - b); const q = (p) => v[Math.round((v.length - 1) * p)]; return { n: v.length, mean: v.reduce((a, b) => a + b, 0) / v.length, p50: q(0.5), p99: q(0.99), max: v[v.length - 1] }; }
async function startPair(bytes) {
  const sab = new SharedArrayBuffer(4096);
  const rules = new Worker(blobUrl(rulesSrc));
  const sim = new Worker(blobUrl(simSrc));
  const ready = (w) => new Promise((ok) => { w.onmessage = (e) => ok(e.data); });
  const r1 = ready(rules); rules.postMessage({ init: true, sab, bytes });
  const r2 = ready(sim); sim.postMessage({ init: true, sab });
  await Promise.all([r1, r2]);
  return { rules, sim, sab };
}
window.s4 = {
  async run(cmd, a) {
    const out = [`crossOriginIsolated=${self.crossOriginIsolated} SharedArrayBuffer=${typeof SharedArrayBuffer}`];
    try { Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 1); out.push('main-thread Atomics.wait: allowed'); }
    catch (e) { out.push(`main-thread Atomics.wait: ${e.constructor.name}: ${e.message}`); }
    for (const t of ['apex_circuit', 'sky_deck', 'feature_lab']) {
      const bytes = new Uint8Array(await (await fetch(`/repo/tables/${t}/rules.wasm`)).arrayBuffer());
      const p = await startPair(bytes);
      const res = await new Promise((ok) => { p.sim.onmessage = (e) => ok(e.data); p.sim.postMessage({ bench: true, n: a.n || 5000, timeoutMs: 50 }); });
      const s = stats(res.samples.map((x) => x * 1000));
      out.push(`${t}: on_tick via worker round trip us mean=${s.mean.toFixed(2)} p50=${s.p50.toFixed(1)} p99=${s.p99.toFixed(1)} max=${s.max.toFixed(1)} n=${s.n} host_calls=${res.hostCalls} last=${res.r} (production rules init fails id_of name lookups with stub hosts; on_tick timing only)`);
      p.rules.terminate(); p.sim.terminate();
    }
    // Watchdog: a module that never returns; the sim worker's wait times out, main terminates the rules worker.
    const hang = new Uint8Array([0,97,115,109,1,0,0,0,1,9,2,96,0,0,96,2,127,127,0,3,4,3,0,0,1,5,3,1,0,1,7,47,4,6,109,101,109,111,114,121,2,0,7,111,110,95,105,110,105,116,0,0,7,111,110,95,116,105,99,107,0,1,14,111,110,95,115,119,105,116,99,104,95,101,100,103,101,0,2,10,15,3,2,0,11,7,0,3,64,12,0,11,11,2,0,11,0,13,4,110,97,109,101,3,6,1,1,1,0,1,108]);
    const p = await startPair(hang);
    const r = await new Promise((ok) => { p.sim.onmessage = (e) => ok(e.data); p.sim.postMessage({ hang: true, timeoutMs: a.timeoutMs || 20 }); });
    const t0 = performance.now();
    p.rules.terminate();
    out.push(`hang: sim worker wait returned '${r.r}' after ${r.waitedMs.toFixed(1)}ms; rules worker terminated by main (terminate() call took ${(performance.now() - t0).toFixed(2)}ms; the sim worker must re-create the rules worker and the rules instance state is lost)`);
    p.sim.terminate();
    return out.join('\n');
  },
};
window.s4ready = true;
