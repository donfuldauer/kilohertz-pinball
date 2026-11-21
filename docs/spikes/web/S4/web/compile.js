// A20: synchronous WebAssembly compile/instantiate on the main thread vs a
// Worker, by module size; and the async route. window.s4.run('sizes', {sizes}).
function leb(n) { const o = []; do { let b = n & 0x7f; n = Math.floor(n / 128); if (n) b |= 0x80; o.push(b); } while (n); return o; }
// A valid module of ~`size` bytes: one function whose body is `nop`s.
function moduleOfSize(size) {
  const bodyLen = Math.max(4, size - 40);
  const body = new Uint8Array(bodyLen);
  body[0] = 0; // no locals
  body.fill(0x01, 1, bodyLen - 1); // nop
  body[bodyLen - 1] = 0x0b; // end
  const sec = (id, b) => [id, ...leb(b.length), ...b];
  const head = [0x00, 0x61, 0x73, 0x6d, 1, 0, 0, 0, ...sec(1, [1, 0x60, 0, 0]), ...sec(3, [1, 0]), ...sec(7, [1, 1, 0x66, 0, 0])];
  const codeBody = [1, ...leb(bodyLen), ...body];
  const out = new Uint8Array(head.length + 1 + leb(codeBody.length).length + codeBody.length);
  out.set(head, 0);
  let o = head.length;
  out[o++] = 10;
  const l = leb(codeBody.length); out.set(l, o); o += l.length;
  out.set(codeBody, o);
  return out;
}
function trySync(bytes) {
  const r = {};
  let t = performance.now();
  try { const m = new WebAssembly.Module(bytes); r.compile = `ok ${(performance.now() - t).toFixed(1)}ms`;
    t = performance.now();
    try { new WebAssembly.Instance(m, {}); r.instantiate = `ok ${(performance.now() - t).toFixed(1)}ms`; } catch (e) { r.instantiate = `${e.constructor.name}: ${e.message}`; }
  } catch (e) { r.compile = `${e.constructor.name}: ${e.message}`; }
  return r;
}
const workerSrc = `${leb}\n${moduleOfSize}\n${trySync}\nonmessage = (e) => postMessage(e.data.map((s) => [s, trySync(moduleOfSize(s))]));`;
window.s4 = {
  async run(cmd, a) {
    const sizes = a.sizes;
    const out = [`crossOriginIsolated=${self.crossOriginIsolated} ua=${navigator.userAgent}`];
    for (const s of sizes) {
      const bytes = moduleOfSize(s);
      const r = trySync(bytes);
      let t = performance.now();
      let asyncR;
      try { const m = await WebAssembly.compile(bytes); asyncR = `ok ${(performance.now() - t).toFixed(1)}ms`;
        t = performance.now(); await WebAssembly.instantiate(m, {}); asyncR += ` inst ok ${(performance.now() - t).toFixed(1)}ms`; } catch (e) { asyncR = `${e.constructor.name}: ${e.message}`; }
      out.push(`main size=${bytes.length} sync_compile=[${r.compile}] sync_instantiate=[${r.instantiate}] async=[${asyncR}]`);
    }
    const w = new Worker(URL.createObjectURL(new Blob([workerSrc], { type: 'text/javascript' })));
    const res = await new Promise((ok) => { w.onmessage = (e) => ok(e.data); w.postMessage(sizes); });
    for (const [s, r] of res) out.push(`worker size~${s} sync_compile=[${r.compile}] sync_instantiate=[${r.instantiate}]`);
    return out.join('\n');
  },
};
window.s4ready = true;
