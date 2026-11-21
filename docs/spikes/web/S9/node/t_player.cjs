// S9 req 5 (b): player bundles via <input type=file> (setInputFiles) and via a real
// DataTransfer drop event; hostile bundles from S4's corpus rejected with the same error
// text as native (S4's native classification, web limits); the engine stays usable (another
// bundle loads and plays afterwards); File.arrayBuffer() -> one copy into linear memory;
// memory after each load. usage: node t_player.cjs <backend> <out.json>
'use strict';
const fs = require('fs');
const path = require('path');
const { S9, startServer, launch, watch, waitReady, writeJson, decodePng, pixelStats, poll } = require('./pw.cjs');

const DIST = path.join(S9, 'dist');
const native = {};
for (const line of fs.readFileSync(path.join(DIST, 'test', 'zip', 'native.web.txt'), 'utf8').split('\n')) {
  const m = line.match(/^(\S+) open=(ok|err\((.*)\)) /);
  if (m) native[m[1]] = m[2] === 'ok' ? 'ok' : m[3];
}

(async () => {
  const backend = process.argv[2] || 'webgpu';
  const out = process.argv[3] || path.join(S9, 'results', 'player.json');
  const srv = await startServer(9000 + Math.floor(Math.random() * 90));
  const L = await launch('webgpu');
  const page = await L.browser.newPage({ viewport: { width: 400, height: 720 } });
  const log = watch(page);
  await page.goto(`${srv.url}?backend=${backend}&table=apex_circuit&w=360&h=640&post=min`);
  await waitReady(page);
  const res = { chromium: L.version, steps: [] };
  let fails = 0;
  const check = (name, cond, detail) => { res.steps.push({ check: name, ok: !!cond, detail }); if (!cond) fails += 1; console.log(`${cond ? 'PASS' : 'FAIL'} ${name} ${detail ? JSON.stringify(detail).slice(0, 500) : ''}`); };
  const loads = () => page.evaluate(() => S9.state.loads.length);
  const lastLoad = () => page.evaluate(() => S9.state.loads[S9.state.loads.length - 1]);
  const status = () => page.evaluate(() => S9.status());
  const banner = () => page.evaluate(() => ({ hidden: document.getElementById('error').hidden, text: document.getElementById('error').textContent }));
  const waitLoad = async (n) => { await poll(page, (k) => S9.state.loads.length > k, n, 60000, 200); return lastLoad(); };
  const playsFor = async (ms) => {
    const a = await status(); await page.waitForTimeout(ms); const b = await status();
    return { tick_before: a.tick, tick_after: b.tick, frames: await page.evaluate(() => S9.state.frames), score: b.score, rules: b.rules, faulted: b.faulted, table_hash: b.table_hash };
  };
  const mem = () => page.evaluate(() => ({ linear_memory: S9.memory(), heap: S9.heap() }));
  res.initial = { load: await lastLoad(), mem: await mem(), plays: await playsFor(1500) };
  check('built-in apex_circuit loaded and plays', res.initial.plays.tick_after > res.initial.plays.tick_before, res.initial.plays);

  // 1) Player bundle via <input type=file>.
  let n = await loads();
  await page.setInputFiles('#bundleFile', path.join(DIST, 'test', 'player', 'feature_lab_player.pinbundle'));
  const l1 = await waitLoad(n);
  const p1 = await playsFor(2500);
  res.input_player = { load: l1, plays: p1, mem: await mem() };
  check('input: modified feature_lab player bundle loads (rules.lua)', l1.ok && l1.via === 'input' && l1.info.rules === 'lua', l1);
  check('input: player bundle plays (ticks advance, its rules set the marker score 900009)', p1.tick_after > p1.tick_before && p1.score === 900009 && !p1.faulted, p1);
  await page.evaluate(() => document.body.classList.add('clean'));
  const png1 = await page.locator('#view').screenshot({ path: path.join(S9, 'shots', `player_bundle_${backend}.png`) });
  await page.evaluate(() => document.body.classList.remove('clean'));
  res.input_player.pixels = pixelStats(decodePng(png1));

  // 2) Hostile bundles via <input type=file>: same error as native; engine keeps playing.
  for (const f of ['20_traversal_dotdot', '35_encrypted_flag', '38_unsupported_method_bzip2']) {
    n = await loads();
    await page.setInputFiles('#bundleFile', path.join(DIST, 'test', 'zip', `${f}.zip`));
    const l = await waitLoad(n);
    const b = await banner();
    const p = await playsFor(1000);
    res[`input_${f}`] = { load: l, banner: b, plays: p };
    check(`input: ${f} rejected with the native error`, !l.ok && l.error === native[f] && !b.hidden && b.text.includes(native[f]), { web: l.error, native: native[f], banner: b.text });
    check(`input: after ${f} the previous table keeps playing`, p.tick_after > p.tick_before && p.score === 900009, p);
  }

  // 3) Drop: a real DataTransfer drop event with a File built in the page.
  const drop = async (file, name) => {
    const b64 = fs.readFileSync(file).toString('base64');
    n = await loads();
    await page.evaluate(([data, nm]) => {
      const bin = atob(data); const u8 = new Uint8Array(bin.length);
      for (let i = 0; i < bin.length; i++) u8[i] = bin.charCodeAt(i);
      const dt = new DataTransfer();
      dt.items.add(new File([u8], nm, { type: 'application/zip' }));
      const target = document.getElementById('view');
      target.dispatchEvent(new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer: dt }));
      target.dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer: dt }));
    }, [b64, name]);
    return waitLoad(n);
  };
  for (const f of ['10_bomb_ratio_single_50MiB', '21_traversal_nested_dotdot', '42_truncated_central_directory']) {
    const l = await drop(path.join(DIST, 'test', 'zip', `${f}.zip`), `${f}.zip`);
    const b = await banner();
    const p = await playsFor(1000);
    res[`drop_${f}`] = { load: l, banner: b, plays: p, mem: await mem() };
    check(`drop: ${f} rejected with the native error`, !l.ok && l.via === 'drop' && l.error === native[f] && !b.hidden, { web: l.error, native: native[f] });
    check(`drop: after ${f} the engine keeps playing`, p.tick_after > p.tick_before, p);
  }
  // A file above the 32 MiB web cap is refused from File.size, before any copy.
  {
    const memBefore = await mem();
    n = await loads();
    await page.setInputFiles('#bundleFile', path.join(DIST, 'test', 'zip', '13_bomb_70MiB_ratio_ok_web_cap.zip'));
    const l = await waitLoad(n);
    const memAfter = await mem();
    res.input_oversize = { load: l, memBefore, memAfter };
    check('input: 70 MiB file refused on File.size before copying (linear memory unchanged)', !l.ok && l.copied === false && memAfter.linear_memory === memBefore.linear_memory, { error: l.error, before: memBefore.linear_memory, after: memAfter.linear_memory });
  }
  // A production bundle dropped after the rejections loads and plays (usable engine).
  {
    const l = await drop(path.join(DIST, 'bundles', 'sky_deck.pinbundle'), 'sky_deck.pinbundle');
    const p = await playsFor(2000);
    res.drop_sky_deck = { load: l, plays: p, mem: await mem() };
    check('drop: sky_deck bundle loads after the rejected ones', l.ok && l.via === 'drop' && l.info.files === 49 && l.info.rules === 'lua', l);
    check('drop: sky_deck plays', p.tick_after > p.tick_before && !p.faulted, p);
    const b = await banner();
    check('error banner cleared after a successful load', b.hidden, b);
  }
  // And the wasm-rules variant of a production table via the picker.
  {
    n = await loads();
    await page.setInputFiles('#bundleFile', path.join(DIST, 'bundles', 'feature_lab_wasm.pinbundle'));
    const l = await waitLoad(n);
    const p = await playsFor(2000);
    res.input_feature_lab_wasm = { load: l, plays: p, mem: await mem() };
    check('input: feature_lab_wasm (rules.wasm via wasmi) loads and plays', l.ok && l.info.rules === 'wasm' && p.tick_after > p.tick_before && !p.faulted, { load: l.info, plays: p });
  }
  res.memory_final = await mem();
  res.loads = await page.evaluate(() => S9.state.loads.map((x) => ({ ok: x.ok, name: x.name, via: x.via, error: x.error, memBefore: x.memBefore, memAfter: x.memAfter, readMs: x.readMs, loadMs: x.loadMs, size: x.size })));
  res.gpuErrors = await page.evaluate(() => S9.gpuErrors());
  const ce = log.console.filter((m) => m.type === 'error');
  check('no console errors / page errors / GPU errors', ce.length === 0 && log.errors.length === 0 && res.gpuErrors.uncaptured.length === 0, { ce, pe: log.errors, gpu: res.gpuErrors });
  res.fails = fails;
  writeJson(out, res);
  console.log(`== player: ${fails} failing check(s); peak linear memory ${res.memory_final.linear_memory} B; heap ${JSON.stringify(res.memory_final.heap)}`);
  await L.browser.close(); srv.proc.kill();
  process.exit(fails ? 1 : 0);
})().catch((e) => { console.error(e); process.exit(2); });
