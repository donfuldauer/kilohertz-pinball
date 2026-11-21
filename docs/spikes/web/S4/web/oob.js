// Bare V8 OOB-trap probe (no engine module). window.s4.run('oob', {variant}).
function mod(maxPages, initPages, offset) {
  // (module (memory (export "m") init max?) (func (export "f") (result i32) (i32.load (i32.const offset))))
  const leb = (n) => { const o = []; do { let b = n & 0x7f; n >>>= 7; if (n) b |= 0x80; o.push(b); } while (n); return o; };
  const sleb = (n) => { const o = []; while (true) { let b = n & 0x7f; n >>= 7; if ((n === 0 && !(b & 0x40)) || (n === -1 && (b & 0x40))) { o.push(b); break; } o.push(b | 0x80); } return o; };
  const sec = (id, body) => [id, ...leb(body.length), ...body];
  const memType = maxPages === null ? [0x00, ...leb(initPages)] : [0x01, ...leb(initPages), ...leb(maxPages)];
  const code = [0x41, ...sleb(offset), 0x28, 0x02, 0x00, 0x0b];
  const body = [0x00, ...code];
  const bytes = [0x00, 0x61, 0x73, 0x6d, 0x01, 0, 0, 0,
    ...sec(1, [1, 0x60, 0, 1, 0x7f]),
    ...sec(3, [1, 0]),
    ...sec(5, [1, ...memType]),
    ...sec(7, [2, 1, 0x6d, 0x02, 0, 1, 0x66, 0x00, 0]),
    ...sec(10, [1, ...leb(body.length), ...body]),
  ];
  return new Uint8Array(bytes);
}
window.s4 = {
  async run(cmd, a) {
    const out = [];
    for (const [maxP, initP, off] of a.variants) {
      try {
        const m = new WebAssembly.Module(mod(maxP, initP, off));
        const i = new WebAssembly.Instance(m, {});
        const r = i.exports.f();
        out.push(`max=${maxP} init=${initP} off=${off}: returned ${r}`);
      } catch (e) {
        out.push(`max=${maxP} init=${initP} off=${off}: ${e.constructor.name}: ${e.message}`);
      }
    }
    return out.join('\n') + `\nuserAgent=${navigator.userAgent}`;
  },
};
window.s4ready = true;
