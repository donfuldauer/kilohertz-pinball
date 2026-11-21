// Static server for the S1 web page. usage: node serve.cjs <root> <port> <isolated 0|1>
// With isolated=1 it sends COOP: same-origin and COEP: require-corp (local dev
// server, the owner's "runs locally" setup), so the page is crossOriginIsolated.
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const [root, port, iso] = process.argv.slice(2);
const types = { '.html': 'text/html', '.js': 'text/javascript', '.wasm': 'application/wasm' };
http.createServer((req, res) => {
  const u = new URL(req.url, 'http://x');
  const p = path.join(root, decodeURIComponent(u.pathname === '/' ? '/index.html' : u.pathname));
  if (!p.startsWith(path.resolve(root))) { res.writeHead(403); res.end(); return; }
  fs.readFile(p, (err, data) => {
    if (err) { res.writeHead(404); res.end(); return; }
    const h = { 'Content-Type': types[path.extname(p)] || 'application/octet-stream' };
    if (iso === '1') { h['Cross-Origin-Opener-Policy'] = 'same-origin'; h['Cross-Origin-Embedder-Policy'] = 'require-corp'; }
    res.writeHead(200, h); res.end(data);
  });
}).listen(Number(port), '127.0.0.1', () => console.log(`serving ${root} on ${port} isolated=${iso}`));
