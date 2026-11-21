// Debug helper: runAdapter on one backend with console output and a timeout.
'use strict';
const { chromium } = require('playwright');
const path = require('path');
const { start } = require('./server');
(async () => {
  const S = path.resolve(__dirname, '..');
  const port = 8950 + Math.floor(Math.random() * 40);
  const server = await start(S, port);
  const icd = '/opt/pw-browsers/chromium_headless_shell-1243/chrome-headless-shell-linux64/vk_swiftshader_icd.json';
  const b = await chromium.launch({ args: ['--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-angle=vulkan', '--ignore-gpu-blocklist'], env: { ...process.env, VK_ICD_FILENAMES: icd, VK_DRIVER_FILES: icd } });
  const p = await b.newPage();
  p.on('console', (m) => console.log(`[console ${m.type()}] ${m.text().slice(0, 300)}`));
  await p.goto(`http://127.0.0.1:${port}/web/index.html?pkg=${process.argv[3] || 'both'}`);
  await p.evaluate(() => window.probeReady);
  await p.evaluate(() => window.wasm.set_log_level('info'));
  const r = await Promise.race([p.evaluate((bk) => window.runAdapter(bk), process.argv[2]), new Promise((res) => setTimeout(() => res('TIMEOUT 20s'), 20000))]);
  console.log(typeof r === 'string' ? r : JSON.stringify(r).slice(0, 600));
  await b.close(); server.close();
})();
