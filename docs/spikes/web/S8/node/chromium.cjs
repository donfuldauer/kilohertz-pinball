// S8: runs web/jobs.json in Playwright's Chromium (COOP/COEP, local server);
// writes each job's output to <out_dir>/<id>.txt.
// usage: node chromium.cjs <web_root> <port> <out_dir>
const { chromium } = require('playwright');
const { spawn } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');
const [root, port, outDir] = process.argv.slice(2);
(async () => {
  const srv = spawn(process.execPath, [path.join(__dirname, '..', 'harness', 'serve.cjs'), root, port, '1'], { stdio: 'inherit' });
  await new Promise((r) => setTimeout(r, 800));
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage();
    page.on('pageerror', (e) => console.log(`[pageerror] ${e}`));
    await page.goto(`http://127.0.0.1:${port}/index.html`);
    await page.waitForFunction(() => window.__s8, null, { timeout: 3600000, polling: 500 });
    const r = await page.evaluate(() => window.__s8);
    fs.mkdirSync(outDir, { recursive: true });
    for (const [k, v] of Object.entries(r.results)) fs.writeFileSync(path.join(outDir, k + '.txt'), v);
    console.log(`chromium ${browser.version()} isolated=${r.crossOriginIsolated} jobs=${Object.keys(r.results).length}${r.error ? ' error=' + r.error : ''}`);
  } finally {
    await browser.close();
    srv.kill();
  }
})();
