"""Run a command, write its stdout to a file, print 'wall_s maxrss_kb rc' to stderr-free stdout.

usage: python3 -I rss.py <stdout_file> <cmd> [args...]
maxrss is the peak resident set of the child process tree (RUSAGE_CHILDREN; for
qemu-aarch64 runs it includes the emulator).
"""
import resource
import subprocess
import sys
import time

out = sys.argv[1]
t0 = time.monotonic()
with open(out, 'wb') as f:
    p = subprocess.run(sys.argv[2:], stdout=f, stderr=subprocess.STDOUT)
wall = time.monotonic() - t0
rss = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
print(f'{wall:.2f} {rss} {p.returncode}')
