// For each sandbox case: smallest V8 --stack-size (KB, step 8) at which it
// runs without a RangeError/trap. usage: node stack_need.cjs <pkg_node>
const { execFileSync } = require('node:child_process');
const path = require('node:path');
const pkg = path.resolve(process.argv[2]);
const run = path.resolve(__dirname, 'run_node.cjs');
const names = execFileSync(process.execPath, [run, pkg, 'sandbox-describe']).toString().trim().split('\n').map((l) => l.split('\t')[0]);
const ok = (name, kb) => {
  try {
    const out = execFileSync(process.execPath, [`--stack-size=${kb}`, run, pkg, 'sandbox', name], { stdio: ['ignore', 'pipe', 'pipe'] }).toString();
    return !/TRAP|RangeError/.test(out) && out.includes(`${name} |`);
  } catch (e) { return false; }
};
const res = [];
for (const n of names) {
  let lo = 8, hi = 400;
  if (!ok(n, hi)) { res.push([n, '>400']); continue; }
  while (hi - lo > 8) { const mid = ((lo + hi) >> 4) << 3; if (ok(n, mid)) hi = mid; else lo = mid; }
  res.push([n, hi]);
}
res.sort((a, b) => (Number(b[1]) || 1e9) - (Number(a[1]) || 1e9));
for (const [n, kb] of res) console.log(`${n} needs_v8_stack_kb<=${kb}`);
