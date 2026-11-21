// S3 render probe harness: loads the wasm-bindgen output of `web_probe`
// (variant from ?pkg=webgpu|webgl|both) and exposes async entry points on
// `window` for the Playwright driver (node/run_probe.js).
const params = new URLSearchParams(location.search);
const pkg = params.get('pkg') || 'both';
const status = (s) => { document.getElementById('status').textContent = s; };

function freshCanvas(w = 64, h = 64, css = null) {
  const stage = document.getElementById('stage');
  stage.replaceChildren();
  const c = document.createElement('canvas');
  c.width = w; c.height = h;
  if (css) Object.assign(c.style, css);
  stage.appendChild(c);
  return c;
}

async function bytes(url) {
  const r = await fetch(url);
  if (!r.ok) throw new Error(`fetch ${url}: ${r.status}`);
  return new Uint8Array(await r.arrayBuffer());
}

async function text(url) {
  const r = await fetch(url);
  if (!r.ok) throw new Error(`fetch ${url}: ${r.status}`);
  return r.text();
}

const errStr = (e) => (e && e.message) ? e.message : String(e);

window.probeReady = (async () => {
  const t0 = performance.now();
  const mod = await import(`./pkg/${pkg}/web_probe.js`);
  await mod.default(); // fetch + WebAssembly.instantiateStreaming
  window.wasm = mod;
  window.wasmLoadMs = performance.now() - t0;
  status(`ready (${pkg})`);
  return { pkg, build: JSON.parse(mod.build_info()), loadMs: window.wasmLoadMs };
})();

window.runAdapter = async (backend) => {
  await window.probeReady;
  try { return { ok: true, report: JSON.parse(await window.wasm.probe_adapter(backend, freshCanvas())) }; }
  catch (e) { return { ok: false, error: errStr(e) }; }
};

window.runRender = async (backend, table, w, h, opts = {}) => {
  await window.probeReady;
  const bundle = await bytes(`/bundles/${table}.zip`);
  const frame = await text(`/dumps/${table}.frame.json`);
  const canvas = freshCanvas(w, h);
  try {
    const r = await window.wasm.render_table(backend, canvas, bundle, frame, w, h, JSON.stringify(opts));
    // Base64 of the RGBA bytes for the driver (no structured clone of big arrays).
    const px = r.pixels;
    let bin = '';
    for (let i = 0; i < px.length; i += 0x8000) bin += String.fromCharCode.apply(null, px.subarray(i, i + 0x8000));
    return { ok: true, report: JSON.parse(r.report), rgbaB64: btoa(bin) };
  } catch (e) {
    return { ok: false, error: errStr(e) };
  }
};

window.runMatrix = async (backend, table, webgl2Limits) => {
  await window.probeReady;
  const bundle = await bytes(`/bundles/${table}.zip`);
  try { return { ok: true, report: JSON.parse(await window.wasm.pipeline_matrix(backend, freshCanvas(), bundle, webgl2Limits)) }; }
  catch (e) { return { ok: false, error: errStr(e) }; }
};

window.runScopes = async (backend) => {
  await window.probeReady;
  try { return { ok: true, report: JSON.parse(await window.wasm.scopes_and_device_lost(backend, freshCanvas())) }; }
  catch (e) { return { ok: false, error: errStr(e) }; }
};

window.runBlockOn = async (backend) => {
  await window.probeReady;
  try { return { ok: true, result: window.wasm.block_on_main_thread(backend) }; }
  catch (e) { return { ok: false, error: errStr(e), name: e && e.name }; }
};

window.runBench = async (backend, table, sizes, frames) => {
  await window.probeReady;
  const bundle = await bytes(`/bundles/${table}.zip`);
  const frame = await text(`/dumps/${table}.frame.json`);
  try { return { ok: true, report: JSON.parse(await window.wasm.bench_table(backend, freshCanvas(), bundle, frame, JSON.stringify(sizes), frames)) }; }
  catch (e) { return { ok: false, error: errStr(e) }; }
};

// Mobile check: a full-viewport canvas sized by devicePixelRatio, plus the
// raw WebGL2 / WebGPU size limits.
window.runViewport = async () => {
  const c = freshCanvas(1, 1, { width: '100vw', height: '100vh' });
  const rect = c.getBoundingClientRect();
  const dpr = window.devicePixelRatio;
  const out = { dpr, cssW: rect.width, cssH: rect.height,
    physW: Math.round(rect.width * dpr), physH: Math.round(rect.height * dpr),
    screen: [screen.width, screen.height], touch: navigator.maxTouchPoints, ua: navigator.userAgent };
  const gl = document.createElement('canvas').getContext('webgl2');
  if (gl) {
    out.webgl2 = { MAX_TEXTURE_SIZE: gl.getParameter(gl.MAX_TEXTURE_SIZE),
      MAX_RENDERBUFFER_SIZE: gl.getParameter(gl.MAX_RENDERBUFFER_SIZE),
      MAX_VIEWPORT_DIMS: Array.from(gl.getParameter(gl.MAX_VIEWPORT_DIMS)),
      MAX_SAMPLES: gl.getParameter(gl.MAX_SAMPLES),
      MAX_UNIFORM_BLOCK_SIZE: gl.getParameter(gl.MAX_UNIFORM_BLOCK_SIZE),
      MAX_FRAGMENT_UNIFORM_BLOCKS: gl.getParameter(gl.MAX_FRAGMENT_UNIFORM_BLOCKS),
      MAX_COMBINED_TEXTURE_IMAGE_UNITS: gl.getParameter(gl.MAX_COMBINED_TEXTURE_IMAGE_UNITS),
      MAX_TEXTURE_IMAGE_UNITS: gl.getParameter(gl.MAX_TEXTURE_IMAGE_UNITS),
      MAX_ARRAY_TEXTURE_LAYERS: gl.getParameter(gl.MAX_ARRAY_TEXTURE_LAYERS),
      MAX_VARYING_COMPONENTS: gl.getParameter(gl.MAX_VARYING_COMPONENTS),
      MAX_DRAW_BUFFERS: gl.getParameter(gl.MAX_DRAW_BUFFERS),
      EXT_color_buffer_float: !!gl.getExtension('EXT_color_buffer_float'),
      EXT_color_buffer_half_float: !!gl.getExtension('EXT_color_buffer_half_float'),
      OES_texture_float_linear: !!gl.getExtension('OES_texture_float_linear'),
      EXT_texture_filter_anisotropic: !!gl.getExtension('EXT_texture_filter_anisotropic'),
      EXT_disjoint_timer_query_webgl2: !!gl.getExtension('EXT_disjoint_timer_query_webgl2'),
      extensions: gl.getSupportedExtensions() };
  }
  if (navigator.gpu) {
    const a = await navigator.gpu.requestAdapter();
    if (a) out.webgpu = { maxTextureDimension2D: a.limits.maxTextureDimension2D,
      maxStorageBuffersPerShaderStage: a.limits.maxStorageBuffersPerShaderStage,
      maxColorAttachmentBytesPerSample: a.limits.maxColorAttachmentBytesPerSample,
      preferredCanvasFormat: navigator.gpu.getPreferredCanvasFormat() };
  }
  return out;
};
