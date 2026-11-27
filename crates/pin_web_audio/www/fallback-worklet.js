// pin_web_audio fallback processor `pin-web-audio-fallback` (REQ-WEB-022, U6).
//
// For browsers that cannot instantiate wasm inside an AudioWorklet: the
// same mixer runs in a wasm instance on the main thread (audio.js) and this
// JS-only processor copies its blocks to the output, bit for bit.
//
// processorOptions { blockFrames = 128, maxBlocks = 64 }.
// Port messages (main -> worklet):
//   {type:'block', buffer: ArrayBuffer}  one planar block: blockFrames left
//       floats then blockFrames right floats (transferred). Up to maxBlocks
//       are queued in preallocated storage; more are dropped and counted.
//   {type:'stats'} -> {type:'stats', underruns, queued, dropped}
// `process` copies one block per quantum; an empty queue outputs silence and
// counts an underrun, but only after the first block has arrived.

'use strict';

function count(v, dflt) {
  return typeof v === 'number' && Number.isFinite(v) && v >= 1 ? Math.floor(v) : dflt;
}

class PinWebAudioFallbackProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const o = (options && options.processorOptions) || {};
    this.blockFrames = count(o.blockFrames, 128);
    this.maxBlocks = count(o.maxBlocks, 64);
    this.store = new Float32Array(this.blockFrames * 2 * this.maxBlocks);
    this.head = 0; // next block to play
    this.queued = 0;
    this.started = false;
    this.underruns = 0;
    this.dropped = 0;
    this.port.onmessage = (ev) => {
      try {
        this.onMessage(ev.data);
      } catch (_) {
        // Malformed messages are ignored.
      }
    };
  }

  onMessage(m) {
    if (m === null || typeof m !== 'object') return;
    if (m.type === 'block') {
      if (!(m.buffer instanceof ArrayBuffer)) return;
      const len = this.blockFrames * 2;
      if (m.buffer.byteLength < len * 4) return;
      this.started = true;
      if (this.queued >= this.maxBlocks) {
        this.dropped++;
        return;
      }
      const slot = (this.head + this.queued) % this.maxBlocks;
      this.store.set(new Float32Array(m.buffer, 0, len), slot * len);
      this.queued++;
    } else if (m.type === 'stats') {
      this.port.postMessage({
        type: 'stats',
        underruns: this.underruns,
        queued: this.queued,
        dropped: this.dropped,
      });
    }
  }

  process(inputs, outputs) {
    const out = outputs[0];
    if (!out || out.length === 0) return true;
    const left = out[0];
    const right = out.length > 1 ? out[1] : null;
    if (this.queued === 0) {
      left.fill(0);
      if (right !== null) right.fill(0);
      if (this.started) this.underruns++;
      return true;
    }
    const bf = this.blockFrames;
    const base = this.head * bf * 2;
    const n = Math.min(left.length, bf);
    const s = this.store;
    for (let i = 0; i < n; i++) left[i] = s[base + i];
    if (right !== null) for (let i = 0; i < n; i++) right[i] = s[base + bf + i];
    if (n < left.length) {
      left.fill(0, n);
      if (right !== null) right.fill(0, n);
    }
    this.head = (this.head + 1) % this.maxBlocks;
    this.queued--;
    return true;
  }
}

registerProcessor('pin-web-audio-fallback', PinWebAudioFallbackProcessor);
