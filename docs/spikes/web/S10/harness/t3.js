const pkg = __args[0];
load(pkg + '/../s4_khz_rules.js');
import(pkg + '/s4_rules.js').then((mod) => {
  const raw = mod.initSync({ module: readBinary(pkg + '/s4_rules_bg.wasm') });
  globalThis.khzRules.sim = raw;
  print('ok exports', Object.keys(mod).length, typeof raw.memory);
}).catch((e) => print('ERR', e, e && e.stack));
