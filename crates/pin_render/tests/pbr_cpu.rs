//! GPU-free parts of 5.1: conservative clustering, light-list capacity,
//! warm-up curves and the material table (property tests).

use pin_render::pbr::{
    ClusterAssignment, ClusterGrid, DEFAULT_CLUSTER_DIMS, LampBank, LampKind, LampLight,
    LampWarmup, LightList, MAX_LIGHTS, MaterialKind, PbrCamera, PointLight, assign_lights,
    build_light_list,
};
use proptest::prelude::*;

fn v3(r: f32) -> impl Strategy<Value = [f32; 3]> {
    [-r..r, -r..r, -r..r]
}

fn light() -> impl Strategy<Value = PointLight> {
    (v3(2.0), 0.01f32..1.5, 0.01f32..5.0)
        .prop_map(|(p, range, i)| PointLight::new(p, [1.0, 0.8, 0.6], i, range))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Verification 3: every light that reaches a point (distance < range)
    /// is in the list of the point's cluster; no list exceeds MAX_LIGHTS;
    /// lights past MAX_LIGHTS are counted, never assigned.
    #[test]
    fn assignment_is_conservative(
        eye in v3(3.0),
        target in v3(0.5),
        fov in 0.3f32..2.0,
        aspect in 0.5f32..2.5,
        near in 0.01f32..0.3,
        far in 3.0f32..30.0,
        dims in [1u32..20, 1u32..12, 1u32..30],
        lights in proptest::collection::vec(light(), 0..140),
        points in proptest::collection::vec(v3(2.5), 1..60),
    ) {
        let cam = PbrCamera::look_at(eye, target, [0.0, 0.0, 1.0], fov, aspect, near, far);
        let grid = ClusterGrid::new(dims, cam.fov_y, cam.aspect, cam.near, cam.far);
        let mut out = ClusterAssignment::new();
        assign_lights(&grid, &cam.view, &lights, &mut out);
        prop_assert!(out.max_count() as usize <= MAX_LIGHTS);
        prop_assert_eq!(out.ignored() as usize, lights.len().saturating_sub(MAX_LIGHTS));
        for i in 0..grid.cluster_count() {
            let l = out.lights_of(i);
            prop_assert!(l.len() <= MAX_LIGHTS);
            prop_assert!(l.iter().all(|&k| (k as usize) < MAX_LIGHTS.min(lights.len())));
        }
        for p in points {
            let Some(c) = grid.cluster_of_world(&cam.view, p) else { continue };
            let list = out.lights_of(c);
            for (k, l) in lights.iter().enumerate().take(MAX_LIGHTS) {
                let d = [p[0] - l.position[0], p[1] - l.position[1], p[2] - l.position[2]];
                let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                if dist < l.range {
                    prop_assert!(list.contains(&(k as u32)), "light {} reaches {:?} but is not in cluster {}", k, p, c);
                }
            }
        }
    }

    /// Points along a light's sphere, including just inside its range in
    /// every direction (the hard case for the box test).
    #[test]
    fn assignment_covers_sphere_surface(
        l in light(),
        dirs in proptest::collection::vec(v3(1.0), 1..40),
    ) {
        let cam = PbrCamera::look_at([0.0, -4.0, 0.5], [0.0; 3], [0.0, 0.0, 1.0], 1.2, 1.6, 0.05, 20.0);
        let grid = ClusterGrid::for_camera(&cam);
        prop_assert_eq!(grid.dims(), DEFAULT_CLUSTER_DIMS);
        let mut out = ClusterAssignment::new();
        assign_lights(&grid, &cam.view, &[l], &mut out);
        for d in dirs {
            let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            if n < 1e-3 { continue; }
            let s = 0.999 * l.range / n;
            let p = [l.position[0] + d[0] * s, l.position[1] + d[1] * s, l.position[2] + d[2] * s];
            if let Some(c) = grid.cluster_of_world(&cam.view, p) {
                prop_assert!(out.lights_of(c).contains(&0));
            }
        }
    }

    #[test]
    fn light_list_never_exceeds_capacity(n in 0usize..400) {
        let mut list = LightList::new();
        for _ in 0..n {
            list.push(PointLight::new([0.0; 3], [1.0; 3], 1.0, 1.0));
        }
        prop_assert_eq!(list.len(), n.min(MAX_LIGHTS));
        prop_assert_eq!(list.rejected() as usize, n.saturating_sub(MAX_LIGHTS));
    }

    /// The emitted level stays in 0..=1, moves monotonically towards the
    /// command, and does not depend on how a span of time is split.
    #[test]
    fn warmup_invariants(
        led: bool,
        start in 0.0f32..=1.0,
        cmd in -1.0f32..2.0,
        steps in proptest::collection::vec(0.0f32..0.01, 1..30),
    ) {
        let kind = if led { LampKind::Led } else { LampKind::Incandescent };
        let mut a = LampWarmup::new(kind);
        a.reset(start);
        let target = cmd.clamp(0.0, 1.0);
        let mut prev = start;
        for &dt in &steps {
            let v = a.update(cmd, dt);
            prop_assert!((0.0..=1.0).contains(&v));
            prop_assert!((v - target).abs() <= (prev - target).abs() + 1e-6);
            prev = v;
        }
        let total: f32 = steps.iter().sum();
        let mut b = LampWarmup::new(kind);
        b.reset(start);
        let once = b.update(cmd, total);
        prop_assert!((once - prev).abs() < 1e-4, "split {} vs once {}", prev, once);
    }

    #[test]
    fn shader_names_are_exact(name in "[a-z_]{0,24}") {
        let k = MaterialKind::from_shader_name(&name);
        prop_assert_eq!(k.is_some(), MaterialKind::ALL.iter().any(|m| m.shader_name() == name));
    }
}

#[test]
fn warmup_timings_from_snapshot_ticks() {
    // 1 tick = 1 ms: an incandescent lamp switched on at tick 100 reaches
    // 95 % by tick 115 and not by tick 110; an LED by tick 101.
    let mut bank = LampBank::new();
    bank.set_kind(1, LampKind::Led);
    let mut levels = [0.0f32; pin_core::snapshot::MAX_LAMPS];
    bank.update_levels(100, &levels);
    levels[0] = 1.0;
    levels[1] = 1.0;
    let mut at = Vec::new();
    for t in 101..=115 {
        bank.update_levels(t, &levels);
        at.push((bank.level(0), bank.level(1)));
    }
    assert!(at[0].1 >= 0.95, "LED at 1 ms: {}", at[0].1);
    assert!(at[9].0 < 0.95, "incandescent at 10 ms: {}", at[9].0);
    assert!(at[14].0 >= 0.95, "incandescent at 15 ms: {}", at[14].0);
}

#[test]
fn light_list_from_lamps_overflows_with_a_count() {
    let mut bank = LampBank::new();
    let levels = [1.0f32; pin_core::snapshot::MAX_LAMPS];
    for k in 0..pin_core::snapshot::MAX_LAMPS {
        bank.set_kind(k, LampKind::Led);
    }
    bank.update_levels(0, &levels);
    bank.update_levels(50, &levels);
    let lamps: Vec<LampLight> = (0..200)
        .map(|k| LampLight {
            lamp: k,
            position: [k as f32 * 0.01, 0.0, 0.0],
            color: [1.0, 0.9, 0.7],
            intensity: 0.5,
            range: 0.2,
            kind: LampKind::Led,
            gi: k % 2 == 0,
        })
        .collect();
    let mut out = LightList::new();
    build_light_list(&lamps, &bank, [1.0, 1.0, 1.0, 1.0], &mut out);
    assert_eq!(out.len(), MAX_LIGHTS);
    assert_eq!(out.rejected(), 200 - MAX_LIGHTS as u64);
    // GI off: the GI strings cast no light, the rest fit.
    build_light_list(&lamps, &bank, [1.0, 1.0, 1.0, 0.0], &mut out);
    assert_eq!((out.len(), out.rejected()), (100, 0));
}
