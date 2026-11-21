// S9 req 4 (a): replay S1's recordings in Chromium WHILE rendering every frame, per-tick
// hash parity against native. usage: node t_replay.cjs <backend> <W> <H> <post> <tpf> <out.json> <runs...>
// runs: <table>:<rules>[:<scenario>[:<tpf>[:<maxFrames>]]]
'use strict';
const { startServer, launch, watch, waitReady, writeJson, decodePng, pixelStats } = require('./pw.cjs');
const path = require('path');
(async () => {
  const [backend, W, H, post, tpf, out, ...runs] = process.argv.slice(2);
  const srv = await startServer(8800 + Math.floor(Math.random() * 100));
  const { browser, version, args } = await launch(backend === 'webgl2' ? 'webgpu' : backend);
  const page = await browser.newPage({ viewport: { width: Number(W), height: Number(H) } });
  const log = watch(page);
  await page.goto(`${srv.url}?mode=idle&backend=${backend}&w=${W}&h=${H}&post=${post}`);
  await waitReady(page);
  const gpu = await page.evaluate(() => S9.gpuInfo());
  const results = [];
  let fails = 0;
  for (const r of runs) {
    const [table, rules, scenario, rtpf, maxFrames] = r.split(':');
    const o = { table, rules, scenario: scenario || undefined, tpf: Number(rtpf || tpf), maxFrames: maxFrames ? Number(maxFrames) : undefined };
    const t0 = Date.now();
    const res = await page.evaluate((x) => S9.runReplay(x), o);
    res.hostWallMs = Date.now() - t0;
    const shot = path.join(path.dirname(out), '..', 'shots', `replay_${res.scenario}_${rules}_${backend}.png`);
    await page.evaluate(() => document.body.classList.add('clean'));
    const png = await page.locator('#view').screenshot({ path: shot });
    await page.evaluate(() => document.body.classList.remove('clean'));
    res.shot = shot; res.pixels = pixelStats(decodePng(png));
    const st = res.status; const rp = st && st.replay;
    const full = !o.maxFrames;
    const ok = !!rp && rp.state_bad === 0 && rp.ext_bad === 0 && rp.cmd_bad === 0 && rp.first_bad === null &&
      (!full || (rp.done && rp.compared === rp.ticks + 1)) && !res.crash &&
      (rules !== 'lua' || (st.lua_stream.tick_mismatch + st.lua_stream.draw_mismatch + st.lua_stream.digest_mismatch + st.lua_stream.queue_mismatch === 0)) &&
      res.gpuErrors.uncaptured.length === 0 && res.gpuErrors.deferred_scope_errors.length === 0;
    if (!ok) fails += 1;
    results.push({ ok, ...res });
    console.log(`${ok ? 'PASS' : 'FAIL'} ${res.scenario} ${rules} ${backend} ${W}x${H} post=${post} ticks=${st && st.tick} compared=${rp && rp.compared} ` +
      `state_bad=${rp && rp.state_bad} ext_bad=${rp && rp.ext_bad} cmd_bad=${rp && rp.cmd_bad} first_bad=${rp && rp.first_bad} ` +
      `lua_stream=${st && st.lua_stream ? JSON.stringify(st.lua_stream) : '-'} final_state=${st && st.state_hash} frames=${res.frames} ` +
      `tpf=${res.tpfMin}..${res.tpfMax} waitRaf=${res.waitRaf} wall_s=${(res.wallMs / 1000).toFixed(1)} fps=${(res.frames / res.wallMs * 1000).toFixed(2)} ` +
      `render_ms_med=${res.renderMsMedian.toFixed(2)} step_ms_med=${res.stepMsMedian.toFixed(2)} gpu_errors=${res.gpuErrors.uncaptured.length + res.gpuErrors.deferred_scope_errors.length} ` +
      `nonbg=${res.pixels.non_background_fraction} colours=${res.pixels.distinct_colours}`);
  }
  const consoleErrors = log.console.filter((m) => m.type === 'error');
  writeJson(out, { chromium: version, flags: args, gpu, results, consoleErrors, pageErrors: log.errors, crashed: log.crashed, fails });
  console.log(`== replay ${backend}: ${fails} failing, console errors ${consoleErrors.length}, page errors ${log.errors.length}`);
  await browser.close(); srv.proc.kill();
  process.exit(fails || consoleErrors.length || log.errors.length ? 1 : 0);
})().catch((e) => { console.error(e); process.exit(2); });
