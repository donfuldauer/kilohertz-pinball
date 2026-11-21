// S9 Playwright helpers: local server, Chromium launch flag sets (S3), PNG decode, pixel stats.
'use strict';
const { chromium } = require('playwright');
const { spawn } = require('child_process');
const path = require('path');
const zlib = require('zlib');
const fs = require('fs');

const S9 = path.resolve(__dirname, '..');
// S3: the flag set that gives a presenting WebGPU adapter AND WebGL2 on SwiftShader in
// the headless shell (no GPU, no X server, no system Vulkan driver).
const FLAGS = {
  webgpu: ['--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-angle=vulkan', '--ignore-gpu-blocklist'],
  // Same ANGLE/Vulkan setup without --enable-unsafe-webgpu: navigator.gpu exists, no adapter.
  nowebgpu: ['--enable-features=Vulkan', '--use-angle=vulkan', '--ignore-gpu-blocklist'],
};
const ICD = path.join(path.dirname(chromium.executablePath()), '..', '..',
  'chromium_headless_shell-1243', 'chrome-headless-shell-linux64', 'vk_swiftshader_icd.json');

function startServer(port, extra = [], root = path.join(S9, 'dist')) {
  return new Promise((resolve, reject) => {
    const p = spawn(process.execPath, [path.join(root, 'serve.cjs'), String(port), ...extra], { stdio: ['ignore', 'pipe', 'inherit'] });
    p.stdout.once('data', (d) => resolve({ proc: p, url: `http://127.0.0.1:${port}/`, banner: String(d).trim() }));
    p.once('error', reject);
  });
}

async function launch(flagSet = 'webgpu', extraArgs = []) {
  const env = { ...process.env, VK_ICD_FILENAMES: ICD, VK_DRIVER_FILES: ICD };
  const args = [...FLAGS[flagSet], ...extraArgs];
  const browser = await chromium.launch({ args, env });
  return { browser, args, icd: ICD, version: browser.version() };
}

// Collects console messages and page errors of a page.
function watch(page) {
  const log = { console: [], errors: [], crashed: false };
  page.on('console', (m) => log.console.push({ type: m.type(), text: m.text() }));
  page.on('pageerror', (e) => log.errors.push(String(e)));
  page.on('crash', () => { log.crashed = true; });
  return log;
}

// Minimal PNG decoder (8-bit RGB/RGBA, non-interlaced: Playwright's screenshots).
function decodePng(buf) {
  let pos = 8; let w = 0; let h = 0; let ct = 0; const idat = [];
  while (pos < buf.length) {
    const len = buf.readUInt32BE(pos); const type = buf.toString('ascii', pos + 4, pos + 8);
    const data = buf.subarray(pos + 8, pos + 8 + len);
    if (type === 'IHDR') { w = data.readUInt32BE(0); h = data.readUInt32BE(4); ct = data[9]; if (data[8] !== 8 || data[12] !== 0) throw new Error('png: unsupported'); }
    if (type === 'IDAT') idat.push(data);
    pos += 12 + len;
  }
  const bpp = ct === 6 ? 4 : ct === 2 ? 3 : 0;
  if (!bpp) throw new Error(`png: colour type ${ct}`);
  const raw = zlib.inflateSync(Buffer.concat(idat));
  const stride = w * bpp; const out = Buffer.alloc(w * h * 4); const prev = Buffer.alloc(stride); const cur = Buffer.alloc(stride);
  for (let y = 0; y < h; y++) {
    const f = raw[y * (stride + 1)];
    raw.copy(cur, 0, y * (stride + 1) + 1, (y + 1) * (stride + 1));
    for (let i = 0; i < stride; i++) {
      const a = i >= bpp ? cur[i - bpp] : 0; const b = prev[i]; const c = i >= bpp ? prev[i - bpp] : 0;
      let v = cur[i];
      if (f === 1) v += a; else if (f === 2) v += b; else if (f === 3) v += (a + b) >> 1;
      else if (f === 4) { const p = a + b - c; const pa = Math.abs(p - a); const pb = Math.abs(p - b); const pc = Math.abs(p - c); v += pa <= pb && pa <= pc ? a : pb <= pc ? b : c; }
      cur[i] = v & 255;
    }
    for (let x = 0; x < w; x++) { out[(y * w + x) * 4] = cur[x * bpp]; out[(y * w + x) * 4 + 1] = cur[x * bpp + 1]; out[(y * w + x) * 4 + 2] = cur[x * bpp + 2]; out[(y * w + x) * 4 + 3] = bpp === 4 ? cur[x * bpp + 3] : 255; }
    cur.copy(prev);
  }
  return { w, h, px: out };
}

// Non-blank statistics: distinct RGB colours, the most frequent colour ("background")
// and the fraction of pixels differing from it by more than 8 in any channel, mean luma.
function pixelStats(img) {
  const n = img.w * img.h; const counts = new Map(); let luma = 0;
  for (let i = 0; i < n; i++) {
    const r = img.px[i * 4]; const g = img.px[i * 4 + 1]; const b = img.px[i * 4 + 2];
    const k = (r << 16) | (g << 8) | b; counts.set(k, (counts.get(k) || 0) + 1);
    luma += 0.2126 * r + 0.7152 * g + 0.0722 * b;
  }
  let bg = 0; let bgN = 0; for (const [k, c] of counts) if (c > bgN) { bg = k; bgN = c; }
  const br = bg >> 16; const bgg = (bg >> 8) & 255; const bb = bg & 255; let nonbg = 0;
  for (let i = 0; i < n; i++) {
    if (Math.abs(img.px[i * 4] - br) > 8 || Math.abs(img.px[i * 4 + 1] - bgg) > 8 || Math.abs(img.px[i * 4 + 2] - bb) > 8) nonbg++;
  }
  return { w: img.w, h: img.h, distinct_colours: counts.size, background: `#${bg.toString(16).padStart(6, '0')}`,
    background_fraction: +(bgN / n).toFixed(4), non_background_fraction: +(nonbg / n).toFixed(4), mean_luma: +(luma / n).toFixed(2) };
}

// Tolerant comparison of two same-size images: mean abs channel difference, share of
// pixels with a channel difference > 8 / > 32, and the same on 8x8 block means.
function compare(a, b) {
  if (a.w !== b.w || a.h !== b.h) return { error: 'size differs' };
  const n = a.w * a.h; let sum = 0; let gt8 = 0; let gt32 = 0; let max = 0;
  for (let i = 0; i < n; i++) {
    let m = 0;
    for (let c = 0; c < 3; c++) { const d = Math.abs(a.px[i * 4 + c] - b.px[i * 4 + c]); sum += d; if (d > m) m = d; }
    if (m > 8) gt8++; if (m > 32) gt32++; if (m > max) max = m;
  }
  const bw = Math.floor(a.w / 8); const bh = Math.floor(a.h / 8); let bmax = 0; let bsum = 0;
  for (let by = 0; by < bh; by++) for (let bx = 0; bx < bw; bx++) for (let c = 0; c < 3; c++) {
    let sa = 0; let sb = 0;
    for (let y = 0; y < 8; y++) for (let x = 0; x < 8; x++) { const i = ((by * 8 + y) * a.w + bx * 8 + x) * 4 + c; sa += a.px[i]; sb += b.px[i]; }
    const d = Math.abs(sa - sb) / 64; bsum += d; if (d > bmax) bmax = d;
  }
  return { mean_abs: +(sum / (n * 3)).toFixed(4), max_abs: max, pct_px_gt8: +(100 * gt8 / n).toFixed(3), pct_px_gt32: +(100 * gt32 / n).toFixed(3),
    block8_mean_abs: +(bsum / (bw * bh * 3)).toFixed(4), block8_max_abs: +bmax.toFixed(2) };
}

function writeJson(p, v) { fs.mkdirSync(path.dirname(p), { recursive: true }); fs.writeFileSync(p, JSON.stringify(v, null, 1)); }

// Polls with page.evaluate (CDP, not subject to the page CSP). page.waitForFunction cannot
// be used: Playwright compiles its predicate inside the page with new Function(), which the
// CSP (no 'unsafe-eval') blocks, so it never resolves.
// (String expressions hang under the CSP as well: always pass functions.)
async function poll(page, fn, arg, timeout = 120000, every = 250) {
  const t0 = Date.now();
  for (;;) {
    const v = await page.evaluate(fn, arg);
    if (v) return v;
    if (Date.now() - t0 > timeout) throw new Error(`timeout waiting for ${fn}`);
    await new Promise((r) => setTimeout(r, every));
  }
}

async function waitReady(page, timeout = 120000) {
  await poll(page, () => !!(window.S9 && (window.S9.state.marks.ready !== undefined || window.S9.state.startupError)), null, timeout);
  const err = await page.evaluate(() => window.S9.state.startupError);
  if (err) throw new Error(`startup failed: ${err}`);
}

module.exports = { poll, S9, FLAGS, ICD, startServer, launch, watch, decodePng, pixelStats, compare, writeJson, waitReady };
