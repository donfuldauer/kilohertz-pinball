// S8: runs one s8 command in the wasm-bindgen (--target nodejs) build.
// usage: node run.cjs <pkg_dir> <cmd> <args...>   (args starting with '@' are files)
const fs = require('node:fs');
const path = require('node:path');
const [pkg, cmd, ...a] = process.argv.slice(2);
const m = require(path.resolve(pkg, 's2_harness.js'));
const enc = new TextEncoder();
const args = a.map((x) => (x.startsWith('@') ? new Uint8Array(fs.readFileSync(x.slice(1))) : enc.encode(x)));
while (args.length < 6) args.push(new Uint8Array(0));
try {
  process.stdout.write(m.s8_call(cmd, ...args.slice(0, 6)));
} catch (e) {
  console.log(`TRAP: ${e.constructor.name}: ${e.message}`);
  process.exitCode = 134;
}
