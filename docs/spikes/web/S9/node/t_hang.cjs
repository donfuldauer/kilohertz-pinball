// Debug: live page from main(); if evaluate stalls, pause JS via CDP and print the stack.
'use strict';
const { startServer, launch } = require('./pw.cjs');
(async () => {
  const srv = await startServer(9301);
  const { browser } = await launch('webgpu');
  const page = await browser.newPage({ viewport: { width: 400, height: 720 } });
  const t0 = Date.now();
  page.on('console', (m) => console.log(Date.now() - t0, 'console', m.type(), m.text().slice(0, 300)));
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('Debugger.enable');
  cdp.on('Debugger.paused', (e) => {
    console.log(Date.now() - t0, 'PAUSED', e.reason, e.callFrames.slice(0, 12).map((f) => `${f.functionName}@${f.url.split('/').pop()}:${f.location.lineNumber}`).join(' <- '));
    cdp.send('Debugger.resume').catch(() => {});
  });
  await page.goto(`${srv.url}?backend=webgpu&table=apex_circuit&w=360&h=640&post=min&debug=1`);
  for (let i = 0; i < 10; i++) {
    const v = await Promise.race([page.evaluate(() => JSON.stringify({ m: Object.keys(S9.state.marks), f: S9.state.frames })), new Promise((r) => setTimeout(() => r('TIMEOUT'), 3000))]);
    console.log(Date.now() - t0, 'eval', v);
    if (v === 'TIMEOUT') { await Promise.race([cdp.send('Debugger.pause'), new Promise((r) => setTimeout(r, 3000))]); await new Promise((r) => setTimeout(r, 1500)); }
  }
  srv.proc.kill(); await browser.close(); process.exit(0);
})().catch((e) => { console.error(e); process.exit(1); });
