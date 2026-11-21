"""S3: groups wasm_attrib.py's per-crate code bytes into the size table's buckets.
Usage: python3 -I attrib_groups.py <attrib.json (from wasm_attrib.py <wasm> 1000)>"""
import json
import sys

GROUPS = [
    ("egui text stack (epaint, vello_cpu/common, fearless_simd, skrifa, read_fonts, harfrust, kurbo, ...)",
     {"epaint", "vello_cpu", "vello_common", "fearless_simd", "skrifa", "read_fonts", "harfrust",
      "font_types", "kurbo", "peniko", "emath", "ecolor", "epaint_default_fonts", "smallvec", "bytemuck", "guillotiere", "euclid"}),
    ("egui + egui_wgpu", {"egui", "egui_wgpu", "type_map", "ahash", "nohash_hasher"}),
    ("wgpu (WebGPU front + web bindings)", {"wgpu", "wgpu_types", "js_sys", "web_sys", "wasm_bindgen", "wasm_bindgen_futures", "webgpu_sys"}),
    ("naga + wgpu_core + wgpu_hal (WebGL2 path)", {"naga", "wgpu_core", "wgpu_hal", "naga_types", "glow", "spirv", "codespan_reporting", "petgraph", "indexmap", "bit_set", "arrayvec", "parking_lot", "parking_lot_core", "lock_api", "unicode_ident", "half", "hexf_parse", "rustc_hash", "bitflags", "thiserror", "log"}),
    ("toml / serde / serde_json", {"toml", "toml_parser", "toml_datetime", "serde", "serde_core", "serde_json", "serde_spanned", "winnow", "ryu", "itoa", "toml_writer"}),
    ("assets: zip, inflate, glTF, image (png, jpeg)", {"zip", "flate2", "miniz_oxide", "zlib_rs", "gltf", "gltf_json", "image", "png", "zune_jpeg", "zune_core", "crc32fast", "fdeflate", "simd_adler32", "adler2", "base64", "byteorder_lite", "getrandom", "zune_inflate"}),
    ("pin_* + probe", {"pin_core", "pin_render", "pin_assets", "pin_physics", "pin_assetgen", "web_probe"}),
    ("physics deps (parry3d, glam, simba, ...)", {"parry3d", "glam", "glamx", "simba", "wide", "approx", "num_traits", "num_complex", "spade", "rstar", "ordered_float", "safe_arch", "paste", "robust", "static_assertions", "libm", "arrayvec_", "rand_pcg", "rand_core"}),
    ("core / alloc / std / hashbrown", {"core", "alloc", "std", "compiler_builtins", "hashbrown", "dlmalloc"}),
]


def main():
    d = json.load(open(sys.argv[1]))
    tot = d["code_bytes"]
    per = dict(d["per_crate"])
    used = set()
    print(f"file {d['file'].split('/')[-1]}: {d['bytes']} bytes total, code {tot}, data {d['data_bytes']}, custom {d['custom_sections']}")
    print("| Group | code KiB | % of code |")
    print("|---|---|---|")
    for label, crates in GROUPS:
        n = sum(v for k, v in per.items() if k in crates)
        used |= {k for k in per if k in crates}
        print(f"| {label} | {n/1024:.0f} | {100*n/tot:.1f} |")
    rest = sum(v for k, v in per.items() if k not in used)
    top_rest = sorted(((v, k) for k, v in per.items() if k not in used), reverse=True)[:8]
    print(f"| other ({', '.join(k for _, k in top_rest)}, ...) | {rest/1024:.0f} | {100*rest/tot:.1f} |")


if __name__ == "__main__":
    main()
