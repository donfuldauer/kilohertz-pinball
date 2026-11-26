//! Shared helpers of the KP-029 cabinet generator tests (`cabinet_*.rs`):
//! presets, `cabinet.toml` overrides, loading `cabinet.glb` through the
//! runtime loader and reading node bounds and extras. Included with
//! `#[path]`; an empty test crate on its own.
#![allow(dead_code)]

use pin_assets::{GltfScene, MeshData, Vfs, load_gltf};
use pin_core::fmath::{max_f32, min_f32};
use pin_core::schema::cabinet::{CabinetGeometry, CabinetManifest};
use pin_tablegen::cabinet::{CABINET_GLB_FILE, GeneratedCabinet, generate_cabinet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The three shipped presets.
pub const PRESETS: [&str; 3] = ["classic_standard", "classic_widebody", "modern_standard"];

/// Tolerance of the dimension checks, metres (1 mm).
pub const MM: f32 = 1e-3;

pub fn preset(id: &str) -> CabinetGeometry {
    CabinetGeometry::preset(id).unwrap()
}

/// A `cabinet.toml` on `classic_standard` with `extra` appended, resolved.
pub fn from_toml(extra: &str) -> CabinetGeometry {
    let text = format!("schema_version = 1\npreset = \"classic_standard\"\n{extra}");
    CabinetManifest::from_toml_str(&text)
        .unwrap_or_else(|e| panic!("{e}"))
        .resolve()
        .unwrap_or_else(|e| panic!("{e}"))
}

pub fn generate(g: &CabinetGeometry) -> GeneratedCabinet {
    generate_cabinet(g).unwrap_or_else(|e| panic!("{e}"))
}

/// Writes `glb` as `cabinet.glb` into a fresh folder and loads it with
/// `pin_assets::load_gltf` (default limits).
pub fn load(glb: &[u8]) -> GltfScene {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "pin_tablegen_cabinet_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(CABINET_GLB_FILE), glb).unwrap();
    let vfs = Vfs::open(&dir).unwrap();
    let scene = load_gltf(&vfs, CABINET_GLB_FILE).unwrap_or_else(|e| panic!("{e}"));
    let _ = std::fs::remove_dir_all(&dir);
    scene
}

/// The GLB's JSON chunk.
pub fn json(glb: &[u8]) -> String {
    let len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
    String::from_utf8(glb[20..20 + len].to_vec()).unwrap()
}

/// The JSON object of node `name` (from its `{"name":` to the matching
/// closing brace).
pub fn node_json(json: &str, name: &str) -> String {
    let nodes = &json[json.find("\"nodes\":[{").unwrap()..];
    let key = format!("{{\"name\":\"{name}\",\"mesh\"");
    let start = nodes.find(&key).unwrap_or_else(|| panic!("node {name}"));
    let mut depth = 0;
    for (i, c) in nodes[start..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return nodes[start..start + i + 1].to_owned();
                }
            }
            _ => {}
        }
    }
    panic!("unterminated node {name}")
}

pub fn mesh<'a>(scene: &'a GltfScene, name: &str) -> &'a MeshData {
    scene
        .meshes
        .iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("no node {name}"))
}

/// World-space positions of a node (its transform applied).
pub fn world(m: &MeshData) -> Vec<[f32; 3]> {
    let t = &m.transform;
    m.positions
        .iter()
        .map(|p| {
            let mut o = [0.0f32; 3];
            for (r, v) in o.iter_mut().enumerate() {
                *v = t[0][r] * p[0] + t[1][r] * p[1] + t[2][r] * p[2] + t[3][r];
            }
            o
        })
        .collect()
}

/// World-space bounds of the named nodes together.
pub fn bounds(scene: &GltfScene, names: &[&str]) -> ([f32; 3], [f32; 3]) {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for n in names {
        for p in world(mesh(scene, n)) {
            for k in 0..3 {
                lo[k] = min_f32(lo[k], p[k]);
                hi[k] = max_f32(hi[k], p[k]);
            }
        }
    }
    (lo, hi)
}

/// World-space bounds of every node.
pub fn scene_bounds(scene: &GltfScene) -> ([f32; 3], [f32; 3]) {
    let names: Vec<&str> = scene.meshes.iter().map(|m| m.name.as_str()).collect();
    bounds(scene, &names)
}

pub fn close(a: f32, b: f64, tol: f32, what: &str) {
    assert!(
        (f64::from(a) - b).abs() <= f64::from(tol),
        "{what}: {a} vs {b} (tolerance {tol})"
    );
}

/// FNV-1a 64-bit.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// `"#rrggbb"` to linear RGB, computed independently of the generator.
pub fn linear(colour: &str) -> [f32; 3] {
    let c = |i: usize| {
        let v = f64::from(u8::from_str_radix(&colour[i..i + 2], 16).unwrap()) / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            libm::pow((v + 0.055) / 1.055, 2.4)
        }
    };
    [c(1) as f32, c(3) as f32, c(5) as f32]
}

/// The leg node names of `leg`.
pub fn leg_nodes(leg: &str) -> [String; 3] {
    [
        format!("cabinet.leg.{leg}"),
        format!("cabinet.leg.{leg}.bracket"),
        format!("cabinet.leg.{leg}.leveler"),
    ]
}
