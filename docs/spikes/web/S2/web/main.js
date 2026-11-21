// S2 page: feature detection (A25), real-Lua parity + per-callback timing,
// sandbox cases and stack probes in the browser. Result in window.__s2.
import init, * as m from './pkg/s2_harness.js';
const q = new URLSearchParams(location.search);
const b64 = (s) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
const EH_LEGACY = 'AGFzbQEAAAABCQJgAX8AYAABfwMCAQENAwEAAAcFAQFmAAAKDQELAAZ/QQcIAAcACwsACwRuYW1lCwQBAAF0';
const EH_EXNREF = 'AGFzbQEAAAABCQJgAX8AYAABfwMCAQENAwEAAAcFAQFmAAAKFAESAAJ/H0ABAAAAQQcIAAtBAAsLABMEbmFtZQMGAQABAAFoCwQBAAF0';
const log = (s) => { document.getElementById('log').textContent += '\n' + s; };
function probe(bytes) {
  const r = { validate: false, run: null };
  try { r.validate = WebAssembly.validate(bytes); } catch (e) { r.validate = 'ERR ' + e.message; }
  try { r.run = new WebAssembly.Instance(new WebAssembly.Module(bytes)).exports.f(); } catch (e) { r.run = 'ERR ' + e.message; }
  return r;
}
async function get(p) { const r = await fetch(p); if (!r.ok) throw new Error(p + ' ' + r.status); return new Uint8Array(await r.arrayBuffer()); }
(async () => {
  const out = { ua: navigator.userAgent, crossOriginIsolated: self.crossOriginIsolated, features: {}, results: {}, timing: {} };
  out.features.eh_legacy = probe(b64(EH_LEGACY));
  out.features.eh_exnref = probe(b64(EH_EXNREF));
  out.features.Tag = typeof WebAssembly.Tag; out.features.Exception = typeof WebAssembly.Exception; out.features.JSTag = typeof WebAssembly.JSTag;
  try {
    const t0 = performance.now();
    await init({ module_or_path: './pkg/s2_harness_bg.wasm' });
    out.init_ms = performance.now() - t0;
    out.layout = m.s2_stack_layout();
    const scen = (q.get('scen') || 'apex_circuit_bot60,sky_deck_bot60,feature_lab_bot60,apex_circuit_mb8,sky_deck_mb8,feature_lab_mb8').split(',').filter(Boolean);
    for (const s of scen) {
      const t = s.replace(/_[a-z0-9]+$/, '');
      const [bun, scn, scr, h] = await Promise.all([get('data/' + t + '.pinbundle'), get('data/' + s + '.scn'), get('data/' + s + '.scr'), get('data/' + s + '.hashes')]);
      const t1 = performance.now();
      const r = m.lua_play(bun, scn, scr, h, q.get('timing') !== '0');
      out.results[s] = { summary: r.split('\n')[0], wall_ms: performance.now() - t1 };
      const tl = r.split('\n').find((l) => l.startsWith('TIMING '));
      if (tl) out.timing[s] = JSON.parse(tl.slice(7));
      log(s + ': ' + out.results[s].summary.slice(0, 160));
    }
    if (q.get('sandbox') !== '0') {
      out.sandbox = m.sandbox('');
      out.sp_after_errors = m.sp_after_errors(20);
      out.case_stack = m.case_stack();
    }
  } catch (e) {
    out.error = String(e && e.stack || e);
  }
  window.__s2 = out;
  log('done');
})();
