// S10 micro-test runner: runs every op_<name>(N) of micro.wasm over inputs.bin and
// writes the raw output slots (N x 16 bytes per op, ops.txt order) to <out.bin>.
// Shells: <shell> shellenv.js run_micro.js [--] <micro_dir> <out.bin>
// Node:   node run_micro.js <micro_dir> <out.bin>
(function () {
  const isNode = typeof process === 'object' && process.versions && process.versions.node;
  let args, rdb, rdt, wrb, say;
  if (isNode) {
    const fs = require('node:fs');
    args = process.argv.slice(2);
    rdb = (p) => new Uint8Array(fs.readFileSync(p));
    rdt = (p) => fs.readFileSync(p, 'utf8');
    wrb = (p, b) => fs.writeFileSync(p, b);
    say = (s) => console.log(s);
  } else {
    args = __args; rdb = readBinary; rdt = readText; wrb = writeBytes; say = print;
  }
  const [dir, outPath] = args;
  const ops = rdt(dir + '/ops.txt').trim().split('\n').map((l) => l.split(' '));
  const inp = rdb(dir + '/inputs.bin');
  const N = new DataView(inp.buffer, inp.byteOffset).getUint32(8, true);
  const mod = new WebAssembly.Module(rdb(dir + '/micro.wasm'));
  const inst = new WebAssembly.Instance(mod, {});
  const mem = new Uint8Array(inst.exports.memory.buffer);
  mem.set(inp.subarray(12), 0x10000);
  const out = new Uint8Array(ops.length * N * 16);
  let k = 0;
  for (const [name] of ops) {
    const fn = inst.exports['op_' + name.replace(/\./g, '_')];
    // Run each op twice over the full input set: the second pass runs after the
    // engine had a chance to tier up; both passes must agree.
    mem.fill(0, 0x100000, 0x100000 + N * 16);
    fn(N);
    const first = mem.slice(0x100000, 0x100000 + N * 16);
    for (let r = 0; r < 20; r++) fn(N);
    const last = mem.subarray(0x100000, 0x100000 + N * 16);
    for (let j = 0; j < first.length; j++) {
      if (first[j] !== last[j]) { say(`TIER-INSTABILITY ${name} record ${j >> 4}`); break; }
    }
    out.set(last, k * N * 16);
    k++;
  }
  wrb(outPath, out);
  say(`micro: ${ops.length} ops x ${N} records -> ${outPath}`);
})();
