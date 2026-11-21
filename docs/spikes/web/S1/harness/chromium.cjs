// Runs the S1 page in Playwright's Chromium. usage:
//   node chromium.cjs <web_root> <port> <isolated 0|1> <out.json> [query]
const { chromium } = require('playwright');
const { spawn } = require('node:child_process');
const fs = require('node:fs');
const [root, port, iso, outFile, query] = process.argv.slice(2);
(async () => {
  const srv = spawn(process.execPath, [`${__dirname}/serve.cjs`, root, port, iso], { stdio: 'inherit' });
  await new Promise((r) => setTimeout(r, 800));
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage();
    page.on('console', (m) => console.log(`[page ${m.type()}] ${m.text()}`));
    page.on('pageerror', (e) => console.log(`[pageerror] ${e}`));
    await page.goto(`http://127.0.0.1:${port}/index.html${query ? '?' + query : ''}`);
    await page.waitForFunction(() => window.__s1, null, { timeout: 1800000, polling: 1000 });
    const r = await page.evaluate(() => window.__s1);
    r.browserVersion = browser.version();
    fs.writeFileSync(outFile, JSON.stringify(r, null, 1));
    for (const [k, v] of Object.entries(r.results || {})) console.log(`${k}: identical=${v.identical} ${v.summary}`);
    if (r.error) console.log(`error: ${r.error}`);
  } finally {
    await browser.close();
    srv.kill();
  }
})();
