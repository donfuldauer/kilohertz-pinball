// Runs the S2 page in Playwright's Chromium.
// usage: node chromium.cjs <web_root> <port> <isolated 0|1> <out.json> [query] [chromium args...]
const { chromium } = require('playwright');
const { spawn } = require('node:child_process');
const fs = require('node:fs');
const [root, port, iso, outFile, query, ...args] = process.argv.slice(2);
(async () => {
  const srv = spawn(process.execPath, [`${__dirname}/serve.cjs`, root, port, iso], { stdio: 'inherit' });
  await new Promise((r) => setTimeout(r, 800));
  const browser = await chromium.launch(args.length ? { args } : undefined);
  try {
    const page = await browser.newPage();
    page.on('console', (m) => console.log(`[page ${m.type()}] ${m.text()}`));
    page.on('pageerror', (e) => console.log(`[pageerror] ${e}`));
    await page.goto(`http://127.0.0.1:${port}/index.html${query ? '?' + query : ''}`);
    await page.waitForFunction(() => window.__s2, null, { timeout: 1800000, polling: 500 });
    const r = await page.evaluate(() => window.__s2);
    r.browserVersion = browser.version();
    fs.writeFileSync(outFile, JSON.stringify(r, null, 1));
    console.log(`browser ${r.browserVersion} isolated=${r.crossOriginIsolated} eh_legacy=${JSON.stringify(r.features.eh_legacy)} eh_exnref=${JSON.stringify(r.features.eh_exnref)} JSTag=${r.features.JSTag}`);
    for (const [k, v] of Object.entries(r.results || {})) console.log(`${k}: ${v.summary}`);
    if (r.error) console.log(`error: ${r.error}`);
  } finally {
    await browser.close();
    srv.kill();
  }
})();
