// S10 shell environment: makes the JavaScriptCore (`jsc`) and SpiderMonkey (`js`)
// shells look enough like Node for the unchanged wasm-bindgen glue of S1/S7/S8
// (`--target nodejs`, CommonJS) and S4 (`--target web`, ES module) to run.
// Only the loader changes: the wasm bytes and the glue JS are the files the V8
// runs used. Feature-detected, no engine-specific behaviour beyond I/O and clocks.
//
// Provides: __shell ('jsc'|'sm'), __args, readBinary, readText, writeBytes,
// writeText, out(str), nowMs(), loadCjs(file), TextEncoder/TextDecoder (WHATWG
// UTF-8, only if absent), console.{log,error}, require('fs'|'path').
(function (g) {
  'use strict';
  const isSM = typeof g.os === 'object' && g.os && typeof g.os.file === 'object';
  const isJSC = !isSM && typeof g.preciseTime === 'function';
  g.__shell = isSM ? 'sm' : (isJSC ? 'jsc' : 'other');
  g.__args = isSM ? Array.from(g.scriptArgs) : Array.from(g.arguments || []);
  // `--out=<file>` as the first argument: out() writes its bytes to that file instead of stdout.
  // (SpiderMonkey's putstr/print stop at the first NUL character; Lua probe output can contain NULs.)
  g.__outFile = null;
  if (g.__args.length && String(g.__args[0]).startsWith('--out=')) g.__outFile = g.__args.shift().slice(6);

  g.readBinary = function (p) { return g.read(p, 'binary'); };
  g.readText = function (p) { return g.read(p); };

  // Clock: JSC's performance.now() is 20 µs-quantised in the shell; preciseTime()
  // is µs-resolution wall time (seconds). SpiderMonkey's performance.now() is fine.
  const baseClock = isJSC ? function () { return g.preciseTime() * 1000; } : (function () {
    const orig = g.performance.now.bind(g.performance); return function () { return orig(); };
  })();
  const t0 = baseClock();
  g.nowMs = function () { return baseClock() - t0; };
  try {
    if (!g.performance) g.performance = {};
    Object.defineProperty(g.performance, 'now', { value: g.nowMs, configurable: true, writable: true });
  } catch (e) { g.performance = { now: g.nowMs }; }

  // ---- UTF-8 (WHATWG encoding standard semantics; replacement on errors) ----
  function utf8Encode(s) {
    let n = 0;
    for (let i = 0; i < s.length; i++) {
      let c = s.charCodeAt(i);
      if (c < 0x80) n += 1; else if (c < 0x800) n += 2;
      else if (c >= 0xd800 && c <= 0xdbff && i + 1 < s.length && (s.charCodeAt(i + 1) & 0xfc00) === 0xdc00) { n += 4; i++; }
      else n += 3;
    }
    const out = new Uint8Array(n);
    let o = 0;
    for (let i = 0; i < s.length; i++) {
      let c = s.charCodeAt(i);
      if (c < 0x80) { out[o++] = c; continue; }
      if (c < 0x800) { out[o++] = 0xc0 | (c >> 6); out[o++] = 0x80 | (c & 63); continue; }
      if (c >= 0xd800 && c <= 0xdbff && i + 1 < s.length && (s.charCodeAt(i + 1) & 0xfc00) === 0xdc00) {
        const cp = 0x10000 + ((c - 0xd800) << 10) + (s.charCodeAt(++i) - 0xdc00);
        out[o++] = 0xf0 | (cp >> 18); out[o++] = 0x80 | ((cp >> 12) & 63); out[o++] = 0x80 | ((cp >> 6) & 63); out[o++] = 0x80 | (cp & 63);
        continue;
      }
      if (c >= 0xd800 && c <= 0xdfff) c = 0xfffd; // lone surrogate
      out[o++] = 0xe0 | (c >> 12); out[o++] = 0x80 | ((c >> 6) & 63); out[o++] = 0x80 | (c & 63);
    }
    return out;
  }
  function utf8Decode(b) {
    const parts = [];
    let chunk = [];
    const flush = function () { if (chunk.length) { parts.push(String.fromCharCode.apply(null, chunk)); chunk = []; } };
    let i = 0;
    const n = b.length;
    while (i < n) {
      // ASCII fast path in blocks
      let j = i;
      while (j < n && j - i < 8192 && b[j] < 0x80) j++;
      if (j > i) { flush(); parts.push(String.fromCharCode.apply(null, b.subarray(i, j))); i = j; continue; }
      const c = b[i];
      let need = 0, cp = 0, lo = 0x80, hi = 0xbf;
      if (c >= 0xc2 && c <= 0xdf) { need = 1; cp = c & 0x1f; }
      else if (c >= 0xe0 && c <= 0xef) { need = 2; cp = c & 0xf; if (c === 0xe0) lo = 0xa0; if (c === 0xed) hi = 0x9f; }
      else if (c >= 0xf0 && c <= 0xf4) { need = 3; cp = c & 0x7; if (c === 0xf0) lo = 0x90; if (c === 0xf4) hi = 0x8f; }
      else { chunk.push(0xfffd); i++; if (chunk.length > 4096) flush(); continue; }
      let k = 1, ok = true;
      for (; k <= need; k++) {
        if (i + k >= n) { ok = false; break; }
        const d = b[i + k];
        if (d < lo || d > hi) { ok = false; break; }
        lo = 0x80; hi = 0xbf;
        cp = (cp << 6) | (d & 0x3f);
      }
      if (!ok) { chunk.push(0xfffd); i += k; if (chunk.length > 4096) flush(); continue; }
      if (cp >= 0x10000) { cp -= 0x10000; chunk.push(0xd800 + (cp >> 10), 0xdc00 + (cp & 0x3ff)); } else chunk.push(cp);
      i += need + 1;
      if (chunk.length > 4096) flush();
    }
    flush();
    return parts.join('');
  }
  if (typeof g.TextEncoder !== 'function') {
    g.TextEncoder = function TextEncoder() {};
    g.TextEncoder.prototype.encode = function (s) { return utf8Encode(String(s === undefined ? '' : s)); };
    g.TextEncoder.prototype.encodeInto = function (s, view) {
      const b = utf8Encode(String(s));
      if (b.length <= view.length) { view.set(b); return { read: s.length, written: b.length }; }
      // partial: only whole code points that fit
      let read = 0, written = 0;
      for (let i = 0; i < s.length; i++) {
        let len = 1;
        const c = s.charCodeAt(i);
        if (c >= 0xd800 && c <= 0xdbff && i + 1 < s.length && (s.charCodeAt(i + 1) & 0xfc00) === 0xdc00) len = 2;
        const e = utf8Encode(s.substr(i, len));
        if (written + e.length > view.length) break;
        view.set(e, written); written += e.length; read += len; i += len - 1;
      }
      return { read: read, written: written };
    };
    g.__polyfilled_text = true;
  }
  if (typeof g.TextDecoder !== 'function') {
    g.TextDecoder = function TextDecoder() {};
    g.TextDecoder.prototype.decode = function (b) {
      if (b === undefined) return '';
      if (!(b instanceof Uint8Array)) b = new Uint8Array(b.buffer ? b.buffer : b, b.byteOffset || 0, b.byteLength);
      return utf8Decode(b);
    };
  }
  g.__utf8Encode = utf8Encode;
  g.__utf8Decode = utf8Decode;

  g.writeBytes = function (p, u8) {
    if (isSM) g.os.file.writeTypedArrayToFile(p, u8);
    else g.writeFile(p, u8);
  };
  g.writeText = function (p, s) { g.writeBytes(p, utf8Encode(s)); };
  // stdout without an added newline where possible
  const outParts = [];
  g.out = function (s) {
    if (g.__outFile) { outParts.push(s); g.writeBytes(g.__outFile, utf8Encode(outParts.join(''))); return; }
    if (typeof g.putstr === 'function') g.putstr(s);
    else if (s.endsWith('\n')) g.print(s.slice(0, -1));
    else g.print(s);
  };
  const errp = typeof g.printErr === 'function' ? g.printErr : g.print;
  g.console = {
    log: function () { g.print(Array.prototype.join.call(arguments, ' ')); },
    error: function () { errp(Array.prototype.join.call(arguments, ' ')); },
    warn: function () { errp(Array.prototype.join.call(arguments, ' ')); },
  };

  const dirname = function (p) { const i = p.lastIndexOf('/'); return i < 0 ? '.' : p.slice(0, i); };
  const pathMod = {
    resolve: function () { let r = ''; for (const a of arguments) r = a.startsWith('/') ? a : (r ? r + '/' + a : a); return r; },
    join: function () { return Array.prototype.join.call(arguments, '/').replace(/\/+/g, '/'); },
    dirname: dirname,
  };
  const fsMod = {
    readFileSync: function (p, enc) { return enc ? g.readText(p) : g.readBinary(p); },
    writeFileSync: function (p, d) { if (typeof d === 'string') g.writeText(p, d); else g.writeBytes(p, d); },
  };
  g.require = function (name) {
    const n = String(name).replace(/^node:/, '');
    if (n === 'fs') return fsMod;
    if (n === 'path') return pathMod;
    throw new Error('shellenv: require(' + name + ') not available');
  };
  // CommonJS loader for wasm-bindgen --target nodejs glue (unchanged file).
  g.loadCjs = function (file) {
    const src = g.readText(file);
    const module = { exports: {} };
    const f = new Function('exports', 'require', 'module', '__filename', '__dirname', src);
    f(module.exports, g.require, module, file, dirname(file));
    return module.exports;
  };
})(globalThis);
