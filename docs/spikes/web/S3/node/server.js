// Minimal static file server for the S3 render probe (local only).
// Sets COOP/COEP (cross-origin isolation) like the planned local dev server.
'use strict';
const http = require('http');
const fs = require('fs');
const path = require('path');
const TYPES = { '.html': 'text/html', '.js': 'text/javascript', '.mjs': 'text/javascript',
  '.wasm': 'application/wasm', '.json': 'application/json', '.zip': 'application/zip',
  '.png': 'image/png', '.bin': 'application/octet-stream', '.pinbundle': 'application/zip' };
function start(root, port) {
  const server = http.createServer((req, res) => {
    const url = new URL(req.url, 'http://localhost');
    let p = path.normalize(decodeURIComponent(url.pathname)).replace(/^([/\\])+/, '');
    const file = path.join(root, p);
    if (!file.startsWith(path.resolve(root))) { res.writeHead(403); res.end(); return; }
    fs.readFile(file, (err, data) => {
      if (err) { res.writeHead(404); res.end('not found'); return; }
      res.writeHead(200, {
        'Content-Type': TYPES[path.extname(file)] || 'application/octet-stream',
        'Cross-Origin-Opener-Policy': 'same-origin',
        'Cross-Origin-Embedder-Policy': 'require-corp',
        'Cache-Control': 'no-store',
      });
      res.end(data);
    });
  });
  return new Promise((resolve) => server.listen(port, '127.0.0.1', () => resolve(server)));
}
module.exports = { start };
if (require.main === module) {
  const root = path.resolve(process.argv[2] || '.');
  const port = Number(process.argv[3] || 8642);
  start(root, port).then(() => console.log(`serving ${root} on http://127.0.0.1:${port}`));
}
