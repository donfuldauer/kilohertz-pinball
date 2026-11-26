//! Property tests for the cabinet generator's backbox (issue #111, KP-030): REQ-CAB-003 (the
//! backbox is data) and R11 (`cabinet.toml` is untrusted input), against the #111 backbox
//! model contract sections C (bounds set) and G (errors and robustness).
//!
//! Proptest runs with its default deterministic-per-run configuration (R3).
#![allow(dead_code)]

use pin_assets::{GltfScene, Vfs, load_gltf};
use pin_core::fmath::{max_f64, min_f64};
use pin_core::schema::cabinet::{
    BackboxFrontProfile, BackboxTopProfile, CabinetGeometry, Grille, ScoreDisplayKind,
};
use pin_tablegen::cabinet::{
    CABINET_GLB_FILE, GeneratedCabinet, MATERIAL_NAMES, MAX_DIMENSION_MM, NODE_NAMES,
    generate_cabinet,
};
use proptest::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};

const PRESETS: [&str; 3] = ["classic_standard", "classic_widebody", "modern_standard"];
const TOL: f64 = 1.0;
const BOUNDS_SET: [&str; 3] = [
    "cabinet.backbox.shell",
    "cabinet.backbox.speaker_panel",
    "cabinet.backbox.translite_frame",
];

/// Contract G keys (plus the blade keys of contract E) a backbox-only input may name.
const G_KEYS: [&str; 22] = [
    "backbox.width",
    "backbox.height",
    "backbox.depth",
    "backbox.front_y",
    "backbox.bottom_z",
    "backbox.speaker_panel_height",
    "backbox.backglass.width",
    "backbox.backglass.height",
    "backbox.backglass.thickness",
    "backbox.translite_opening.width",
    "backbox.translite_opening.height",
    "backbox.translite_opening.bottom",
    "backbox.score_display.width",
    "backbox.score_display.height",
    "backbox.score_display.centre_v",
    "backbox.score_display.pixels",
    "backbox.grilles",
    "backbox.front_profile",
    "backbox.top_profile",
    "mount.inset",
    "mount.height",
    "mount.pitch_deg",
];

fn load(cab: &GeneratedCabinet) -> GltfScene {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "spec_cabinet_backbox_props_{}_{}",
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

fn preset(i: usize) -> CabinetGeometry {
    CabinetGeometry::preset(PRESETS[i]).unwrap()
}

/// `element` is a contract G key, or `backbox.grilles[i].{u,v,diameter}` with i in range.
fn allowed_key(element: &str, n_grilles: usize) -> bool {
    if G_KEYS.contains(&element) {
        return true;
    }
    let Some(rest) = element.strip_prefix("backbox.grilles[") else {
        return false;
    };
    let Some((idx, field)) = rest.split_once("].") else {
        return false;
    };
    idx.parse::<usize>().is_ok_and(|i| i < n_grilles) && ["u", "v", "diameter"].contains(&field)
}

/// Ok output: stable node and material lists, finite floats, UVs in [0, 1].
fn check_ok_output(cab: &GeneratedCabinet) -> Result<GltfScene, TestCaseError> {
    prop_assert_eq!(&cab.nodes, &NODE_NAMES.to_vec());
    let s = load(cab);
    let mats: Vec<&str> = s.materials.iter().map(|m| m.name.as_str()).collect();
    prop_assert_eq!(mats, MATERIAL_NAMES.to_vec());
    for m in &s.meshes {
        prop_assert!(NODE_NAMES.contains(&m.name.as_str()), "mesh {}", m.name);
        prop_assert!(
            m.positions.iter().flatten().all(|v| v.is_finite()),
            "{}",
            m.name
        );
        prop_assert!(
            m.normals.iter().flatten().all(|v| v.is_finite()),
            "{}",
            m.name
        );
        prop_assert!(
            m.transform.iter().flatten().all(|v| v.is_finite()),
            "{}",
            m.name
        );
        prop_assert!(
            m.uvs
                .iter()
                .flatten()
                .all(|v| v.is_finite() && (0.0..=1.0).contains(v)),
            "{}: UV outside [0, 1]",
            m.name
        );
        prop_assert!(m.indices.iter().all(|&i| (i as usize) < m.positions.len()));
    }
    Ok(s)
}

/// A finite value: ordinary, tiny, zero, negative, at and beyond MAX_DIMENSION_MM.
fn any_finite() -> impl Strategy<Value = f32> {
    let max = MAX_DIMENSION_MM;
    prop_oneof![
        3 => 0.0f32..2000.0,
        1 => -2000.0f32..0.0,
        1 => -1e-3f32..1e-3,
        1 => Just(0.0f32),
        1 => Just(max),
        1 => max..max * 4.0,
        1 => -max * 4.0..-max,
        1 => Just(f32::MAX),
        1 => Just(f32::MIN),
        1 => Just(f32::MIN_POSITIVE),
    ]
}

/// Sets field `k` of the backbox (16 numeric fields) to `v`.
fn set_field(g: &mut CabinetGeometry, k: usize, v: f32) {
    let bb = &mut g.backbox;
    match k {
        0 => bb.width = v,
        1 => bb.height = v,
        2 => bb.depth = v,
        3 => bb.front_y = v,
        4 => bb.bottom_z = v,
        5 => bb.speaker_panel_height = v,
        6 => bb.backglass.width = v,
        7 => bb.backglass.height = v,
        8 => bb.backglass.thickness = v,
        9 => bb.translite_opening.width = v,
        10 => bb.translite_opening.height = v,
        11 => bb.translite_opening.bottom = v,
        12 => bb.score_display.width = v,
        13 => bb.score_display.height = v,
        14 => bb.score_display.centre_v = v,
        _ => {
            if let Some(gr) = bb.grilles.first_mut() {
                match k % 3 {
                    0 => gr.u = v,
                    1 => gr.v = v,
                    _ => gr.diameter = v,
                }
            }
        }
    }
}

fn front(i: u8) -> BackboxFrontProfile {
    if i.is_multiple_of(2) {
        BackboxFrontProfile::Flat
    } else {
        BackboxFrontProfile::Angled
    }
}

fn top(i: u8) -> BackboxTopProfile {
    match i % 3 {
        0 => BackboxTopProfile::Flat,
        1 => BackboxTopProfile::Angled,
        _ => BackboxTopProfile::Arched,
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// R11 / REQ-CAB-003 (contract G): random but finite `[backbox]` values, including extremes
    /// within and beyond MAX_DIMENSION_MM, random profiles, pixels and grille counts: never a
    /// panic; Ok implies the stable lists, finite floats and UVs in [0, 1]; Err implies a
    /// non-empty `element` that is one of contract G's keys.
    #[test]
    fn r11_random_finite_backbox_never_panics(
        p in 0usize..3,
        edits in proptest::collection::vec((0usize..18, any_finite()), 1..5),
        fp in 0u8..2,
        tp in 0u8..3,
        px in (0u32..5000, 0u32..5000),
        n_grilles in 0usize..11,
        grille in (any_finite(), any_finite(), any_finite()),
        lcd in any::<bool>(),
    ) {
        let mut g = preset(p);
        g.backbox.front_profile = front(fp);
        g.backbox.top_profile = top(tp);
        g.backbox.score_display.pixels = [px.0, px.1];
        if lcd {
            g.backbox.score_display.kind = ScoreDisplayKind::Lcd;
        }
        if n_grilles != g.backbox.grilles.len() {
            g.backbox.grilles = (0..n_grilles)
                .map(|_| Grille { u: grille.0, v: grille.1, diameter: grille.2 })
                .collect();
        }
        for (k, v) in &edits {
            set_field(&mut g, *k, *v);
        }
        let n = g.backbox.grilles.len();
        match generate_cabinet(&g) {
            Ok(cab) => {
                check_ok_output(&cab)?;
            }
            Err(e) => {
                prop_assert!(!e.element.is_empty(), "empty element: {}", e.message);
                prop_assert!(
                    allowed_key(&e.element, n),
                    "element {:?} is not a contract G key ({})",
                    e.element,
                    e.message
                );
            }
        }
    }

    /// REQ-CAB-003 (contract A to D, G): a random geometry built so every pin_core containment
    /// rule and every generator rule holds always generates, and the bounds set spans
    /// x +-W/2, y [Y0, Y0 + D], z [zb, zb + H] within 1 mm.
    #[test]
    fn req_cab_003_consistent_random_backbox_generates(
        p in 0usize..3,
        w in 500.0f64..900.0,
        h in 500.0f64..900.0,
        d in 150.0f64..400.0,
        fy in 0.0f64..1.0,
        bz in 0.0f64..200.0,
        sph_f in 0.2f64..0.35,
        fr in (0.0f64..1.0, 0.0f64..1.0, 0.0f64..1.0, 0.0f64..1.0),
        disp in (0.2f64..0.9, 0.2f64..0.8, 0.0f64..1.0),
        n_grilles in 1usize..5,
        px in (1u32..4096, 1u32..4096),
        fp in 0u8..2,
        tp in 0u8..3,
    ) {
        let mut g = preset(p);
        let t = f64::from(g.body.wall_thickness);
        let tan10 = libm::tan(10.0f64.to_radians());
        let front_p = front(fp);
        let top_p = top(tp);
        let d = if front_p == BackboxFrontProfile::Angled {
            max_f64(d, h * tan10 + 2.0 * t + 5.0)
        } else {
            d
        };
        let sph = h * sph_f;
        // Translite opening between the speaker panel and the top.
        let ob = sph + 10.0 * fr.0;
        let mut oh = (h - ob - 10.0) * (0.5 + 0.45 * fr.1);
        let ow = (w - 20.0) * (0.6 + 0.35 * fr.2);
        if top_p == BackboxTopProfile::Arched {
            let crown_at = h - 0.06 * w * (ow / w) * (ow / w);
            oh = min_f64(oh, crown_at - ob - 2.0);
        }
        // Backglass: at least the opening, at most W, its v extent centred and inside [0, H].
        let bw = ow + (w - ow) * fr.3;
        let room = min_f64(ob, h - ob - oh);
        let bh = oh + 2.0 * room * 0.5;
        // Score-display window inside the speaker panel.
        let sw = (w - 20.0) * 0.6 * disp.0;
        let sh = (sph - 10.0) * disp.1;
        let cv = sh / 2.0 + 2.0 + (sph - sh - 4.0) * disp.2;
        // Grilles evenly spaced across the speaker panel.
        let spacing = w / n_grilles as f64;
        let gd = min_f64(min_f64(spacing - 10.0, sph - 20.0), 150.0);
        let bb = &mut g.backbox;
        bb.width = w as f32;
        bb.height = h as f32;
        bb.depth = d as f32;
        bb.front_y = (f64::from(g.body.length) - 400.0 + 300.0 * fy) as f32;
        bb.bottom_z = (f64::from(g.body.rear_height) + bz) as f32;
        bb.front_profile = front_p;
        bb.top_profile = top_p;
        bb.speaker_panel_height = sph as f32;
        bb.translite_opening.width = ow as f32;
        bb.translite_opening.height = oh as f32;
        bb.translite_opening.bottom = ob as f32;
        bb.backglass.width = bw as f32;
        bb.backglass.height = bh as f32;
        bb.score_display.width = sw as f32;
        bb.score_display.height = sh as f32;
        bb.score_display.centre_v = cv as f32;
        bb.score_display.pixels = [px.0, px.1];
        bb.grilles = (0..n_grilles)
            .map(|k| Grille {
                u: (-w / 2.0 + spacing * (k as f64 + 0.5)) as f32,
                v: (sph / 2.0) as f32,
                diameter: gd as f32,
            })
            .collect();
        // The construction itself must satisfy pin_core (else this test is wrong, not the code).
        prop_assert!(g.check_consistency().is_ok(), "{:?}", g.check_consistency());
        let cab = generate_cabinet(&g)
            .map_err(|e| TestCaseError::fail(format!("{} / {}", e.element, e.message)))?;
        let s = check_ok_output(&cab)?;
        let bb = &g.backbox;
        let y0 = f64::from(bb.front_y);
        let zb = g.floor_height_mm(y0, f64::from(bb.bottom_z));
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for name in BOUNDS_SET {
            let m = s.meshes.iter().find(|m| m.name == name).unwrap();
            for p in &m.positions {
                for a in 0..3 {
                    let v = f64::from(p[a]) * 1000.0;
                    lo[a] = min_f64(lo[a], v);
                    hi[a] = max_f64(hi[a], v);
                }
            }
        }
        let want_lo = [-w / 2.0, y0, zb];
        let want_hi = [w / 2.0, y0 + d, zb + h];
        for a in 0..3 {
            prop_assert!((lo[a] - want_lo[a]).abs() <= TOL, "axis {} min {} vs {}", a, lo[a], want_lo[a]);
            prop_assert!((hi[a] - want_hi[a]).abs() <= TOL, "axis {} max {} vs {}", a, hi[a], want_hi[a]);
        }
    }
}
