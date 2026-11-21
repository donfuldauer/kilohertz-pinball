// Feeds bundle files to web/files.html through the file input (Playwright
// setInputFiles) and through a synthetic drop, one fresh page per file.
// usage: node chromium_files.cjs <out> <limits default|web> <files...>
const { chromium } = require('playwright');
const { spawn } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');
const [out, limits, ...files] = process.argv.slice(2);
(async () => {
  const port = 8715;
  const srv = spawn(process.execPath, [`${__dirname}/serve.cjs`, String(port)], { stdio: ['ignore', 'ignore', 'inherit'] });
  await new Promise((r) => setTimeout(r, 600));
  const args = process.env.S4_CHROMIUM_ARGS ? { args: process.env.S4_CHROMIUM_ARGS.split(' ') } : {};
  const browser = await chromium.launch(args);
  const lines = [];
  try {
    for (const f of files) {
      for (const route of ['picker', 'drop']) {
        const page = await browser.newPage();
        page.on('pageerror', (e) => console.error(`[pageerror] ${e}`));
        await page.goto(`http://127.0.0.1:${port}/web/files.html?iso=1&limits=${limits}`);
        await page.waitForFunction(() => window.s4ready, null, { timeout: 60000 });
        let r;
        try {
          if (route === 'picker') {
            await page.setInputFiles('#f', f);
            r = await page.evaluate(() => window.__picked);
          } else {
            const b64 = fs.readFileSync(f).toString('base64');
            r = await page.evaluate(async ([name, data]) => {
              const bytes = Uint8Array.from(atob(data), (c) => c.charCodeAt(0));
              const dt = new DataTransfer();
              dt.items.add(new File([bytes], name));
              document.getElementById('drop').dispatchEvent(new DragEvent('drop', { dataTransfer: dt, bubbles: true, cancelable: true }));
              return window.__dropped;
            }, [path.basename(f), b64]);
          }
        } catch (e) { r = `${route} name=${path.basename(f)} ERROR ${e.message.split('\n')[0]}`; }
        lines.push(r);
        console.error(r.slice(0, 160));
        await page.close();
      }
    }
  } finally {
    await browser.close();
    srv.kill();
  }
  fs.writeFileSync(out, lines.join('\n') + '\n');
})();
