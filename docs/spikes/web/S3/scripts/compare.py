"""S3: pixel differences between two RGBA PNGs (same size).

Usage: python3 -I compare.py <a.png> <b.png> [diff_out.png]
Prints JSON: max abs difference (any RGB channel), mean abs difference,
pixel counts whose max channel difference exceeds 2/8/16/32, and the same
for the playfield region only (the display PiPs excluded).
"""
import json
import sys

import numpy as np
from PIL import Image


def load(p):
    return np.asarray(Image.open(p).convert("RGBA"), dtype=np.int16)


def stats(d):
    m = d.max(axis=2)
    n = m.size
    return {
        "max_abs": int(d.max()),
        "mean_abs": round(float(d.mean()), 4),
        "pixels": n,
        "px_gt_2": int((m > 2).sum()),
        "px_gt_8": int((m > 8).sum()),
        "px_gt_16": int((m > 16).sum()),
        "px_gt_32": int((m > 32).sum()),
        "pct_gt_8": round(100.0 * float((m > 8).sum()) / n, 3),
    }


def main():
    a, b = load(sys.argv[1]), load(sys.argv[2])
    if a.shape != b.shape:
        print(json.dumps({"error": f"size {a.shape} vs {b.shape}"}))
        return 1
    d = np.abs(a[:, :, :3] - b[:, :, :3])
    h, w = d.shape[:2]
    # probe_routes(): backglass PiP top-left (w*0.05.., h/40..), score PiP at
    # the bottom; the playfield region excludes both.
    bw = max(w * 3 // 10, 8); bh = max(bw * 3 // 4, 6)
    sw = max(w * 9 // 10, 8); sh = max(sw // 4, 4)
    mask = np.ones((h, w), dtype=bool)
    mask[h // 40: h // 40 + bh, w // 20: w // 20 + bw] = False
    y0 = h - sh - h // 40
    mask[y0: y0 + sh, (w - sw) // 2: (w - sw) // 2 + sw] = False
    out = {"a": sys.argv[1], "b": sys.argv[2], "all": stats(d), "playfield_only": stats(d[mask][:, None, :])}
    if len(sys.argv) > 3:
        amp = np.clip(d.max(axis=2) * 8, 0, 255).astype(np.uint8)
        Image.fromarray(amp, "L").save(sys.argv[3])
        out["diff_png"] = sys.argv[3]
    print(json.dumps(out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
