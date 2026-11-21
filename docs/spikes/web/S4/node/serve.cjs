// S4 static server. usage: node serve.cjs <port>
// Serves <S4 root>/ and S1's recordings under /s1rec/. Per request query:
//   iso=1      -> COOP: same-origin + COEP: require-corp (local dev server)
//   csp=strict -> Content-Security-Policy: script-src 'self'
//   csp=wasm   -> Content-Security-Policy: script-src 'self' 'wasm-unsafe-eval'
//   csp=unsafe -> Content-Security-Policy: script-src 'self' 'unsafe-eval'
//   csp=rec    -> the recommended full policy (see below)
// The headers go on every response of a page whose URL carries the query;
// subresources inherit the document's policy anyway.
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const root = path.resolve(__dirname, '..');
const s1rec = path.resolve(root, '..', 'S1', 'rec');
const port = Number(process.argv[2] || 8714);
const types = { '.html': 'text/html', '.js': 'text/javascript', '.mjs': 'text/javascript', '.wasm': 'application/wasm' };
http.createServer((req, res) => {
  const u = new URL(req.url, 'http://x');
  let p;
  if (u.pathname.startsWith('/s1rec/')) p = path.join(s1rec, decodeURIComponent(u.pathname.slice(7)));
  else p = path.join(root, decodeURIComponent(u.pathname));
  if (!(p.startsWith(root) || p.startsWith(s1rec))) { res.writeHead(403); res.end(); return; }
  fs.readFile(p, (err, data) => {
    if (err) { res.writeHead(404); res.end(); return; }
    const h = { 'Content-Type': types[path.extname(p)] || 'application/octet-stream', 'Cache-Control': 'no-store' };
    const ref = new URL(req.headers.referer || req.url, 'http://x').searchParams;
    const q = (k) => u.searchParams.get(k) ?? ref.get(k);
    if (q('iso') === '1') { h['Cross-Origin-Opener-Policy'] = 'same-origin'; h['Cross-Origin-Embedder-Policy'] = 'require-corp'; }
    const csp = q('csp');
    if (csp === 'strict') h['Content-Security-Policy'] = "script-src 'self'";
    if (csp === 'wasm') h['Content-Security-Policy'] = "script-src 'self' 'wasm-unsafe-eval'";
    if (csp === 'unsafe') h['Content-Security-Policy'] = "script-src 'self' 'unsafe-eval'";
    // The recommended local-dev-server policy (RESULTS §CSP).
    if (csp === 'rec') h['Content-Security-Policy'] = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";
    res.writeHead(200, h); res.end(data);
  });
}).listen(port, '127.0.0.1', () => console.log(`serving ${root} on ${port}`));
