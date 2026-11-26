//! Unit tests of the cabinet generator (REQ-CAB-001, REQ-CAB-003,
//! REQ-CAB-007): mesh orientation, sections, styles, materials and the
//! error keys.

use super::mesh::{Axis, MeshMm, ccw_profile};
use super::*;
use pin_core::fmath::{max_f32, max_f64, min_f32, min_f64};
use pin_core::schema::cabinet::{
    BarProfile, BoltStyle, BracketStyle, CrossSectionShape, FootShape, LegSurface, RailProfile,
};

fn preset(id: &str) -> CabinetGeometry {
    CabinetGeometry::preset(id).unwrap()
}

fn all_parts(g: &CabinetGeometry) -> Vec<Part> {
    let mut out: Vec<Part> = parts::body(g).into_iter().collect();
    out.extend(parts::rails(g));
    out.push(parts::lockdown_bar(g));
    out.push(parts::glass(g));
    for (name, leg) in g.legs.all() {
        out.push(parts::leg_shaft(g, name, leg));
        out.push(parts::bracket(g, name, leg));
        out.push(parts::leveler(g, name, leg));
    }
    out.push(parts::coin_door(g));
    out.push(parts::button(g, g.controls.start_button, Axis::PosY));
    out.push(parts::button(g, g.controls.left_flipper_button, Axis::PosX));
    out.push(parts::button(
        g,
        g.controls.right_flipper_button,
        Axis::NegX,
    ));
    out.push(parts::plunger_housing(g));
    out.push(parts::plunger_rod(g));
    out
}

/// Every style the schema allows, applied to all four legs and the trim.
fn style_variants() -> Vec<CabinetGeometry> {
    let mut out = Vec::new();
    for shape in [
        CrossSectionShape::Square,
        CrossSectionShape::Round,
        CrossSectionShape::Tapered,
    ] {
        for surface in [LegSurface::Plain, LegSurface::Ribbed] {
            for (bracket, bolt, foot, rail, bar) in [
                (
                    BracketStyle::CornerPlate,
                    BoltStyle::Hex,
                    FootShape::Flat,
                    RailProfile::Flat,
                    BarProfile::Flat,
                ),
                (
                    BracketStyle::FlushPlate,
                    BoltStyle::Carriage,
                    FootShape::Domed,
                    RailProfile::Rounded,
                    BarProfile::Rounded,
                ),
                (
                    BracketStyle::CornerPlate,
                    BoltStyle::ButtonHead,
                    FootShape::Swivel,
                    RailProfile::Angled,
                    BarProfile::Rounded,
                ),
            ] {
                let mut g = preset("classic_standard");
                for leg in [
                    &mut g.legs.front_left,
                    &mut g.legs.front_right,
                    &mut g.legs.rear_left,
                    &mut g.legs.rear_right,
                ] {
                    leg.section.shape = shape;
                    leg.surface = surface;
                    leg.bracket = bracket;
                    leg.bolt = bolt;
                    leg.leveler.foot_shape = foot;
                }
                g.body.side_rail.profile = rail;
                g.body.lockdown_bar.profile = bar;
                out.push(g);
            }
        }
    }
    out
}

/// REQ-CAB-001: every part of every style is closed with outward,
/// counter-clockwise faces (positive enclosed volume, winding agrees with
/// the stored normal) and unit normals.
#[test]
fn every_part_faces_outward_counter_clockwise() {
    let mut geoms: Vec<CabinetGeometry> = pin_core::schema::cabinet::preset_ids()
        .iter()
        .map(|id| preset(id))
        .collect();
    geoms.extend(style_variants());
    for g in &geoms {
        for (i, p) in all_parts(g).iter().enumerate() {
            let m = &p.mesh;
            assert!(!m.indices.is_empty(), "part {i} empty");
            assert!(m.signed_volume() > 0.0, "part {i} faces inward");
            assert!(m.windings_match_normals(), "part {i} winding");
            for n in &m.normals {
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                assert!((len - 1.0).abs() < 1e-9);
            }
        }
        assert!(generate_cabinet(g).is_ok());
    }
}

/// The axis bases are right-handed (`u x v = dir`) and labelled.
#[test]
fn axis_bases_are_right_handed() {
    for a in [Axis::PosX, Axis::NegX, Axis::PosY, Axis::NegY, Axis::PosZ] {
        let (u, v) = a.basis();
        let c = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        assert_eq!(c, a.dir(), "{a:?}");
        assert_eq!(a.label().len(), 2);
    }
    assert_eq!(Axis::PosX.opposite(), Axis::NegX);
    assert_eq!(Axis::NegX.opposite(), Axis::PosX);
    assert_eq!(Axis::PosY.opposite(), Axis::NegY);
    assert_eq!(Axis::NegY.opposite(), Axis::PosY);
    assert_eq!(Axis::PosZ.opposite(), Axis::PosZ);
}

/// Profiles are de-duplicated and made counter-clockwise.
#[test]
fn ccw_profile_dedupes_and_orients() {
    let cw = [
        [0.0, 0.0],
        [0.0, 1.0],
        [0.0, 1.0],
        [1.0, 1.0],
        [1.0, 0.0],
        [0.0, 0.0],
    ];
    let p = ccw_profile(&cw);
    assert_eq!(p.len(), 4);
    let mut area = 0.0;
    for (i, a) in p.iter().enumerate() {
        let b = p[(i + 1) % p.len()];
        area += a[0] * b[1] - b[0] * a[1];
    }
    assert!(area > 0.0);
}

/// Degenerate input adds nothing (no panic): fewer than three points, a
/// ring-count mismatch, an empty loft; a zero-area polygon gets a fallback
/// normal and centred UVs.
#[test]
fn degenerate_polygons_and_lofts_are_skipped() {
    let mut m = MeshMm::default();
    m.polygon(&[[0.0; 3], [1.0, 0.0, 0.0]]);
    m.loft(&[]);
    m.loft(&[
        vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        vec![[0.0; 3], [1.0, 0.0, 0.0]],
    ]);
    assert!(m.indices.is_empty());
    m.polygon(&[[0.0; 3]; 4]);
    assert_eq!(m.normals[0], [0.0, 0.0, 1.0]);
    let glb = m.to_glb();
    assert!(glb.uvs.iter().flatten().all(|&u| u == 0.5));
}

/// D32 art surfaces: UVs cover each body side's (and the front's) outer
/// face 0..1 and stay in [0, 1] everywhere.
#[test]
fn side_outer_face_uvs_cover_unit_square() {
    let g = preset("classic_standard");
    let [_, left, right, front, _] = parts::body(&g);
    for (part, outward) in [
        (left, [-1.0, 0.0, 0.0]),
        (right, [1.0, 0.0, 0.0]),
        (front, [0.0, -1.0, 0.0]),
    ] {
        let m = part.mesh.to_glb();
        let mut lo = [f32::INFINITY; 2];
        let mut hi = [f32::NEG_INFINITY; 2];
        for (uv, n) in m.uvs.iter().zip(&m.normals) {
            assert!(uv.iter().all(|c| (0.0..=1.0).contains(c)));
            if n.iter().zip(outward).all(|(a, b)| (a - b).abs() < 1e-6) {
                for k in 0..2 {
                    lo[k] = min_f32(lo[k], uv[k]);
                    hi[k] = max_f32(hi[k], uv[k]);
                }
            }
        }
        assert_eq!(lo, [0.0, 0.0]);
        assert_eq!(hi, [1.0, 1.0]);
    }
}

/// REQ-CAB-007: the shaft's section follows shape and surface; ribs keep
/// the outer size; a tapered leg narrows from `top` to `bottom`.
#[test]
fn leg_sections_follow_shape_and_surface() {
    let base = preset("classic_standard");
    let probe = |shape, surface| {
        let mut g = base.clone();
        g.legs.front_left.section.shape = shape;
        g.legs.front_left.surface = surface;
        let leg = &g.legs.front_left;
        let p = parts::leg_shaft(&g, "front_left", leg);
        let (x, y, _, _) = parts::leg_xy(&g, "front_left", leg);
        let z_top = f64::from(leg.leveler.extension + leg.length);
        let z_bot = f64::from(leg.leveler.extension);
        let extent = |z: f64| {
            p.mesh
                .positions
                .iter()
                .filter(|q| (q[2] - z).abs() < 1e-9)
                .map(|q| max_f64((q[0] - x).abs(), (q[1] - y).abs()))
                .fold(0.0, max_f64)
        };
        (p.mesh.positions.len(), extent(z_top), extent(z_bot))
    };
    let top = f64::from(base.legs.front_left.section.top) * 0.5;
    let bottom = f64::from(base.legs.front_left.section.bottom) * 0.5;
    let (sq, sq_t, sq_b) = probe(CrossSectionShape::Square, LegSurface::Plain);
    let (sq_r, sq_rt, _) = probe(CrossSectionShape::Square, LegSurface::Ribbed);
    let (rd, rd_t, rd_b) = probe(CrossSectionShape::Round, LegSurface::Plain);
    let (rd_r, rd_rt, _) = probe(CrossSectionShape::Round, LegSurface::Ribbed);
    let (_, tp_t, tp_b) = probe(CrossSectionShape::Tapered, LegSurface::Plain);
    assert!(sq_r > sq && rd_r > rd && rd != sq, "ribs add geometry");
    for e in [sq_t, sq_b, sq_rt, rd_t, rd_b, rd_rt, tp_t] {
        assert!((e - top).abs() < 1e-9, "outer half size {e} vs {top}");
    }
    assert!((tp_b - bottom).abs() < 1e-9);
}

/// REQ-CAB-007: bracket geometry differs per bracket and bolt style.
#[test]
fn bracket_geometry_differs_per_style() {
    let base = preset("classic_standard");
    let mut seen: Vec<Vec<[f64; 3]>> = Vec::new();
    for bracket in [BracketStyle::CornerPlate, BracketStyle::FlushPlate] {
        for bolt in [BoltStyle::Hex, BoltStyle::Carriage, BoltStyle::ButtonHead] {
            let mut g = base.clone();
            g.legs.rear_right.bracket = bracket;
            g.legs.rear_right.bolt = bolt;
            let p = parts::bracket(&g, "rear_right", &g.legs.rear_right);
            assert!(!seen.contains(&p.mesh.positions), "{bracket:?} {bolt:?}");
            seen.push(p.mesh.positions);
        }
    }
}

/// REQ-CAB-007: each foot shape stands on z = 0 and the leveler reaches
/// the shaft bottom; the shapes differ.
#[test]
fn leveler_feet_stand_on_the_floor() {
    let base = preset("modern_standard");
    let mut seen = Vec::new();
    for foot in [FootShape::Flat, FootShape::Domed, FootShape::Swivel] {
        let mut g = base.clone();
        g.legs.front_left.leveler.foot_shape = foot;
        let leg = &g.legs.front_left;
        let p = parts::leveler(&g, "front_left", leg);
        let lo = p
            .mesh
            .positions
            .iter()
            .map(|q| q[2])
            .fold(f64::INFINITY, min_f64);
        let hi = p
            .mesh
            .positions
            .iter()
            .map(|q| q[2])
            .fold(f64::NEG_INFINITY, max_f64);
        assert_eq!(lo, 0.0, "{foot:?}");
        assert!(hi >= f64::from(leg.leveler.extension));
        assert!(!seen.contains(&p.mesh.positions));
        seen.push(p.mesh.positions);
    }
}

/// REQ-CAB-007: sRGB to linear (IEC 61966-2-1) and the finish factors.
#[test]
fn colours_and_finishes() {
    assert_eq!(
        srgb_to_linear("k", "#000000").unwrap(),
        [0.0, 0.0, 0.0, 1.0]
    );
    assert_eq!(
        srgb_to_linear("k", "#ffffff").unwrap(),
        [1.0, 1.0, 1.0, 1.0]
    );
    let grey = srgb_to_linear("k", "#808080").unwrap();
    assert!((grey[0] - 0.215_860_5).abs() < 1e-6, "{grey:?}");
    let dark = srgb_to_linear("k", "#0a0000").unwrap();
    assert!((dark[0] - (10.0 / 255.0 / 12.92) as f32).abs() < 1e-9);
    for bad in ["", "#12345", "#gg0000", "123456#", "#1234é", "#1234567"] {
        let e = srgb_to_linear("legs.front_left.colour", bad).unwrap_err();
        assert_eq!(e.element, "legs.front_left.colour");
    }
    assert_eq!(finish_factors(Finish::Chrome), (1.0, 0.15));
    assert_eq!(finish_factors(Finish::BrushedSteel), (1.0, 0.4));
    assert_eq!(finish_factors(Finish::PowderCoat), (0.0, 0.6));
    assert_eq!(finish_factors(Finish::Wrinkle), (0.0, 0.8));
    assert_eq!(finish_factors(Finish::Painted), (0.0, 0.5));
    assert_eq!(travel_text(4.0), "4");
    assert_eq!(travel_text(2.5), "2.5");
}

/// REQ-CAB-007: wood legs are never metallic; the others follow the
/// finish; the 17 materials are in the documented order.
#[test]
fn wood_legs_are_not_metallic() {
    let mut g = preset("classic_standard");
    g.legs.rear_left.material = LegMaterial::Wood;
    let mats = materials(&g).unwrap();
    assert_eq!(mats.len(), 17);
    for (m, name) in mats.iter().zip(MATERIAL_NAMES) {
        assert_eq!(m.name, name);
    }
    assert_eq!(mats[M_LEG0 + 2].metallic, 0.0);
    assert_eq!(mats[M_LEG0 + 3].metallic, 1.0);
    assert!(mats[M_GLASS].blend && mats[M_GLASS].base_color[3] < 1.0);
}

type Mutate = fn(&mut CabinetGeometry, f32);

/// Error path: every checked key is named when its value is NaN, infinite,
/// negative or beyond the size limit; never a panic.
#[test]
fn invalid_values_name_their_key() {
    let cases: &[(&str, Mutate)] = &[
        ("body.width", |g, v| g.body.width = v),
        ("body.length", |g, v| g.body.length = v),
        ("body.front_height", |g, v| g.body.front_height = v),
        ("body.rear_height", |g, v| g.body.rear_height = v),
        ("body.wall_thickness", |g, v| g.body.wall_thickness = v),
        ("body.leg_overlap", |g, v| g.body.leg_overlap = v),
        ("body.glass_channel.width", |g, v| {
            g.body.glass_channel.width = v
        }),
        ("body.glass_channel.depth", |g, v| {
            g.body.glass_channel.depth = v
        }),
        ("body.lockdown_bar.height", |g, v| {
            g.body.lockdown_bar.height = v
        }),
        ("body.lockdown_bar.depth", |g, v| {
            g.body.lockdown_bar.depth = v
        }),
        ("body.side_rail.width", |g, v| g.body.side_rail.width = v),
        ("body.side_rail.height", |g, v| g.body.side_rail.height = v),
        ("legs.front_left.length", |g, v| {
            g.legs.front_left.length = v
        }),
        ("legs.front_right.section.top", |g, v| {
            g.legs.front_right.section.top = v
        }),
        ("legs.rear_left.section.bottom", |g, v| {
            g.legs.rear_left.section.bottom = v
        }),
        ("legs.rear_right.leveler.thread_length", |g, v| {
            g.legs.rear_right.leveler.thread_length = v
        }),
        ("legs.rear_right.leveler.thread_diameter", |g, v| {
            g.legs.rear_right.leveler.thread_diameter = v
        }),
        ("legs.rear_left.leveler.extension", |g, v| {
            g.legs.rear_left.leveler.extension = v
        }),
        ("legs.front_left.leveler.foot_diameter", |g, v| {
            g.legs.front_left.leveler.foot_diameter = v
        }),
        ("legs.rear_left.length", |g, v| g.legs.rear_left.length = v),
        ("glass_panel.width", |g, v| g.glass_panel.width = v),
        ("glass_panel.length", |g, v| g.glass_panel.length = v),
        ("glass_panel.thickness", |g, v| g.glass_panel.thickness = v),
        ("controls.coin_door.width", |g, v| {
            g.controls.coin_door.width = v
        }),
        ("controls.coin_door.height", |g, v| {
            g.controls.coin_door.height = v
        }),
    ];
    let base = preset("classic_standard");
    for (key, set) in cases {
        for v in [f32::NAN, f32::INFINITY, -1.0, 1e9] {
            let mut g = base.clone();
            set(&mut g, v);
            let e = generate_cabinet(&g).unwrap_err();
            assert_eq!(&e.element, key, "{v}: {e}");
            assert!(!e.message.is_empty());
        }
    }
    let positions: &[(&str, Mutate)] = &[
        ("controls.left_flipper_button", |g, v| {
            g.controls.left_flipper_button[0] = v
        }),
        ("controls.right_flipper_button", |g, v| {
            g.controls.right_flipper_button[1] = v
        }),
        ("controls.start_button", |g, v| {
            g.controls.start_button[2] = v
        }),
        ("controls.action_button", |g, v| {
            g.controls.action_button[0] = v
        }),
        ("controls.plunger", |g, v| g.controls.plunger[2] = v),
        ("controls.coin_door.centre", |g, v| {
            g.controls.coin_door.centre[1] = v
        }),
        ("legs.front_right.corner_inset", |g, v| {
            g.legs.front_right.corner_inset[0] = v
        }),
    ];
    for (key, set) in positions {
        for v in [f32::NAN, f32::NEG_INFINITY, -1e9] {
            let mut g = base.clone();
            set(&mut g, v);
            assert_eq!(&generate_cabinet(&g).unwrap_err().element, key);
        }
    }
    let mut g = base.clone();
    g.trim.rail_colour = "red".into();
    assert_eq!(
        generate_cabinet(&g).unwrap_err().element,
        "trim.rail_colour"
    );
    let mut g = base.clone();
    g.legs.rear_right.colour = "#12".into();
    assert_eq!(
        generate_cabinet(&g).unwrap_err().element,
        "legs.rear_right.colour"
    );
    // Zero is allowed where it is meaningful.
    let mut g = base.clone();
    g.body.leg_overlap = 0.0;
    g.body.lockdown_bar.height = 0.0;
    g.legs.front_left.leveler.extension = 0.0;
    assert!(generate_cabinet(&g).is_ok());
}

/// The final guard names `cabinet` for a non-finite or empty node.
#[test]
fn node_guard_rejects_non_finite_or_empty_nodes() {
    let g = preset("classic_standard");
    let good = node("cabinet.glass", parts::glass(&g), M_GLASS);
    assert!(check_nodes(std::slice::from_ref(&good)).is_ok());
    let mut nan = good.clone();
    nan.mesh.positions[0][1] = f32::NAN;
    assert_eq!(
        check_nodes(&[good.clone(), nan]).unwrap_err().element,
        "cabinet"
    );
    let mut empty = good;
    empty.mesh.indices.clear();
    assert_eq!(check_nodes(&[empty]).unwrap_err().element, "cabinet");
}
