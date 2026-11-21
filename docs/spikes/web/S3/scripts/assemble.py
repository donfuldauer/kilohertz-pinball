"""S3: assembles RESULTS.md from body_part*.md and the generated tables.
Usage: python3 -I assemble.py <S>"""
import json
import subprocess
import sys

S = sys.argv[1]


def run(*a):
    return subprocess.run(["python3", "-I", f"{S}/scripts/tables.py", S, *a], capture_output=True, text=True).stdout.strip()


def read(p):
    return open(f"{S}/{p}").read()


parts = ["body_part1.md", "body_part2.md", "body_part3.md", "body_part4.md", "body_part5.md",
         "body_part6.md", "body_part7.md", "body_part8.md", "body_part9.md", "body_part10.md",
         "body_part11.md"]
body = "\n".join(read(p) for p in parts)
subs = json.load(open(f"{S}/subs.json"))
# generated tables
log = read("logs/run_both_minimal.log")
status = "\n".join("- `" + l.strip()[:400] + "`" for l in log.splitlines() if l.startswith("[render/"))
subs["RENDER_STATUS"] = status
subs["COMPARE_TABLE"] = "WebGPU (Chromium/SwiftShader) vs native (wgpu/Vulkan/lavapipe):\n\n" + run("compare", f"{S}/results/compare_webgpu_vs_native.jsonl")
subs["WEBGL2_COMPARE"] = ("WebGL2 (prototype) vs native lavapipe:\n\n" + run("compare", f"{S}/results/compare_webgl2_vs_native.jsonl")
                          + "\n\nWebGL2 (prototype, no clusters) vs WebGPU (clustered), same browser:\n\n"
                          + run("compare", f"{S}/results/compare_webgl2_vs_webgpu.jsonl"))
subs["BENCH"] = (run("bench", f"{S}/results/bench_webgpu_both_minimal.json") + "\n\n"
                 + run("bench", f"{S}/results/bench_webgl2_both_minimal.json"))
subs["WEBGL2_EFFORT"] = read("effort_webgl2.md")
try:
    subs["SIZE_TABLE"] = run("sizes", f"{S}/logs/size_opt.log")
except Exception as e:  # noqa
    pass
for k, v in subs.items():
    body = body.replace(f"<<{k}>>", v)
head = read("RESULTS.head.md")
out = head.replace("<<BODY>>", body)
open(f"{S}/RESULTS.md", "w").write(out)
left = sorted(set(x.split(">>")[0] for x in out.split("<<")[1:]))
print("unfilled:", left)
