//! Property and robustness tests for the cabinet generator (issue #110, KP-029):
//! REQ-CAB-001, REQ-CAB-003, REQ-CAB-007 and R11 (a `cabinet.toml` is untrusted input).
#![allow(dead_code)]

use pin_assets::{GltfScene, MeshData, Vfs, load_gltf};
use pin_core::schema::cabinet::{CabinetGeometry, CabinetManifest};
use pin_tablegen::cabinet::{CABINET_GLB_FILE, GeneratedCabinet, NODE_NAMES, generate_cabinet};
use proptest::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};

const PRESETS: [&str; 3] = ["classic_standard", "classic_widebody", "modern_standard"];
const MM: f64 = 0.001;

fn load(cab: &GeneratedCabinet) -> GltfScene {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "spec_cabinet_props_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(CABINET_GLB_FILE), &cab.glb).unwrap();
    let vfs = Vfs::open(&dir).unwrap();
    let scene = load_gltf(&vfs, CABINET_GLB_FILE).unwrap_or_else(|e| panic!("{e:?}"));
    let _ = std::fs::remove_dir_all(&dir);
    scene
}

fn placed_z(m: &MeshData) -> Vec<f64> {
    let t = &m.transform;
    m.positions
        .iter()
        .map(|p| {
            f64::from(t[0][2]) * f64::from(p[0])
                + f64::from(t[1][2]) * f64::from(p[1])
                + f64::from(t[2][2]) * f64::from(p[2])
                + f64::from(t[3][2])
        })
        .collect()
}

fn z_range(m: &MeshData) -> (f64, f64) {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for z in placed_z(m) {
        if z < lo {
            lo = z;
        }
        if z > hi {
            hi = z;
        }
    }
    (lo, hi)
}

/// Builds the override text. The glass panel and backbox position are scaled with the body so
/// that pin_core's cross-field checks accept the whole file. `ext` are fractions in [0, 1] of each leg's own
/// [extension_min, extension_max] for the preset.
fn toml_for(preset: usize, width: u32, length: u32, leg_len: u32, ext: [f32; 4]) -> String {
    let id = PRESETS[preset];
    let base = CabinetGeometry::preset(id).unwrap();
    let mut s = format!(
        "schema_version = 1\npreset = \"{id}\"\n[body]\nwidth = {width}.0\nlength = {length}.0\n\
         [glass_panel]\nwidth = {}.0\nlength = {}.0\n[backbox]\nfront_y = {}.0\n\
         [legs.default]\nlength = {leg_len}.0\n",
        width - 30,
        length - 250,
        length - 100
    );
    let legs = base.legs.all();
    for (i, (name, leg)) in legs.iter().enumerate() {
        let l = &leg.leveler;
        let e = l.extension_min + (l.extension_max - l.extension_min) * ext[i];
        s += &format!("[legs.{name}.leveler]\nextension = {e:?}\n");
    }
    s
}

fn resolve(text: &str) -> Option<CabinetGeometry> {
    CabinetManifest::from_toml_str(text).ok()?.resolve().ok()
}

fn check_output(g: &CabinetGeometry) -> Result<(), TestCaseError> {
    let a = generate_cabinet(g).map_err(|e| TestCaseError::fail(format!("{e:?}")))?;
    let b = generate_cabinet(g).map_err(|e| TestCaseError::fail(format!("{e:?}")))?;
    prop_assert_eq!(&a.glb, &b.glb);
    let names: Vec<&str> = a.nodes.iter().map(String::as_str).collect();
    prop_assert_eq!(names, NODE_NAMES.to_vec());
    let s = load(&a);
    let mesh_names: Vec<&str> = s.meshes.iter().map(|m| m.name.as_str()).collect();
    prop_assert_eq!(mesh_names, NODE_NAMES.to_vec());
    for m in &s.meshes {
        prop_assert!(m.positions.iter().flatten().all(|v| v.is_finite()));
        prop_assert!(m.normals.iter().flatten().all(|v| v.is_finite()));
        prop_assert!(m.uvs.iter().flatten().all(|v| v.is_finite()));
        prop_assert!(m.transform.iter().flatten().all(|v| v.is_finite()));
    }
    for (leg, l) in g.legs.all() {
        let lev = s
            .meshes
            .iter()
            .find(|m| m.name == format!("cabinet.leg.{leg}.leveler"))
            .unwrap();
        let (lo, _) = z_range(lev);
        prop_assert!(lo.abs() <= MM, "{leg} leveler min z {lo}");
        let sh = s
            .meshes
            .iter()
            .find(|m| m.name == format!("cabinet.leg.{leg}"))
            .unwrap();
        let (lo, hi) = z_range(sh);
        prop_assert!(
            ((hi - lo) - f64::from(l.length) * MM).abs() <= MM,
            "{leg} shaft extent {} vs {}",
            hi - lo,
            l.length
        );
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    /// REQ-CAB-001 / REQ-CAB-003 / REQ-CAB-007: over valid overrides the output is
    /// deterministic, loads, is finite, has the fixed node list, legs touch the floor and
    /// shafts equal their leg length.
    #[test]
    fn req_cab_003_generated_cabinet_invariants(
        preset in 0usize..3,
        width in 450u32..=700,
        length in 1000u32..=1500,
        leg_len in 600u32..=800,
        ext in proptest::array::uniform4(0.0f32..=1.0),
    ) {
        let text = toml_for(preset, width, length, leg_len, ext);
        let g = CabinetManifest::from_toml_str(&text)
            .map_err(|e| TestCaseError::fail(format!("authoring error {e:?}\n{text}")))?
            .resolve();
        // A combination pin_core rejects is a different unit's error, not ours: skip it.
        let Ok(g) = g else { return Ok(()); };
        check_output(&g)?;
    }
}

/// REQ-CAB-003: the property above is not vacuous: its corner values resolve and pass.
#[test]
fn req_cab_003_property_inputs_are_not_all_rejected() {
    let mut accepted = 0;
    for preset in 0..3 {
        for (w, l, ll) in [(560, 1315, 724), (600, 1200, 700), (520, 1100, 650)] {
            if let Some(g) = resolve(&toml_for(preset, w, l, ll, [0.0, 0.5, 1.0, 0.25])) {
                check_output(&g).unwrap();
                accepted += 1;
            }
        }
    }
    assert!(
        accepted >= 3,
        "only {accepted} of the sample overrides resolved"
    );
}

/// Small fixed-seed PCG-style generator (no OS entropy, R3).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let x = self.0;
        ((x >> 18) ^ x).rotate_right(((x >> 59) & 63) as u32)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn feed(text: &str) {
    if let Ok(m) = CabinetManifest::from_toml_str(text)
        && let Ok(g) = m.resolve()
    {
        let _ = generate_cabinet(&g);
    }
}

/// R11: 256 random byte strings up to 2 KiB through the parser; anything that resolves goes to
/// the generator, which must not panic.
#[test]
fn r11_random_bytes_never_panic() {
    let mut rng = Rng(0x5eed_cab1_0000_0110);
    for _ in 0..256 {
        let n = rng.below(2049);
        let bytes: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
        feed(&String::from_utf8_lossy(&bytes));
    }
}

/// R11: byte-level and number-level mutations of a valid manifest (so that some parse and
/// resolve) never panic the generator.
#[test]
fn r11_mutated_manifests_never_panic() {
    let seed = "schema_version = 1\npreset = \"classic_standard\"\n[body]\nwidth = 560.0\n\
        length = 1315.0\nfront_height = 405.0\n[legs.default]\nfinish = \"powder_coat\"\n\
        colour = \"#202020\"\n[legs.rear_left.leveler]\nextension = 12.0\n[legs.rear_right]\n\
        length = 736.0\n[controls]\nplunger = [230.0, 0.0, 235.0]\n";
    let numbers = [
        "0.0", "-1.0", "1e30", "1e-30", "nan", "inf", "-inf", "100000.0", "0.001", "3",
    ];
    let mut rng = Rng(0xc0ff_ee00_0000_0110);
    let mut resolved = 0;
    for _ in 0..400 {
        let mut text = seed.to_owned();
        for _ in 0..1 + rng.below(3) {
            // Replace one numeric literal with a hostile one.
            let spots: Vec<usize> = text.match_indices(" = ").map(|(i, _)| i + 3).collect();
            let at = spots[rng.below(spots.len())];
            if text[at..].starts_with(|c: char| c.is_ascii_digit() || c == '-') {
                let end = text[at..]
                    .find(['\n', ',', ']'])
                    .map_or(text.len(), |e| at + e);
                text.replace_range(at..end, numbers[rng.below(numbers.len())]);
            }
        }
        if let Some(g) = resolve(&text) {
            resolved += 1;
            let _ = generate_cabinet(&g);
        }
    }
    assert!(
        resolved > 0,
        "no mutated manifest resolved; the run exercised nothing"
    );
    // And raw byte flips of the valid text.
    for _ in 0..256 {
        let mut bytes = seed.as_bytes().to_vec();
        for _ in 0..1 + rng.below(6) {
            let i = rng.below(bytes.len());
            bytes[i] = rng.next() as u8;
        }
        feed(&String::from_utf8_lossy(&bytes));
    }
}
