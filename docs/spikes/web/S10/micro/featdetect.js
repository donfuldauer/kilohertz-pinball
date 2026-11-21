// S10 feature detection: for each micro/feat/<f>.wasm, WebAssembly.validate and, if valid,
// instantiate and call f() (expected 7 or 9 for reference_types). Also the JS-API pieces
// the wasm-bindgen glue uses (WebAssembly.Tag / Exception / JSTag, Exception.is/getArg).
// Shells: <shell> shellenv.js featdetect.js [--] <feat_dir>   Node: node featdetect.js <feat_dir>
(function () {
  const isNode = typeof process === 'object' && process.versions && process.versions.node;
  const args = isNode ? process.argv.slice(2) : __args;
  const rdb = isNode ? (p) => new Uint8Array(require('node:fs').readFileSync(p)) : readBinary;
  const say = isNode ? console.log : print;
  const dir = args[0];
  const feats = ['mvp', 'mutable_global', 'sat_f2i', 'sign_ext', 'multi_value', 'bulk_memory', 'reference_types',
    'simd128', 'relaxed_simd', 'eh_legacy', 'eh_exnref', 'tail_call', 'extended_const', 'memory64', 'multi_memory',
    'threads', 'gc'];
  const out = [];
  for (const f of feats) {
    let bytes;
    try { bytes = rdb(`${dir}/${f}.wasm`); } catch (e) { out.push(`${f}=nofile`); continue; }
    let v = false, r = '-';
    try { v = WebAssembly.validate(bytes); } catch (e) { v = 'throw:' + e.message; }
    if (v === true) {
      try {
        const inst = new WebAssembly.Instance(new WebAssembly.Module(bytes), {});
        r = f === 'reference_types' ? inst.exports.f({}) : inst.exports.f();
      } catch (e) { r = 'run-error:' + (e && e.constructor && e.constructor.name) + ':' + (e && e.message); }
    }
    out.push(`${f}=${v === true ? 'valid' : 'invalid'}${v === true ? '/' + r : ''}`);
  }
  say(out.join(' '));
  const js = [];
  js.push('Tag=' + typeof WebAssembly.Tag, 'Exception=' + typeof WebAssembly.Exception, 'JSTag=' + typeof WebAssembly.JSTag);
  try {
    const t = new WebAssembly.Tag({ parameters: ['externref'] });
    const e = new WebAssembly.Exception(t, [42], { traceStack: true });
    js.push('Exception.is=' + e.is(t), 'getArg=' + e.getArg(t, 0), 'stack=' + (typeof e.stack));
  } catch (e) { js.push('tag-api-error:' + e.message); }
  js.push('validate_streaming=' + typeof WebAssembly.compileStreaming);
  say(js.join(' '));
})();
