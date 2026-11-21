"""S9: groups wasm_attrib.py's per-crate code bytes (from the name section of the cargo output)
into the size table's buckets for the combined engine module. Unmangled symbols (shown by
wasm_attrib as <name>) are C: the vendored Lua 5.4.9 + KHZ patches and wasi-libc, or
wasm-bindgen's describe shims. LTO-less builds attribute inlined code to the caller's crate:
read the buckets as +/-10 %.
Usage: python3 -I s9_groups.py <attrib.json from wasm_attrib.py <wasm> 1000>
"""
import json
import re
import sys

GROUPS = [
    ("egui text stack (epaint, vello_cpu, fearless_simd, skrifa, read_fonts, harfrust, kurbo, ...)",
     {"epaint", "vello_cpu", "vello_common", "fearless_simd", "skrifa", "read_fonts", "harfrust", "font_types",
      "kurbo", "peniko", "emath", "ecolor", "epaint_default_fonts", "guillotiere", "euclid", "bytemuck", "smallvec"}),
    ("egui + egui_wgpu", {"egui", "egui_wgpu", "type_map", "ahash", "nohash_hasher", "accesskit", "unicode_segmentation"}),
    ("wgpu WebGPU front + web bindings", {"wgpu", "wgpu_types", "js_sys", "web_sys", "wasm_bindgen", "wasm_bindgen_futures", "webgpu_sys"}),
    ("WebGL2 path: naga + wgpu_core + wgpu_hal + glow", {"naga", "wgpu_core", "wgpu_hal", "glow", "spirv", "codespan_reporting",
      "petgraph", "indexmap", "bit_set", "bit_vec", "parking_lot", "parking_lot_core", "lock_api", "unicode_ident", "half",
      "hexf_parse", "rustc_hash", "bitflags", "thiserror", "log", "termcolor", "once_cell", "profiling"}),
    ("rules engine: wasmi + wasmparser (0.228 wasmi, 0.261 S4 pre-scan) + wasm_encoder (instrumentation)",
     {"wasmi", "wasmi_core", "wasmi_ir", "wasmi_collections", "wasmparser", "wasm_encoder", "string_interner", "spin", "leb128fmt"}),
    ("WAT text parser (wast/wat)", {"wast", "wat", "unicode_width", "bumpalo", "memchr"}),
    ("Lua: mlua (Rust side)", {"mlua", "mlua_sys", "lua_src"}),
    ("C: Lua 5.4.9 + KHZ 1-32 and wasi-libc (unmangled symbols)", set()),
    ("sim: pin_core, pin_physics, pin_sim, pin_input, pin_script + parry3d/glam/libm", {"pin_core", "pin_physics", "pin_sim",
      "pin_input", "pin_script", "parry3d", "glam", "glamx", "simba", "wide", "approx", "num_traits", "num_complex", "spade",
      "rstar", "ordered_float", "safe_arch", "robust", "libm", "arrayvec", "rand_pcg", "rand_core", "slab", "foldhash"}),
    ("renderer: pin_render + web_probe (S3 port) + pin_assetgen", {"pin_render", "web_probe", "pin_assetgen"}),
    ("assets: pin_assets, zip, inflate, glTF, image (png, jpeg)", {"pin_assets", "zip", "flate2", "miniz_oxide", "zlib_rs", "gltf",
      "gltf_json", "image", "png", "zune_jpeg", "zune_core", "crc32fast", "fdeflate", "simd_adler32", "adler2", "base64",
      "byteorder_lite", "getrandom", "zune_inflate", "zopfli", "typed_path", "indexmap_"}),
    ("toml / serde / serde_json", {"toml", "toml_parser", "toml_datetime", "serde", "serde_core", "serde_json", "serde_spanned",
      "winnow", "ryu", "itoa", "toml_writer", "zmij"}),
    ("spike harness + glue: pin_web, s1_harness, s2_harness, s4_rules, sha2", {"pin_web", "s1_harness", "s2_harness", "s4_rules",
      "sha2", "digest", "block_buffer", "generic_array", "cpufeatures", "typenum"}),
    ("core / alloc / std / hashbrown / dlmalloc (Rust)", {"core", "alloc", "std", "compiler_builtins", "hashbrown", "dlmalloc", "rustc_demangle"}),
]


def main():
    d = json.load(open(sys.argv[1]))
    tot = d["code_bytes"]
    per = dict(d["per_crate"])
    out = {g: 0 for g, _ in GROUPS}
    other = {}
    for name, n in per.items():
        if name.startswith("<"):
            if name.startswith("<__wbindgen") or name.startswith("<__wbg"):
                out["wgpu WebGPU front + web bindings"] += n
            else:
                out["C: Lua 5.4.9 + KHZ 1-32 and wasi-libc (unmangled symbols)"] += n
            continue
        for g, crates in GROUPS:
            if name in crates:
                out[g] += n
                break
        else:
            other[name] = n
    print(f"code bytes {tot} ({tot / 1024:.0f} KiB), data {d['data_bytes']} B, functions {d['functions']}\n")
    print("| group | code KiB | % of code |\n|---|---|---|")
    for g, _ in GROUPS:
        print(f"| {g} | {out[g] / 1024:.0f} | {100 * out[g] / tot:.1f} |")
    o = sum(other.values())
    top = ", ".join(f"{k} {v // 1024}" for k, v in sorted(other.items(), key=lambda x: -x[1])[:8])
    print(f"| other ({top}, ...) | {o / 1024:.0f} | {100 * o / tot:.1f} |")
    print("\nTop 15 crates / symbols (code KiB): " + ", ".join(f"{k} {v / 1024:.0f}" for k, v in d["per_crate"][:15]))


main()
