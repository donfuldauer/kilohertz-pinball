// Req 6: what happens to the engine instance after a wasm trap, and that no
// sandbox case traps. usage: node trap_test.cjs <pkg_node> <bundle>
const fs = require('node:fs');
const m = require(require('node:path').resolve(process.argv[2], 's2_harness.js'));
const bundle = new Uint8Array(fs.readFileSync(process.argv[3]));
const tryit = (label, f) => { try { const r = f(); console.log(`${label}: ok ${String(r).slice(0, 120).replace(/\n/g, ' | ')}`); } catch (e) { console.log(`${label}: THROWS ${e.constructor.name}: ${e.message}`); } };
tryit('lua_smoke before', () => m.lua_smoke('function on_tick() pinball.pulse_coil(1,2) end', 2));
tryit('sandbox (all cases) before', () => m.sandbox('').split('\n').length - 1 + ' cases ran');
tryit('panic_in_step (Rust panic -> wasm trap)', () => m.panic_in_step(bundle));
tryit('lua_smoke after trap', () => m.lua_smoke('function on_tick() pinball.pulse_coil(1,2) end', 2));
tryit('s2_stack_layout after trap', () => m.s2_stack_layout());
