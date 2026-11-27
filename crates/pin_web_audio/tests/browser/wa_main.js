// pin_web_audio browser test page script (REQ-WEB-022). Served under the
// strict CSP by wa_run.cjs; driven through page.evaluate.
import { createWebAudio, logLatency, resumeOnGesture } from '/www/audio.js';

const QUANTA = 600;
const RATE = 48000;
// (quantum, id, gain): the S6 script, all three buses.
const SCRIPT = [[0, 2, 0.6], [3, 1, 0.9], [10, 1, 0.7], [11, 3, 1.0], [40, 1, 1.2], [41, 1, 0.3],
  [42, 1, 0.5], [90, 3, 0.8], [91, 2, 0.4], [200, 1, 1.0], [201, 7, 1.0], [300, 1, NaN]];

function fnv(h, f32) {
  const u8 = new Uint8Array(f32.buffer, f32.byteOffset, f32.byteLength);
  for (let i = 0; i < u8.length; i++) {
    h ^= BigInt(u8[i]);
    h = (h * 0x100000001b3n) & 0xffffffffffffffffn;
  }
  return h;
}

async function bytes(url) {
  const r = await fetch(url);
  if (!r.ok) throw new Error('fetch ' + url + ': ' + r.status);
  return r.arrayBuffer();
}

// Renders the script in an OfflineAudioContext and digests the AudioBuffer.
window.runDigest = async (forceFallback) => {
  const wasmBytes = await bytes('/data/pin_web_audio.wasm');
  const mod = new WebAssembly.Module(wasmBytes);
  const ctx = new OfflineAudioContext({ numberOfChannels: 2, length: 128 * QUANTA, sampleRate: RATE });
  const audio = await createWebAudio(ctx, { wasmBytes, base: '/www/', forceFallback });
  const codes = [];
  codes.push(await audio.register(1, 2, 1, new Float32Array(await bytes('/data/pcm1.f32'))));
  codes.push(await audio.register(2, 0, 1, new Float32Array(await bytes('/data/pcm2.f32'))));
  codes.push(await audio.register(3, 1, 2, new Float32Array(await bytes('/data/pcm3.f32'))));
  for (const [at, id, g] of SCRIPT) audio.trigger(id, g, at);
  await audio.prerender(QUANTA);
  const before = await audio.stats(); // round trip: every message above has arrived
  audio.node.connect(ctx.destination);
  const buf = await ctx.startRendering();
  const L = buf.getChannelData(0);
  const R = buf.getChannelData(1);
  const inter = new Float32Array(L.length * 2);
  for (let i = 0; i < L.length; i++) {
    inter[2 * i] = L[i];
    inter[2 * i + 1] = R[i];
  }
  const digest = fnv(0xcbf29ce484222325n, inter).toString(16).padStart(16, '0');
  const result = {
    mode: audio.mode,
    digest,
    frames: L.length,
    channels: buf.numberOfChannels,
    sampleRate: buf.sampleRate,
    codes,
    statsBefore: before,
    imports: WebAssembly.Module.imports(mod).length,
    exports: WebAssembly.Module.exports(mod).map((e) => e.name).sort(),
  };
  if (audio.mode === 'worklet') {
    const w = await audio.workletStats();
    result.stats = w.stats;
    result.quantumFrames = w.quantumFrames;
    result.processCalls = w.processCalls;
  } else {
    result.stats = await audio.stats();
    result.processor = await audio.processorStats();
  }
  audio.close();
  return result;
};

// The worklet's failure path: a module it cannot instantiate posts one
// {type:'error'} and the processor outputs silence; messages are ignored.
window.runErrorPath = async () => {
  const ctx = new OfflineAudioContext({ numberOfChannels: 2, length: 128 * 8, sampleRate: RATE });
  await ctx.audioWorklet.addModule('/www/worklet.js');
  const node = new AudioWorkletNode(ctx, 'pin-web-audio', {
    numberOfInputs: 0, numberOfOutputs: 1, outputChannelCount: [2],
    processorOptions: { wasm: new ArrayBuffer(8) },
  });
  const messages = [];
  node.port.onmessage = (e) => messages.push(e.data);
  node.port.postMessage({ type: 'stats' });
  node.port.postMessage({ type: 'cmds', buffer: 'not a buffer' });
  node.port.postMessage(null);
  node.connect(ctx.destination);
  const buf = await ctx.startRendering();
  await new Promise((r) => setTimeout(r, 50));
  let silent = true;
  for (let c = 0; c < 2; c++) for (const v of buf.getChannelData(c)) if (v !== 0) silent = false;
  return { messages, silent };
};

// Gesture test (`?gesture`): a realtime AudioContext under the default
// autoplay policy, created at module load, i.e. before any user activation
// (the test driver's page.evaluate calls carry a user gesture, so the
// context must not be created from one).
if (new URLSearchParams(location.search).has('gesture')) {
  const ctx = new AudioContext();
  window.gestureInitial = ctx.state;
  window.gestureCtx = ctx;
  window.gestureDone = resumeOnGesture(ctx, document).then((state) => {
    const l = logLatency(ctx);
    return { state, latency: l };
  });
}
window.gestureState = () => window.gestureCtx.state;
window.gestureResult = () => window.gestureDone;
