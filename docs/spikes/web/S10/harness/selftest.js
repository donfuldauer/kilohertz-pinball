// S10 startup self-test prototype (req 5): what a page runs once, before it loads any bundle,
// to check that this browser's engine computes the same bits as the native x86_64 build.
//   1. sim:   replay the repo's CI fixture (replay_table, 10 000 ticks of recorded inputs, 6 KB
//             bundle + 5 KB inputs + 244 KB script stream in this spike harness) and compare the
//             final state_hash/ext_hash with the native constants (documented 25097c21c2c3adc1).
//   2. ops:   the op-level digests of the SIMD-backed math the sim uses (S1 `glam_ops` probe,
//             200 000 inputs) against the constants of the +simd128 build.
//   3. stack: (optional, separate instance of the Lua module) the deepest legal Lua C recursion
//             (S2 case cstack_pcall_nest: 200 nested pcall) must end in Lua's own result, not in
//             an engine stack overflow; a failure means "this engine's stack is too small for the
//             Lua C-call limit": refuse user bundles (the 3 included tables need < 20 KiB).
// usage (shell): <shell> shellenv.js selftest.js [--] <s1_pkg_dir> <replay_table.pinbundle> <fx.scn> <fx.scr> [<lua_pkg_dir>]
// usage (node):  node selftest.js <same args>
(function () {
  const isNode = typeof process === 'object' && process.versions && process.versions.node;
  let args, rdb, now, load, say;
  if (isNode) {
    const fs = require('node:fs');
    args = process.argv.slice(2);
    rdb = (p) => new Uint8Array(fs.readFileSync(p));
    now = () => performance.now();
    load = (p) => require(p);
    say = (s) => console.log(s);
  } else {
    args = __args; rdb = readBinary; now = nowMs; load = loadCjs; say = print;
  }
  const EXPECT = {
    fixture: { ticks: '10000', state_hash: '25097c21c2c3adc1', ext_hash: '643e148850cf462b' },
    glam_ops: 'quat_mul=fc16b055f9183578 quat_normalize=b883ddc5f62e282e quat_mul_vec3=5da4fcba241cdaa3 ' +
      'quat_conj_mul_vec3=38578744ec37e5cf quat_from_rotation_arc=d51ae0b67788166a integrate_orientation=0b2a08bb65849acb ' +
      'vec4_dot=27bd81258b83f772 vec4_length=01c0887331ebfb9b vec4_normalize=ee5fae8698c5afab vec4_min_max=b968422b2e284151 ' +
      'mat4_mul_mat4=13135b2e0bf070d7 mat4_transform_point3=776850b385479b00 mat4_inverse=3235440eb031dc11 ' +
      'vec3_ops=0c5d9fcd4bce9ee0 libm_trig=eb9eae02899865f5',
    cstack: 'cstack_pcall_nest | events=ok*2 | cmd=f0fe52c18d5c80cd | faults=0 faulted=false rng=946d8d8f651b91e9 | first_msg=""',
  };
  const [pkg, bundle, scn, scr, luaPkg] = args;
  const res = [];
  let ok = true;
  const t0 = now();
  const m = load(pkg + (pkg.endsWith('.js') ? '' : '/s1_harness.js'));
  const tLoad = now() - t0;
  // 1. sim fixture
  let t = now();
  try {
    const out = m.play(rdb(bundle), rdb(scn), rdb(scr));
    const nl = out.indexOf(10);
    const head = String.fromCharCode.apply(null, out.subarray(0, nl));
    const g = (k) => (head.match(new RegExp('\\b' + k + '=([0-9a-f]+)')) || [])[1];
    const good = g('ticks') === EXPECT.fixture.ticks && g('state_hash') === EXPECT.fixture.state_hash && g('ext_hash') === EXPECT.fixture.ext_hash;
    ok = ok && good;
    res.push(`sim_fixture=${good ? 'ok' : 'MISMATCH(' + g('state_hash') + ')'} (${(now() - t).toFixed(0)} ms)`);
  } catch (e) { ok = false; res.push(`sim_fixture=TRAP(${e && e.message})`); }
  // 2. op digests
  t = now();
  try {
    const got = m.probe('glam_ops').replace(/^glam_ops: /, '').trim();
    const good = got === EXPECT.glam_ops;
    ok = ok && good;
    res.push(`glam_ops=${good ? 'ok' : 'MISMATCH'} (${(now() - t).toFixed(0)} ms)`);
  } catch (e) { ok = false; res.push(`glam_ops=TRAP(${e && e.message})`); }
  // 3. stack budget for the Lua C-call limit (own instance; a trap kills only that instance)
  let stack = 'skipped';
  if (luaPkg) {
    t = now();
    try {
      const L = load(luaPkg + '/s2_harness.js');
      const line = L.sandbox('cstack_pcall_nest').split('\n').find((l) => l.startsWith('cstack_pcall_nest |')) || '';
      stack = line.replace(/ \| mem=[0-9]*$/, '') === EXPECT.cstack ? 'ok' : 'MISMATCH';
    } catch (e) { stack = 'INSUFFICIENT(' + (e && e.message) + ')'; }
    res.push(`lua_cstack=${stack} (${(now() - t).toFixed(0)} ms)`);
  }
  say(`selftest ${ok ? 'PASS' : 'FAIL'} module_load=${tLoad.toFixed(0)} ms ${res.join(' ')}` +
    (stack.startsWith('INSUFFICIENT') ? ' -> refuse user bundles (included tables only)' : ''));
})();
