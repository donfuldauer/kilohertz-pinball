// S9: the S1/S2/S4 harness checks that apply, run INSIDE the combined module (Node), compared
// with those spikes' recorded outputs. usage: node probes_node.mjs <dist> <out.json>
//  * S2 Lua sandbox (79 adversarial cases) == S2's recommended-build wasm output (umath.node.txt);
//    vs native only the known pointer-size differences (mem=..., mem_gradual_keep).
//  * S2 budget-edge == native; S2 SP after Lua errors: delta 0.
//  * S4 adversarial rules corpus (82 lines, wasmi-instr-depth) == S4's Node and native runs.
//  * S4 stack probe (depth counter) == S4's.
//  * S4 zip corpus (33 files) classification == S4 native (web limits).
//  * S1 glam_ops digests == native x86_64 SSE2 (S1 a13).
//  * D5 allocation guard with the wasmi backend: 0 allocations in step_1ms.
import { readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';
import path from 'node:path';

const dist = path.resolve(process.argv[2]);
const out = process.argv[3];
const SPK = path.resolve(dist, '..', '..');
const glue = await import(pathToFileURL(path.join(dist, 'pkg', 'pin_web.js')).href);
glue.initSync({ module: readFileSync(path.join(dist, 'pkg', 'pin_web_bg.wasm')) });
const rd = (p) => readFileSync(p, 'utf8');
const res = { checks: [] };
let fails = 0;
const check = (name, ok, detail) => { res.checks.push({ check: name, ok: !!ok, detail }); if (!ok) fails += 1; console.log(`${ok ? 'PASS' : 'FAIL'} ${name} ${detail ? JSON.stringify(detail).slice(0, 400) : ''}`); };
const lines = (s) => s.split('\n').filter((l) => l.trim());

// S2 sandbox.
{
  const got = lines(glue.probe_lua_sandbox(''));
  const node = lines(rd(path.join(SPK, 'S2/runs/sandbox/umath.node.txt')));
  const nat = lines(rd(path.join(SPK, 'S2/runs/sandbox/umath.native.txt')));
  const diffNode = got.filter((l, i) => l !== node[i]);
  const strip = (l) => l.replace(/ \| mem=\d+/, '');
  const diffNat = got.map((l, i) => [l, nat[i]]).filter(([a, b]) => strip(a) !== strip(b)).map(([a]) => a.split(' ')[0]);
  res.s2_sandbox = { cases: got.length, diff_vs_s2_wasm: diffNode, diff_vs_native_ignoring_mem: diffNat };
  check('S2 sandbox: 79 cases identical to S2 recommended wasm build', got.length === 79 && diffNode.length === 0, { n: got.length, diff: diffNode.slice(0, 3) });
  check('S2 sandbox: vs native only the known pointer-size case(s) (mem= ignored)', diffNat.every((n) => n === 'mem_gradual_keep'), diffNat);
  writeFileSync(path.join(path.dirname(out), 's2_sandbox_combined.txt'), got.join('\n') + '\n');
}
{
  const got = glue.probe_lua_budget_edge().trim();
  const nat = rd(path.join(SPK, 'S2/runs/sandbox/umath.budget_edge.native')).trim();
  res.s2_budget_edge = got;
  check('S2 budget-edge == native', got === nat, { got: got.split('\n').join(' ') });
}
{
  const sp = glue.probe_sp_after_errors(50);
  res.s2_sp = sp;
  check('S2 stack pointer after 50 x (4 faults + 2000 pcall errors) restored', /delta=0 /.test(sp), sp);
}
// S4 adversarial corpus + stack probe.
{
  const prod = new Uint8Array(readFileSync(path.join(dist, 'test', 'apex_rules.wasm')));
  const got = lines(glue.probe_rules_adversarial('wasmi-instr-depth', prod));
  const node = lines(rd(path.join(SPK, 'S4/out/adv/node.wasmi-instr-depth.txt')));
  const nat = lines(rd(path.join(SPK, 'S4/out/adv/native.native-instr-depth.txt')));
  const body = (l) => l.replace(/^\S+ /, '');
  const dn = got.filter((l, i) => l !== node[i]);
  const dnat = got.filter((l, i) => body(l) !== body(nat[i] || ''));
  res.s4_adversarial = { lines: got.length, diff_vs_s4_node: dn, diff_vs_native_instr_depth: dnat };
  check('S4 adversarial: 82 lines identical to S4 wasmi-instr-depth (Node)', got.length === 82 && dn.length === 0, { n: got.length, diff: dn.slice(0, 2) });
  check('S4 adversarial: identical to native wasmtime running the instrumented module', dnat.length === 0, dnat.slice(0, 2));
  writeFileSync(path.join(path.dirname(out), 's4_adversarial_combined.txt'), got.join('\n') + '\n');
}
{
  const got = lines(glue.probe_rules_stack('wasmi-instr-depth', 2));
  const ref = lines(rd(path.join(SPK, 'S4/out/stack_all.txt'))).filter((l) => l.startsWith('wasmi-instr-depth '));
  res.s4_stack = got;
  check('S4 stack probe (depth counter) == S4', got.length === 5 && got.every((l, i) => l === ref[i]), { got, ref });
}
// S4 zip corpus.
{
  const nat = {};
  for (const l of lines(rd(path.join(dist, 'test', 'zip', 'native.web.txt')))) { const [n, ...r] = l.split(' '); nat[n] = r.join(' ').replace(/ peak_alloc_above_base=\d+$/, ''); }
  const diff = []; let n = 0;
  for (const f of readdirSync(path.join(dist, 'test', 'zip')).filter((x) => x.endsWith('.zip')).sort()) {
    const name = f.replace(/\.zip$/, '');
    const got = glue.probe_zip_check(new Uint8Array(readFileSync(path.join(dist, 'test', 'zip', f)))).replace(/ peak_alloc_above_base=\d+$/, '');
    n += 1;
    if (got !== nat[name]) diff.push({ name, got, native: nat[name] });
  }
  res.s4_zip = { files: n, diff };
  check('S4 zip corpus classification == native (web limits)', n === 33 && diff.length === 0, { n, diff: diff.slice(0, 2) });
}
// S1 glam ops (simd128 == SSE2).
{
  const got = glue.probe_s1('glam_ops');
  const ref = rd(path.join(SPK, 'S1/a13/glam_ops.txt')).split('\n');
  const nat = ref.slice(ref.indexOf('  glam_ops:') + 1).filter((l) => /^  \S+=/.test(l)).slice(0, 15);
  const g = got.split(/\s+/).filter((l) => /=/.test(l));
  const natMap = Object.fromEntries(nat.map((l) => l.trim().split('=')));
  const gotMap = Object.fromEntries(g.map((l) => l.split('=')));
  const differ = Object.keys(natMap).filter((k) => gotMap[k] !== natMap[k]);
  res.s1_glam_ops = { got: gotMap, native: natMap, differ };
  check('S1 glam_ops: every op the sim uses == native x86_64 SSE2 (only vec4_min/max may differ)', differ.every((k) => /min|max/.test(k)), differ);
}
// D5 allocation guard, wasmi backend, 60 000 ticks per table.
{
  const r = {};
  for (const t of ['apex_circuit', 'sky_deck', 'feature_lab']) {
    r[t] = glue.probe_alloc_check_wasm(new Uint8Array(readFileSync(path.join(dist, 'bundles', `${t}_wasm.pinbundle`))), new Uint8Array(readFileSync(path.join(dist, 'test', 'rec', `${t}_bot60.scn`))));
  }
  res.d5 = r;
  check('D5 guard: 0 allocations in step_1ms (wasmi rules, 3 tables x 60 000 ticks)', Object.values(r).every((s) => /installed=true/.test(s) && /allocs_in_step=0 /.test(s)), r);
}
res.fails = fails;
writeFileSync(out, JSON.stringify(res, null, 1));
console.log(`== probes_node: ${fails} failing check(s)`);
process.exit(fails ? 1 : 0);
