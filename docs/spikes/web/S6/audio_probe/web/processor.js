function fnv(h, f32) { const u8 = new Uint8Array(f32.buffer, f32.byteOffset, f32.byteLength);
  for (const b of u8) { h ^= BigInt(b); h = (h * 0x100000001b3n) & 0xffffffffffffffffn; } return h; }
class S6Mixer extends AudioWorkletProcessor {
  constructor(opts) {
    super();
    const o = opts.processorOptions;
    this.script = o.script; this.quanta = o.quanta; this.q = 0; this.h = 0xcbf29ce484222325n; this.sizes = new Set();
    try {
      this.mod = new WebAssembly.Module(o.bytes);           // sync compile inside AudioWorkletGlobalScope
      this.inst = new WebAssembly.Instance(this.mod, {});
      this.x = this.inst.exports; this.st = this.x.s6_new(sampleRate, 128); this.err = null;
    } catch (e) { this.err = String(e); }
  }
  process(inputs, outputs) {
    const out = outputs[0]; const n = out[0].length; this.sizes.add(n);
    if (this.err) { if (this.q++ === 0) this.port.postMessage({ error: this.err }); return true; }
    for (const [at, id, g] of this.script) if (at === this.q) this.x.s6_trigger(this.st, id, g);
    const p = this.x.s6_render(this.st, n);
    const v = new Float32Array(this.x.memory.buffer, p, n * 2);
    this.h = fnv(this.h, v);
    for (let i = 0; i < n; i++) { out[0][i] = v[2*i]; out[1][i] = v[2*i+1]; }
    this.q++;
    if (this.q === this.quanta) this.port.postMessage({ quanta: this.q, render_quantum_sizes: [...this.sizes], fnv: this.h.toString(16).padStart(16, '0'), sampleRate });
    return true;
  }
}
registerProcessor('s6-mixer', S6Mixer);
