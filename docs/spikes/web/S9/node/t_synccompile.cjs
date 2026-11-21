// S9: can the 11 MB engine module be compiled synchronously on the main thread (initSync)?
'use strict';
const { startServer, launch, waitReady, writeJson } = require('./pw.cjs');
(async () => {
  const srv = await startServer(9551);
  const L = await launch('webgpu');
  const page = await L.browser.newPage();
  await page.goto(`${srv.url}?mode=idle&backend=webgpu&w=64&h=64`);
  await waitReady(page);
  const r = await page.evaluate(async () => {
    const b = new Uint8Array(await (await fetch('pkg/pin_web_bg.wasm')).arrayBuffer());
    const out = { bytes: b.length };
    try { const t = performance.now(); new WebAssembly.Module(b); out.sync = `ok ${(performance.now() - t).toFixed(0)} ms`; } catch (e) { out.sync = String(e); }
    try { const t = performance.now(); await WebAssembly.compile(b); out.async = `ok ${(performance.now() - t).toFixed(0)} ms`; } catch (e) { out.async = String(e); }
    try { const t = performance.now(); await WebAssembly.compileStreaming(fetch('pkg/pin_web_bg.wasm')); out.streaming = `ok ${(performance.now() - t).toFixed(0)} ms`; } catch (e) { out.streaming = String(e); }
    return out;
  });
  console.log(JSON.stringify(r));
  writeJson(process.argv[2], r);
  await L.browser.close(); srv.proc.kill();
})().catch((e) => { console.error(e); process.exit(2); });
