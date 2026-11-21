const { chromium } = require('playwright');
const { spawn } = require('child_process');
(async () => {
  const srv = spawn(process.execPath, [__dirname + '/serve.cjs', __dirname + '/web', '8797'], { stdio: 'inherit' });
  await new Promise(r => setTimeout(r, 500));
  const b = await chromium.launch();
  const pg = await b.newPage();
  const errs = []; pg.on('console', m => { if (m.type() === 'error') errs.push(m.text()); }); pg.on('pageerror', e => errs.push(String(e)));
  await pg.goto('http://127.0.0.1:8797/');
  await pg.waitForFunction ? null : null;
  for (let i = 0; i < 50 && !(await pg.evaluate(() => typeof window.runProbe === 'function')); i++) await new Promise(r => setTimeout(r, 100));
  const res = await pg.evaluate(() => window.runProbe());
  console.log(JSON.stringify({ browser: b.version(), ...res, console_errors: errs }));
  await b.close(); srv.kill();
})().catch(e => { console.error(e); process.exit(1); });
