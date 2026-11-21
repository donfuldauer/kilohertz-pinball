// S9 local static server for dist/ (the "local dev server" of the owner's decision).
// usage: node serve.cjs [port] [--coop] [--no-csp] [--root <dir>]
// Sends S4's recommended CSP on every response. Cross-Origin-Opener/Embedder-Policy are
// NOT sent by default: nothing in the engine needs cross-origin isolation (no threads,
// no SharedArrayBuffer); --coop adds them for comparison runs only.
'use strict';
const http = require('http');
const fs = require('fs');
const path = require('path');
const args = process.argv.slice(2);
const port = Number(args.find((a) => /^\d+$/.test(a)) || 8790);
const coop = args.includes('--coop');
const noCsp = args.includes('--no-csp');
const ri = args.indexOf('--root');
const root = path.resolve(ri >= 0 ? args[ri + 1] : __dirname);
const CSP = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; " +
  "object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";
const MIME = {
  '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8', '.css': 'text/css; charset=utf-8',
  '.wasm': 'application/wasm', '.json': 'application/json', '.png': 'image/png',
  '.pinbundle': 'application/octet-stream', '.zip': 'application/zip',
  '.scn': 'application/octet-stream', '.scr': 'application/octet-stream',
  '.ref': 'application/octet-stream', '.txt': 'text/plain; charset=utf-8',
};
http.createServer((req, res) => {
  const u = new URL(req.url, 'http://x');
  let p = decodeURIComponent(u.pathname);
  if (p.endsWith('/')) p += 'index.html';
  const f = path.resolve(root, '.' + p);
  if (!f.startsWith(root + path.sep)) { res.writeHead(403); res.end(); return; }
  fs.stat(f, (e, st) => {
    if (e || !st.isFile()) { res.writeHead(404); res.end('not found'); return; }
    const etag = `"${st.size.toString(16)}-${Math.floor(st.mtimeMs).toString(16)}"`;
    const h = {
      'Content-Type': MIME[path.extname(f)] || 'application/octet-stream',
      'ETag': etag,
      // HTML revalidates; everything else may come from the HTTP cache (warm loads).
      'Cache-Control': f.endsWith('.html') ? 'no-cache' : 'max-age=3600',
      'X-Content-Type-Options': 'nosniff',
    };
    if (!noCsp) h['Content-Security-Policy'] = CSP;
    if (coop) {
      h['Cross-Origin-Opener-Policy'] = 'same-origin';
      h['Cross-Origin-Embedder-Policy'] = 'require-corp';
    }
    if (req.headers['if-none-match'] === etag) { res.writeHead(304, h); res.end(); return; }
    h['Content-Length'] = st.size;
    res.writeHead(200, h);
    if (req.method === 'HEAD') { res.end(); return; }
    fs.createReadStream(f).pipe(res);
  });
}).listen(port, '127.0.0.1', () => {
  console.log(`serving ${root} on http://127.0.0.1:${port}/ csp=${!noCsp} coop_coep=${coop}`);
});
