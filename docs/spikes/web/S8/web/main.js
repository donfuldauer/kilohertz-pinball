// S8 page: runs the jobs listed in jobs.json (each {id, cmd, args}; an arg
// starting with '@' is fetched from data/) through s8_call; result in window.__s8.
import init, * as m from './pkg/s2_harness.js';
const log = (s) => { document.getElementById('log').textContent += '\n' + s; };
async function get(p) { const r = await fetch(p); if (!r.ok) throw new Error(p + ' ' + r.status); return new Uint8Array(await r.arrayBuffer()); }
(async () => {
  const out = { ua: navigator.userAgent, crossOriginIsolated: self.crossOriginIsolated, results: {} };
  try {
    await init({ module_or_path: './pkg/s2_harness_bg.wasm' });
    const jobs = JSON.parse(new TextDecoder().decode(await get('jobs.json')));
    const enc = new TextEncoder();
    for (const j of jobs) {
      const args = [];
      for (const a of j.args) args.push(a.startsWith('@') ? await get('data/' + a.slice(1)) : enc.encode(a));
      while (args.length < 6) args.push(new Uint8Array(0));
      try { out.results[j.id] = m.s8_call(j.cmd, ...args.slice(0, 6)); }
      catch (e) { out.results[j.id] = 'TRAP: ' + e; }
      log(j.id + ' done');
    }
  } catch (e) { out.error = String(e && e.stack || e); }
  window.__s8 = out;
  log('done');
})();
