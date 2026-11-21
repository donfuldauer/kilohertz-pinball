// S10: how far a Wasm memory (no declared maximum) grows in this engine: memory.grow in 256-page
// (16 MiB) steps from 1 page until it fails; prints the largest size reached.
(function () {
  const isNode = typeof process === 'object' && process.versions && process.versions.node;
  const say = isNode ? console.log : print;
  const m = new WebAssembly.Memory({ initial: 1 });
  let pages = 1;
  for (;;) { try { m.grow(256); pages += 256; } catch (e) { say(`grow failed at ${pages} pages (${(pages * 65536 / 2 ** 20).toFixed(0)} MiB): ${e.constructor.name}: ${e.message}`); break; } if (pages > 65536) break; }
  say(`max pages ${pages} = ${(pages * 65536 / 2 ** 30).toFixed(3)} GiB`);
})();
