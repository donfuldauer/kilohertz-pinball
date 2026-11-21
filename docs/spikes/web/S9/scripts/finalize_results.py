"""S9: refreshes RESULTS.md's Appendix A (integration table + generated tables) and the shipped
module's sha256 line after a run_integration.sh run. usage: finalize_results.py <S9 root>"""
import hashlib, os, re, subprocess, sys
S = sys.argv[1]
p = os.path.join(S, "RESULTS.md"); s = open(p).read()
sha = hashlib.sha256(open(os.path.join(S, "dist/pkg/pin_web_bg.wasm"), "rb").read()).hexdigest()
s = re.sub(r"with the same `pin_web_bg.wasm` sha256\n`[0-9a-f]{16}…`", f"with the same `pin_web_bg.wasm` sha256\n`{sha[:16]}…`", s)
i = s.index("## Appendix A")
log = open(os.path.join(S, "logs/run_integration_final.time")).read().split("\n")[0]
end = open(os.path.join(S, "logs/run_integration_final.log")).read().strip().split("\n")[-1]
tables = subprocess.run([sys.executable, "-I", os.path.join(S, "scripts/summarize.py"), S], capture_output=True, text=True).stdout
s = s[:i] + f"## Appendix A — final `run_integration.sh` (started {log}, finished {end}; `logs/run_integration_final.log`)\n\nShipped module sha256 `{sha}`.\n\n" + open(os.path.join(S, "results/integration_table.md")).read() + "\n" + tables + "\n"
open(p, "w").write(s)
print(sha)
