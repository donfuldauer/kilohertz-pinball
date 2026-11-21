// Runs a wasm32-wasip1 libtest binary under node:wasi (preopen / -> /).
// usage: node run_wasi_test.mjs <test.wasm> [libtest args...]
import { WASI } from 'node:wasi';
import fs from 'node:fs';
const [wasm, ...args] = process.argv.slice(2);
const wasi = new WASI({ version: 'preview1', args: [wasm, ...args], env: { RUST_BACKTRACE: '0' }, preopens: { '/': '/' }, returnOnExit: true });
const mod = await WebAssembly.compile(fs.readFileSync(wasm));
const inst = await WebAssembly.instantiate(mod, wasi.getImportObject());
process.exitCode = wasi.start(inst);
