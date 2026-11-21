// S9 req 6 (c): render on WebGPU and on WebGL2 in one build; backend selection
// (?backend=webgpu|webgl2|auto); auto -> WebGPU when an adapter exists, WebGL2 when
// WebGPU is unavailable (no adapter, or no navigator.gpu at all). Screenshots of the
// same simulated tick on both backends, non-blank statistics, tolerant cross-backend
// comparison, console/page/GPU errors. usage: node t_backends.cjs <out.json> [W H]
'use strict';
const path = require('path');
const { S9, startServer, launch, watch, waitReady, writeJson, decodePng, pixelStats, compare, poll } = require('./pw.cjs');

const TABLES = ['apex_circuit', 'sky_deck', 'feature_lab'];

async function jsAdapter(page) {
  return page.evaluate(async () => {
    if (!('gpu' in navigator) || !navigator.gpu) return { navigator_gpu: false };
    const a = await navigator.gpu.requestAdapter();
    if (!a) return { navigator_gpu: true, adapter: null };
    const i = a.info || {};
    return { navigator_gpu: true, adapter: { vendor: i.vendor, architecture: i.architecture, device: i.device, description: i.description,
      isFallbackAdapter: i.isFallbackAdapter, subgroupMinSize: i.subgroupMinSize }, features: [...a.features].sort(),
      maxTextureDimension2D: a.limits.maxTextureDimension2D, maxStorageBuffersPerShaderStage: a.limits.maxStorageBuffersPerShaderStage,
      preferredCanvasFormat: navigator.gpu.getPreferredCanvasFormat() };
  });
}

async function webgl2Info(page) {
  return page.evaluate(() => {
    const c = document.createElement('canvas');
    const gl = c.getContext('webgl2');
    if (!gl) return null;
    const d = gl.getExtension('WEBGL_debug_renderer_info');
    return { version: gl.getParameter(gl.VERSION), renderer: d ? gl.getParameter(d.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER),
      ext_color_buffer_float: !!gl.getExtension('EXT_color_buffer_float'), max_samples: gl.getParameter(gl.MAX_SAMPLES) };
  });
}

(async () => {
  const out = process.argv[2] || path.join(S9, 'results', 'backends.json');
  const W = Number(process.argv[3] || 540); const H = Number(process.argv[4] || 960);
  const srv = await startServer(8900 + Math.floor(Math.random() * 90));
  const res = { size: [W, H], pages: [], shots: {}, compare: {}, auto: {} };
  let fails = 0;
  const check = (name, cond, detail) => { res.pages.push({ check: name, ok: !!cond, detail }); if (!cond) fails += 1; console.log(`${cond ? 'PASS' : 'FAIL'} ${name} ${detail ? JSON.stringify(detail).slice(0, 400) : ''}`); };

  // 1) Explicit backends, the S3 flag set (WebGPU adapter + WebGL2 available).
  const L = await launch('webgpu');
  res.chromium = L.version; res.flags = L.args; res.icd = L.icd;
  const imgs = {};
  for (const backend of ['webgpu', 'webgl2']) {
    const page = await L.browser.newPage({ viewport: { width: W, height: H } });
    const log = watch(page);
    await page.goto(`${srv.url}?mode=idle&backend=${backend}&w=${W}&h=${H}&post=full`);
    await waitReady(page);
    const gpuInfo = await page.evaluate(() => S9.gpuInfo());
    const adapter = await jsAdapter(page);
    const gl = await webgl2Info(page);
    res.auto[`explicit_${backend}`] = { gpuInfo, adapter, webgl2: gl };
    check(`backend=${backend} selects ${backend}`, gpuInfo.backend === (backend === 'webgpu' ? 'BrowserWebGpu' : 'Gl'), { backend: gpuInfo.backend, adapter: gpuInfo.adapter, format: gpuInfo.format });
    for (const table of TABLES) {
      // The same simulated tick on both backends: S1's bot60 replay to tick 10 000 (40 frames x 250 ticks).
      const r = await page.evaluate((t) => S9.runReplay({ table: t, rules: 'lua', tpf: 250, maxFrames: 40 }), table);
      await page.evaluate(() => document.body.classList.add('clean'));
      const shot = path.join(S9, 'shots', `frame_${backend}_${table}_tick${r.status.tick}.png`);
      const png = await page.locator('#view').screenshot({ path: shot });
      await page.evaluate(() => document.body.classList.remove('clean'));
      const img = decodePng(png); imgs[`${backend}/${table}`] = img;
      const st = pixelStats(img);
      res.shots[`${backend}/${table}`] = { shot, tick: r.status.tick, state_hash: r.status.state_hash, frames: r.frames, render_ms_med: r.renderMsMedian, stats: st, gpuErrors: r.gpuErrors, parity: r.status.replay };
      check(`${backend} ${table} frame non-blank`, st.distinct_colours >= 256 && st.non_background_fraction >= 0.25, st);
      check(`${backend} ${table} replay parity to tick ${r.status.tick}`, r.status.replay.state_bad + r.status.replay.ext_bad === 0 && r.status.replay.first_bad === null, r.status.replay);
      check(`${backend} ${table} no GPU errors`, r.gpuErrors.uncaptured.length === 0 && r.gpuErrors.deferred_scope_errors.length === 0, r.gpuErrors);
    }
    // The egui overlay (pin_render's menu theme + OverlayPainter) over a live table frame.
    {
      const p2 = await L.browser.newPage({ viewport: { width: W, height: H } });
      const log2 = watch(p2);
      await p2.goto(`${srv.url}?backend=${backend}&table=apex_circuit&w=${W}&h=${H}&post=full&overlay=1&autostart=0`);
      await waitReady(p2);
      await poll(p2, () => S9.state.frames >= 3, null, 120000, 250);
      await p2.evaluate(() => document.body.classList.add('clean'));
      const shot = path.join(S9, 'shots', `overlay_egui_${backend}.png`);
      const png = await p2.locator('#view').screenshot({ path: shot });
      const st = pixelStats(decodePng(png));
      const ge = await p2.evaluate(() => S9.gpuErrors());
      res.shots[`${backend}/egui_overlay`] = { shot, stats: st, gpuErrors: ge };
      check(`${backend} egui overlay painted over the table, no GPU errors`, st.distinct_colours >= 256 && ge.uncaptured.length === 0 && ge.deferred_scope_errors.length === 0 &&
        log2.console.filter((m) => m.type === 'error').length === 0 && log2.errors.length === 0, { st, ge });
      await p2.close();
    }
    // A UI screenshot (overlays visible) for the layout record.
    await page.screenshot({ path: path.join(S9, 'shots', `page_${backend}_ui.png`) });
    const ce = log.console.filter((m) => m.type === 'error');
    check(`${backend} page: no console errors / page errors / crash`, ce.length === 0 && log.errors.length === 0 && !log.crashed, { consoleErrors: ce, pageErrors: log.errors });
    await page.close();
  }
  for (const table of TABLES) {
    const c = compare(imgs[`webgpu/${table}`], imgs[`webgl2/${table}`]);
    res.compare[table] = c;
    // Tolerant, not pixel-exact: S3 measured <= 4 px differing per frame (MSAA edges).
    check(`webgpu vs webgl2 ${table} (tolerant)`, c.mean_abs <= 1.0 && c.pct_px_gt32 <= 1.0, c);
  }
  // 2) auto with WebGPU available -> WebGPU.
  {
    const page = await L.browser.newPage({ viewport: { width: W, height: H } });
    const log = watch(page);
    await page.goto(`${srv.url}?backend=auto&w=360&h=640&post=min&autostart=0`);
    await waitReady(page);
    await page.waitForTimeout(1500);
    const g = await page.evaluate(() => ({ gpu: S9.gpuInfo(), frames: S9.state.frames }));
    res.auto.auto_with_webgpu = g;
    check('auto picks WebGPU when an adapter exists', g.gpu.backend === 'BrowserWebGpu' && g.frames > 0, g.gpu);
    check('auto (WebGPU) page: no console/page errors', log.console.filter((m) => m.type === 'error').length === 0 && log.errors.length === 0, log.errors);
    await page.close();
  }
  // 3) auto with navigator.gpu removed (a browser without WebGPU) -> WebGL2.
  {
    const ctx = await L.browser.newContext({ viewport: { width: W, height: H } });
    await ctx.addInitScript(() => { Object.defineProperty(Navigator.prototype, 'gpu', { get: () => undefined, configurable: true }); });
    const page = await ctx.newPage();
    const log = watch(page);
    await page.goto(`${srv.url}?backend=auto&w=360&h=640&post=min&autostart=0`);
    await waitReady(page);
    await page.waitForTimeout(1500);
    const g = await page.evaluate(() => ({ gpu: S9.gpuInfo(), frames: S9.state.frames, hasGpu: !!navigator.gpu }));
    res.auto.auto_without_navigator_gpu = g;
    check('auto falls back to WebGL2 without navigator.gpu', g.gpu.backend === 'Gl' && g.frames > 0 && !g.hasGpu, g.gpu);
    check('auto (no navigator.gpu) page: no console/page errors', log.console.filter((m) => m.type === 'error').length === 0 && log.errors.length === 0, log.errors);
    await ctx.close();
  }
  await L.browser.close();
  // 4) auto when navigator.gpu exists but no adapter (Chromium without --enable-unsafe-webgpu here).
  {
    const L2 = await launch('nowebgpu');
    const page = await L2.browser.newPage({ viewport: { width: W, height: H } });
    const log = watch(page);
    await page.goto(`${srv.url}?backend=auto&w=360&h=640&post=min&autostart=0`);
    await waitReady(page);
    await page.waitForTimeout(1500);
    const g = await page.evaluate(() => ({ gpu: S9.gpuInfo(), frames: S9.state.frames }));
    const a = await jsAdapter(page);
    res.auto.auto_without_adapter = { flags: L2.args, ...g, js: a };
    check('auto falls back to WebGL2 when requestAdapter() is null', g.gpu.backend === 'Gl' && g.frames > 0 && a.navigator_gpu && a.adapter === null, { gpu: g.gpu, js: a });
    check('auto (no adapter) page: no console/page errors', log.console.filter((m) => m.type === 'error').length === 0 && log.errors.length === 0, { console: log.console.filter((m) => m.type === 'error'), errors: log.errors });
    await page.screenshot({ path: path.join(S9, 'shots', 'page_auto_fallback_webgl2_ui.png') });
    await L2.browser.close();
  }
  res.fails = fails;
  writeJson(out, res);
  console.log(`== backends: ${fails} failing check(s)`);
  srv.proc.kill();
  process.exit(fails ? 1 : 0);
})().catch((e) => { console.error(e); process.exit(2); });
