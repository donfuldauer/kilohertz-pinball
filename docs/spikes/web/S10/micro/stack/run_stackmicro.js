// S10: engine wasm stack limits and failure modes. For rec0/rec16: largest n that returns
// (searched, 3 repetitions in one process: tier-up changes frame sizes), and the error seen
// past it (legacy-EH module), then:  is the engine's stack overflow catchable by wasm catch_all (legacy module / separate exnref module:
// JSC refuses a module that mixes the two encodings)?
// Shells: <shell> shellenv.js run_stackmicro.js [--] <dir>   Node: node run_stackmicro.js <dir>
(function () {
  const isNode = typeof process === 'object' && process.versions && process.versions.node;
  const args = isNode ? process.argv.slice(2) : __args;
  const rdb = isNode ? (p) => new Uint8Array(require('node:fs').readFileSync(p)) : readBinary;
  const say = isNode ? console.log : print;
  const e = Object.assign({}, new WebAssembly.Instance(new WebAssembly.Module(rdb(args[0] + '/stackmicro.wasm')), {}).exports);
  try { e.caught_exnref = new WebAssembly.Instance(new WebAssembly.Module(rdb(args[0] + '/stackmicro_exnref.wasm')), {}).exports.caught_exnref; } catch (x) { say('exnref module: ' + x); }
  const tryN = (f, n) => { try { return { ok: f(n) === n }; } catch (x) { return { ok: false, err: `${x && x.constructor && x.constructor.name}: ${x && x.message}` }; } };
  for (const fn of ['rec0', 'rec16']) {
    const res = [];
    let err = '';
    for (let rep = 0; rep < 3; rep++) {
      let lo = 1, hi = 1;
      while (tryN(e[fn], hi).ok) { lo = hi; hi *= 2; if (hi > 1 << 24) break; }
      while (hi - lo > 1) { const mid = (lo + hi) >> 1; if (tryN(e[fn], mid).ok) lo = mid; else hi = mid; }
      res.push(lo);
      err = tryN(e[fn], hi * 2).err;
    }
    say(`${fn} max_depth=[${res.join(', ')}] past_limit=${err}`);
  }
  for (const fn of ['caught_legacy', 'caught_exnref']) {
    let r;
    try { r = 'returned ' + e[fn](1 << 24); } catch (x) { r = `escaped to JS as ${x && x.constructor && x.constructor.name}: ${x && x.message}`; }
    say(`${fn}(2^24): ${r}`);
  }
})();
