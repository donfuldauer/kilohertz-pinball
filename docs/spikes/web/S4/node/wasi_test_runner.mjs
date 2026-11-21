// Cargo runner for wasm32-wasip1 test binaries under Node's node:wasi.
// CARGO_TARGET_WASM32_WASIP1_RUNNER="node <this file>"
// Preopens the scratch repo (fixtures are read by absolute paths baked in
// with CARGO_MANIFEST_DIR) and a scratch temp dir as /tmp; single-threaded
// libtest (wasip1 has no threads).
import { WASI } from 'node:wasi';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
const here = path.dirname(new URL(import.meta.url).pathname);
const s4 = path.resolve(here, '..');
const [wasmPath, ...args] = process.argv.slice(2);
const extra = args.includes('--test-threads=1') ? [] : ['--test-threads=1'];
const wasi = new WASI({
  version: 'preview1',
  args: [path.basename(wasmPath), ...args, ...extra],
  env: { TMPDIR: '/tmp', RUST_BACKTRACE: '0' },
  preopens: { [path.join(s4, 'repo')]: path.join(s4, 'repo'), '/tmp': path.join(s4, 'tmp_wasi') },
  returnOnExit: true,
});
const mod = await WebAssembly.compile(await readFile(wasmPath));
// node:wasi (Node 22.23.5) segfaults in fd_write once guest memory passes
// ~32-40 MiB (S1 RESULTS §12): stdout/stderr writes are done here instead.
import { writeSync } from 'node:fs';
const io = wasi.getImportObject();
const p1 = io.wasi_snapshot_preview1;
const nodeFdWrite = p1.fd_write;
let memRef = null;
p1.fd_write = (fd, iovs, iovsLen, nwritten) => {
  if (fd !== 1 && fd !== 2) return nodeFdWrite(fd, iovs, iovsLen, nwritten);
  const dv = new DataView(memRef.buffer);
  let total = 0;
  for (let i = 0; i < iovsLen; i++) {
    const ptr = dv.getUint32(iovs + 8 * i, true);
    const len = dv.getUint32(iovs + 8 * i + 4, true);
    if (len) writeSync(fd, new Uint8Array(memRef.buffer, ptr, len));
    total += len;
  }
  dv.setUint32(nwritten, total, true);
  return 0;
};
const inst = await WebAssembly.instantiate(mod, io);
memRef = inst.exports.memory;
let code;
try {
  code = wasi.start(inst);
} catch (e) {
  console.log(`TRAP: ${e.constructor.name}: ${e.message}`);
  code = 134;
}
process.exitCode = code;
