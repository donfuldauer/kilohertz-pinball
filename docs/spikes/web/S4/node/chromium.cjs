// Runs one command on an S4 page in Playwright's Chromium.
// usage: node chromium.cjs <page path+query> <cmd> <json args> <out file|-> [port]
// The page must set window.s4ready and expose window.s4.run(cmd, args) -> string.
const { chromium } = require('playwright');
const { spawn } = require('node:child_process');
const fs = require('node:fs');
const [pagePath, cmd, argsJson, outFile, portArg] = process.argv.slice(2);
const port = Number(portArg || 8714);
(async () => {
  const srv = spawn(process.execPath, [`${__dirname}/serve.cjs`, String(port)], { stdio: ['ignore', 'ignore', 'inherit'] });
  await new Promise((r) => setTimeout(r, 600));
  const browser = await chromium.launch(process.env.S4_CHROMIUM_ARGS ? { args: process.env.S4_CHROMIUM_ARGS.split(' ') } : {});
  let code = 0;
  try {
    const page = await browser.newPage();
    page.on('console', (m) => { if (m.type() !== 'log' || process.env.S4_VERBOSE) console.error(`[page ${m.type()}] ${m.text()}`); });
    page.on('pageerror', (e) => console.error(`[pageerror] ${e}`));
    await page.goto(`http://127.0.0.1:${port}/${pagePath}`);
    await page.waitForFunction(() => window.s4ready, null, { timeout: 120000 });
    const r = await page.evaluate(async ([c, a]) => {
      try { return await window.s4.run(c, a); } catch (e) { return 'ERROR ' + (e && e.stack || e); }
    }, [cmd, JSON.parse(argsJson)]);
    const out = `${r}`;
    if (outFile === '-') process.stdout.write(out.endsWith('\n') ? out : out + '\n');
    else fs.writeFileSync(outFile, out);
    if (out.startsWith('ERROR')) { console.error(out.slice(0, 2000)); code = 1; }
    console.error(`chromium ${browser.version()} done ${cmd}`);
  } finally {
    await browser.close();
    srv.kill();
  }
  process.exit(code);
})();
