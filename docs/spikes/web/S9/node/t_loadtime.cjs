// S9 req 7 (e): load-to-first-frame in Chromium, median of 5, cold and warm cache.
// Marks (performance.now(), ms from navigation start = timeOrigin): js-start, wasm-ready
// (glue import + streaming compile + instantiate), gpu-ready (adapter/device/surface),
// bundle-fetched, bundle-loaded (zip parse + validation + physics build + rules on_init),
// renderer-built (visuals + pipelines), first-frame-submitted, first-frame-presented (next rAF).
// cold = a fresh persistent profile per run (empty disk HTTP cache and code cache); warm = 5 new
// navigations in one persistent profile after a priming load (disk HTTP cache + V8's wasm code
// cache). Persistent profiles, because the in-memory cache of an incognito-like context did not
// keep the 11 MB module (transferSize = full size on every load; measured in the first version).
// usage: node t_loadtime.cjs <backend> <out.json> [runs]
'use strict';
const os = require('os');
const path = require('path');
const { S9, startServer, FLAGS, ICD, waitReady, writeJson, poll } = require('./pw.cjs');
const fs = require('fs');
const { chromium } = require('playwright');

const MARKS = ['js-start', 'wasm-ready', 'gpu-ready', 'bundle-fetched', 'bundle-loaded', 'renderer-built', 'first-frame-submitted', 'first-frame-presented'];

async function one(page, url, reload) {
  if (reload) await page.reload(); else await page.goto(url);
  await waitReady(page);
  await poll(page, () => S9.state.marks['first-frame-presented'] !== undefined, null, 60000, 100);
  return page.evaluate(() => {
    const nav = performance.getEntriesByType('navigation')[0];
    const res = performance.getEntriesByType('resource');
    const pick = (s) => { const r = res.find((x) => x.name.includes(s)); return r ? { duration: +r.duration.toFixed(1), transferSize: r.transferSize, encodedBodySize: r.encodedBodySize, responseEnd: +r.responseEnd.toFixed(1) } : null; };
    return { marks: S9.state.marks, nav: { responseEnd: +nav.responseEnd.toFixed(1), domContentLoaded: +nav.domContentLoadedEventEnd.toFixed(1), load: +nav.loadEventEnd.toFixed(1), transferSize: nav.transferSize },
      wasm: pick('pin_web_bg.wasm'), glue: pick('pin_web.js'), bundle: pick('.pinbundle'), crossOriginIsolated: self.crossOriginIsolated, gpu: S9.gpuInfo().backend, mem: S9.memory() };
  });
}

const median = (a) => { const v = [...a].sort((x, y) => x - y); return v[Math.floor(v.length / 2)]; };

(async () => {
  const backend = process.argv[2] || 'webgpu';
  const out = process.argv[3] || path.join(S9, 'results', `loadtime_${backend}.json`);
  const runs = Number(process.argv[4] || 5);
  const srv = await startServer(9400 + Math.floor(Math.random() * 90));
  const url = `${srv.url}?backend=${backend}&table=apex_circuit&w=540&h=960&post=full&autostart=0`;
  const persistent = async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 's9-profile-'));
    const ctx = await chromium.launchPersistentContext(dir, { args: FLAGS.webgpu, env: { ...process.env, VK_ICD_FILENAMES: ICD, VK_DRIVER_FILES: ICD }, viewport: { width: 540, height: 960 } });
    const page = ctx.pages()[0] || await ctx.newPage();
    return { dir, ctx, page };
  };
  const res = { url, backend, runs, flags: FLAGS.webgpu, cold: [], warm: [], loadavg: [] };
  for (let i = 0; i < runs; i++) {
    const P = await persistent();
    res.chromium = P.ctx.browser() ? P.ctx.browser().version() : undefined;
    res.loadavg.push(os.loadavg()[0]);
    res.cold.push(await one(P.page, url, false));
    await P.ctx.close(); fs.rmSync(P.dir, { recursive: true, force: true });
  }
  {
    const P = await persistent();
    res.prime = await one(P.page, url, false);
    await P.page.waitForTimeout(8000); // let V8 finish tier-up and write the wasm code cache
    for (let i = 0; i < runs; i++) {
      await P.page.goto('about:blank');
      res.loadavg.push(os.loadavg()[0]);
      res.warm.push(await one(P.page, url, false));
      await P.page.waitForTimeout(3000);
    }
    await P.ctx.close(); fs.rmSync(P.dir, { recursive: true, force: true });
  }
  res.median = {};
  for (const k of ['cold', 'warm']) {
    res.median[k] = Object.fromEntries(MARKS.map((m) => [m, +median(res[k].map((r) => r.marks[m])).toFixed(1)]));
    res.median[k].wasm_fetch_ms = +median(res[k].map((r) => r.wasm.duration)).toFixed(1);
    res.median[k].wasm_transfer_bytes = median(res[k].map((r) => r.wasm.transferSize));
    res.median[k].bundle_fetch_ms = +median(res[k].map((r) => r.bundle ? r.bundle.duration : 0)).toFixed(1);
    res.median[k].bundle_transfer_bytes = median(res[k].map((r) => r.bundle ? r.bundle.transferSize : 0));
  }
  writeJson(out, res);
  console.log(JSON.stringify({ backend, median: res.median, loadavg: res.loadavg.map((x) => x.toFixed(2)) }, null, 1));
  srv.proc.kill();
})().catch((e) => { console.error(e); process.exit(2); });
