// S9 page: ONE engine module (sim + Lua + wasmi rules + wgpu renderer + bundle loader).
// Loads the module, a bundle, creates the GPU context (WebGPU or WebGL2), and runs the
// fixed-step loop: every animation frame steps the sim (1 ms ticks) and renders once.
// Query: backend=webgpu|webgl2|auto, table=<id>, rules=lua|wasm, mode=live|idle,
//        scale=<render scale>, w/h=<fixed drawing buffer>, post=full|min, autostart=0|1.
// Test hooks on window.S9 (Playwright drives them; the CSP blocks eval for the page itself).

const params = new URLSearchParams(location.search);
const P = {
  backend: params.get('backend') || 'auto',
  table: params.get('table') || 'apex_circuit',
  rules: params.get('rules') || 'lua',
  mode: params.get('mode') || 'live',
  scale: parseFloat(params.get('scale') || '1'),
  w: Number(params.get('w') || 0),
  h: Number(params.get('h') || 0),
  post: params.get('post') || 'full',
  autostart: params.get('autostart') !== '0',
  debug: params.get('debug') === '1',
  overlay: params.get('overlay') === '1',
};
const LIVE_SEED = 0x4b485a;
const MAX_BUNDLE = 32 * 1024 * 1024; // S4: checked on File.size before any copy
const CATCH_UP_CAP = 50; // S1: per-frame tick cap; real time beyond it is skipped

let wasm = null; // the glue module namespace (one per instance generation)
let exportsObj = null; // init() result: raw exports incl. memory
let engine = null;
let gpu = null;
let generation = 0;
let currentBundle = null;
let currentName = null;

const state = {
  params: P, marks: {}, frames: 0, running: false, kind: null, raf: 0,
  tpf: [], frameMs: [], stepMs: [], renderMs: [], errorsShown: [], crash: null, restarts: 0,
  faultShown: false, inputs: [], loads: [], replay: null, gpuInfo: null, buildInfo: null,
};

function mark(name) {
  if (state.marks[name] === undefined) {
    performance.mark(name);
    state.marks[name] = performance.now();
  }
}
mark('js-start');

const $ = (id) => document.getElementById(id);
const canvasEl = () => $('view');

function showError(msg) {
  const e = $('error');
  e.textContent = msg;
  e.hidden = false;
  state.errorsShown.push({ t: performance.now(), msg });
}
function clearError() { $('error').hidden = true; }

function memBytes() { return exportsObj ? exportsObj.memory.buffer.byteLength : 0; }

async function fetchBytes(url) {
  const r = await fetch(url);
  if (!r.ok) throw new Error(`${url}: HTTP ${r.status}`);
  return new Uint8Array(await r.arrayBuffer());
}

async function loadModule() {
  generation += 1;
  // A fresh glue-module instance (new URL) gives a fresh wasm instance: the
  // way back after an engine trap ("Module terminated").
  const url = generation === 1 ? './pkg/pin_web.js' : `./pkg/pin_web.js?instance=${generation}`;
  mark('wasm-fetch-start');
  wasm = await import(url);
  exportsObj = await wasm.default({ module_or_path: new URL('./pkg/pin_web_bg.wasm', import.meta.url) });
  mark('wasm-ready');
  state.buildInfo = JSON.parse(wasm.build_info());
  engine = new wasm.WebEngine();
  globalThis.__pinWebPanic = undefined;
}

function sizeCanvas(c) {
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  const w = P.w || Math.round(c.clientWidth * dpr * P.scale);
  const h = P.h || Math.round(c.clientHeight * dpr * P.scale);
  const max = state.gpuInfo ? state.gpuInfo.max_texture_dimension_2d : 8192;
  c.width = Math.max(1, Math.min(w, max));
  c.height = Math.max(1, Math.min(h, max));
}

function postOpts() {
  const o = P.post === 'min' ? { msaa: false, bloom: false, blur: false } : {};
  o.overlay = P.overlay; // egui overlay (pin_render's menu theme) over the table
  return o;
}

async function createGpu() {
  const c = canvasEl();
  sizeCanvas(c);
  gpu = await wasm.create_gpu(c, P.backend, JSON.stringify(postOpts()));
  state.gpuInfo = JSON.parse(gpu.info());
  mark('gpu-ready');
}

function buildRenderer() {
  const r = JSON.parse(gpu.build(engine));
  mark('renderer-built');
  return r;
}

function updateHud() {
  let st = null;
  try { st = JSON.parse(engine.status()); } catch (_) { return; }
  state.lastStatus = st;
  const be = state.gpuInfo ? state.gpuInfo.backend : '?';
  $('hud').textContent = `${be} ${state.gpuInfo ? state.gpuInfo.size.join('x') : ''} ` +
    `tick ${st.tick} balls ${st.balls} score ${st.score} rules ${st.rules}` +
    `${st.faulted ? ' (TILT)' : ''} frames ${state.frames} mem ${(memBytes() / 1048576).toFixed(1)} MiB`;
  if (st.faulted && !state.faultShown) {
    state.faultShown = true;
    showError(`Rules stopped (tilt): ${st.error} - the table keeps running without its rules.`);
  }
  if (state.kind === 'replay' && st.replay && st.replay.first_bad !== null && !state.replayBadShown) {
    state.replayBadShown = true;
    showError(`Replay diverged from native at tick ${st.replay.first_bad}`);
  }
}

function stopLoop() {
  state.running = false;
  if (state.raf) cancelAnimationFrame(state.raf);
  state.raf = 0;
}

function startLoop(kind, opts = {}) {
  stopLoop();
  state.kind = kind;
  state.running = true;
  let last = performance.now();
  let debt = 0;
  const frame = (now) => {
    if (!state.running) return;
    try {
      // Frame pacing: never queue a frame behind one the GPU has not finished.
      const busy = gpu.busy();
      if (P.debug && now - (state.dbgT || 0) > 2000) {
        state.dbgT = now;
        console.log(`dbg frames=${state.frames} waitRaf=${state.waitRaf} skippedRender=${state.skippedRender} busy=${busy}`);
      }
      if (busy && kind === 'replay') {
        state.waitRaf += 1;
        state.raf = requestAnimationFrame(frame);
        return;
      }
      let n;
      if (kind === 'replay') {
        n = opts.tpf;
      } else {
        // rAF's timestamp can precede the performance.now() taken in startLoop: clamp, or a
        // negative count reaches the u32 parameter as 2^32 - k (wasm-bindgen's `>>> 0`) and the
        // engine is asked for 4 billion ticks (found by this spike: the page froze in step()).
        debt = Math.max(0, debt + (now - last));
        last = Math.max(last, now);
        n = Math.min(Math.floor(debt), CATCH_UP_CAP);
        debt -= n;
        if (debt > CATCH_UP_CAP) debt = 0; // skip real time beyond the cap
      }
      const t0 = performance.now();
      const ran = engine.step(n);
      const t1 = performance.now();
      if (busy) {
        state.skippedRender += 1; // live: the sim keeps time, this frame is not drawn
        state.raf = requestAnimationFrame(frame);
        return;
      }
      gpu.render(engine);
      const t2 = performance.now();
      state.frames += 1;
      state.tpf.push(ran);
      state.stepMs.push(t1 - t0);
      state.renderMs.push(t2 - t1);
      if (state.marks['first-frame-submitted'] === undefined) {
        mark('first-frame-submitted');
        requestAnimationFrame(() => mark('first-frame-presented'));
      }
      if (state.frames % 10 === 1) updateHud();
      if (kind === 'replay' && engine.replay_done()) {
        updateHud();
        state.running = false;
        if (opts.onDone) opts.onDone();
        return;
      }
      if (opts.maxFrames && state.frames >= opts.maxFrames) {
        state.running = false;
        if (opts.onDone) opts.onDone();
        return;
      }
      state.raf = requestAnimationFrame(frame);
    } catch (e) {
      onCrash(e, opts);
    }
  };
  state.raf = requestAnimationFrame(frame);
}

function resetFrameStats() {
  state.waitRaf = 0; state.skippedRender = 0;
  state.frames = 0; state.tpf = []; state.stepMs = []; state.renderMs = [];
  state.faultShown = false; state.replayBadShown = false;
}

// After a trap, promise callbacks the dead instance registered earlier (wgpu's
// onSubmittedWorkDone, device-lost, uncaptured-error handlers) still fire and throw
// "Module terminated" from wasm-bindgen's closure trampolines: expected, swallowed, counted.
function lateTrapError(ev, msg) {
  if (state.crash && /Module terminated/.test(msg)) {
    state.crash.lateCallbacks = (state.crash.lateCallbacks || 0) + 1;
    ev.preventDefault();
  }
}
addEventListener('error', (e) => lateTrapError(e, String(e.message || e.error)));
addEventListener('unhandledrejection', (e) => lateTrapError(e, String(e.reason)));

async function onCrash(e, opts) {
  stopLoop();
  const panic = globalThis.__pinWebPanic;
  state.crash = { error: String(e), panic: panic || null, at: performance.now(), generation };
  // Any later call into the trapped instance must fail (wasm-bindgen marks it terminated).
  try { engine.status(); state.crash.afterTrap = 'call succeeded'; } catch (e2) { state.crash.afterTrap = String(e2); }
  showError(`Engine crashed: ${panic || e} - restarting the engine...`);
  if (opts && opts.onDone) { opts.onDone(); return; }
  await restart();
}

async function restart() {
  // Discard the trapped instance: new canvas (a canvas keeps its first context),
  // new wasm instance, reload the current bundle, continue live play.
  const old = canvasEl();
  const c = document.createElement('canvas');
  c.id = 'view';
  old.replaceWith(c);
  attachPointer(c);
  gpu = null; engine = null; state.gpuInfo = null;
  await loadModule();
  await createGpu();
  state.restarts += 1;
  if (currentBundle) {
    const r = loadBytes(currentBundle, currentName);
    state.restartLoad = r;
    if (r.ok) showError(`Engine restarted after a crash (instance ${generation}); table reloaded.`);
  }
}

function loadBytes(bytes, name) {
  const memBefore = memBytes();
  const t0 = performance.now();
  try {
    // The Uint8Array is copied once into linear memory and moved into the loader (Vec<u8>).
    const info = JSON.parse(engine.load_bundle(bytes, LIVE_SEED));
    const loadMs = performance.now() - t0;
    const build = JSON.parse(gpu.build(engine));
    currentBundle = bytes; currentName = name;
    clearError();
    resetFrameStats();
    startLoop('live');
    if (P.autostart) pressFor('start', 120);
    const r = { ok: true, name, info, build, loadMs, memBefore, memAfter: memBytes() };
    state.loads.push(r);
    return r;
  } catch (e) {
    const r = { ok: false, name, error: String(e), memBefore, memAfter: memBytes() };
    state.loads.push(r);
    showError(`Bundle rejected: ${e}`);
    return r;
  }
}

async function loadFile(file, via) {
  if (file.size > MAX_BUNDLE) {
    const msg = `bundle file is too large (${file.size} bytes; the web limit is ${MAX_BUNDLE})`;
    showError(`Bundle rejected: ${msg}`);
    const r = { ok: false, name: file.name, via, error: msg, copied: false, memAfter: memBytes() };
    state.loads.push(r);
    return r;
  }
  const t0 = performance.now();
  const bytes = new Uint8Array(await file.arrayBuffer());
  const readMs = performance.now() - t0;
  const r = loadBytes(bytes, file.name);
  r.via = via; r.readMs = readMs; r.size = file.size;
  return r;
}

function pressFor(name, ms) {
  engine.input(name, true);
  state.inputs.push({ name, pressed: true, t: performance.now(), via: 'auto' });
  setTimeout(() => {
    try { engine.input(name, false); } catch (_) { /* engine replaced */ }
  }, ms);
}

function attachPointer(c) {
  const active = new Map();
  const zoneOf = (e) => {
    const r = c.getBoundingClientRect();
    const x = (e.clientX - r.left) / r.width;
    const y = (e.clientY - r.top) / r.height;
    if (y > (innerWidth > innerHeight ? 0.82 : 0.85) && x > 0.333 && x < 0.667) return 'launch';
    return x < 0.5 ? 'left' : 'right';
  };
  c.addEventListener('pointerdown', (e) => {
    e.preventDefault();
    const name = zoneOf(e);
    active.set(e.pointerId, name);
    try { engine.input(name, true); } catch (_) { return; }
    state.inputs.push({ name, pressed: true, t: performance.now(), via: e.pointerType });
  });
  const up = (e) => {
    const name = active.get(e.pointerId);
    if (!name) return;
    active.delete(e.pointerId);
    try { engine.input(name, false); } catch (_) { return; }
    state.inputs.push({ name, pressed: false, t: performance.now(), via: e.pointerType });
  };
  c.addEventListener('pointerup', up);
  c.addEventListener('pointercancel', up); // a cancelled touch must release the flipper (S5 F1.1)
}

const KEYS = { ShiftLeft: 'left', ShiftRight: 'right', Space: 'launch', Enter: 'start', KeyZ: 'left', Slash: 'right' };
function attachKeys() {
  const on = (pressed) => (e) => {
    const name = KEYS[e.code];
    if (!name || e.repeat || !engine) return;
    e.preventDefault();
    try { engine.input(name, pressed); } catch (_) { return; }
    state.inputs.push({ name, pressed, t: performance.now(), via: 'key' });
  };
  addEventListener('keydown', on(true));
  addEventListener('keyup', on(false));
}

function attachUi() {
  $('bundleFile').addEventListener('change', async (e) => {
    const f = e.target.files && e.target.files[0];
    if (f) await loadFile(f, 'input');
    e.target.value = '';
  });
  addEventListener('dragover', (e) => { e.preventDefault(); $('dropHint').hidden = false; });
  addEventListener('dragleave', () => { $('dropHint').hidden = true; });
  addEventListener('drop', async (e) => {
    e.preventDefault();
    $('dropHint').hidden = true;
    const f = e.dataTransfer && e.dataTransfer.files && e.dataTransfer.files[0];
    if (f) await loadFile(f, 'drop');
  });
  $('tableSel').value = P.table;
  $('tableSel').addEventListener('change', async (e) => {
    const t = e.target.value;
    loadBytes(await fetchBytes(bundleUrl(t, P.rules)), t);
  });
  const sb = $('startBtn');
  sb.addEventListener('pointerdown', (e) => { e.preventDefault(); try { engine.input('start', true); } catch (_) { /* */ } });
  const sbUp = () => { try { engine.input('start', false); } catch (_) { /* */ } };
  sb.addEventListener('pointerup', sbUp);
  sb.addEventListener('pointercancel', sbUp);
  attachPointer(canvasEl());
  attachKeys();
  addEventListener('resize', () => {
    if (!gpu) return;
    const c = canvasEl();
    sizeCanvas(c);
    try { gpu.resize(c.width, c.height); } catch (e) { showError(`resize: ${e}`); }
  });
  document.addEventListener('visibilitychange', () => { state.hiddenAt = document.hidden ? performance.now() : null; });
}

function bundleUrl(table, rules) {
  return rules === 'wasm' ? `bundles/${table}_wasm.pinbundle` : `bundles/${table}.pinbundle`;
}

// Replay of a recorded scenario while rendering every frame (req 4).
async function runReplay(o) {
  const { table, rules, scenario, tpf = 250, scr = true, refs = true, bundle } = o;
  stopLoop();
  const scn = scenario || `${table}_bot60`;
  const t0 = performance.now();
  const [b, s, sc, rf] = await Promise.all([
    fetchBytes(bundle || bundleUrl(table, rules)),
    fetchBytes(`test/rec/${scn}.scn`),
    rules === 'lua' && scr ? fetchBytes(`test/rec/${scn}.scr`) : Promise.resolve(undefined),
    refs ? fetchBytes(o.refUrl || `test/refs/${scn}.${rules}.ref`) : Promise.resolve(undefined),
  ]);
  const fetchMs = performance.now() - t0;
  const info = JSON.parse(engine.load_replay(b, s, sc, rf));
  const build = JSON.parse(gpu.build(engine));
  resetFrameStats();
  const tStart = performance.now();
  await new Promise((resolve) => startLoop('replay', { tpf, maxFrames: o.maxFrames, onDone: resolve }));
  const wallMs = performance.now() - tStart;
  const st = state.crash ? null : JSON.parse(engine.status());
  const sum = (a) => a.reduce((x, y) => x + y, 0);
  const med = (a) => { const v = [...a].sort((x, y) => x - y); return v.length ? v[Math.floor(v.length / 2)] : 0; };
  const r = {
    scenario: scn, table, rules, tpf, info, build, fetchMs, wallMs, status: st,
    frames: state.frames, ticks: sum(state.tpf), tpfMin: Math.min(...state.tpf), tpfMax: Math.max(...state.tpf),
    stepMsMedian: med(state.stepMs), renderMsMedian: med(state.renderMs), waitRaf: state.waitRaf,
    gpuErrors: JSON.parse(gpu.errors()), crash: state.crash, memBytes: memBytes(),
  };
  state.replay = r;
  return r;
}

async function main() {
  attachUi();
  await loadModule();
  if (P.mode === 'idle') {
    await createGpu();
    return;
  }
  const tb = performance.now();
  const bundleP = fetchBytes(bundleUrl(P.table, P.rules));
  await createGpu();
  const bytes = await bundleP;
  mark('bundle-fetched');
  state.bundleFetchMs = performance.now() - tb;
  const memBefore = memBytes();
  const info = JSON.parse(engine.load_bundle(bytes, LIVE_SEED));
  mark('bundle-loaded'); // zip parse + validation + physics build + Lua/rules on_init
  currentBundle = bytes; currentName = P.table;
  buildRenderer();
  state.loads.push({ ok: true, name: P.table, via: 'builtin', info, memBefore, memAfter: memBytes() });
  resetFrameStats();
  startLoop('live');
  if (P.autostart) pressFor('start', 120);
}

window.S9 = {
  state,
  ready: main().then(() => { mark('ready'); return true; }, (e) => {
    showError(`Startup failed: ${e}`);
    state.startupError = String(e);
    return false;
  }),
  status: () => JSON.parse(engine.status()),
  memory: memBytes,
  heap: () => JSON.parse(wasm.heap_stats()),
  gpuInfo: () => state.gpuInfo,
  gpuErrors: () => JSON.parse(gpu.errors()),
  layout: () => JSON.parse(gpu.layout()),
  runReplay,
  stop: stopLoop,
  live: () => { resetFrameStats(); startLoop('live'); },
  loadUrl: async (url) => loadBytes(await fetchBytes(url), url),
  probe: (name, ...args) => wasm[name](...args),
  armTrap: (ticksAhead) => engine.arm_trap(JSON.parse(engine.status()).tick + ticksAhead),
  measureStack: (on) => engine.set_measure_stack(on),
  stackReport: () => JSON.parse(engine.stack_report()),
  input: (n, p) => engine.input(n, p),
  generation: () => generation,
  engine: () => engine,
  gpu: () => gpu,
  fetchBytes,
  startLoop,
};
