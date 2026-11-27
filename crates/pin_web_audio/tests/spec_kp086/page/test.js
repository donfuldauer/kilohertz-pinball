// Independent Tester, KP-086 (#180), REQ-WEB-022 / D43: browser-side driver.
//
// Loaded as an external ES module (the page CSP forbids inline script). It
// drives the black-box `www/` files (audio.js, worklet.js,
// fallback-worklet.js) and the built wasm, and computes the FNV-1a 64 digest
// of the rendered AudioBuffer (interleaved L, R; little-endian f32 bytes).
// Expected digests are computed natively in Rust and checked by the runner.

import { createWebAudio, logLatency, resumeOnGesture } from '/www/audio.js';

const QUANTUM = 128;
const cfgP = fetch('/fixture/config.json').then((r) => r.json());
let wasmCache = null;

async function wasmBytes() {
  if (!wasmCache) wasmCache = await (await fetch('/wasm/pin_web_audio.wasm')).arrayBuffer();
  return wasmCache.slice(0);
}

async function pcm(file) {
  return new Float32Array(await (await fetch('/fixture/' + file)).arrayBuffer());
}

function f32FromBits(bits) {
  return new Float32Array(new Uint32Array([bits >>> 0]).buffer)[0];
}

// Hand-built 16-byte little-endian records: [op, id, gainBits, at].
function records(list) {
  const b = new ArrayBuffer(16 * list.length);
  const v = new DataView(b);
  list.forEach((r, i) => {
    for (let k = 0; k < 4; k++) v.setUint32(i * 16 + 4 * k, r[k] >>> 0, true);
  });
  return b;
}

// FNV-1a 64 over interleaved L, R f32 little-endian bytes, as two u32 halves.
function fnv(buf) {
  const L = buf.getChannelData(0);
  const R = buf.getChannelData(1);
  const inter = new Float32Array(L.length * 2);
  for (let i = 0; i < L.length; i++) {
    inter[2 * i] = L[i];
    inter[2 * i + 1] = R[i];
  }
  const bytes = new Uint8Array(inter.buffer);
  let hi = 0xcbf29ce4 >>> 0;
  let lo = 0x84222325 >>> 0;
  for (let i = 0; i < bytes.length; i++) {
    lo = (lo ^ bytes[i]) >>> 0;
    // h * (2^40 + 0x1b3) mod 2^64
    const a = lo * 0x1b3;
    const carry = Math.floor(a / 4294967296);
    const nlo = a >>> 0;
    hi = (hi * 0x1b3 + carry + ((lo << 8) >>> 0)) >>> 0;
    lo = nlo;
  }
  return hi.toString(16).padStart(8, '0') + lo.toString(16).padStart(8, '0');
}

// A message inbox over a MessagePort with predicate waits (bounded).
function inbox(port) {
  const msgs = [];
  const waiters = [];
  port.onmessage = (e) => {
    msgs.push(e.data);
    for (const w of waiters.slice()) w();
  };
  return {
    msgs,
    next(pred, ms = 10000, from = 0) {
      return new Promise((resolve, reject) => {
        let done = false;
        const check = () => {
          if (done) return;
          for (let i = from; i < msgs.length; i++) {
            if (pred(msgs[i])) {
              done = true;
              waiters.splice(waiters.indexOf(check), 1);
              clearTimeout(t);
              resolve(msgs[i]);
              return;
            }
          }
        };
        const t = setTimeout(() => {
          if (done) return;
          done = true;
          waiters.splice(waiters.indexOf(check), 1);
          reject(new Error('timeout waiting for a port message'));
        }, ms);
        waiters.push(check);
        check();
      });
    },
  };
}

async function newProcessor(ctx, wasm, opts = {}) {
  await ctx.audioWorklet.addModule('/www/worklet.js');
  const node = new AudioWorkletNode(ctx, 'pin-web-audio', {
    numberOfInputs: 0,
    numberOfOutputs: 1,
    outputChannelCount: [2],
    processorOptions: { wasm, bufferFrames: QUANTUM, triggerCapacity: 64, budgetMiB: 64, ...opts },
  });
  const errors = [];
  node.onprocessorerror = (e) => errors.push(String(e && (e.message || e.type)));
  const box = inbox(node.port);
  return { node, box, errors };
}

async function stats(box, port, ms = 5000) {
  const from = box.msgs.length;
  port.postMessage({ type: 'stats' });
  return box.next((d) => d && d.type === 'stats', ms, from);
}

const JUNK = () => [
  null,
  42,
  'cmds',
  [],
  {},
  { type: 'nope' },
  { type: 'cmds' },
  { type: 'cmds', buffer: 5 },
  { type: 'cmds', buffer: 'abc' },
  { type: 'cmds', buffer: new ArrayBuffer(7) },
  { type: 'cmds', buffer: new Uint8Array(16) },
  { type: 'register' },
  { type: 'register', id: 900, bus: 2, channels: 1, frames: 10, pcm: 'not a buffer' },
  { type: 'register', id: 'x', bus: 'y', channels: 1, frames: 1, pcm: new ArrayBuffer(3) },
  { type: 'register', id: 901, bus: 2, channels: 1, frames: 1e12, pcm: new ArrayBuffer(8) },
  { type: 'register', id: -1, bus: -1, channels: -1, frames: -1, pcm: new ArrayBuffer(8) },
  { type: ['cmds'] },
  { type: 'cmds', buffer: null },
  { type: 'stats', extra: true },
  new ArrayBuffer(16),
];

// Worklet processor run, driven through its own message protocol.
async function runDirect({ rate, late = false, junk = false }) {
  const cfg = await cfgP;
  const ctx = new OfflineAudioContext({ numberOfChannels: 2, length: cfg.quanta * QUANTUM, sampleRate: rate });
  const { node, box, errors } = await newProcessor(ctx, await wasmBytes());
  const first = await box.next((d) => d && (d.type === 'ready' || d.type === 'error'));
  if (first.type !== 'ready') throw new Error('processor did not become ready: ' + JSON.stringify(first));
  node.connect(ctx.destination);
  const codes = [];
  for (const s of cfg.sounds) {
    const p = await pcm(s.file);
    const from = box.msgs.length;
    node.port.postMessage(
      { type: 'register', id: s.id, bus: s.bus, channels: s.channels, frames: p.length / s.channels, pcm: p.buffer },
      [p.buffer],
    );
    codes.push((await box.next((d) => d && d.type === 'registered' && d.id === s.id, 10000, from)).code);
  }
  if (junk) for (const j of JUNK()) node.port.postMessage(j);
  const buf = records(cfg.triggers.map((t) => [1, t[1], t[2], t[0]]));
  node.port.postMessage({ type: 'cmds', buffer: buf }, [buf]);
  if (junk) for (const j of JUNK()) node.port.postMessage(j);
  const statsBefore = (await stats(box, node.port)).stats;
  let suspendedStats = null;
  if (late) {
    const L = cfg.late;
    ctx.suspend((L.quantum * QUANTUM) / rate).then(async () => {
      suspendedStats = (await stats(box, node.port)).stats;
      const b = records([[1, L.id, L.gainBits, L.at]]);
      node.port.postMessage({ type: 'cmds', buffer: b }, [b]);
      await stats(box, node.port);
      await ctx.resume();
    });
  }
  const out = await ctx.startRendering();
  let statsAfter = null;
  let quantumFrames = null;
  let processCalls = null;
  try {
    const m = await stats(box, node.port, 3000);
    statsAfter = m.stats;
    // Optional extras of the reply (not in the brief's contract).
    if (m.quantumFrames !== undefined) quantumFrames = m.quantumFrames;
    if (m.processCalls !== undefined) processCalls = m.processCalls;
  } catch (e) {
    statsAfter = null;
  }
  return {
    digest: fnv(out),
    length: out.length,
    ctxLength: ctx.length,
    channels: out.numberOfChannels,
    sampleRate: out.sampleRate,
    readySampleRate: first.sampleRate,
    codes,
    statsBefore,
    suspendedStats,
    statsAfter,
    quantumFrames,
    processCalls,
    processorErrors: errors,
    errorMessages: box.msgs.filter((d) => d && d.type === 'error').length,
  };
}

// The page helper (`createWebAudio`), worklet or forced fallback.
async function runHelper({ rate, forceFallback = false, base = '/www/' }) {
  const cfg = await cfgP;
  const ctx = new OfflineAudioContext({ numberOfChannels: 2, length: cfg.quanta * QUANTUM, sampleRate: rate });
  const wa = await createWebAudio(ctx, { wasmBytes: await wasmBytes(), forceFallback, base });
  wa.node.connect(ctx.destination);
  const codes = [];
  for (const s of cfg.sounds) {
    codes.push(await wa.register(s.id, s.bus, s.channels, await pcm(s.file)));
  }
  for (const t of cfg.triggers) wa.trigger(t[1], f32FromBits(t[2]), t[0]);
  if (wa.mode === 'fallback') await wa.prerender(cfg.quanta);
  const statsBefore = await wa.stats();
  const out = await ctx.startRendering();
  const latency = wa.latency();
  return {
    mode: wa.mode,
    digest: fnv(out),
    length: out.length,
    channels: out.numberOfChannels,
    codes,
    statsBefore,
    latencyKeys: Object.keys(latency || {}).sort(),
  };
}

// Own main-thread driver of the wasm ABI posting planar blocks to the
// JS-only `pin-web-audio-fallback` processor.
async function runOwnFallback({ rate }) {
  const cfg = await cfgP;
  const ctx = new OfflineAudioContext({ numberOfChannels: 2, length: cfg.quanta * QUANTUM, sampleRate: rate });
  await ctx.audioWorklet.addModule('/www/fallback-worklet.js');
  const node = new AudioWorkletNode(ctx, 'pin-web-audio-fallback', {
    numberOfInputs: 0,
    numberOfOutputs: 1,
    outputChannelCount: [2],
    processorOptions: { blockFrames: QUANTUM, maxBlocks: cfg.quanta },
  });
  const box = inbox(node.port);
  node.connect(ctx.destination);
  const { instance } = await WebAssembly.instantiate(await wasmBytes(), {});
  const x = instance.exports;
  const e = x.pwa_new(rate, QUANTUM, 64, 64);
  if (!e) throw new Error('pwa_new returned null');
  const codes = [];
  for (const s of cfg.sounds) {
    const p = await pcm(s.file);
    const ptr = x.pwa_stage(e, p.length) >>> 0;
    new Float32Array(x.memory.buffer, ptr, p.length).set(p);
    codes.push(x.pwa_register(e, s.id, s.bus, s.channels, p.length / s.channels));
  }
  const rec = new Uint8Array(records(cfg.triggers.map((t) => [1, t[1], t[2], t[0]])));
  const cb = x.pwa_cmd_buffer(e) >>> 0;
  new Uint8Array(x.memory.buffer, cb, rec.length).set(rec);
  const accepted = x.pwa_submit(e, rec.length) >>> 0;
  for (let q = 0; q < cfg.quanta; q++) {
    const p = x.pwa_render(e, QUANTUM) >>> 0;
    const v = new Float32Array(x.memory.buffer, p, 2 * QUANTUM);
    const block = new Float32Array(2 * QUANTUM);
    for (let i = 0; i < QUANTUM; i++) {
      block[i] = v[2 * i];
      block[QUANTUM + i] = v[2 * i + 1];
    }
    node.port.postMessage({ type: 'block', buffer: block.buffer }, [block.buffer]);
  }
  const abiStats = [];
  for (let w = 0; w < 9; w++) abiStats.push(x.pwa_stat(e, w));
  const from = box.msgs.length;
  node.port.postMessage({ type: 'stats' });
  const before = await box.next((d) => d && d.type === 'stats', 10000, from);
  const out = await ctx.startRendering();
  let after = null;
  try {
    const f2 = box.msgs.length;
    node.port.postMessage({ type: 'stats' });
    after = await box.next((d) => d && d.type === 'stats', 3000, f2);
  } catch (err) {
    after = null;
  }
  x.pwa_free(e);
  return {
    digest: fnv(out),
    length: out.length,
    codes,
    accepted,
    abiStats,
    fallbackBefore: { underruns: before.underruns, queued: before.queued, dropped: before.dropped },
    fallbackAfter: after && { underruns: after.underruns, queued: after.queued, dropped: after.dropped },
  };
}

// The wasm module itself: no imports; exports memory and the ABI.
async function moduleShape() {
  const m = await WebAssembly.compile(await wasmBytes());
  return {
    imports: WebAssembly.Module.imports(m).length,
    exports: WebAssembly.Module.exports(m).map((d) => d.name + ':' + d.kind).sort(),
  };
}

// `register` reply codes under a 1 MiB budget, through the processor.
async function registerCodes() {
  const ctx = new OfflineAudioContext({ numberOfChannels: 2, length: 4 * QUANTUM, sampleRate: 48000 });
  const { node, box } = await newProcessor(ctx, await wasmBytes(), { budgetMiB: 1 });
  const first = await box.next((d) => d && (d.type === 'ready' || d.type === 'error'));
  if (first.type !== 'ready') throw new Error('not ready: ' + JSON.stringify(first));
  const reg = async (id, bus, channels, n) => {
    const p = new Float32Array(n).fill(0.125);
    const from = box.msgs.length;
    node.port.postMessage({ type: 'register', id, bus, channels, frames: n / channels, pcm: p.buffer }, [p.buffer]);
    return (await box.next((d) => d && d.type === 'registered' && d.id === id, 10000, from)).code;
  };
  const out = {
    channels3: await reg(1, 2, 3, 30),
    overBudgetPcm: await reg(2, 2, 1, 262145),
    threeQuarterMiB: await reg(3, 2, 1, 196608),
    halfMiBOver: await reg(4, 2, 1, 131072),
    quarterMiBExact: await reg(5, 2, 2, 65536),
    bus3: await reg(6, 3, 1, 4),
    id65536: await reg(65536, 2, 1, 4),
  };
  out.stats = (await stats(box, node.port)).stats;
  return out;
}

// Bad wasm: the processor posts one {type:'error', error: String}; the
// helper falls back. Good wasm: the helper uses the worklet.
async function badAndGoodWasm() {
  const garbage = new Uint8Array(64).map((_, i) => (i * 37 + 11) & 0xff).buffer;
  const ctx = new OfflineAudioContext({ numberOfChannels: 2, length: 8 * QUANTUM, sampleRate: 48000 });
  const { node, box, errors } = await newProcessor(ctx, garbage.slice(0));
  const first = await box.next((d) => d && (d.type === 'ready' || d.type === 'error'));
  node.connect(ctx.destination);
  const out = await ctx.startRendering();
  let silent = true;
  for (const c of [0, 1]) for (const v of out.getChannelData(c)) if (v !== 0) silent = false;
  const res = {
    firstType: first.type,
    errorIsString: typeof first.error === 'string',
    errorText: String(first.error),
    errorCount: box.msgs.filter((d) => d && d.type === 'error').length,
    readyCount: box.msgs.filter((d) => d && d.type === 'ready').length,
    silent,
    processorErrors: errors,
  };
  const ctx2 = new OfflineAudioContext({ numberOfChannels: 2, length: 8 * QUANTUM, sampleRate: 48000 });
  // Bounded wait: a hang must fail the check, not stall the run.
  const TIMEOUT = Symbol('timeout');
  try {
    const r = await Promise.race([
      createWebAudio(ctx2, { wasmBytes: garbage.slice(0), base: '/www/' }),
      new Promise((res) => setTimeout(() => res(TIMEOUT), 10000)),
    ]);
    res.badOutcome = r === TIMEOUT ? 'timeout' : 'resolved:' + r.mode;
  } catch (e) {
    res.badOutcome = 'rejected';
    res.badRejectMessage = e && e.message !== undefined ? e.message : e;
  }
  const ctx3 = new OfflineAudioContext({ numberOfChannels: 2, length: 8 * QUANTUM, sampleRate: 48000 });
  const wa3 = await createWebAudio(ctx3, { wasmBytes: await wasmBytes(), base: '/www/' });
  res.goodMode = wa3.mode;
  const st = await wa3.stats();
  res.goodStatsLen = st.length;
  return res;
}

// Realtime context for the gesture and latency checks. Created by the module
// at load time (`/?gesture`), never from `page.evaluate`, which Playwright
// runs with a user activation that would let the context start by itself.
function gestureSetup() {
  window.__kpCtx = new AudioContext();
  window.__kpCreatedState = window.__kpCtx.state;
  window.__kpResolved = null;
  resumeOnGesture(window.__kpCtx, document).then((s) => {
    window.__kpResolved = s;
  });
}
if (location.search === '?gesture') gestureSetup();

function gestureStart() {
  return window.__kpCreatedState;
}

function gestureState() {
  return { state: window.__kpCtx.state, resolved: window.__kpResolved };
}

function latency() {
  const r = logLatency(window.__kpCtx);
  return {
    keys: Object.keys(r).sort(),
    baseLatency: r.baseLatency,
    outputLatency: r.outputLatency,
    baseType: r.baseLatency === null ? 'null' : typeof r.baseLatency,
    outputType: r.outputLatency === null ? 'null' : typeof r.outputLatency,
  };
}

window.kp086 = {
  runDirect,
  runHelper,
  runOwnFallback,
  moduleShape,
  registerCodes,
  badAndGoodWasm,
  gestureStart,
  gestureState,
  latency,
};
document.getElementById('status').textContent = 'ready';
