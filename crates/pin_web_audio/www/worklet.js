// pin_web_audio AudioWorklet processor `pin-web-audio` (REQ-WEB-022).
//
// Runs pin_audio's Mixer (the pin_web_audio wasm module, no imports) inside
// the AudioWorkletGlobalScope. Plain ES2020, no dependencies, strict-CSP safe
// (no eval, no new Function): the module is compiled synchronously with
// `new WebAssembly.Module`, which needs only 'wasm-unsafe-eval'.
//
// Node options: numberOfInputs 0, numberOfOutputs 1, outputChannelCount [2],
// processorOptions { wasm: ArrayBuffer, bufferFrames = 128,
// triggerCapacity = 64, budgetMiB = 64 }.
//
// Port messages (main -> worklet):
//   {type:'register', id, bus, channels, frames, pcm: ArrayBuffer}
//       -> {type:'registered', id, code}   (the pwa_register code; -5 when
//          the staging area cannot hold the PCM: pwa_stage returned null)
//   {type:'cmds', buffer: ArrayBuffer}     16-byte records -> pwa_submit
//   {type:'stats'} -> {type:'stats', stats:[stat0..stat8],
//                      quantumFrames:{min, max}, processCalls}
// Posts {type:'ready', sampleRate} once constructed, or
// {type:'error', error} once on failure (then outputs silence forever).
// Unknown or malformed messages are ignored.

'use strict';

const CMD_BYTES = 4096;
const STATS = 9;

function num(v, dflt) {
  return typeof v === 'number' && Number.isFinite(v) ? v : dflt;
}

class PinWebAudioProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    this.x = null;
    this.e = 0;
    this.heap = null;
    this.failed = false;
    this.minN = 0;
    this.maxN = 0;
    this.calls = 0;
    this.port.onmessage = (ev) => {
      try {
        this.onMessage(ev.data);
      } catch (_) {
        // Malformed messages are ignored.
      }
    };
    try {
      const o = (options && options.processorOptions) || {};
      this.bufferFrames = num(o.bufferFrames, 128) >>> 0;
      const triggerCapacity = num(o.triggerCapacity, 64) >>> 0;
      const budgetMiB = num(o.budgetMiB, 64) >>> 0;
      const module = new WebAssembly.Module(o.wasm);
      const instance = new WebAssembly.Instance(module, {});
      const x = instance.exports;
      const e = x.pwa_new(sampleRate, this.bufferFrames, triggerCapacity, budgetMiB) >>> 0;
      if (e === 0) {
        throw new Error('pwa_new rejected sampleRate=' + sampleRate +
          ' bufferFrames=' + this.bufferFrames + ' triggerCapacity=' + triggerCapacity);
      }
      this.x = x;
      this.e = e;
      this.view();
      this.port.postMessage({ type: 'ready', sampleRate });
    } catch (err) {
      this.fail(err);
    }
  }

  fail(err) {
    if (!this.failed) {
      this.failed = true;
      this.x = null;
      this.port.postMessage({ type: 'error', error: String(err) });
    }
  }

  // The Float32Array view of wasm memory, re-created only when memory grew.
  view() {
    const buf = this.x.memory.buffer;
    if (this.heap === null || this.heap.buffer !== buf) {
      this.heap = new Float32Array(buf);
    }
    return this.heap;
  }

  onMessage(m) {
    if (this.x === null || m === null || typeof m !== 'object') return;
    const x = this.x;
    if (m.type === 'register') {
      if (!(m.pcm instanceof ArrayBuffer)) return;
      const n = Math.floor(m.pcm.byteLength / 4);
      const p = x.pwa_stage(this.e, n) >>> 0;
      let code = -5;
      if (p !== 0) {
        this.view().set(new Float32Array(m.pcm, 0, n), p >>> 2);
        code = x.pwa_register(this.e, m.id >>> 0, m.bus >>> 0, m.channels >>> 0, m.frames >>> 0);
      }
      this.port.postMessage({ type: 'registered', id: m.id, code });
    } else if (m.type === 'cmds') {
      if (!(m.buffer instanceof ArrayBuffer)) return;
      const src = new Uint8Array(m.buffer);
      for (let off = 0; off < src.length; off += CMD_BYTES) {
        const len = Math.min(CMD_BYTES, src.length - off);
        const p = x.pwa_cmd_buffer(this.e) >>> 0;
        new Uint8Array(x.memory.buffer, p, len).set(src.subarray(off, off + len));
        x.pwa_submit(this.e, len);
      }
    } else if (m.type === 'stats') {
      const stats = [];
      for (let i = 0; i < STATS; i++) stats.push(x.pwa_stat(this.e, i));
      this.port.postMessage({
        type: 'stats',
        stats,
        quantumFrames: { min: this.minN, max: this.maxN },
        processCalls: this.calls,
      });
    }
  }

  process(inputs, outputs) {
    const out = outputs[0];
    if (!out || out.length === 0) return true;
    const left = out[0];
    const right = out.length > 1 ? out[1] : null;
    const n = left.length;
    if (this.x === null) {
      left.fill(0);
      if (right !== null) right.fill(0);
      return true;
    }
    this.calls++;
    if (this.calls === 1 || n < this.minN) this.minN = n;
    if (n > this.maxN) this.maxN = n;
    const x = this.x;
    for (let done = 0; done < n;) {
      const k = Math.min(this.bufferFrames, n - done);
      const p = x.pwa_render(this.e, k) >>> 0;
      if (p === 0) {
        left.fill(0, done);
        if (right !== null) right.fill(0, done);
        break;
      }
      const h = this.view();
      let s = p >>> 2;
      if (right !== null) {
        for (let i = 0; i < k; i++) {
          left[done + i] = h[s];
          right[done + i] = h[s + 1];
          s += 2;
        }
      } else {
        for (let i = 0; i < k; i++) {
          left[done + i] = h[s];
          s += 2;
        }
      }
      done += k;
    }
    return true;
  }
}

registerProcessor('pin-web-audio', PinWebAudioProcessor);
