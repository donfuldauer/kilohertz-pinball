// Prints raw / gzip -9 / brotli -11 (lgwin 24) sizes of each file argument as JSON lines.
'use strict';
const fs = require('fs');
const zlib = require('zlib');
for (const f of process.argv.slice(2)) {
  const b = fs.readFileSync(f);
  const gz = zlib.gzipSync(b, { level: 9 }).length;
  const br = zlib.brotliCompressSync(b, { params: {
    [zlib.constants.BROTLI_PARAM_QUALITY]: 11,
    [zlib.constants.BROTLI_PARAM_LGWIN]: 24,
    [zlib.constants.BROTLI_PARAM_SIZE_HINT]: b.length } }).length;
  console.log(JSON.stringify({ file: f, raw: b.length, gzip9: gz, brotli11: br }));
}
