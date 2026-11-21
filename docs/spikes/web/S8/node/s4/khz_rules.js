// S4 design B (browser JIT) JS glue: `globalThis.khzRules`, used by the
// engine module's `js_backend` imports. Works in Node and in a page.
// `khzRules.sim` must be set to the engine module's raw wasm exports.
(function () {
  const IMPORTS = [
    'pulse_coil', 'hold_coil', 'set_magnet', 'set_zipper', 'set_diverter', 'spawn_ball',
    'set_lamp', 'set_gi_tint', 'set_score', 'show_scene', 'play_sound', 'persist_set',
    'persist_get', 'switch', 'tick', 'balls_in_play', 'random',
  ];
  function fnv(bytes) {
    let h = 0x811c9dc5;
    for (let i = 0; i < bytes.length; i++) { h ^= bytes[i]; h = Math.imul(h, 0x01000193) >>> 0; }
    return bytes.length + ':' + h.toString(16);
  }
  const R = {
    sim: null,
    insts: [],
    free: [],
    lastErr: '',
    // 'sync': new WebAssembly.Module(bytes) at load; 'cache': use a module
    // compiled ahead with precompile() (async), keyed by content.
    mode: 'sync',
    cache: new Map(),
    stats: { compiles: 0, cacheHits: 0, compileMs: 0, instantiateMs: 0 },
    async precompile(bytes) {
      const key = fnv(bytes);
      if (!R.cache.has(key)) R.cache.set(key, await WebAssembly.compile(bytes));
      return key;
    },
    importObject(rec) {
      const s = R.sim;
      const pinball = {};
      for (const n of IMPORTS) pinball[n] = s['khz_host_' + n];
      pinball.id_of = (kind, ptr, len) => {
        const mem = new Uint8Array(rec.e.memory.buffer);
        const st = ptr >>> 0, n = len >>> 0;
        const oob = st + n > mem.length ? 1 : 0;
        if (!oob && n > 0) {
          const p = s.khz_scratch(n);
          new Uint8Array(s.memory.buffer, p, n).set(mem.subarray(st, st + n));
        }
        const id = s.khz_host_id_of(kind, ptr, len, oob, rec.getFuel());
        rec.setFuel(s.khz_id_of_fuel());
        if (id === -2) {
          // wasmtime: OutOfFuel or a host error; the engine module knows which.
          rec.whyG.value = 3;
          throw new Error('khz host error (id_of)');
        }
        return id;
      };
      return { pinball, khz: { host_err: s.khz_host_err } };
    },
    instantiate(bytes, namesCsv, startFuel) {
      const [begin, getFuel, setFuel, getWhy, start, whyG] = namesCsv.split(',');
      let rec;
      try {
        let mod;
        const t0 = performance.now();
        if (R.mode === 'cache') {
          mod = R.cache.get(fnv(bytes));
          if (!mod) throw new Error('module not precompiled');
          R.stats.cacheHits++;
        } else {
          mod = new WebAssembly.Module(bytes);
          R.stats.compiles++;
          R.stats.compileMs += performance.now() - t0;
        }
        rec = { e: null };
        const t1 = performance.now();
        const inst = new WebAssembly.Instance(mod, R.importObject(rec));
        R.stats.instantiateMs += performance.now() - t1;
        const e = inst.exports;
        rec.e = e;
        rec.begin = e[begin]; rec.getFuel = e[getFuel]; rec.setFuel = e[setFuel]; rec.getWhy = e[getWhy];
        rec.whyG = e[whyG];
        rec.entries = [e.on_init, e.on_tick, e.on_switch_edge];
      } catch (err) {
        R.lastErr = String(err && err.message || err);
        return -1;
      }
      if (start) {
        rec.begin(startFuel);
        try { rec.e[start](); } catch (err) { return -(R.classify(rec, err) + 1); }
      }
      const h = R.free.length ? R.free.pop() : R.insts.length;
      R.insts[h] = rec;
      return h;
    },
    classify(rec, err) {
      const why = rec.getWhy();
      if (why) return why; // 1 fuel, 2 depth, 3 host
      if (err instanceof WebAssembly.RuntimeError) { R.lastErr = err.message; return 4; }
      if (err instanceof RangeError) { R.lastErr = err.message; return 5; }
      R.lastErr = String(err && err.message || err);
      return 6;
    },
    call(h, entry, a, b, fuel) {
      const r = R.insts[h];
      r.begin(fuel);
      try {
        if (entry === 2) r.entries[2](a, b); else r.entries[entry]();
        return 0;
      } catch (err) {
        return R.classify(r, err);
      }
    },
    fuel(h) { return R.insts[h].getFuel(); },
    lastError() { return R.lastErr; },
    release(h) { if (R.insts[h]) { R.insts[h] = null; R.free.push(h); } },
    memory(h) { return new Uint8Array(R.insts[h].e.memory.buffer).slice(); },
  };
  globalThis.khzRules = R;
})();
