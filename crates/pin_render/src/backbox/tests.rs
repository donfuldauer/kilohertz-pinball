//! GPU-free tests of the backbox pass: toy maths (unit + property), the
//! depth split, the camera, reel geometry, glTF conversion and the shaders.

use super::renderer::BackboxLighting;
use super::*;
use crate::pbr::math::{IDENTITY, Mat4, transform_point};
use pin_assetgen::fonts::FONT_5X7;
use proptest::prelude::*;

fn reel(place: u32, rate: f32) -> ScoreReel {
    ScoreReel {
        meshes: vec![format!("reel{place}"), format!("reel{place}.digits")],
        player: 0,
        place,
        axis: [1.0, 0.0, 0.0],
        pivot: [0.0, 0.0, 0.3],
        settle_deg_per_s: rate,
    }
}

fn approx(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

fn mat_close(a: &Mat4, b: &Mat4, tol: f32) -> bool {
    (0..4).all(|c| (0..4).all(|r| approx(a[c][r], b[c][r], tol)))
}

#[test]
fn reel_angle_is_digit_times_36() {
    for d in 0..30u32 {
        assert_eq!(reel_angle_deg(d), (d % 10) as f32 * 36.0);
    }
    assert_eq!(digit_at(0.0), 0);
    assert_eq!(digit_at(36.0 * 7.0), 7);
    assert_eq!(digit_at(359.0), 0);
    assert_eq!(digit_at(-36.0), 9);
}

#[test]
fn score_digits() {
    assert_eq!(score_digit(1_234_567, 0), 7);
    assert_eq!(score_digit(1_234_567, 3), 4);
    assert_eq!(score_digit(1_234_567, 6), 1);
    assert_eq!(score_digit(1_234_567, 9), 0);
    assert_eq!(score_digit(u64::MAX, 19), 1);
    assert_eq!(score_digit(u64::MAX, 40), 0);
}

#[test]
fn shortest_path_wraps_through_zero() {
    assert_eq!(shortest_delta_deg(9.0 * 36.0, 0.0), 36.0); // 9 → 0 forward
    assert_eq!(shortest_delta_deg(0.0, 9.0 * 36.0), -36.0); // 0 → 9 back
    assert_eq!(shortest_delta_deg(0.0, 180.0), 180.0);
    assert_eq!(shortest_delta_deg(10.0, 10.0), 0.0);
    assert_eq!(wrap_deg(-0.0), 0.0);
    assert_eq!(wrap_deg(f32::NAN), 0.0);
    assert_eq!(wrap_deg(720.5), 0.5);
}

#[test]
fn reels_settle_along_the_shortest_path() {
    let mut rig = ToyRig::new();
    let units = rig.add_reel(reel(0, 360.0));
    let tens = rig.add_reel(reel(1, 360.0));
    rig.set_scores(&[19]);
    assert!(!rig.settled());
    // 360°/s: digit 9 is 324° forward but 36° back: 0.1 s.
    let mut ticks = 0;
    while !rig.settled() {
        rig.advance(0.001);
        ticks += 1;
        assert!(ticks < 1000);
        // Never passes through the far side.
        let a = rig.reel_angle(units);
        assert!(a == 0.0 || a >= 324.0 - 1e-3, "units at {a}");
    }
    assert!((99..=101).contains(&ticks), "{ticks} ticks");
    assert_eq!(rig.reel_angle(units), 324.0);
    assert_eq!(rig.reel_angle(tens), 36.0);
    assert_eq!(digit_at(rig.reel_angle(units)), 9);
    // 19 → 20: units 9 → 0 is one step forward.
    rig.set_scores(&[20]);
    rig.advance(0.05);
    let a = rig.reel_angle(units);
    assert!(a > 324.0 && a < 360.0, "forward through 360: {a}");
    rig.advance(1.0);
    assert_eq!(rig.reel_angle(units), 0.0);
    assert_eq!(rig.reel_angle(tens), 72.0);
}

#[test]
fn snap_missing_players_and_bad_inputs() {
    let mut rig = ToyRig::new();
    let mut r = reel(2, f32::NAN);
    r.player = 3;
    let i = rig.add_reel(r);
    rig.set_scores(&[999]); // player 3 missing: shows 0
    rig.advance(0.01);
    assert_eq!(rig.reel_angle(i), 0.0);
    rig.set_scores(&[0, 0, 0, 500]);
    rig.advance(f32::NAN);
    rig.advance(-1.0);
    assert_eq!(rig.reel_angle(i), 0.0);
    // A non-positive rate snaps.
    rig.advance(0.001);
    assert_eq!(rig.reel_angle(i), 180.0);
    rig.set_scores(&[0, 0, 0, 700]);
    rig.snap_reels();
    assert_eq!(rig.reel_angle(i), 252.0);
    assert_eq!(rig.reel_angle(99), 0.0);
    assert_eq!(rig.reel_transform(99), IDENTITY);
    assert_eq!(rig.channel_transform(99), IDENTITY);
    assert_eq!(rig.channel_value(99), 0.0);
    assert_eq!(rig.reel_count(), 1);
}

#[test]
fn reel_transform_turns_about_its_pivot() {
    let mut rig = ToyRig::new();
    let i = rig.add_reel(reel(0, 1.0e6));
    rig.set_scores(&[5]);
    rig.advance(1.0);
    let m = rig.reel_transform(i);
    // The pivot is fixed; a point below it swings by 180°.
    let p = transform_point(&m, [0.0, 0.0, 0.3]);
    assert!(approx(p[2], 0.3, 1e-6) && approx(p[1], 0.0, 1e-6));
    let q = transform_point(&m, [0.0, -0.1, 0.3]);
    assert!(approx(q[1], 0.1, 1e-5) && approx(q[2], 0.3, 1e-5), "{q:?}");
    // Digit 1's face normal (at −36°) comes to the front (−y) at 36°.
    let r = rotation_about([1.0, 0.0, 0.0], [0.0; 3], 36f32.to_radians());
    let face1 = [
        0.0,
        -libm::cosf(36f32.to_radians()),
        libm::sinf(36f32.to_radians()),
    ];
    let n = transform_point(&r, face1);
    assert!(approx(n[1], -1.0, 1e-6) && approx(n[2], 0.0, 1e-6), "{n:?}");
}

#[test]
fn mechanism_channels_ease_to_clamped_targets() {
    let mut rig = ToyRig::new();
    let rot = rig.add_channel(MechChannel {
        meshes: vec!["arm".into()],
        motion: MechMotion::Rotate {
            axis: [0.0, 0.0, 1.0],
            pivot: [0.1, 0.0, 0.0],
        },
        range: (90.0, -10.0), // reversed: sanitised to (−10, 90)
        time_constant_s: 0.1,
    });
    let lift = rig.add_channel(MechChannel {
        meshes: vec!["lift".into()],
        motion: MechMotion::Translate {
            axis: [0.0, 0.0, 2.0],
        },
        range: (0.0, 0.05),
        time_constant_s: 0.0,
    });
    assert_eq!(rig.channel_value(rot), -10.0);
    rig.set_channel(rot, 500.0);
    rig.set_channel(lift, 0.02);
    rig.set_channel(lift, f32::NAN); // ignored
    rig.advance(0.1);
    // One time constant: 1 − 1/e of the way.
    let want = -10.0 + 100.0 * (1.0 - libm::expf(-1.0));
    assert!(approx(rig.channel_value(rot), want, 1e-3));
    assert_eq!(rig.channel_value(lift), 0.02, "τ ≤ 0 snaps");
    for _ in 0..2000 {
        rig.advance(0.001);
    }
    assert!(rig.settled());
    assert!(approx(rig.channel_value(rot), 90.0, 1e-3));
    let t = rig.channel_transform(lift);
    assert!(approx(t[3][2], 0.02, 1e-7) && t[3][0] == 0.0);
    let mut seen = Vec::new();
    rig.for_each_transform(|name, _| seen.push(name.to_owned()));
    assert_eq!(seen, ["arm", "lift"]);
    assert_eq!(rig.channel_count(), 2);
    // A degenerate axis is the identity.
    let mut bad = ToyRig::new();
    let k = bad.add_channel(MechChannel {
        meshes: vec![],
        motion: MechMotion::Translate { axis: [0.0; 3] },
        range: (f32::NAN, 1.0),
        time_constant_s: 1.0,
    });
    bad.set_channel(k, 1.0);
    bad.advance(1.0);
    assert_eq!(bad.channel_transform(k), IDENTITY);
    assert_eq!(rotation_about([0.0; 3], [1.0; 3], 1.0), IDENTITY);
}

#[test]
fn depth_split_puts_layers_below_the_plane_behind() {
    assert_eq!(layers_behind(&[0, 1], DEFAULT_PLANE_DEPTH), 1);
    assert_eq!(layers_behind(&[0, 0, 1, 2], 0.5), 2);
    assert_eq!(layers_behind(&[0, 1], 1.5), 2);
    assert_eq!(layers_behind(&[0, 1], -1.0), 0);
    assert_eq!(layers_behind(&[0, 1], f32::NAN), 2);
    assert_eq!(layers_behind(&[], 0.5), 0);
    let m = pin_core::BackglassManifest::from_toml_str(include_str!(
        "../../../pin_core/fixtures/backglass.toml"
    ))
    .unwrap();
    let n = m.translite.layers.iter().filter(|l| l.depth == 0).count();
    assert_eq!(manifest_layers_behind(&m, DEFAULT_PLANE_DEPTH), n);
}

#[test]
fn camera_from_manifest_projects_the_target_to_the_centre() {
    let cam = pin_core::schema::backglass::BackboxCamera {
        position: [0.0, -600.0, 300.0],
        look_at: [0.0, 0.0, 300.0],
        fov_deg: 35.0,
    };
    let v = BackboxView::from_camera(&cam);
    assert_eq!(v.eye, [0.0, -0.6, 0.3]);
    assert!(approx(v.fov_y, 35f32.to_radians(), 1e-7));
    let c = v.project(400, 300, [0.0, 0.0, 0.3]).unwrap();
    assert!(approx(c[0], 200.0, 1e-3) && approx(c[1], 150.0, 1e-3));
    // +x is right, +z is up on screen.
    let r = v.project(400, 300, [0.05, 0.0, 0.35]).unwrap();
    assert!(r[0] > 200.0 && r[1] < 150.0);
    // Half the vertical field of view reaches the top edge.
    let top = 0.3 + 0.6 * libm::tanf(17.5f32.to_radians());
    let t = v.project(400, 300, [0.0, 0.0, top]).unwrap();
    assert!(approx(t[1], 0.0, 0.05), "{t:?}");
    assert!(
        v.project(400, 300, [0.0, -1.0, 0.3]).is_none(),
        "behind the eye"
    );
}

#[test]
fn reel_scene_has_every_digit_glyph() {
    let spec = ReelSpec {
        center: [0.0, 0.0, 0.3],
        face_height: 0.03,
        width: 0.025,
    };
    let s = reel_scene("reels[0]", &spec);
    assert_eq!(s.meshes.len(), 2);
    assert_eq!(s.meshes[1].name, "reels[0].digits");
    let lit: usize = (0..10)
        .map(|d| {
            let g = FONT_5X7.glyph_or_fallback(char::from(b'0' + d as u8));
            (0..7)
                .flat_map(|r| (0..5).map(move |c| (c, r)))
                .filter(|&(c, r)| g.pixel(c, r))
                .count()
        })
        .sum();
    assert_eq!(s.meshes[1].geometry.positions.len(), lit * 4);
    assert_eq!(s.meshes[0].geometry.indices.len(), 10 * 6 + 2 * 10 * 3);
    // The faces tile the decagon: face height = 2·apothem·tan 18°.
    assert!(approx(
        2.0 * spec.apothem() * libm::tanf(18f32.to_radians()),
        spec.face_height,
        1e-6
    ));
    // Digit d's cell centres sit in front of the drum after turning by d·36°.
    for d in 0..10 {
        let m = rotation_about([1.0, 0.0, 0.0], spec.center, reel_angle_deg(d).to_radians());
        let c = transform_point(&m, spec.cell_center(d, 2, 3));
        assert!(
            approx(c[0], 0.0, 1e-6) && approx(c[2], 0.3, 1e-5),
            "{d}: {c:?}"
        );
        assert!(c[1] < -spec.apothem() + 1e-6);
    }
    // Every glyph quad faces outward (winding agrees with its normal).
    for m in &s.meshes {
        let g = &m.geometry;
        for t in g.indices.chunks(3) {
            let p = |k: usize| g.positions[t[k] as usize];
            let (a, b, c) = (p(0), p(1), p(2));
            let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let n = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            let stored = g.normals[t[0] as usize];
            let d = n[0] * stored[0] + n[1] * stored[1] + n[2] * stored[2];
            assert!(d > 0.0, "{}: {t:?}", m.name);
        }
    }
}

#[test]
fn gltf_scene_converts_colours_and_transforms() {
    use pin_assets::{GltfScene, MeshData, PbrMaterial};
    let mesh = |name: &str, material| MeshData {
        name: name.into(),
        positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        normals: vec![[0.0, 0.0, 1.0]; 3],
        uvs: vec![],
        indices: vec![0, 1, 2],
        material,
        transform: crate::pbr::math::translation([0.0, 0.0, 2.0]),
        extras: Default::default(),
    };
    let g = GltfScene {
        meshes: vec![mesh("a", Some(0)), mesh("b", None), mesh("a", Some(7))],
        materials: vec![PbrMaterial {
            name: "m".into(),
            base_color: [0.214_041_14, 1.0, 0.0, 0.5],
            metallic: 0.0,
            roughness: 1.0,
            emissive: [0.0; 3],
            base_color_image: None,
            metallic_roughness_image: None,
            normal_image: None,
            emissive_image: None,
            double_sided: false,
        }],
        images: vec![],
    };
    let mut s = BackboxScene::from_gltf(&g);
    assert_eq!(s.meshes.len(), 3);
    assert!(
        approx(s.meshes[0].color[0], 0.5, 1e-3),
        "linear 0.214 → sRGB 0.5"
    );
    assert_eq!(s.meshes[0].color[3], 1.0);
    assert_eq!(s.meshes[1].color, [0.8, 0.8, 0.8, 1.0]);
    assert_eq!(s.meshes[2].color, [0.8, 0.8, 0.8, 1.0], "unknown material");
    assert_eq!(s.meshes[0].transform[3][2], 2.0);
    assert_eq!(s.indices_of("a").collect::<Vec<_>>(), [0, 2]);
    s.extend(reel_scene(
        "r",
        &ReelSpec {
            center: [0.0; 3],
            face_height: 0.02,
            width: 0.02,
        },
    ));
    assert_eq!(s.meshes.len(), 5);
    assert_eq!(linear_to_srgb(f32::NAN), 0.0);
    assert!(approx(linear_to_srgb(2.0), 1.0, 1e-6));
    assert_eq!(
        compose(&IDENTITY, &s.meshes[0].transform),
        s.meshes[0].transform
    );
}

#[test]
fn lighting_uniform_is_normalised() {
    let u = BackboxLighting::default();
    assert!(u.ambient > 0.0 && u.ambient < 1.0);
    let w = BackboxLighting {
        ambient: 7.0,
        key_dir: [0.0, 0.0, 2.0],
    };
    assert_eq!(w.uniform(), [0.0, 0.0, 1.0, 1.0]);
    let bad = BackboxLighting {
        ambient: f32::NAN,
        key_dir: [0.0; 3],
    };
    assert_eq!(bad.uniform(), [0.0, 0.0, 0.0, 1.0]);
    let d = u.uniform();
    assert!(approx(d[0] * d[0] + d[1] * d[1] + d[2] * d[2], 1.0, 1e-6));
    assert_eq!(BackboxLighting::UNLIT.uniform()[3], 1.0);
}

#[test]
fn shaders_validate_with_naga() {
    for (name, src) in shader_sources() {
        let module = naga::front::wgsl::parse_str(src).unwrap_or_else(|e| panic!("{name}: {e}"));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name}: {e:?}"));
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    /// Reel angle for digit d is d·36° modulo 360.
    #[test]
    fn reel_angle_mod_360(d in any::<u32>()) {
        let a = reel_angle_deg(d);
        prop_assert_eq!(a, ((d % 10) * 36) as f32);
        prop_assert!((0.0..360.0).contains(&a));
        prop_assert_eq!(digit_at(a), d % 10);
    }

    /// The shortest path is never longer than half a turn and lands on the
    /// target.
    #[test]
    fn shortest_delta_is_at_most_half_a_turn(from in -1.0e4f32..1.0e4, to in -1.0e4f32..1.0e4) {
        let d = shortest_delta_deg(from, to);
        prop_assert!(d > -180.0 - 1e-3 && d <= 180.0 + 1e-3, "{}", d);
        let landed = wrap_deg(from + d);
        let want = wrap_deg(to);
        let err = shortest_delta_deg(landed, want).abs();
        prop_assert!(err < 0.05, "{} vs {}", landed, want);
    }

    /// For any scores and steps, reels settle on the score's digits along
    /// the shortest path, and every transform stays finite and bounded.
    #[test]
    fn reels_settle_on_the_score(
        scores in proptest::collection::vec(any::<u64>(), 1..4),
        rate in prop_oneof![Just(f32::NAN), 1.0f32..5000.0],
        steps in proptest::collection::vec(-0.01f32..0.05, 1..40),
    ) {
        let mut rig = ToyRig::new();
        for place in 0..4 {
            rig.add_reel(ScoreReel { player: place as usize % scores.len(), ..reel(place, rate) });
        }
        let before: Vec<f32> = (0..4).map(|i| rig.reel_angle(i)).collect();
        rig.set_scores(&scores);
        let mut travelled = [0.0f32; 4];
        for dt in steps {
            let prev: Vec<f32> = (0..4).map(|i| rig.reel_angle(i)).collect();
            rig.advance(dt);
            for i in 0..4 {
                let a = rig.reel_angle(i);
                prop_assert!((0.0..360.0).contains(&a));
                travelled[i] += shortest_delta_deg(prev[i], a).abs();
                let m = rig.reel_transform(i);
                prop_assert!(m.iter().flatten().all(|v| v.is_finite() && v.abs() <= 2.0));
            }
        }
        let pre_final: Vec<f32> = (0..4).map(|i| rig.reel_angle(i)).collect();
        rig.advance(1.0e3);
        for i in 0..4u32 {
            let s = scores[i as usize % scores.len()];
            prop_assert_eq!(digit_at(rig.reel_angle(i as usize)), score_digit(s, i));
            let total = travelled[i as usize]
                + shortest_delta_deg(pre_final[i as usize], rig.reel_angle(i as usize)).abs();
            let shortest = shortest_delta_deg(before[i as usize], reel_angle_deg(score_digit(s, i))).abs();
            prop_assert!(total <= shortest + 1e-2, "{} > {}", total, shortest);
        }
        prop_assert!(rig.settled());
    }

    /// Channel values stay in range and transforms finite for any inputs.
    #[test]
    fn channels_stay_bounded(
        lo in -1.0e3f32..1.0e3,
        span in 0.0f32..1.0e3,
        tau in prop_oneof![Just(0.0f32), Just(f32::INFINITY), 1.0e-4f32..10.0],
        targets in proptest::collection::vec(prop_oneof![Just(f32::NAN), -1.0e6f32..1.0e6], 1..20),
        rotate in any::<bool>(),
    ) {
        let mut rig = ToyRig::new();
        let motion = if rotate {
            MechMotion::Rotate { axis: [0.3, -0.2, 0.9], pivot: [0.1, 0.2, 0.3] }
        } else {
            MechMotion::Translate { axis: [0.0, 1.0, 0.0] }
        };
        let i = rig.add_channel(MechChannel {
            meshes: vec!["m".into()],
            motion,
            range: (lo, lo + span),
            time_constant_s: tau,
        });
        for t in targets {
            rig.set_channel(i, t);
            rig.advance(0.004);
            let v = rig.channel_value(i);
            prop_assert!(v >= lo && v <= lo + span, "{} outside", v);
            let m = rig.channel_transform(i);
            prop_assert!(m.iter().flatten().all(|x| x.is_finite()));
        }
    }

    /// `rotation_about` is a rigid motion: lengths preserved, pivot fixed.
    #[test]
    fn rotation_is_rigid(
        axis in proptest::array::uniform3(-1.0f32..1.0),
        pivot in proptest::array::uniform3(-1.0f32..1.0),
        p in proptest::array::uniform3(-1.0f32..1.0),
        angle in -10.0f32..10.0,
    ) {
        let m = rotation_about(axis, pivot, angle);
        let q = transform_point(&m, pivot);
        for k in 0..3 {
            prop_assert!((q[k] - pivot[k]).abs() < 1e-4);
        }
        let d0: f32 = (0..3).map(|k| (p[k] - pivot[k]).powi(2)).sum::<f32>().sqrt();
        let r = transform_point(&m, p);
        let d1: f32 = (0..3).map(|k| (r[k] - pivot[k]).powi(2)).sum::<f32>().sqrt();
        prop_assert!((d0 - d1).abs() < 1e-4);
        prop_assert!(mat_close(&m, &m, 0.0));
    }
}
