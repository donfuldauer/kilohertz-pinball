// S4 page: the engine wasm with the rules designs; window.s4.run(cmd, args).
import init, * as m from '/pkg/s4_rules.js';
const raw = await init({ module_or_path: '/pkg/s4_rules_bg.wasm' });
khzRules.sim = raw;
m.install_panic_hook();
const bytes = async (u) => new Uint8Array(await (await fetch(u)).arrayBuffer());
const rulesOf = (bundle) => bundle.replace(/_wasm\.pinbundle$/, '/rules.wasm');
async function pre(design, bundle) {
  if (!design.startsWith('jit') && design !== 'hybrid') return;
  if (khzRules.mode !== 'cache') return;
  const b = m.instrument_jit(await bytes(rulesOf(bundle)), design !== 'jit');
  await khzRules.precompile(b);
}
window.s4 = {
  async run(cmd, a) {
    if (a.mode) khzRules.mode = a.mode;
    const env = `crossOriginIsolated=${self.crossOriginIsolated} mode=${khzRules.mode}`;
    switch (cmd) {
      case 'play': {
        await pre(a.design, a.bundle);
        const out = m.play(a.design, await bytes(a.bundle), await bytes(a.scn), a.every || 1);
        return out;
      }
      case 'bench': {
        await pre(a.design, a.bundle);
        return m.bench(a.design, await bytes(a.bundle), await bytes(a.scn)) + '\n' + env;
      }
      case 'alloc': return m.alloc_check(a.design, await bytes(a.bundle), await bytes(a.scn));
      case 'adv': return m.adversarial(a.design, await bytes(a.prod));
      case 'case': return m.adv_case(a.design, a.name, await bytes(a.prod));
      case 'worst': return m.worst(a.design, a.reps || 5, a.only || '') + env + '\n';
      case 'loadtime': {
        if (khzRules.mode === 'cache' && a.design.startsWith('jit')) await khzRules.precompile(m.instrument_jit(await bytes(a.rules), a.design !== 'jit'));
        return m.load_time(a.design, await bytes(a.rules), a.reps || 20) + ' ' + JSON.stringify(khzRules.stats) + ' ' + env;
      }
      case 'stack': return m.stack_probe(a.design, a.reps || 3) + env + '\n';
      default: throw new Error('unknown ' + cmd);
    }
  },
};
window.s4ready = true;
