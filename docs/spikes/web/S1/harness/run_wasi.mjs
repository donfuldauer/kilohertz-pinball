// Runs a wasm32-wasip1 command module under Node's node:wasi.
// usage: S1_DIR=<dir> node run_wasi.mjs <module.wasm> <args...>
// <dir> is preopened as /s inside the guest.
import { WASI } from 'node:wasi';
import { readFile } from 'node:fs/promises';
const [wasmPath, ...args] = process.argv.slice(2);
const wasi = new WASI({
  version: 'preview1',
  args: ['s1', ...args],
  env: {},
  preopens: { '/s': process.env.S1_DIR },
  returnOnExit: true,
});
const mod = await WebAssembly.compile(await readFile(wasmPath));
const inst = await WebAssembly.instantiate(mod, wasi.getImportObject());
let code;
try {
  code = wasi.start(inst);
} catch (e) {
  console.log(`TRAP: ${e.constructor.name}: ${e.message}`);
  code = 134;
}
process.exitCode = code;
