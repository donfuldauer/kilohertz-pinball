"""S10 req 3: smallest engine stack at which a probe still gives the native result.

usage: python3 -I stack_need.py <engine> <tier> <probe> [<probe> ...]
engine: v8 (Node --stack-size=KB), jsc (--maxPerThreadStackUsage=bytes; the thread stack is
        8 MiB), sm160/sm153 (no stack flag: the shell's quota is fixed, so the knob is
        `ulimit -s` KB; below the quota a deep recursion is a SIGSEGV of the shell).
probes:
  sandbox:<case>   S2 sandbox case through the v2 Lua module: line == native line (mem stripped)
  fixture          S1 +simd128 sim module, CI fixture 10 000 ticks: hashes == native
  rules:<design>   S4 stack probe (wasmi-instr-depth | jit-depth): depth-counter limits == native
Binary search over KB (step 8) between 16 and the engine default; prints one line per probe:
  <engine>/<tier> <probe> min_kb=<n> (fails at <n-8>: <failure>) default=<d>
"""
import os
import re
import subprocess
import sys

W = '${WEB_SPIKES}'
S1, S4, S8, S10 = f'{W}/S1', f'{W}/S4', f'{W}/S8', f'{W}/S10'
H, M, REF = f'{S10}/harness', f'{S10}/mods', f'{S10}/runs/ref'
TMP = f'{S10}/runs/stack/tmp'
os.makedirs(TMP, exist_ok=True)
DEFAULT = {'v8': 984, 'jsc': 5120, 'sm160': 8192, 'sm153': 8192}
EXP_RULES = None


def native_line(case):
    for line in open(f'{REF}/lua/native/sandbox.txt'):
        if line.startswith(case + ' |'):
            return re.sub(r' \| mem=[0-9]*$', '', line.rstrip('\n'))
    raise SystemExit(f'no native line for {case}')


def expected_rules():
    out = []
    for line in open(f'{S4}/out/stack_all.txt'):
        if line.startswith('native-instr-depth stack'):
            m = re.search(r'locals=(\d+) max_ok_depth=\[(\d+)', line)
            out.append((m.group(1), m.group(2)))
    return out[:5]


def run(engine, tier, kb, argv_node, argv_shell):
    env = dict(os.environ)
    env['S4_PKG'] = f'{M}/s4_pkg'
    pre = []
    if engine == 'v8':
        cmd = ['node', f'--stack-size={kb}'] + argv_node
    else:
        if engine == 'jsc':
            env['EXTRA'] = f'--maxPerThreadStackUsage={kb * 1024}'
        else:
            env['ULIMIT_S'] = str(kb)
        cmd = [f'{H}/eng.sh', engine, tier] + argv_shell
    try:
        p = subprocess.run(pre + cmd, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=600)
        return p.returncode, p.stdout.decode('utf-8', 'replace')
    except subprocess.TimeoutExpired:
        return -999, 'TIMEOUT'


def check(engine, tier, probe, kb):
    """(ok, failure-description)"""
    if probe.startswith('sandbox:'):
        case = probe.split(':', 1)[1]
        rc, out = run(engine, tier, kb, [f'{S8}/harness/run_node.cjs', f'{M}/{os.environ.get('LUAMOD', 's8_v2')}/pkg_node', 'sandbox', case],
                      [f'{H}/run_s8.js', f'{M}/{os.environ.get('LUAMOD', 's8_v2')}/pkg_node', 'sandbox', case])
        want = native_line(case)
        got = [re.sub(r' \| mem=[0-9]*$', '', l) for l in out.splitlines() if l.startswith(case + ' |')]
        if rc < 0 or rc >= 128:
            return False, f'signal/rc={rc}'
        if got and got[0] == want and 'TRAP' not in out:
            return True, ''
        t = re.search(r'TRAP: [^\n]*|RangeError[^\n]*|InternalError[^\n]*', out)
        return False, (t.group(0)[:120] if t else ('different line: ' + (got[0][:100] if got else out[:100])))
    if probe.startswith('lua:'):
        f = probe.split(':', 1)[1]
        lm = os.environ.get('LUAMOD', 's8_v2')
        want = subprocess.run([f'{M}/s8_v2/s2', 's8', 'probe', '@' + f, 'big', '1'], stdout=subprocess.PIPE).stdout.decode()
        rc, out = run(engine, tier, kb, [f'{S8}/node/run.cjs', f'{M}/{lm}/pkg_node', 'probe', '@' + f, 'big', '1'],
                      [f'{H}/run_s8.js', f'{M}/{lm}/pkg_node', 's8', 'probe', '@' + f, 'big', '1'])
        obs = lambda t: '\n'.join(l for l in t.splitlines() if not re.match(r'^(HEAP|mem|REAL) ', l))
        if rc < 0 or rc >= 128:
            return False, f'signal/rc={rc}'
        if obs(out) == obs(want) and 'TRAP' not in out:
            return True, ''
        t = re.search(r'TRAP: [^\n]*|RangeError[^\n]*|InternalError[^\n]*', out)
        return False, (t.group(0)[:120] if t else out[:120])
    if probe.startswith('tick:'):
        sc = probe.split(':', 1)[1]
        tb = sc.rsplit('_', 1)[0]
        lm = os.environ.get('LUAMOD', 's8_v2')
        a = [f'@{S1}/bundles/{tb}.pinbundle', f'@{S1}/rec/{sc}.scn', f'@{S1}/rec/{sc}.scr', f'@{S1}/native/{sc}.hashes', 'off']
        rc, out = run(engine, tier, kb, [f'{S8}/node/run.cjs', f'{M}/{lm}/pkg_node', 'tickstats'] + a,
                      [f'{H}/run_s8.js', f'{M}/{lm}/pkg_node', 's8', 'tickstats'] + a)
        g = lambda k: (re.search(r'\b' + k + r'=([^ ]+)', out) or [None, 'x'])[1]
        try:
            good = sum(int(g(k)) for k in ('queue_mm', 'digest_mm', 'draw_mm', 'hash_mm')) == 0 and g('faults') == '0' and int(g('hash_lines')) > 0
        except ValueError:
            good = False
        if rc < 0 or rc >= 128:
            return False, f'signal/rc={rc}'
        if good and 'TRAP' not in out:
            return True, ''
        t = re.search(r'TRAP: [^\n]*|RangeError[^\n]*|InternalError[^\n]*', out)
        return False, (t.group(0)[:120] if t else out[:120])
    if probe == 'fixture':
        h = f'{TMP}/{engine}_{tier}_fx.hashes'
        if os.path.exists(h):
            os.remove(h)
        args = [f'{M}/s1_simd', 'play', f'{S1}/bundles/replay_table.pinbundle', f'{S1}/rec/replay_table_fixture10k.scn',
                f'{S1}/rec/replay_table_fixture10k.scr', h]
        rc, out = run(engine, tier, kb, [f'{S1}/harness/run_bindgen.mjs'] + args, [f'{H}/run_s1.js'] + args)
        ok = os.path.exists(h) and open(h, 'rb').read() == open(f'{S1}/native/replay_table_fixture10k.hashes', 'rb').read()
        if ok and 'TRAP' not in out:
            return True, ''
        t = re.search(r'TRAP: [^\n]*|RangeError[^\n]*|InternalError[^\n]*', out)
        return False, (f'signal/rc={rc}' if rc < 0 or rc >= 128 else (t.group(0)[:120] if t else out[:120]))
    if probe.startswith('rules:'):
        design = probe.split(':', 1)[1]
        rc, out = run(engine, tier, kb, [f'{S4}/node/run_stack.mjs', design, '1'],
                      [f'{H}/run_s4.js', f'{M}/s4_pkg', f'{M}/s4_khz_rules.js', 'stack', design, '1'])
        if engine == 'v8':
            pass
        got = re.findall(r'stack locals=(\d+) max_ok_depth=\[(\d+)', out)
        if rc < 0 or rc >= 128:
            return False, f'signal/rc={rc}'
        if got == expected_rules():
            return True, ''
        return False, 'got ' + ' '.join(f'{a}:{b}' for a, b in got)[:120] + (' ' + out[:80] if not got else '')
    raise SystemExit(f'unknown probe {probe}')


def search(engine, tier, probe):
    hi = DEFAULT[engine]
    ok, why = check(engine, tier, probe, hi)
    if not ok:
        return f'{engine}/{tier} {probe} min_kb=>{hi} (fails at the default {hi}: {why})'
    lo = 8
    lo_why = 'not tried'
    ok16, why16 = check(engine, tier, probe, 16)
    if ok16:
        return f'{engine}/{tier} {probe} min_kb<=16 default={hi}'
    lo, lo_why = 16, why16
    while hi - lo > 8:
        mid = (lo + hi) // 2 // 8 * 8
        if mid <= lo:
            mid = lo + 8
        ok, why = check(engine, tier, probe, mid)
        if ok:
            hi = mid
        else:
            lo, lo_why = mid, why
    return f'{engine}/{tier} {probe} min_kb={hi} (fails at {lo}: {lo_why}) default={DEFAULT[engine]}'


if __name__ == '__main__':
    eng, tier = sys.argv[1], sys.argv[2]
    for pr in sys.argv[3:]:
        print(search(eng, tier, pr), flush=True)
