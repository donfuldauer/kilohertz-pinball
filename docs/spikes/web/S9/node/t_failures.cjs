// S9 req 9: failure and recovery in the page.
// (i) Lua script error and Lua budget kill (S2's fault injections appended to feature_lab's
//     rules.lua, as player bundles): the rules tilt, the sim continues, every tick equals S2's
//     native fault-injection run, the page shows the error.
// (ii) a rules.wasm fuel kill (hostile player rules.wasm): tilt, sim continues, error shown.
// (iii) an engine trap mid-run (Rust panic -> wasm unreachable): the page shows it, later
//     calls fail with "Module terminated", the page re-instantiates the module and continues.
// (iv) the 4 MiB shadow stack in the combined module: S1/S2/S4 stack probes + the high-water
//     mark of load/step/render with Lua and with wasmi.
// usage: node t_failures.cjs <backend> <out.json>
'use strict';
const fs = require('fs');
const path = require('path');
const { S9, startServer, launch, watch, waitReady, writeJson, poll } = require('./pw.cjs');

(async () => {
  const backend = process.argv[2] || 'webgpu';
  const out = process.argv[3] || path.join(S9, 'results', 'failures.json');
  const srv = await startServer(9100 + Math.floor(Math.random() * 90));
  const L = await launch('webgpu');
  const res = { chromium: L.version, checks: [] };
  let fails = 0;
  const check = (name, cond, detail) => { res.checks.push({ check: name, ok: !!cond, detail }); if (!cond) fails += 1; console.log(`${cond ? 'PASS' : 'FAIL'} ${name} ${detail ? JSON.stringify(detail).slice(0, 600) : ''}`); };

  // (i) Lua faults, replayed against S2's native fault-injection hashes while rendering.
  {
    const page = await L.browser.newPage({ viewport: { width: 400, height: 720 } });
    const log = watch(page);
    await page.goto(`${srv.url}?mode=idle&backend=${backend}&w=360&h=640&post=min`);
    await waitReady(page);
    for (const inj of ['runtime_tick', 'budget_tick', 'memory_tick', 'runtime_switch']) {
      const r = await page.evaluate((i) => S9.runReplay({ table: 'feature_lab', rules: 'lua', scenario: 'feature_lab_bot60', tpf: 500, scr: false,
        bundle: `test/player/feature_lab_${i}.pinbundle`, refUrl: `test/refs/feature_lab_bot60.${i}.lua.ref` }), inj);
      const b = await page.evaluate(() => ({ hidden: document.getElementById('error').hidden, text: document.getElementById('error').textContent }));
      const st = r.status; const rp = st.replay;
      res[`lua_${inj}`] = { status: st, banner: b, frames: r.frames, gpuErrors: r.gpuErrors };
      // ScriptError's Display texts. memory_tick (100 x 65 000-byte strings in one callback) is
      // charged to the instruction budget before the 2 MiB cap is reached, natively too (S2 sandbox
      // case mem_big_strings_fast: first_msg="BudgetExceeded" on native and wasm).
      const expect = { runtime_tick: 'injected fault', budget_tick: 'script instruction budget exceeded', memory_tick: 'script instruction budget exceeded', runtime_switch: 'injected switch fault' }[inj];
      check(`(i) lua ${inj}: tilt with "${expect}", sim ran to the end`, st.faulted && st.fault_count === 1 && String(st.error).includes(expect) && st.tick === 60000, { error: st.error, faults: st.fault_count, tick: st.tick });
      check(`(i) lua ${inj}: every tick == S2 native fault-injection run`, rp.compared === 60001 && rp.state_bad + rp.ext_bad === 0, rp);
      check(`(i) lua ${inj}: page shows the rules error`, !b.hidden && b.text.includes('Rules stopped (tilt)') && b.text.includes(expect), b.text);
    }
    res.lua_console_errors = log.console.filter((m) => m.type === 'error');
    check('(i) no console/page errors', res.lua_console_errors.length === 0 && log.errors.length === 0, { ce: res.lua_console_errors, pe: log.errors });
    await page.close();
  }

  // (ii) rules.wasm fuel kill in live play.
  {
    const page = await L.browser.newPage({ viewport: { width: 400, height: 720 } });
    const log = watch(page);
    await page.goto(`${srv.url}?backend=${backend}&table=apex_circuit&w=360&h=640&post=min`);
    await waitReady(page);
    const l = await page.evaluate(() => S9.loadUrl('test/player/feature_lab_fuelkill.pinbundle'));
    await poll(page, () => S9.status().tick > 4500, null, 120000, 250);
    const a = await page.evaluate(() => S9.status());
    await page.waitForTimeout(1500);
    const b = await page.evaluate(() => S9.status());
    const ban = await page.evaluate(() => ({ hidden: document.getElementById('error').hidden, text: document.getElementById('error').textContent }));
    res.wasm_fuelkill = { load: l, status: b, banner: ban };
    check('(ii) hostile rules.wasm loads through wasmi + instrumentation', l.ok && l.info.rules === 'wasm', l.info || l.error);
    check('(ii) fuel kill -> tilt (script instruction budget exceeded), score set by on_init kept', b.faulted && b.fault_count === 1 && String(b.error).includes('script instruction budget exceeded') && b.score === 777, { error: b.error, faults: b.fault_count, score: b.score });
    check('(ii) sim keeps running after the kill', b.tick > a.tick, { a: a.tick, b: b.tick });
    check('(ii) page shows the rules error', !ban.hidden && ban.text.includes('script instruction budget exceeded'), ban.text);
    check('(ii) no console/page errors', log.console.filter((m) => m.type === 'error').length === 0 && log.errors.length === 0, log.errors);
    await page.close();
  }

  // (iii) Engine trap mid-run -> visible error -> re-instantiation -> play continues.
  {
    const page = await L.browser.newPage({ viewport: { width: 400, height: 720 } });
    const log = watch(page);
    await page.goto(`${srv.url}?backend=${backend}&table=sky_deck&w=360&h=640&post=min`);
    await waitReady(page);
    await poll(page, () => S9.status().tick > 500, null, 60000, 250);
    const before = await page.evaluate(() => ({ gen: S9.generation(), tick: S9.status().tick, mem: S9.memory() }));
    await page.evaluate(() => S9.armTrap(300));
    await poll(page, () => S9.state.crash !== null && S9.state.restarts > 0, null, 120000, 250);
    const crash = await page.evaluate(() => ({ crash: S9.state.crash, restarts: S9.state.restarts, gen: S9.generation(), restartLoad: S9.state.restartLoad }));
    const ban = await page.evaluate(() => ({ hidden: document.getElementById('error').hidden, text: document.getElementById('error').textContent, shown: S9.state.errorsShown.map((e) => e.msg) }));
    await poll(page, () => S9.status().tick > 500, null, 60000, 250);
    const after = await page.evaluate(() => ({ tick: S9.status().tick, frames: S9.state.frames, gen: S9.generation(), mem: S9.memory() }));
    res.trap = { before, crash, banner: ban, after, console: log.console.filter((m) => m.type === 'error').map((m) => m.text) };
    check('(iii) trap caught by the page (RuntimeError: unreachable + panic message)', /unreachable/i.test(crash.crash.error) && /S9 injected engine trap/.test(crash.crash.panic || ''), crash.crash);
    check('(iii) a later call into the trapped instance fails with "Module terminated"', /Module terminated/.test(crash.crash.afterTrap || ''), crash.crash.afterTrap);
    check('(iii) page showed "Engine crashed" then restarted', ban.shown.some((m) => m.startsWith('Engine crashed')) && ban.shown.some((m) => m.startsWith('Engine restarted')), ban.shown);
    check('(iii) re-instantiated module (new generation) reloaded the table and plays', crash.gen === before.gen + 1 && crash.restartLoad && crash.restartLoad.ok && after.tick > 500 && after.frames > 0, { before, after, restartLoad: crash.restartLoad && crash.restartLoad.ok });
    // Late async callbacks of the discarded instance throw "Module terminated" (wasm-bindgen's
    // closure trampolines): the page swallows and counts them; nothing else may surface.
    check('(iii) the only console errors are the panic report; the only page errors are late callbacks of the dead instance',
      res.trap.console.every((t) => /pin_web panic|unreachable|Module terminated/.test(t)) && log.errors.every((t) => /Module terminated/.test(t)),
      { console: res.trap.console, pe: log.errors, lateCallbacks: crash.crash.lateCallbacks || 0 });
    await page.close();
  }

  // (iv) Stack: 4 MiB shadow stack, probes inside the combined module.
  {
    const page = await L.browser.newPage({ viewport: { width: 400, height: 720 } });
    const log = watch(page);
    await page.goto(`${srv.url}?mode=idle&backend=${backend}&w=360&h=640&post=full`);
    await waitReady(page);
    const layout = await page.evaluate(() => S9.probe('probe_stack_layout'));
    await page.evaluate(() => S9.measureStack(true));
    const lua = await page.evaluate(() => S9.runReplay({ table: 'sky_deck', rules: 'lua', scenario: 'sky_deck_mb8', tpf: 500 }));
    const wasm = await page.evaluate(() => S9.runReplay({ table: 'sky_deck', rules: 'wasm', scenario: 'sky_deck_mb8', tpf: 500 }));
    const hw = await page.evaluate(() => S9.stackReport());
    await page.evaluate(() => S9.measureStack(false));
    const caseStack = await page.evaluate(() => S9.probe('probe_lua_case_stack'));
    const caseTop = caseStack.trim().split('\n').map((l) => { const m = l.match(/^(\S+) stack_used=(\d+)/); return m ? [m[1], Number(m[2])] : null; }).filter(Boolean).sort((a, b) => b[1] - a[1]).slice(0, 3);
    const rulesStack = await page.evaluate(() => S9.probe('probe_rules_stack', 'wasmi-instr-depth', 1));
    const worst = await page.evaluate(() => S9.probe('probe_rules_worst', 'wasmi-instr-depth', 1, 'loop_br_only'));
    const recurse = await page.evaluate(() => { const r = {}; for (const n of [1000, 3000, 3500]) { try { S9.probe('probe_recurse_big', n); r[n] = 'ok'; } catch (e) { r[n] = String(e); } } return r; });
    const sp = await page.evaluate(() => S9.probe('probe_sp_after_errors', 20));
    res.stack = { layout, highwater: hw, lua_parity: lua.status.replay, wasm_parity: wasm.status.replay, caseTop, rulesStack, worst, recurse, sp_after_errors: sp };
    const maxHw = Math.max(...Object.values(hw));
    check('(iv) stack-first layout, 4 MiB shadow stack', /stack-first/.test(layout) && /sp~0x3f/.test(layout), layout);
    check('(iv) high-water of load/step/build/render (Lua + wasmi, 9-ball sky_deck_mb8) < 4 MiB with margin', maxHw > 0 && maxHw < 2 * 1024 * 1024 && lua.status.replay.state_bad + wasm.status.replay.state_bad === 0, hw);
    check('(iv) S2 deepest Lua C recursion case runs (cstack_gsub) in the combined module', caseTop.length && caseTop[0][1] < 1024 * 1024, caseTop);
    check('(iv) S4 depth counter limits 2338/711/229/78/15 in the combined module (no RangeError)', /locals=0 max_ok_depth=\[2338/.test(rulesStack) && /locals=1000 max_ok_depth=\[15/.test(rulesStack), rulesStack);
    check('(iv) S4 F-A2: 100 000-fuel loop ends as a budget fault (auto-dispatch, no RangeError)', /budget|Budget/.test(worst) && !/RangeError/.test(worst), worst);
    check('(iv) S1 A28: 3500 x 1 KiB frames fit the 4 MiB stack', recurse[3500] === "ok", recurse);
    check('(iv) S2 SP restored after 20x(4 faults + 2000 pcall errors)', /delta=0 /.test(sp), sp);
    check('(iv) no console/page errors', log.console.filter((m) => m.type === 'error').length === 0 && log.errors.length === 0, log.errors);
    await page.close();
  }
  res.fails = fails;
  writeJson(out, res);
  console.log(`== failures: ${fails} failing check(s)`);
  await L.browser.close(); srv.proc.kill();
  process.exit(fails ? 1 : 0);
})().catch((e) => { console.error(e); process.exit(2); });
