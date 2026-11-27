// pin_web_audio Chromium test driver (REQ-WEB-022), spawned by tests/wa_browser.rs.
//
//   node wa_run.cjs <www dir> <browser dir> <data dir> <expected fnv hex>
//
// Serves /www/ (the shipped worklet files), /t/ (this directory) and /data/
// (the wasm module and the PCM written by the Rust test) from 127.0.0.1 on
// an ephemeral port with the strict CSP and no COOP/COEP, then in Chromium:
//   1. renders the S6 script through the worklet in an OfflineAudioContext;
//   2. renders it through the fallback path (forceFallback + prerender);
//   3. checks resumeOnGesture / logLatency on a realtime AudioContext.
// Prints `RESULT <json>` lines and exits 0 when every check passes, 1 on a
// failure, 2 (after a `SKIP:` line) when playwright or Chromium is missing.
'use strict';

const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');

const CSP = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; " +
  "object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";
const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript',
  '.cjs': 'text/javascript',
  '.wasm': 'application/wasm',
};

function skip(msg) {
  console.log('SKIP: ' + msg);
  process.exit(2);
}

function serve(roots) {
  const server = http.createServer((req, res) => {
    const headers = { 'Content-Security-Policy': CSP, 'Cache-Control': 'no-store' };
    const url = decodeURIComponent((req.url || '/').split('?')[0]);
    const m = /^\/([a-z]+)\/(.*)$/.exec(url);
    const root = m && roots[m[1]];
    if (!root) {
      res.writeHead(404, headers).end();
      return;
    }
    const file = path.normalize(path.join(root, m[2] || 'wa_index.html'));
    if (!file.startsWith(root + path.sep)) {
      res.writeHead(403, headers).end();
      return;
    }
    fs.readFile(file, (err, data) => {
      if (err) {
        res.writeHead(404, headers).end();
        return;
      }
      headers['Content-Type'] = MIME[path.extname(file)] || 'application/octet-stream';
      res.writeHead(200, headers).end(data);
    });
  });
  return new Promise((resolve) => server.listen(0, '127.0.0.1', () => resolve(server)));
}

async function main() {
  const [wwwDir, browserDir, dataDir, expected] = process.argv.slice(2);
  if (!expected) {
    console.error('usage: wa_run.cjs <www> <browser> <data> <expected>');
    process.exit(1);
  }
  let chromium;
  try {
    ({ chromium } = require('playwright'));
  } catch (e) {
    skip("require('playwright') failed: " + e.message);
  }
  let browser;
  try {
    browser = await chromium.launch();
  } catch (e) {
    skip('Chromium launch failed: ' + String(e).split('\n')[0]);
  }
  const server = await serve({
    www: path.resolve(wwwDir),
    t: path.resolve(browserDir),
    data: path.resolve(dataDir),
  });
  const origin = 'http://127.0.0.1:' + server.address().port;
  const failures = [];
  const check = (ok, what) => {
    if (!ok) failures.push(what);
  };
  try {
    const page = await browser.newPage();
    const errors = [];
    const infos = [];
    page.on('console', (m) => {
      if (m.type() === 'error') errors.push(m.text());
      if (m.type() === 'info') infos.push(m.text());
    });
    page.on('pageerror', (e) => errors.push(String(e)));
    const resp = await page.goto(origin + '/t/wa_index.html');
    check(resp.headers()['content-security-policy'] === CSP, 'CSP header served');
    check(!resp.headers()['cross-origin-opener-policy'], 'no COOP');
    check(!resp.headers()['cross-origin-embedder-policy'], 'no COEP');
    await page.waitForFunction(() => typeof window.runDigest === 'function');
    check(await page.evaluate(() => !window.crossOriginIsolated), 'not cross-origin isolated');

    for (const fallback of [false, true]) {
      const r = await page.evaluate((f) => window.runDigest(f), fallback);
      const label = fallback ? 'fallback' : 'worklet';
      console.log('RESULT ' + JSON.stringify({ path: label, ...r }));
      check(r.mode === label, label + ': mode');
      check(r.digest === expected, `${label}: digest ${r.digest} == native ${expected}`);
      check(r.frames === 600 * 128 && r.channels === 2 && r.sampleRate === 48000, label + ': buffer shape');
      check(JSON.stringify(r.codes) === '[0,0,0]', label + ': register codes');
      check(r.imports === 0, label + ': wasm has no imports');
      check(JSON.stringify(r.exports) === JSON.stringify(['memory', 'pwa_cmd_buffer', 'pwa_free', 'pwa_new',
        'pwa_register', 'pwa_render', 'pwa_stage', 'pwa_stat', 'pwa_submit', 'pwa_trigger']), label + ': exports');
      check(r.stats[5] === 600 && r.stats[4] === 600 * 128, label + ': 600 quanta of 128 frames rendered');
      check(r.stats[0] === 0 && r.stats[1] === 0 && r.stats[2] === 1, label + ': drops/rejects/unknown');
      check(r.stats[6] === (9600 + 24000 + 9600) * 4 && r.stats[7] === 64 * 1048576, label + ': budget');
      if (!fallback) {
        check(r.processCalls === 600 && r.quantumFrames.min === 128 && r.quantumFrames.max === 128,
          'worklet: every quantum is 128 frames');
      } else {
        check(r.processor.underruns === 0 && r.processor.dropped === 0 && r.processor.queued === 0,
          'fallback: no underruns/drops');
      }
    }

    const ep = await page.evaluate(() => window.runErrorPath());
    console.log('RESULT ' + JSON.stringify({ path: 'error', ...ep }));
    check(ep.silent, 'error path: silence');
    check(ep.messages.length === 1 && ep.messages[0].type === 'error' && typeof ep.messages[0].error === 'string',
      'error path: exactly one error message, other messages ignored');
    console.log('RESULT ' + JSON.stringify({ path: 'console', errors, browser: browser.version() }));
    check(errors.length === 0, 'zero console and page errors');
  } finally {
    await browser.close();
  }
  // Gesture and latency on a realtime context, default autoplay policy.
  let gestureBrowser;
  try {
    gestureBrowser = await chromium.launch();
  } catch (e) {
    server.close();
    skip('Chromium launch failed: ' + String(e).split('\n')[0]);
  }
  try {
    const page = await gestureBrowser.newPage();
    const errors = [];
    const infos = [];
    page.on('console', (m) => {
      if (m.type() === 'error') errors.push(m.text());
      if (m.type() === 'info') infos.push(m.text());
    });
    page.on('pageerror', (e) => errors.push(String(e)));
    await page.goto(origin + '/t/wa_index.html?gesture');
    await page.waitForFunction(() => typeof window.gestureInitial === 'string');
    const s0 = await page.evaluate(() => window.gestureInitial);
    const s1 = await page.evaluate(() => new Promise((r) => setTimeout(() => r(window.gestureState()), 300)));
    await page.mouse.click(10, 10);
    const g = await page.evaluate(() => window.gestureResult());
    const line = infos.find((t) => t.startsWith('pin_web_audio latency: '));
    console.log('RESULT ' + JSON.stringify({ path: 'gesture', before: s0, beforeLater: s1, after: g, line }));
    check(s0 === 'suspended' && s1 === 'suspended', 'context suspended before the first gesture');
    check(g.state === 'running', 'context running after the click');
    check(typeof line === 'string' &&
      /^pin_web_audio latency: baseLatency=(n\/a|[0-9.e+-]+) outputLatency=(n\/a|[0-9.e+-]+)$/.test(line),
    'latency line logged');
    check(g.latency && 'baseLatency' in g.latency && 'outputLatency' in g.latency, 'latency shape');
    check(errors.length === 0, 'gesture page: zero console and page errors');
  } finally {
    await gestureBrowser.close();
    server.close();
  }
  if (failures.length) {
    console.log('FAIL: ' + failures.join('; '));
    process.exit(1);
  }
  console.log('PASS');
}

main().catch((e) => {
  console.log('FAIL: ' + (e && e.stack ? e.stack : String(e)));
  process.exit(1);
});
