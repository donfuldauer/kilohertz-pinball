//! KP-030 (#111) property tests (R3, R11, REQ-CAB-001, REQ-CAB-003):
//! random `[backbox]` values (sane and hostile, every profile, 0 to 9
//! grilles) never panic the generator; an `Ok` model is deterministic,
//! loads, has finite floats, UVs in [0, 1] and the stable node and
//! material lists, and keeps the bounds set inside the backbox; an `Err`
//! names one of the backbox contract's keys.

#[path = "cabinet_common.rs"]
mod common;

use common::*;
use pin_core::fmath::{max_f32, min_f32};
use pin_core::schema::cabinet::{BackboxFrontProfile, BackboxTopProfile, CabinetGeometry, Grille};
use pin_tablegen::cabinet::{MATERIAL_NAMES, NODE_NAMES, generate_cabinet};
use proptest::prelude::*;

/// The `element` keys the backbox contract (section G) allows.
fn is_contract_key(key: &str) -> bool {
    const KEYS: [&str; 22] = [
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
    if KEYS.contains(&key) {
        return true;
    }
    // `backbox.grilles[i].u|v|diameter`.
    key.strip_prefix("backbox.grilles[")
        .and_then(|r| r.split_once("]."))
        .is_some_and(|(i, f)| i.parse::<usize>().is_ok() && ["u", "v", "diameter"].contains(&f))
}

/// Hostile values for one field.
const HOSTILE: [f32; 7] = [
    f32::NAN,
    f32::INFINITY,
    f32::NEG_INFINITY,
    0.0,
    -1.0,
    1e9,
    1e-6,
];

/// One `[backbox]` field of a geometry by index (17 fields, then the first
/// grille's three).
fn field(g: &mut CabinetGeometry, i: usize) -> Option<&mut f32> {
    let bb = &mut g.backbox;
    Some(match i {
        0 => &mut bb.width,
        1 => &mut bb.height,
        2 => &mut bb.depth,
        3 => &mut bb.front_y,
        4 => &mut bb.bottom_z,
        5 => &mut bb.speaker_panel_height,
        6 => &mut bb.backglass.width,
        7 => &mut bb.backglass.height,
        8 => &mut bb.backglass.thickness,
        9 => &mut bb.translite_opening.width,
        10 => &mut bb.translite_opening.height,
        11 => &mut bb.translite_opening.bottom,
        12 => &mut bb.score_display.width,
        13 => &mut bb.score_display.height,
        14 => &mut bb.score_display.centre_v,
        15 => &mut bb.grilles.first_mut()?.u,
        16 => &mut bb.grilles.first_mut()?.v,
        17 => &mut bb.grilles.first_mut()?.diameter,
        _ => return None,
    })
}

/// A preset with a random but mostly consistent `[backbox]` (sizes drawn
/// relative to each other, so most cases generate), every profile, 0 to 9
/// grilles, and sometimes one field replaced by a hostile value or a
/// random out-of-range one.
fn geometry() -> impl Strategy<Value = CabinetGeometry> {
    (
        (0usize..3, 0usize..2, 0usize..3),
        (
            300.0f32..900.0,
            300.0f32..900.0,
            150.0f32..400.0,
            800.0f32..1400.0,
            400.0f32..900.0,
        ),
        prop::array::uniform8(0.0f32..1.0),
        (0u32..2000, 0u32..2000, 0.5f32..8.0),
        prop::collection::vec((0.0f32..1.0, 0.0f32..1.0, 0.0f32..1.0), 0..10),
        (
            0usize..30,
            0usize..HOSTILE.len(),
            -2000.0f32..2000.0,
            any::<bool>(),
        ),
    )
        .prop_map(
            |(p, size, fr, (px, py, th), grilles, (fi, hi, wild, use_wild))| {
                let mut g = preset(PRESETS[p.0]);
                let bb = &mut g.backbox;
                bb.front_profile = [BackboxFrontProfile::Flat, BackboxFrontProfile::Angled][p.1];
                bb.top_profile = [
                    BackboxTopProfile::Flat,
                    BackboxTopProfile::Angled,
                    BackboxTopProfile::Arched,
                ][p.2];
                let (w, h, d, fy, bz) = size;
                (bb.width, bb.height, bb.depth, bb.front_y, bb.bottom_z) = (w, h, d, fy, bz);
                let sph = h * (0.1 + 0.3 * fr[0]);
                bb.speaker_panel_height = sph;
                let o = &mut bb.translite_opening;
                o.width = w * (0.3 + 0.7 * fr[1]);
                o.bottom = sph + h * 0.05 * fr[2];
                o.height = (h - o.bottom) * (0.3 + 0.65 * fr[3]);
                let (ob, oh, ow) = (o.bottom, o.height, o.width);
                let room = min_f32(ob, h - ob - oh);
                bb.backglass.width = ow + (w - ow) * fr[4];
                bb.backglass.height = oh + 2.0 * room * fr[5];
                bb.backglass.thickness = th;
                let sd = &mut bb.score_display;
                sd.width = w * (0.1 + 0.5 * fr[6]);
                sd.height = sph * (0.1 + 0.7 * fr[7]);
                sd.centre_v = sd.height * 0.5 + (sph - sd.height) * 0.5;
                sd.pixels = [px, py];
                bb.grilles = grilles
                    .into_iter()
                    .map(|(a, b, c)| {
                        let r = 0.5 * (5.0 + max_f32(min_f32(w, sph) * 0.3 - 5.0, 0.0) * c);
                        Grille {
                            u: (w * 0.5 - r) * (2.0 * a - 1.0),
                            v: r + (h - 2.0 * r) * b,
                            diameter: 2.0 * r,
                        }
                    })
                    .collect();
                if let Some(f) = field(&mut g, fi) {
                    *f = if use_wild { wild } else { HOSTILE[hi] };
                }
                g
            },
        )
}

/// A preset with only small, valid changes: always generates.
fn valid() -> impl Strategy<Value = CabinetGeometry> {
    (
        0usize..2,
        0usize..2,
        -20.0f32..20.0,
        -40.0f32..40.0,
        0usize..3,
    )
        .prop_map(|(p, front, dw, dz, top)| {
            // classic presets only for the arched top (modern's opening is
            // taller than the arch allows).
            let mut g = preset(PRESETS[p]);
            let bb = &mut g.backbox;
            bb.front_profile = [BackboxFrontProfile::Flat, BackboxFrontProfile::Angled][front];
            bb.top_profile = [
                BackboxTopProfile::Flat,
                BackboxTopProfile::Angled,
                BackboxTopProfile::Arched,
            ][top];
            bb.width += dw;
            bb.bottom_z += dz;
            g
        })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    /// R11 + REQ-CAB-001: random backbox values never panic; `Ok` gives a
    /// loadable, finite, deterministic model with the stable lists and the
    /// bounds set inside the backbox; `Err` names a contract key.
    #[test]
    fn random_backboxes_never_panic(g in geometry()) {
        match generate_cabinet(&g) {
            Ok(a) => {
                let b = generate_cabinet(&g).unwrap();
                prop_assert_eq!(&a.glb, &b.glb);
                prop_assert_eq!(a.nodes.clone(), NODE_NAMES.map(String::from).to_vec());
                let scene = load(&a.glb);
                let mats: Vec<&str> = scene.materials.iter().map(|m| m.name.as_str()).collect();
                prop_assert_eq!(mats, MATERIAL_NAMES.to_vec());
                let names: Vec<&str> = scene.meshes.iter().map(|m| m.name.as_str()).collect();
                let want: Vec<&str> = NODE_NAMES
                    .iter()
                    .copied()
                    .filter(|n| !g.backbox.grilles.is_empty() || *n != "cabinet.backbox.grilles")
                    .collect();
                prop_assert_eq!(names, want);
                for m in &scene.meshes {
                    prop_assert!(m.positions.iter().flatten().all(|v| v.is_finite()));
                    prop_assert!(m.normals.iter().flatten().all(|v| v.is_finite()));
                    prop_assert!(m.uvs.iter().flatten().all(|v| (0.0..=1.0).contains(v)));
                }
                let bb = &g.backbox;
                let (lo, hi) = bounds(
                    &scene,
                    &[
                        "cabinet.backbox.shell",
                        "cabinet.backbox.speaker_panel",
                        "cabinet.backbox.translite_frame",
                    ],
                );
                let zb = g.floor_height_mm(f64::from(bb.front_y), f64::from(bb.bottom_z));
                close(lo[2], zb / 1000.0, MM, "bottom");
                close(hi[2], g.backbox_top_height_mm() / 1000.0, MM, "apex");
                close(hi[0] - lo[0], f64::from(bb.width) / 1000.0, MM, "width");
                close(lo[1], f64::from(bb.front_y) / 1000.0, MM, "front");
                close(hi[1], f64::from(bb.front_y + bb.depth) / 1000.0, MM, "rear");
            }
            Err(e) => prop_assert!(is_contract_key(&e.element), "unexpected key {}", e.element),
        }
    }

    /// REQ-CAB-003: small valid changes on the classic presets with every
    /// profile always generate, with the apex at `backbox_top_height_mm`.
    #[test]
    fn valid_backboxes_generate(g in valid()) {
        let out = generate_cabinet(&g);
        prop_assert!(out.is_ok(), "{:?}", out.err());
        let scene = load(&out.unwrap().glb);
        let (_, hi) = bounds(&scene, &["cabinet.backbox.translite_frame", "cabinet.backbox.shell"]);
        close(hi[2], g.backbox_top_height_mm() / 1000.0, MM, "apex");
    }
}

/// The key matcher accepts the contract's keys only.
#[test]
fn contract_keys_are_recognised() {
    assert!(is_contract_key("backbox.grilles[7].diameter"));
    assert!(is_contract_key("mount.inset"));
    assert!(!is_contract_key("cabinet"));
    assert!(!is_contract_key("backbox.grilles[x].u"));
    assert!(!is_contract_key("backbox.grilles[0].w"));
}
