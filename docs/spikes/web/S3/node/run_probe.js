// S3 Playwright driver: runs the render probe pages in headless Chromium.
// Usage: node run_probe.js <stage>[,<stage>...] [--pkg both|webgpu|webgl] [--flags minimal|brief|none]
// Stages: adapter, render, egui, matrix, scopes, blockon, bench, mobile, detect, canvasshot
'use strict';
const { chromium, devices } = require('playwright');
const fs = require('fs');
const path = require('path');
const { start } = require('./server');
const { encodePng } = require('./png');

const S = path.resolve(__dirname, '..');
const OUT = path.join(S, 'results');
const SHOTS = path.join(S, 'shots');
fs.mkdirSync(OUT, { recursive: true });
fs.mkdirSync(SHOTS, { recursive: true });
const TABLES = ['apex_circuit', 'sky_deck', 'feature_lab'];
const W = 540, H = 960;

// Flag sets. `minimal` is what was found sufficient for WebGPU on SwiftShader;
// `brief` is the brief's suggested list; `none` is Chromium defaults.
const FLAGS = {
  none: [],
  // Minimal set with which WebGPU renders AND presents to a canvas and
  // WebGL2 works in the same browser (found by elimination, RESULTS.md):
  // with only --enable-unsafe-webgpu the adapter works but presenting a
  // canvas frame destroys the device; without --ignore-gpu-blocklist
  // ANGLE-on-Vulkan refuses WebGL2.
  minimal: ['--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-angle=vulkan', '--ignore-gpu-blocklist'],
  unsafe_only: ['--enable-unsafe-webgpu'],
  brief: ['--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-angle=swiftshader',
    '--use-webgpu-adapter=swiftshader', '--enable-unsafe-swiftshader', '--ignore-gpu-blocklist',
    '--disable-vulkan-surface'],
  webgl_swiftshader: ['--use-gl=angle', '--use-angle=swiftshader', '--enable-unsafe-swiftshader'],
};

const args = process.argv.slice(2);
const stages = (args[0] || 'adapter,render').split(',');
const opt = (k, d) => { const i = args.indexOf(k); return i >= 0 ? args[i + 1] : d; };
const pkg = opt('--pkg', 'both');
const flagSet = opt('--flags', 'minimal');
const tag = opt('--tag', `${pkg}_${flagSet}`);
const backendsArg = opt('--backends', 'webgpu,webgl2').split(',');

function save(name, obj) {
  fs.writeFileSync(path.join(OUT, name), JSON.stringify(obj, null, 2));
}

async function openPage(browser, contextOpts = {}) {
  const ctx = await browser.newContext(contextOpts);
  const page = await ctx.newPage();
  const consoleLines = [];
  page.on('console', (m) => consoleLines.push(`[${m.type()}] ${m.text()}`));
  page.on('pageerror', (e) => consoleLines.push(`[pageerror] ${e.message}`));
  await page.goto(`http://127.0.0.1:${PORT}/web/index.html?pkg=${pkg}`);
  const ready = await page.evaluate(() => window.probeReady);
  return { ctx, page, consoleLines, ready };
}

const PORT = 8650 + Math.floor(Math.random() * 300);

(async () => {
  const server = await start(S, PORT);
  // Only Chromium's bundled SwiftShader Vulkan ICD is visible (as on a
  // runner without mesa-vulkan-drivers): WebGPU (Dawn) and WebGL2
  // (ANGLE-on-Vulkan) then both run on SwiftShader. SYSTEM_ICD=1 keeps the
  // system ICDs (lavapipe here) visible to ANGLE.
  const icd = path.join(path.dirname(chromium.executablePath()), '..', '..', 'chromium_headless_shell-1243', 'chrome-headless-shell-linux64', 'vk_swiftshader_icd.json');
  const env = { ...process.env };
  if (!process.env.SYSTEM_ICD) { env.VK_ICD_FILENAMES = icd; env.VK_DRIVER_FILES = icd; }
  const browser = await chromium.launch({ args: FLAGS[flagSet], env });
  const version = browser.version();
  const summary = { chromium: version, pkg, flags: FLAGS[flagSet], vk_icd: env.VK_ICD_FILENAMES || 'system', stages, results: [] };
  console.log(`chromium ${version}, pkg ${pkg}, flags ${flagSet}: ${FLAGS[flagSet].join(' ')}`);
  for (const stage of stages) {
    for (const backend of backendsArg) {
      if (['mobile', 'canvasshot'].includes(stage) && backend !== backendsArg[0]) continue;
      const { ctx, page, consoleLines, ready } = await openPage(browser,
        stage === 'mobile' ? { viewport: { width: 393, height: 852 }, deviceScaleFactor: 3, isMobile: true, hasTouch: true,
          userAgent: 'Mozilla/5.0 (Linux; Android 14; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/157.0.0.0 Mobile Safari/537.36' } : {});
      const rec = { stage, backend, wasmLoadMs: ready.loadMs, build: ready.build };
      const t0 = Date.now();
      try {
        if (stage === 'adapter') {
          rec.result = await page.evaluate((b) => window.runAdapter(b), backend);
          save(`adapter_${backend}_${tag}.json`, rec.result);
          rec.status = rec.result.ok ? `adapter ${rec.result.report.info.name} (${rec.result.report.info.backend})` : `FAIL ${rec.result.error}`;
        } else if (stage === 'render' || stage === 'egui') {
          rec.tables = [];
          for (const table of TABLES) {
            const opts = stage === 'egui' ? { egui: true } : { canvas_readback: true };
            if (process.env.WEBGL2_LIMITS && backend === 'webgl2') opts.webgl2_limits = true;
            const r = await page.evaluate(([b, t, w, h, o]) => window.runRender(b, t, w, h, o), [backend, table, W, H, opts]);
            const entry = { table, ok: r.ok };
            if (r.ok) {
              const rgba = Buffer.from(r.rgbaB64, 'base64');
              const file = path.join(SHOTS, `${stage === 'egui' ? 'egui_' : ''}web_${backend}_${tag}_${table}.png`);
              fs.writeFileSync(file, encodePng(W, H, rgba));
              entry.png = file;
              entry.report = r.report;
              const nonblack = rgba.reduce((n, v, i) => n + ((i % 4 !== 3 && v > 8) ? 1 : 0), 0);
              entry.nonBlackChannels = nonblack;
            } else {
              entry.error = r.error;
            }
            rec.tables.push(entry);
          }
          save(`${stage}_${backend}_${tag}.json`, rec);
          rec.status = rec.tables.map((t) => `${t.table}:${t.ok ? 'OK ' + path.basename(t.png) : 'FAIL ' + t.error.slice(0, 300)}`).join(' | ');
        } else if (stage === 'matrix') {
          for (const lim of backend === 'webgl2' ? [false, true] : [false]) {
            const r = await page.evaluate(([b, t, l]) => window.runMatrix(b, t, l), [backend, 'feature_lab', lim]);
            save(`matrix_${backend}_${tag}${lim ? '_webgl2limits' : ''}.json`, r);
            rec[`limits_${lim ? 'webgl2' : 'engine'}`] = r.ok ? r.report.items.map((i) => `${i.ok ? 'OK  ' : 'ERR '}${i.item}`) : `FAIL ${r.error}`;
          }
          rec.status = 'see results/matrix_*.json';
        } else if (stage === 'scopes') {
          rec.result = await page.evaluate((b) => window.runScopes(b), backend);
          save(`scopes_${backend}_${tag}.json`, rec.result);
          rec.status = rec.result.ok ? JSON.stringify({ pop: rec.result.report.scope_pop_error, sync: rec.result.report.pop_future_ready_synchronously, lost: rec.result.report.device_lost }) : `FAIL ${rec.result.error}`;
        } else if (stage === 'blockon') {
          // The call may never return (a busy-waiting main thread): race it
          // against a 20 s timer and sample the renderer's responsiveness.
          const t = Date.now();
          const call = page.evaluate((b) => window.runBlockOn(b), backend).then((r) => ({ returned: r }), (e) => ({ threw: String(e).slice(0, 300) }));
          const timer = new Promise((res) => setTimeout(() => res({ timeout: '20 s, no return' }), 20000));
          rec.result = await Promise.race([call, timer]);
          rec.result.elapsedMs = Date.now() - t;
          rec.status = JSON.stringify(rec.result);
        } else if (stage === 'bench') {
          const sizes = [['desktop 1080x1920', 1080, 1920], ['phone 1179x2556 (DPR 3)', 1179, 2556], ['phone half-res 590x1278', 590, 1278]];
          rec.result = await page.evaluate(([b, t, s, f]) => window.runBench(b, t, s, f), [backend, 'apex_circuit', sizes, Number(opt('--frames', '3'))]);
          save(`bench_${backend}_${tag}.json`, rec.result);
          rec.status = rec.result.ok ? 'see results' : `FAIL ${rec.result.error}`;
        } else if (stage === 'mobile') {
          rec.result = await page.evaluate(() => window.runViewport());
          const r = rec.result;
          // Render one frame at the phone's physical canvas size on each backend.
          rec.frames = [];
          for (const b of backendsArg) {
            const fr = await page.evaluate(([b, t, w, h]) => window.runRender(b, t, w, h, {}), [b, 'apex_circuit', r.physW, r.physH]);
            const e = { backend: b, ok: fr.ok, w: r.physW, h: r.physH };
            if (fr.ok) {
              const file = path.join(SHOTS, `mobile_${b}_${tag}_apex_circuit_${r.physW}x${r.physH}.png`);
              fs.writeFileSync(file, encodePng(r.physW, r.physH, Buffer.from(fr.rgbaB64, 'base64')));
              e.png = file; e.timing = fr.report.timing_ms; e.meta = fr.report.meta;
            } else { e.error = fr.error; }
            rec.frames.push(e);
          }
          save(`mobile_${tag}.json`, rec);
          rec.status = JSON.stringify({ dpr: r.dpr, css: [r.cssW, r.cssH], phys: [r.physW, r.physH], webgl2Max: r.webgl2 && r.webgl2.MAX_TEXTURE_SIZE, frames: rec.frames.map((f) => `${f.backend}:${f.ok ? 'OK' : f.error}`) });
        } else if (stage === 'detect') {
          // Backend choice in a build with both backends.
          rec.result = {};
          for (const mode of ['auto', 'auto-detect']) {
            rec.result[mode] = await page.evaluate((m) => window.runAdapter(m), mode);
            if (rec.result[mode].ok) rec.result[mode] = { ok: true, backend: rec.result[mode].report.info.backend, name: rec.result[mode].report.info.name };
          }
          save(`detect_${tag}.json`, rec.result);
          rec.status = JSON.stringify(rec.result);
        } else if (stage === 'canvasshot') {
          // Cross-check: Playwright's own screenshot of the presented canvas.
          const r = await page.evaluate(([b, t, w, h]) => window.runRender(b, t, w, h, {}), [backend, 'apex_circuit', W, H]);
          const file = path.join(SHOTS, `pwshot_${backend}_${tag}_apex_circuit.png`);
          await page.locator('canvas').screenshot({ path: file });
          rec.status = r.ok ? `screenshot ${file}` : `FAIL ${r.error}`;
        }
      } catch (e) {
        rec.status = `EXCEPTION ${e.message}`;
      }
      rec.wallMs = Date.now() - t0;
      rec.console = consoleLines.slice(0, 400);
      fs.writeFileSync(path.join(OUT, `console_${stage}_${backend}_${tag}.txt`), consoleLines.join('\n'));
      console.log(`[${stage}/${backend}] ${rec.status} (${rec.wallMs} ms)`);
      summary.results.push({ stage, backend, status: rec.status, wallMs: rec.wallMs });
      await ctx.close();
    }
  }
  save(`summary_${stages.join('-')}_${tag}.json`, summary);
  await browser.close();
  server.close();
})().catch((e) => { console.error(e); process.exit(1); });
