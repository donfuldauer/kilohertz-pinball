"""Drop job lines whose result file already exists (resume a run_engines.sh job list).

usage: python3 -I skip_done.py <joblist> <res_dir>   -> remaining job lines on stdout
Mirrors the result-file naming of harness/job.sh: <section>__<engine>__<tier>__<item>.res
with '/' and ' ' in <item> replaced by '_'.
"""
import os
import sys


def name(line):
    w = line.split()
    env = {}
    while w and w[0] in ('env',) or (w and '=' in w[0] and not w[0].startswith('/')):
        if w[0] == 'env':
            w = w[1:]
            continue
        k, v = w[0].split('=', 1)
        env[k] = v
        w = w[1:]
    kind, a = w[1], w[2:]
    lsec = env.get('LUASEC', 'lua')
    if kind == 'sim':
        sec, e, t, item = f'sim_{a[0]}', a[1], a[2], a[3]
    elif kind == 'probe':
        sec, e, t, item = f'probe_{a[0]}', a[1], a[2], a[3]
    elif kind == 'micro':
        sec, e, t, item = 'micro', a[0], a[1], '115 ops x 6144'
    else:
        e, t = a[0], a[1]
        r = a[2:]
        sec = lsec if kind.startswith('lua') else 'rules'
        item = {
            'lua_tick': lambda: f'tick {r[0]} cap={r[1]}',
            'lua_probe': lambda: f'probe {r[0]}',
            'lua_sandbox': lambda: 'sandbox corpus',
            'lua_budget': lambda: 'budget-edge',
            'lua_sp': lambda: f'sp-after-errors {r[0]}',
            'lua_adv': lambda: f'advtick {r[0]} edges={r[1]} cap={r[2]}',
            'lua_dump': lambda: 'dump ' + (r[0] if r[0] != 'libm' else 'libm_' + r[1]),
            'rules_play': lambda: f'play {r[0]} {r[1]}',
            'rules_adv': lambda: f'adversarial {r[0]}',
            'rules_stack': lambda: f'stack {r[0]}',
            'rules_cap': lambda: f'tick-cap 200000 {r[0]}',
            'rules_s8adv': lambda: f'adv-tick {r[0]} edges={r[1]} cap={r[2]}',
        }[kind]()
    return f"{sec}__{e}__{t}__{item.replace('/', '_').replace(' ', '_')}.res"


res = sys.argv[2]
for line in open(sys.argv[1]):
    line = line.rstrip('\n')
    if line and not os.path.exists(os.path.join(res, name(line))):
        print(line)
