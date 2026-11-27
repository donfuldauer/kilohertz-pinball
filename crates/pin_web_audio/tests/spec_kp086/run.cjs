// Independent Tester, KP-086 (#180), REQ-WEB-022 / D43: Chromium runner.
//
// Usage: node run.cjs <config.json>
// The config (written by tests/spec_kp086_browser.rs) names the mode
// (digest | protocol | gesture), the directories to serve and the natively
// computed expected digests. Serves 127.0.0.1 on an ephemeral port with the
// D43 CSP (no COOP/COEP) and drives the page with Playwright.
// Prints `KP086 <key> <json>` lines. Exit 0 = pass, 1 = fail, 77 = the
// environment cannot run the test (playwright or Chromium unavailable).
'use strict';
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');

const CSP =
  "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; " +
  "object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";
const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.json': 'application/json',
  '.wasm': 'application/wasm',
  '.bin': 'application/octet-stream',
};

const cfg = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
const failures = [];
const out = (k, v) => console.log('KP086 ' + k + ' ' + JSON.stringify(v));
const check = (cond, what, detail) => {
  if (!cond) failures.push(what + (detail === undefined ? '' : ': ' + JSON.stringify(detail)));
};

function serve() {
  const mounts = [
    ['/page/', cfg.pageDir],
    ['/www/', cfg.wwwDir],
    ['/fixture/', cfg.fixtureDir],
  ];
  const server = http.createServer((req, res) => {
    const h = { 'Content-Security-Policy': CSP, 'Cache-Control': 'no-store' };
    const url = decodeURIComponent(req.url.split('?')[0]);
    let file = null;
    // `/www_noworklet/` serves `www/` without worklet.js (404), so the
    // helper's worklet cannot load and it must fall back.
    if (url === '/www_noworklet/worklet.js') file = null;
    else if (url.startsWith('/www_noworklet/')) {
      const root = path.resolve(cfg.wwwDir);
      const p = path.resolve(root, '.' + url.slice('/www_noworklet'.length));
      if (p.startsWith(root + path.sep)) file = p;
    } else if (url === '/' || url === '/index.html') file = path.join(cfg.pageDir, 'index.html');
    else if (url === '/wasm/pin_web_audio.wasm') file = cfg.wasmPath;
    else {
      for (const [pre, dir] of mounts) {
        if (url.startsWith(pre)) {
          const root = path.resolve(dir);
          const p = path.resolve(root, '.' + url.slice(pre.length - 1));
          if (p.startsWith(root + path.sep)) file = p;
        }
      }
    }
    if (!file) {
      res.writeHead(404, h).end();
      return;
    }
    fs.readFile(file, (err, data) => {
      if (err) {
        res.writeHead(404, h).end();
        return;
      }
      h['Content-Type'] = MIME[path.extname(file)] || 'application/octet-stream';
      res.writeHead(200, h).end(data);
    });
  });
  return new Promise((resolve) => server.listen(0, '127.0.0.1', () => resolve(server)));
}

async function openPage(browser, origin, label, query = '') {
  const page = await browser.newPage();
  const consoleErrors = [];
  const consoleLines = [];
  const pageErrors = [];
  page.on('console', (m) => {
    consoleLines.push(m.text());
    if (m.type() === 'error') consoleErrors.push(m.text());
  });
  page.on('pageerror', (e) => pageErrors.push(String(e)));
  const resp = await page.goto(origin + '/' + query);
  const csp = resp.headers()['content-security-policy'];
  check(csp === CSP, label + ': page served with the D43 CSP', csp);
  check(!resp.headers()['cross-origin-opener-policy'], label + ': no COOP');
  check(!resp.headers()['cross-origin-embedder-policy'], label + ': no COEP');
  await page.waitForFunction(() => window.kp086 !== undefined, null, { timeout: 20000 });
  return { page, consoleErrors, consoleLines, pageErrors };
}

const ev = (page, fn, arg) => page.evaluate(([f, a]) => window.kp086[f](a), [fn, arg]);

async function digestMode(browser, origin) {
  const P = await openPage(browser, origin, 'digest');
  const shape = await ev(P.page, 'moduleShape');
  out('module', shape);
  check(shape.imports === 0, 'wasm has no imports', shape.imports);
  const want = ['memory:memory'].concat(
    ['cmd_buffer', 'free', 'new', 'register', 'render', 'stage', 'stat', 'submit', 'trigger'].map(
      (n) => 'pwa_' + n + ':function',
    ),
  );
  for (const w of want) check(shape.exports.includes(w), 'wasm exports ' + w, shape.exports);

  const quanta = cfg.quanta;
  const runs = [
    ['worklet_48000', 'runDirect', { rate: 48000 }],
    ['worklet_44100', 'runDirect', { rate: 44100 }],
    ['helper_worklet_48000', 'runHelper', { rate: 48000 }],
    ['helper_fallback_48000', 'runHelper', { rate: 48000, forceFallback: true }],
    ['own_fallback_48000', 'runOwnFallback', { rate: 48000 }],
  ];
  for (const [name, fn, arg] of runs) {
    let r;
    try {
      r = await ev(P.page, fn, arg);
    } catch (e) {
      check(false, name + ' ran', String(e));
      continue;
    }
    out('run_' + name, r);
    out('digest', { name, actual: r.digest });
    check(r.length === quanta * 128, name + ': AudioBuffer is 600 quanta of 128 frames', r.length);
    if (r.ctxLength !== undefined) check(r.ctxLength === quanta * 128, name + ': context length', r.ctxLength);
    if (r.channels !== undefined) check(r.channels === 2, name + ': stereo', r.channels);
    check(JSON.stringify(r.codes) === '[0,0,0]', name + ': register codes 0', r.codes);
    if (fn === 'runDirect') {
      check(r.readySampleRate === arg.rate, name + ': ready reports sampleRate', r.readySampleRate);
      check(r.errorMessages === 0 && r.processorErrors.length === 0, name + ': no processor error', r);
      check(r.statsBefore.length === 9, name + ': stats has 9 entries', r.statsBefore);
      check(r.statsBefore[5] === 0 && r.statsBefore[4] === 0, name + ': nothing rendered before start', r.statsBefore);
      if (r.statsAfter) {
        // The worklet's own count: every process() call rendered 128 frames.
        check(r.statsAfter[5] === quanta, name + ': worklet quantum count', r.statsAfter);
        check(r.statsAfter[4] === quanta * 128, name + ': worklet frames rendered', r.statsAfter);
        check(r.statsAfter[2] === 1, name + ': one unknown sound (id 7)', r.statsAfter);
      } else {
        out('note', name + ': worklet stats unavailable after offline rendering finished');
      }
      if (r.quantumFrames) {
        check(r.quantumFrames.min === 128 && r.quantumFrames.max === 128, name + ': every quantum 128 frames', r);
      }
      if (r.processCalls !== null && r.processCalls !== undefined) {
        check(r.processCalls === quanta, name + ': 600 process calls', r.processCalls);
      }
    }
    if (fn === 'runHelper') {
      check(r.mode === (arg.forceFallback ? 'fallback' : 'worklet'), name + ': mode', r.mode);
      check(
        JSON.stringify(r.latencyKeys) === '["baseLatency","outputLatency"]',
        name + ': latency() keys',
        r.latencyKeys,
      );
    }
    if (fn === 'runOwnFallback') {
      check(r.accepted === 12, name + ': 12 records accepted', r.accepted);
      check(r.abiStats[5] === quanta, name + ': ABI quantum count', r.abiStats);
      check(r.abiStats[4] === quanta * 128, name + ': ABI frames rendered', r.abiStats);
      check(r.fallbackBefore.queued === quanta, name + ': all blocks queued', r.fallbackBefore);
      check(r.fallbackBefore.dropped === 0, name + ': no block dropped', r.fallbackBefore);
      if (r.fallbackAfter) {
        check(r.fallbackAfter.underruns === 0, name + ': no underrun', r.fallbackAfter);
        check(r.fallbackAfter.queued === 0, name + ': queue drained', r.fallbackAfter);
      }
    }
  }
  check(P.consoleErrors.length === 0, 'digest page: zero console errors', P.consoleErrors);
  check(P.pageErrors.length === 0, 'digest page: zero page errors', P.pageErrors);
  out('console_errors', P.consoleErrors);
  out('page_errors', P.pageErrors);
}

async function protocolMode(browser, origin) {
  const P = await openPage(browser, origin, 'protocol');
  const quanta = cfg.quanta;
  for (const [name, arg] of [
    ['late_48000', { rate: 48000, late: true }],
    ['junk_48000', { rate: 48000, junk: true }],
  ]) {
    let r;
    try {
      r = await ev(P.page, 'runDirect', arg);
    } catch (e) {
      check(false, name + ' ran', String(e));
      continue;
    }
    out('run_' + name, r);
    out('digest', { name, actual: r.digest });
    check(r.length === quanta * 128, name + ': length', r.length);
    check(r.errorMessages === 0 && r.processorErrors.length === 0, name + ': no processor error', r);
    if (arg.late) {
      check(r.suspendedStats !== null, name + ': stats while suspended', r.suspendedStats);
      if (r.suspendedStats) {
        check(r.suspendedStats[5] === cfg.late.quantum, name + ': quantum index at suspend', r.suspendedStats);
        check(r.suspendedStats[4] === cfg.late.quantum * 128, name + ': frames at suspend', r.suspendedStats);
      }
    }
  }
  const codes = await ev(P.page, 'registerCodes');
  out('register_codes', codes);
  check(codes.channels3 === -3, 'register channels 3 -> -3', codes);
  check(codes.overBudgetPcm === -5, 'register PCM over budgetMiB -> -5', codes);
  check(codes.threeQuarterMiB === 0, 'register 0.75 MiB of 1 -> 0', codes);
  check(codes.halfMiBOver === -5, 'register over the remaining budget -> -5', codes);
  check(codes.quarterMiBExact === 0, 'register exactly the remaining budget -> 0', codes);
  check(codes.bus3 === -2, 'register bus 3 -> -2', codes);
  check(codes.id65536 === -1, 'register id 65536 -> -1', codes);
  check(codes.stats[6] === 1048576 && codes.stats[7] === 1048576, 'budget used = limit = 1 MiB', codes.stats);
  check(P.consoleErrors.length === 0, 'protocol page: zero console errors', P.consoleErrors);
  check(P.pageErrors.length === 0, 'protocol page: zero page errors', P.pageErrors);
  out('console_errors', P.consoleErrors);
}

async function badWasmMode(browser, origin) {
  // Own page: a fallback and a 404 may legitimately log console errors.
  const B = await openPage(browser, origin, 'badwasm');
  let nw;
  try {
    nw = await ev(B.page, 'runHelper', { rate: 48000, base: '/www_noworklet/' });
    out('run_noworklet_fallback_48000', nw);
    out('digest', { name: 'noworklet_fallback_48000', actual: nw.digest });
    check(nw.mode === 'fallback', 'worklet.js cannot load: createWebAudio selects fallback', nw.mode);
    check(JSON.stringify(nw.codes) === '[0,0,0]', 'noworklet: register codes 0', nw.codes);
  } catch (e) {
    check(false, 'noworklet fallback ran', String(e));
  }
  const bad = await ev(B.page, 'badAndGoodWasm');
  out('bad_wasm', bad);
  check(bad.firstType === 'error' && bad.errorIsString, 'bad wasm: {type:error, error:String}', bad);
  check(bad.errorCount === 1 && bad.readyCount === 0, 'bad wasm: error posted once, never ready', bad);
  check(bad.silent, 'bad wasm: silence', bad);
  check(bad.badMode === 'fallback', 'bad wasm: createWebAudio selects fallback', bad.badMode);
  check(bad.goodMode === 'worklet', 'good wasm: createWebAudio selects worklet', bad.goodMode);
  check(bad.goodStatsLen === 9, 'good wasm: stats() has 9 numbers', bad.goodStatsLen);
  check(B.pageErrors.length === 0, 'badwasm page: zero page errors', B.pageErrors);
  out('badwasm_console_errors', B.consoleErrors);
}

async function gestureMode(browser, origin) {
  const P = await openPage(browser, origin, 'gesture', '?gesture');
  const initial = await ev(P.page, 'gestureStart');
  out('gesture_initial_state', initial);
  check(initial === 'suspended', 'context suspended at creation, before any input', initial);
  // Bounded wait (the browser's own event loop) without input: still suspended.
  await P.page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
  await P.page.evaluate(() => new Promise((r) => setTimeout(r, 300)));
  const idle = await ev(P.page, 'gestureState');
  out('gesture_idle', idle);
  check(idle.state === 'suspended' && idle.resolved === null, 'no resume without a gesture', idle);
  await P.page.mouse.click(20, 20);
  let st = null;
  for (let i = 0; i < 100; i++) {
    st = await ev(P.page, 'gestureState');
    if (st.resolved !== null && st.state === 'running') break;
    await P.page.waitForTimeout(50);
  }
  out('gesture_after_click', st);
  check(st.state === 'running', 'running after a real click', st);
  check(st.resolved === 'running', 'resumeOnGesture resolves with running', st);
  const before = P.consoleLines.length;
  const lat = await ev(P.page, 'latency');
  out('latency', lat);
  check(JSON.stringify(lat.keys) === '["baseLatency","outputLatency"]', 'logLatency keys', lat);
  check(['number', 'null'].includes(lat.baseType), 'baseLatency number or null', lat);
  check(['number', 'null'].includes(lat.outputType), 'outputLatency number or null', lat);
  let lines = [];
  for (let i = 0; i < 40; i++) {
    lines = P.consoleLines.slice(before).filter((l) => l.startsWith('pin_web_audio latency: baseLatency='));
    if (lines.length) break;
    await P.page.waitForTimeout(25);
  }
  out('latency_lines', lines);
  check(lines.length === 1, 'one latency console line', P.consoleLines.slice(before));
  check(P.consoleErrors.length === 0, 'gesture page: zero console errors', P.consoleErrors);
  check(P.pageErrors.length === 0, 'gesture page: zero page errors', P.pageErrors);
}

(async () => {
  let pw;
  try {
    pw = require('playwright');
  } catch (e) {
    out('skip', 'require("playwright") failed: ' + e.message);
    process.exit(77);
  }
  let browser;
  try {
    browser = await pw.chromium.launch();
  } catch (e) {
    out('skip', 'Chromium launch failed: ' + e.message.split('\n')[0]);
    process.exit(77);
  }
  out('browser', browser.version());
  const server = await serve();
  const origin = 'http://127.0.0.1:' + server.address().port;
  try {
    if (cfg.mode === 'digest') await digestMode(browser, origin);
    else if (cfg.mode === 'protocol') await protocolMode(browser, origin);
    else if (cfg.mode === 'badwasm') await badWasmMode(browser, origin);
    else if (cfg.mode === 'gesture') await gestureMode(browser, origin);
    else failures.push('unknown mode ' + cfg.mode);
  } catch (e) {
    failures.push('runner exception: ' + (e && e.stack ? e.stack : String(e)));
  }
  await browser.close();
  server.close();
  out('failures', failures);
  process.exit(failures.length ? 1 : 0);
})();
