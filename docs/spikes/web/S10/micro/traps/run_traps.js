// S10: RuntimeError class and message per trap kind (engine-specific texts), plus the error an
// out-of-bounds active data segment raises at instantiation.
(function () {
  const isNode = typeof process === 'object' && process.versions && process.versions.node;
  const args = isNode ? process.argv.slice(2) : __args;
  const rdb = isNode ? (p) => new Uint8Array(require('node:fs').readFileSync(p)) : readBinary;
  const say = isNode ? console.log : print;
  const e = new WebAssembly.Instance(new WebAssembly.Module(rdb(args[0] + '/traps.wasm')), {}).exports;
  for (const k of ['call_indirect_null', 'call_indirect_sig', 'call_indirect_oob', 'trunc_nan', 'trunc_overflow', 'div_zero', 'int_overflow', 'unreachable', 'oob_load']) {
    try { e[k](); say(`${k}: no trap`); } catch (x) { say(`${k}: ${x.constructor.name}: ${x.message}`); }
  }
  try { new WebAssembly.Instance(new WebAssembly.Module(rdb(args[0] + '/dataseg.wasm')), {}); say('data_segment_oob: no error'); }
  catch (x) { say(`data_segment_oob: ${x.constructor.name}: ${x.message}`); }
})();
