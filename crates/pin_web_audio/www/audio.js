// pin_web_audio main-thread helper (REQ-WEB-022, D43). ES module, ES2020,
// no dependencies, strict-CSP safe (no eval, no inline script).
//
//   createWebAudio(ctx, { wasmBytes, budgetMiB = 64, triggerCapacity = 64,
//                         forceFallback = false, base = '', maxBlocks })
//     -> Promise<controller>
//   logLatency(ctx, log = console.info) -> { baseLatency, outputLatency }
//   resumeOnGesture(ctx, target = document) -> Promise<ctx.state>
//
// The controller: { mode: 'worklet' | 'fallback', node,
//   register(id, bus, channels, Float32Array) -> Promise<code>,
//   trigger(id, gain, at?), stats() -> Promise<number[]>,
//   latency() -> { baseLatency, outputLatency }, prerender(nQuanta) -> Promise,
//   close() }.
//
// 'worklet' runs the mixer in the `pin-web-audio` AudioWorkletProcessor.
// 'fallback' (when the worklet cannot instantiate wasm, or forceFallback)
// runs the same wasm on the main thread and feeds rendered planar blocks to
// the JS-only `pin-web-audio-fallback` processor by transferred
// postMessage (no SharedArrayBuffer; no COOP/COEP is needed). prerender(n)
// renders n quanta ahead (how an OfflineAudioContext is driven
// deterministically); a realtime context is kept at least 8 blocks ahead by
// a setInterval pump.

const QUANTUM = 128;
const NEXT_QUANTUM = 0xffffffff;
const PUMP_AHEAD = 8;
const STATS = 9;

/** Encodes one 16-byte trigger record (little-endian op, id, gain, at). */
export function encodeTrigger(id, gain, at = NEXT_QUANTUM) {
  const buf = new ArrayBuffer(16);
  const v = new DataView(buf);
  v.setUint32(0, 1, true);
  v.setUint32(4, id >>> 0, true);
  v.setFloat32(8, gain, true);
  v.setUint32(12, at === undefined || at === null ? NEXT_QUANTUM : at >>> 0, true);
  return buf;
}

function toArrayBuffer(bytes) {
  if (bytes instanceof ArrayBuffer) return bytes.slice(0);
  if (ArrayBuffer.isView(bytes)) {
    return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
  }
  throw new TypeError('wasmBytes must be an ArrayBuffer or a typed array');
}

function latencyOf(ctx) {
  const pick = (v) => (typeof v === 'number' ? v : null);
  return { baseLatency: pick(ctx.baseLatency), outputLatency: pick(ctx.outputLatency) };
}

function isOffline(ctx) {
  return typeof OfflineAudioContext !== 'undefined' && ctx instanceof OfflineAudioContext;
}

/** Logs `pin_web_audio latency: baseLatency=<s|n/a> outputLatency=<s|n/a>`. */
export function logLatency(ctx, log = console.info) {
  const l = latencyOf(ctx);
  const f = (v) => (v === null ? 'n/a' : String(v));
  log(`pin_web_audio latency: baseLatency=${f(l.baseLatency)} outputLatency=${f(l.outputLatency)}`);
  return l;
}

/**
 * Resolves with `ctx.state` once the context runs. The context is not
 * resumed before the first `pointerdown`, `keydown` or `touchend` on
 * `target`; that gesture calls `ctx.resume()` and removes the listeners.
 */
export function resumeOnGesture(ctx, target = document) {
  return new Promise((resolve, reject) => {
    if (ctx.state === 'running') {
      resolve(ctx.state);
      return;
    }
    const events = ['pointerdown', 'keydown', 'touchend'];
    const onGesture = () => {
      for (const ev of events) target.removeEventListener(ev, onGesture, true);
      ctx.resume().then(() => resolve(ctx.state), reject);
    };
    for (const ev of events) target.addEventListener(ev, onGesture, true);
  });
}

// Replies of one kind resolve in order (the port preserves message order).
class Replies {
  constructor() {
    this.q = [];
  }
  wait() {
    return new Promise((resolve) => this.q.push(resolve));
  }
  resolve(v) {
    const r = this.q.shift();
    if (r) r(v);
  }
}

async function createWorklet(ctx, o) {
  await ctx.audioWorklet.addModule(o.base + 'worklet.js');
  const node = new AudioWorkletNode(ctx, 'pin-web-audio', {
    numberOfInputs: 0,
    numberOfOutputs: 1,
    outputChannelCount: [2],
    processorOptions: {
      wasm: toArrayBuffer(o.wasmBytes),
      bufferFrames: QUANTUM,
      triggerCapacity: o.triggerCapacity,
      budgetMiB: o.budgetMiB,
    },
  });
  const registered = new Replies();
  const stats = new Replies();
  const first = new Promise((resolve) => {
    node.port.onmessage = (ev) => {
      const m = ev.data || {};
      if (m.type === 'ready' || m.type === 'error') resolve(m);
      else if (m.type === 'registered') registered.resolve(m.code);
      else if (m.type === 'stats') stats.resolve(m);
    };
  });
  const m = await first;
  if (m.type !== 'ready') {
    node.port.close();
    throw new Error('pin-web-audio worklet: ' + m.error);
  }
  return {
    mode: 'worklet',
    node,
    register(id, bus, channels, pcm) {
      const copy = new Float32Array(pcm);
      const done = registered.wait();
      node.port.postMessage(
        { type: 'register', id, bus, channels, frames: Math.floor(copy.length / channels), pcm: copy.buffer },
        [copy.buffer],
      );
      return done;
    },
    trigger(id, gain, at) {
      const buffer = encodeTrigger(id, gain, at);
      node.port.postMessage({ type: 'cmds', buffer }, [buffer]);
    },
    async stats() {
      const done = stats.wait();
      node.port.postMessage({ type: 'stats' });
      return (await done).stats;
    },
    // The worklet's own stats reply (stats, quantumFrames, processCalls).
    workletStats() {
      const done = stats.wait();
      node.port.postMessage({ type: 'stats' });
      return done;
    },
    latency: () => latencyOf(ctx),
    prerender: () => Promise.resolve(),
    close() {
      node.disconnect();
      node.port.close();
    },
  };
}

async function createFallback(ctx, o) {
  await ctx.audioWorklet.addModule(o.base + 'fallback-worklet.js');
  const offline = isOffline(ctx);
  const maxBlocks = o.maxBlocks || (offline ? Math.max(64, Math.ceil(ctx.length / QUANTUM)) : 64);
  const node = new AudioWorkletNode(ctx, 'pin-web-audio-fallback', {
    numberOfInputs: 0,
    numberOfOutputs: 1,
    outputChannelCount: [2],
    processorOptions: { blockFrames: QUANTUM, maxBlocks },
  });
  const { instance } = await WebAssembly.instantiate(toArrayBuffer(o.wasmBytes), {});
  const x = instance.exports;
  const e = x.pwa_new(ctx.sampleRate, QUANTUM, o.triggerCapacity, o.budgetMiB) >>> 0;
  if (e === 0) throw new Error('pwa_new rejected sampleRate=' + ctx.sampleRate);
  const procStats = new Replies();
  node.port.onmessage = (ev) => {
    const m = ev.data || {};
    if (m.type === 'stats') procStats.resolve(m);
  };
  const askProc = () => {
    const done = procStats.wait();
    node.port.postMessage({ type: 'stats' });
    return done;
  };
  let posted = 0;
  let startQuantum = -1;
  let closed = false;
  const postBlock = () => {
    const p = x.pwa_render(e, QUANTUM) >>> 0;
    const block = new Float32Array(QUANTUM * 2);
    const h = new Float32Array(x.memory.buffer, p, QUANTUM * 2);
    for (let i = 0; i < QUANTUM; i++) {
      block[i] = h[2 * i];
      block[QUANTUM + i] = h[2 * i + 1];
    }
    node.port.postMessage({ type: 'block', buffer: block.buffer }, [block.buffer]);
    if (startQuantum < 0) startQuantum = Math.floor((ctx.currentTime * ctx.sampleRate) / QUANTUM);
    posted++;
  };
  let timer = null;
  if (!offline) {
    timer = setInterval(() => {
      if (closed) return;
      const now = Math.floor((ctx.currentTime * ctx.sampleRate) / QUANTUM);
      const consumed = startQuantum < 0 ? 0 : Math.max(0, now - startQuantum);
      let ahead = posted - consumed;
      while (ahead < PUMP_AHEAD && ahead < maxBlocks) {
        postBlock();
        ahead++;
      }
    }, 10);
  }
  return {
    mode: 'fallback',
    node,
    register(id, bus, channels, pcm) {
      const n = pcm.length;
      const p = x.pwa_stage(e, n) >>> 0;
      if (p === 0) return Promise.resolve(-5);
      new Float32Array(x.memory.buffer, p, n).set(pcm);
      return Promise.resolve(x.pwa_register(e, id >>> 0, bus >>> 0, channels >>> 0, Math.floor(n / channels) >>> 0));
    },
    trigger(id, gain, at) {
      const p = x.pwa_cmd_buffer(e) >>> 0;
      new Uint8Array(x.memory.buffer, p, 16).set(new Uint8Array(encodeTrigger(id, gain, at)));
      x.pwa_submit(e, 16);
    },
    async stats() {
      await askProc(); // every earlier block has reached the processor
      const s = [];
      for (let i = 0; i < STATS; i++) s.push(x.pwa_stat(e, i));
      return s;
    },
    // The fallback processor's { underruns, queued, dropped }.
    processorStats: askProc,
    latency: () => latencyOf(ctx),
    async prerender(nQuanta) {
      for (let i = 0; i < nQuanta; i++) postBlock();
      await askProc();
    },
    close() {
      closed = true;
      if (timer !== null) clearInterval(timer);
      node.disconnect();
      node.port.close();
      x.pwa_free(e);
    },
  };
}

/** Creates the web audio path; see the module header. */
export async function createWebAudio(ctx, options = {}) {
  const o = {
    wasmBytes: options.wasmBytes,
    budgetMiB: options.budgetMiB === undefined ? 64 : options.budgetMiB,
    triggerCapacity: options.triggerCapacity === undefined ? 64 : options.triggerCapacity,
    forceFallback: !!options.forceFallback,
    base: options.base || '',
    maxBlocks: options.maxBlocks,
  };
  if (!o.forceFallback) {
    try {
      return await createWorklet(ctx, o);
    } catch (_) {
      // Fall through to the main-thread mixer (U6).
    }
  }
  return createFallback(ctx, o);
}
