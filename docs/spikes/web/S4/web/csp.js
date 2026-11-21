// A21: which WebAssembly entry points a Content-Security-Policy allows.
// The page is served with the policy chosen by ?csp=... (node/serve.cjs).
window.s4 = {
  async run(cmd, a) {
    const out = [`csp=${new URL(location.href).searchParams.get('csp')}`];
    const tiny = new Uint8Array([0, 0x61, 0x73, 0x6d, 1, 0, 0, 0]);
    const engine = new Uint8Array(await (await fetch('/pkg/s4_rules_bg.wasm')).arrayBuffer());
    const rules = new Uint8Array(await (await fetch('/repo/tables/apex_circuit/rules.wasm')).arrayBuffer());
    const tests = {
      'WebAssembly.validate(tiny)': async () => WebAssembly.validate(tiny),
      'new WebAssembly.Module(tiny)': async () => !!new WebAssembly.Module(tiny),
      'WebAssembly.compile(tiny)': async () => !!(await WebAssembly.compile(tiny)),
      'WebAssembly.instantiate(tiny)': async () => !!(await WebAssembly.instantiate(tiny)),
      'WebAssembly.compile(engine module)': async () => !!(await WebAssembly.compile(engine)),
      'WebAssembly.instantiateStreaming(fetch(engine))': async () => !!(await WebAssembly.compileStreaming(fetch('/pkg/s4_rules_bg.wasm'))),
      'WebAssembly.compile(rules.wasm)': async () => !!(await WebAssembly.compile(rules)),
      'eval("1+1")': async () => eval('1+1'),
    };
    for (const [k, f] of Object.entries(tests)) {
      try { out.push(`${k}: ok (${await f()})`); } catch (e) { out.push(`${k}: ${e.constructor.name}: ${e.message}`); }
    }
    // A Worker created from a same-origin script inherits its own CSP (none here).
    return out.join('\n');
  },
};
window.s4ready = true;
