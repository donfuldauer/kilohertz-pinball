// S9 req 8 (g): mobile smoke in Chromium with Playwright device descriptors (Pixel 7, iPhone 14;
// portrait and landscape). THIS IS CHROMIUM WITH MOBILE VIEWPORT/TOUCH/UA EMULATION, NOT Safari/
// iOS WebKit and not a phone GPU: it proves layout and input mapping only.
// Checks: usable layout (canvas fills the viewport, controls and launch zone inside it, no
// scrolling), a frame renders, touch events (CDP Input.dispatchTouchEvent = real touchstart/
// touchend, which the page receives as pointer events) map to flipper/launch inputs and move the
// sim (flipper angles, live events, ball launched), short live session.
// usage: node t_mobile.cjs <out.json>
'use strict';
const path = require('path');
const fs = require('fs');
const { devices, chromium } = require('playwright');
const { S9, startServer, launch, watch, waitReady, writeJson, decodePng, pixelStats, poll } = require('./pw.cjs');

(async () => {
  const out = process.argv[2] || path.join(S9, 'results', 'mobile.json');
  const srv = await startServer(9200 + Math.floor(Math.random() * 90));
  const L = await launch('webgpu');
  const res = { chromium: L.version, note: 'Chromium device emulation (viewport, DPR, touch, UA); not WebKit/Safari, not phone hardware', runs: [] };
  let fails = 0;
  const check = (name, cond, detail) => { res.runs.push({ check: name, ok: !!cond, detail }); if (!cond) fails += 1; console.log(`${cond ? 'PASS' : 'FAIL'} ${name} ${detail ? JSON.stringify(detail).slice(0, 420) : ''}`); };
  for (const dev of ['Pixel 7', 'Pixel 7 landscape', 'iPhone 14', 'iPhone 14 landscape']) {
    const d = { ...devices[dev] };
    delete d.defaultBrowserType; // run the descriptor in Chromium
    const ctx = await L.browser.newContext(d);
    const page = await ctx.newPage();
    const log = watch(page);
    await page.goto(`${srv.url}?backend=auto&table=sky_deck&scale=0.5&post=min`);
    await waitReady(page);
    await poll(page, () => S9.state.frames > 2, null, 60000, 250);
    const layout = await page.evaluate(() => {
      const r = (id) => { const b = document.getElementById(id).getBoundingClientRect(); return [Math.round(b.left), Math.round(b.top), Math.round(b.width), Math.round(b.height)]; };
      return { inner: [innerWidth, innerHeight], dpr: devicePixelRatio, maxTouchPoints: navigator.maxTouchPoints, ua: navigator.userAgent.slice(0, 80),
        canvasCss: r('view'), canvasPx: [document.getElementById('view').width, document.getElementById('view').height],
        controls: r('controls'), launch: r('launchZone'), scroll: [document.scrollingElement.scrollWidth, document.scrollingElement.scrollHeight],
        touchAction: getComputedStyle(document.getElementById('view')).touchAction, gpu: S9.gpuInfo().backend };
    });
    const tag = dev.replace(/ /g, '_');
    await page.screenshot({ path: path.join(S9, 'shots', `mobile_${tag}.png`) });
    const inside = (b) => b[0] >= 0 && b[1] >= 0 && b[0] + b[2] <= layout.inner[0] + 1 && b[1] + b[3] <= layout.inner[1] + 1;
    check(`${dev}: layout usable (canvas fills ${layout.inner}, controls + launch zone inside, no scroll, touch-action none)`,
      layout.canvasCss[2] === layout.inner[0] && layout.canvasCss[3] === layout.inner[1] && inside(layout.controls) && inside(layout.launch) &&
      layout.scroll[0] <= layout.inner[0] && layout.scroll[1] <= layout.inner[1] && layout.touchAction === 'none' && layout.maxTouchPoints > 0, layout);
    // PiPs (backglass, score display) stay off the lower playfield / the table column.
    const lay = await page.evaluate(() => S9.layout());
    const [cw, ch] = lay.canvas;
    const land = cw * 10 >= ch * 12;
    const clear = lay.pips.every((r) => land
      ? (r.x + r.w <= cw / 2 - 0.275 * ch || r.x >= cw / 2 + 0.275 * ch)
      : r.y + r.h <= 0.6 * ch);
    res[`${tag}_pips`] = lay;
    check(`${dev}: PiPs off the flipper area (${land ? 'side columns' : 'top band'})`, clear && lay.pips.length === 2, lay);
    // Frame renders.
    await page.evaluate(() => document.body.classList.add('clean'));
    const png = await page.locator('#view').screenshot();
    await page.evaluate(() => document.body.classList.remove('clean'));
    const st = pixelStats(decodePng(png));
    check(`${dev}: frame renders (${layout.gpu}, drawing buffer ${layout.canvasPx})`, st.distinct_colours >= 128 && st.non_background_fraction >= 0.2, st);
    // Touch input -> flipper / launch.
    const cdp = await ctx.newCDPSession(page);
    const touch = async (x, y, holdMs) => {
      const t0 = await page.evaluate(() => S9.status().tick);
      await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x, y, id: 1 }] });
      // Hold for holdMs of SIMULATED time (the loop runs slower than real time on a loaded VM).
      await poll(page, (t) => S9.status().tick > t, t0 + holdMs, 60000, 50);
      const mid = await page.evaluate(() => S9.status());
      await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
      return mid;
    };
    const [W, H] = layout.inner;
    const s0 = await page.evaluate(() => S9.status());
    const left = await touch(Math.round(W * 0.2), Math.round(H * 0.6), 400);
    const right = await touch(Math.round(W * 0.8), Math.round(H * 0.6), 400);
    // Wait in simulated time (the live loop runs slower than real time on a loaded VM).
    await poll(page, (t) => S9.status().tick > t + 400, right.tick, 60000, 100);
    const rest = await page.evaluate(() => S9.status());
    const launch1 = await touch(Math.round(W * 0.5), Math.round(H * 0.95), 600);
    await page.waitForTimeout(2500);
    const s1 = await page.evaluate(() => S9.status());
    const inputs = await page.evaluate(() => S9.state.inputs.filter((i) => i.via === 'touch').map((i) => `${i.name}:${i.pressed ? 'down' : 'up'}`));
    res[`${tag}_touch`] = { s0, left, right, rest, launch: launch1, s1, inputs };
    const moved = (a, b) => a.flippers.some((v, i) => Math.abs(v - b.flippers[i]) > 0.05);
    const up = (a, i) => Math.abs(a.flippers[i]) > 0.5; // flipper i raised (rad)
    check(`${dev}: touch left half -> left flipper, right half -> right flipper, bottom centre -> launch (pointer events, pointerType touch)`,
      inputs.join(' ') === 'left:down left:up right:down right:up launch:down launch:up', inputs);
    check(`${dev}: left touch raises flipper 0 only, right touch flipper 1 only, both return`, up(left, 0) && !up(left, 1) && up(right, 1) && !up(right, 0) && !moved(rest, s0), { s0: s0.flippers, left: left.flippers, right: right.flippers, rest: rest.flippers });
    check(`${dev}: live session runs with injected touch input (ticks advance, inputs reached the sim)`, s1.tick > s0.tick + 200 && s1.live_events >= 8 && !s1.faulted, { ticks: [s0.tick, s1.tick], live_events: s1.live_events, balls: s1.balls, max_balls: s1.max_balls, score: s1.score });
    const ce = log.console.filter((m) => m.type === 'error');
    check(`${dev}: no console/page errors`, ce.length === 0 && log.errors.length === 0, { ce, pe: log.errors });
    res[`${tag}_layout`] = layout;
    await ctx.close();
  }
  await L.browser.close();
  // Other Playwright engines in this environment.
  const cands = ['firefox', 'webkit'].map((n) => { const b = require('playwright')[n]; let p = ''; try { p = b.executablePath(); } catch (e) { p = String(e); } return { name: n, path: p, exists: fs.existsSync(p) }; });
  const home = path.join(process.env.HOME || '/root', '.cache', 'ms-playwright');
  res.engines = { candidates: cands, home_cache: fs.existsSync(home) ? fs.readdirSync(home) : 'absent', PLAYWRIGHT_BROWSERS_PATH: process.env.PLAYWRIGHT_BROWSERS_PATH, browsers_path: fs.readdirSync(process.env.PLAYWRIGHT_BROWSERS_PATH || '/opt/pw-browsers') };
  console.log(`engines: ${JSON.stringify(res.engines)}`);
  res.fails = fails;
  writeJson(out, res);
  console.log(`== mobile: ${fails} failing check(s)`);
  srv.proc.kill();
  process.exit(fails ? 1 : 0);
})().catch((e) => { console.error(e); process.exit(2); });
