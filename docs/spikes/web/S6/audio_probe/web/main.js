const SCRIPT = [[0,2,0.6],[3,1,0.9],[10,1,0.7],[11,3,1.0],[40,1,1.2],[41,1,0.3],[42,1,0.5],[90,3,0.8],[91,2,0.4],[200,1,1.0],[201,7,1.0],[300,1,NaN]];
const QUANTA = 600;
function fnv(h, f32) { const u8 = new Uint8Array(f32.buffer, f32.byteOffset, f32.byteLength);
  for (const b of u8) { h ^= BigInt(b); h = (h * 0x100000001b3n) & 0xffffffffffffffffn; } return h; }
window.runProbe = async () => {
  const bytes = await (await fetch('s6_audio_probe.wasm')).arrayBuffer();
  const ctx = new OfflineAudioContext({ numberOfChannels: 2, length: 128 * QUANTA, sampleRate: 48000 });
  await ctx.audioWorklet.addModule('processor.js');
  const node = new AudioWorkletNode(ctx, 's6-mixer', { numberOfInputs: 0, numberOfOutputs: 1, outputChannelCount: [2],
    processorOptions: { bytes, script: SCRIPT, quanta: QUANTA } });
  const fromWorklet = new Promise((res) => { node.port.onmessage = (e) => res(e.data); });
  node.connect(ctx.destination);
  const buf = await ctx.startRendering();
  const w = await fromWorklet;
  const L = buf.getChannelData(0), R = buf.getChannelData(1);
  const inter = new Float32Array(L.length * 2); for (let i = 0; i < L.length; i++) { inter[2*i] = L[i]; inter[2*i+1] = R[i]; }
  const h = fnv(0xcbf29ce484222325n, inter);
  return { worklet: w, destination_fnv: h.toString(16).padStart(16, '0'), frames: L.length, sampleRate: buf.sampleRate };
};
