// Bundle from a file picker or a drop -> File.arrayBuffer() -> copy into the
// engine's linear memory (wasm-bindgen &[u8]) -> Vfs::from_zip_bytes.
import init, * as m from '/pkg/s4_rules.js';
const raw = await init({ module_or_path: '/pkg/s4_rules_bg.wasm' });
const web = new URL(location.href).searchParams.get('limits') === 'web';
async function check(file, route) {
  const before = raw.memory.buffer.byteLength;
  const t0 = performance.now();
  const buf = await file.arrayBuffer();
  const t1 = performance.now();
  const r = m.zip_check(new Uint8Array(buf), web);
  const t2 = performance.now();
  return `${route} name=${file.name} size=${file.size} arrayBuffer_ms=${(t1 - t0).toFixed(1)} check_ms=${(t2 - t1).toFixed(1)} ${r} wasm_mem_before=${before} wasm_mem_after=${raw.memory.buffer.byteLength}`;
}
window.__picked = new Promise((ok) => {
  document.getElementById('f').addEventListener('change', async (e) => ok(await check(e.target.files[0], 'picker')));
});
window.__dropped = new Promise((ok) => {
  const z = document.getElementById('drop');
  z.addEventListener('dragover', (e) => e.preventDefault());
  z.addEventListener('drop', async (e) => { e.preventDefault(); ok(await check(e.dataTransfer.files[0], 'drop')); });
});
window.s4ready = true;
