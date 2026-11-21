// Tries Chromium flag sets for WebGPU/WebGL2 in headless mode and prints what each gives.
'use strict';
const { chromium } = require('playwright');
const path = require('path');
const { start } = require('./server');
const SETS = {
  none: [],
  unsafe_only: ['--enable-unsafe-webgpu'],
  swiftshader_vk: ['--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-angle=swiftshader', '--use-webgpu-adapter=swiftshader', '--enable-unsafe-swiftshader', '--ignore-gpu-blocklist', '--disable-vulkan-surface'],
  swiftshader_vk2: ['--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=swiftshader', '--use-angle=swiftshader', '--use-webgpu-adapter=swiftshader', '--enable-unsafe-swiftshader', '--ignore-gpu-blocklist', '--disable-vulkan-surface'],
  minimal: ['--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-angle=vulkan', '--ignore-gpu-blocklist'],
  minimal_swiftshader_icd: ['--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-angle=vulkan', '--ignore-gpu-blocklist', '--use-vulkan=swiftshader'],
  webgl_swiftshader: ['--use-gl=angle', '--use-angle=swiftshader', '--enable-unsafe-swiftshader', '--ignore-gpu-blocklist'],
};
(async () => {
  const server = await start(path.resolve(__dirname, '../web'), 8643);
  const which = process.argv.slice(2);
  const headlessModes = (process.env.HEADLESS_MODES || 'shell,new').split(',');
  for (const [name, args] of Object.entries(SETS)) {
    if (which.length && !which.includes(name)) continue;
    for (const mode of headlessModes) {
      const opts = { args };
      if (mode === 'new') opts.channel = 'chromium'; // full chromium in new headless mode
      let b;
      try {
        b = await chromium.launch(opts);
        const p = await b.newPage();
        await p.goto('http://127.0.0.1:8643/gpucheck.html');
        const r = await p.evaluate(() => window.check());
        if (r.glExt) r.glExt = r.glExt.length + ' exts';
        console.log(JSON.stringify({ set: name, mode, version: b.version(), ...r }));
      } catch (e) { console.log(JSON.stringify({ set: name, mode, launchError: String(e).slice(0, 300) })); }
      if (b) await b.close();
    }
  }
  server.close();
})();
