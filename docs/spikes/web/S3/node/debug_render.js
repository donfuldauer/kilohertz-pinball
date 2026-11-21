// Debug helper: one render call with given opts, prints console + result.
'use strict';
const { chromium } = require('playwright');
const path = require('path');
const { start } = require('./server');
(async () => {
  const S = path.resolve(__dirname, '..');
  const port = 8900 + Math.floor(Math.random() * 50);
  const server = await start(S, port);
  const [backend, table, opts, pkg, flags] = [process.argv[2] || 'webgpu', process.argv[3] || 'apex_circuit',
    JSON.parse(process.argv[4] || '{}'), process.argv[5] || 'both', (process.argv[6] || '--enable-unsafe-webgpu').split(' ').filter(Boolean)];
  const b = await chromium.launch({ args: flags });
  const p = await b.newPage();
  p.on('console', (m) => console.log(`[console ${m.type()}] ${m.text().slice(0, 400)}`));
  p.on('pageerror', (e) => console.log(`[pageerror] ${e.message}`));
  await p.goto(`http://127.0.0.1:${port}/web/index.html?pkg=${pkg}`);
  await p.evaluate(() => window.probeReady);
  const r = await p.evaluate(([b, t, o]) => window.runRender(b, t, 540, 960, o), [backend, table, opts]);
  if (r.ok) { delete r.rgbaB64; console.log(JSON.stringify(r.report).slice(0, 1500)); } else console.log('ERR', r.error.slice(0, 1500));
  await b.close(); server.close();
})();
