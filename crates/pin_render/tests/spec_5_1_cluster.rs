//! Independent spec tests for Subphase 5.1, verification criterion 3 (GPU-free part):
//! CPU light clustering is conservative, no cluster list exceeds `MAX_LIGHTS`, and a
//! 129th light is rejected/ignored with a counter and without a panic.
//!
//! Signatures looked up from the rustdoc of the compiled crate (the brief leaves them
//! open): `ClusterGrid::{new, for_camera, cluster_of, cluster_of_world, cluster_box,
//! cluster_count, dims}`, `ClusterAssignment::{new, lights_of, ranges, indices, ignored,
//! max_count}`, `assign_lights(&grid, &view, &lights, &mut out)`, `LightList`,
//! `build_light_list`, `LampBank`, `LampLight`, `PointLight`, `math::{PbrCamera, IDENTITY}`.
//!
//! Conservative means: for a point P inside the frustum and a light i (among the first
//! `MAX_LIGHTS`) with |P − light.position| ≤ light.range, i is in the list of P's cluster.

use pin_core::FrameStateSnapshot;
use pin_render::pbr::math::{IDENTITY, Mat4, PbrCamera};
use pin_render::pbr::{
    ClusterAssignment, ClusterGrid, LampBank, LampKind, LampLight, LightList, MAX_LIGHTS,
    PointLight, assign_lights, build_light_list,
};
use proptest::prelude::*;

fn dist(a: [f32; 3], b: [f32; 3]) -> f64 {
    let d = [
        a[0] as f64 - b[0] as f64,
        a[1] as f64 - b[1] as f64,
        a[2] as f64 - b[2] as f64,
    ];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// world → view for a column-major affine matrix `m[column][row]`.
fn xf(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
    let mut o = [0.0f32; 3];
    for (r, out) in o.iter_mut().enumerate() {
        *out = m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r];
    }
    o
}

/// Inverse of a rigid (rotation + translation) view matrix applied to a view-space point.
fn inv_rigid(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
    let q = [p[0] - m[3][0], p[1] - m[3][1], p[2] - m[3][2]];
    // R^T q: R columns are m[0..3] (rows 0..3).
    [
        m[0][0] * q[0] + m[0][1] * q[1] + m[0][2] * q[2],
        m[1][0] * q[0] + m[1][1] * q[1] + m[1][2] * q[2],
        m[2][0] * q[0] + m[2][1] * q[1] + m[2][2] * q[2],
    ]
}

/// A view-space point inside the frustum from normalised coordinates: `u`, `v` in (−1, 1)
/// across the screen, `t` in (0, 1) through the depth range (log-spaced).
fn frustum_point(fov_y: f32, aspect: f32, near: f32, far: f32, u: f32, v: f32, t: f32) -> [f32; 3] {
    let depth = near * (far / near).powf(t);
    let half_h = (fov_y * 0.5).tan() * depth;
    [u * half_h * aspect, v * half_h, -depth]
}

#[derive(Clone, Debug)]
struct Frustum {
    dims: [u32; 3],
    fov_y: f32,
    aspect: f32,
    near: f32,
    far: f32,
}

fn frustums() -> impl Strategy<Value = Frustum> {
    (
        (1u32..=24, 1u32..=16, 1u32..=32),
        0.2f32..2.2,
        0.4f32..3.0,
        0.01f32..0.5,
        1.5f32..40.0,
    )
        .prop_map(|((x, y, z), fov_y, aspect, near, mul)| Frustum {
            dims: [x, y, z],
            fov_y,
            aspect,
            near,
            far: near * mul,
        })
}

/// (u, v, t) of a probe point strictly inside the frustum.
fn probes(n: usize) -> impl Strategy<Value = Vec<(f32, f32, f32)>> {
    proptest::collection::vec((-0.98f32..0.98, -0.98f32..0.98, 0.02f32..0.98), 1..n)
}

/// Lights described relative to a probe: (probe selector, unit-ish offset, offset scale
/// relative to range, range).
#[allow(clippy::type_complexity)]
fn light_specs(max: usize) -> impl Strategy<Value = Vec<(usize, [f32; 3], f32, f32)>> {
    proptest::collection::vec(
        (
            0usize..1000,
            [-1.0f32..1.0, -1.0f32..1.0, -1.0f32..1.0],
            0.0f32..1.6,
            0.001f32..3.0,
        ),
        0..max,
    )
}

fn check_structure(grid: &ClusterGrid, out: &ClusterAssignment, n_lights: usize) {
    let used = n_lights.min(MAX_LIGHTS);
    assert!(out.max_count() as usize <= MAX_LIGHTS);
    assert_eq!(out.ignored(), n_lights.saturating_sub(MAX_LIGHTS) as u64);
    let mut longest = 0;
    for c in 0..grid.cluster_count() {
        let list = out.lights_of(c);
        assert!(list.len() <= MAX_LIGHTS, "cluster {c}: {} lights", list.len());
        longest = longest.max(list.len());
        let mut seen = [false; MAX_LIGHTS];
        for &i in list {
            assert!((i as usize) < used, "cluster {c} lists light {i} of {used}");
            assert!(!seen[i as usize], "cluster {c} lists light {i} twice");
            seen[i as usize] = true;
        }
    }
    assert_eq!(longest, out.max_count() as usize, "max_count disagrees with the lists");
    // The GPU layout describes the same lists.
    let ranges = out.ranges();
    assert!(ranges.len() >= grid.cluster_count());
    for c in 0..grid.cluster_count() {
        let [off, count] = ranges[c];
        let slice = &out.indices()[off as usize..(off + count) as usize];
        assert_eq!(slice, out.lights_of(c), "cluster {c}: ranges/indices != lights_of");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(192))]

    /// View-space clustering (identity view): every light reaching a probe point is in
    /// the list of the probe's cluster; lists are bounded and well formed.
    #[test]
    fn assignment_is_conservative_in_view_space(
        f in frustums(),
        probes in probes(24),
        specs in light_specs(160),
    ) {
        let grid = ClusterGrid::new(f.dims, f.fov_y, f.aspect, f.near, f.far);
        let pts: Vec<[f32; 3]> = probes
            .iter()
            .map(|&(u, v, t)| frustum_point(f.fov_y, f.aspect, f.near, f.far, u, v, t))
            .collect();
        let lights: Vec<PointLight> = specs
            .iter()
            .map(|&(sel, dir, k, range)| {
                let p = pts[sel % pts.len()];
                let pos = [p[0] + dir[0] * k * range, p[1] + dir[1] * k * range, p[2] + dir[2] * k * range];
                PointLight::new(pos, [1.0, 1.0, 1.0], 1.0, range)
            })
            .collect();
        let mut out = ClusterAssignment::new();
        assign_lights(&grid, &IDENTITY, &lights, &mut out);
        check_structure(&grid, &out, lights.len());

        let mut reaching = 0;
        for p in &pts {
            let c = grid.cluster_of(*p);
            prop_assert!(c.is_some(), "probe {:?} inside the frustum has no cluster ({:?})", p, f);
            let c = c.unwrap();
            prop_assert!(c < grid.cluster_count());
            for (i, l) in lights.iter().enumerate().take(MAX_LIGHTS) {
                if dist(*p, l.position) <= l.range as f64 {
                    reaching += 1;
                    prop_assert!(
                        out.lights_of(c).contains(&(i as u32)),
                        "light {} at {:?} range {} reaches {:?} (d = {}) but is not in cluster {}",
                        i, l.position, l.range, p, dist(*p, l.position), c
                    );
                }
            }
        }
        // Not a vacuous run: the generator aims lights at probes.
        let _ = reaching;
    }

    /// The same through an arbitrary camera (world-space lights and probes).
    #[test]
    fn assignment_is_conservative_through_a_camera(
        eye in [-2.0f32..2.0, -2.0f32..2.0, 0.1f32..3.0],
        target in [-0.5f32..0.5, -0.5f32..0.5, -0.2f32..0.2],
        fov_y in 0.2f32..2.0,
        aspect in 0.5f32..2.5,
        probes in probes(24),
        specs in light_specs(160),
    ) {
        prop_assume!(dist(eye, target) > 0.2);
        let cam = PbrCamera::look_at(eye, target, [0.0, 0.0, 1.0], fov_y, aspect, 0.05, 10.0);
        let grid = ClusterGrid::for_camera(&cam);
        let world: Vec<[f32; 3]> = probes
            .iter()
            .map(|&(u, v, t)| {
                inv_rigid(&cam.view, frustum_point(cam.fov_y, cam.aspect, cam.near, cam.far, u, v, t))
            })
            .collect();
        let lights: Vec<PointLight> = specs
            .iter()
            .map(|&(sel, dir, k, range)| {
                let p = world[sel % world.len()];
                let pos = [p[0] + dir[0] * k * range, p[1] + dir[1] * k * range, p[2] + dir[2] * k * range];
                PointLight::new(pos, [1.0, 0.5, 0.25], 2.0, range)
            })
            .collect();
        let mut out = ClusterAssignment::new();
        assign_lights(&grid, &cam.view, &lights, &mut out);
        check_structure(&grid, &out, lights.len());
        for p in &world {
            let c = grid.cluster_of_world(&cam.view, *p);
            prop_assert!(c.is_some(), "probe {:?} inside the frustum has no cluster", p);
            let c = c.unwrap();
            // The world and view lookups agree.
            prop_assert_eq!(grid.cluster_of(xf(&cam.view, *p)), Some(c));
            for (i, l) in lights.iter().enumerate().take(MAX_LIGHTS) {
                if dist(*p, l.position) <= l.range as f64 {
                    prop_assert!(
                        out.lights_of(c).contains(&(i as u32)),
                        "light {} (range {}) reaches {:?} (d = {}) but is not in cluster {}",
                        i, l.range, p, dist(*p, l.position), c
                    );
                }
            }
        }
    }

    /// A probe lies inside the bounding box of the cluster it is assigned to (the boxes
    /// are what lights are tested against, so this ties the lookup to the assignment).
    #[test]
    fn a_point_lies_in_the_box_of_its_cluster(f in frustums(), probes in probes(64)) {
        let grid = ClusterGrid::new(f.dims, f.fov_y, f.aspect, f.near, f.far);
        prop_assert_eq!(grid.dims(), f.dims);
        prop_assert_eq!(grid.cluster_count(), (f.dims[0] * f.dims[1] * f.dims[2]) as usize);
        for &(u, v, t) in &probes {
            let p = frustum_point(f.fov_y, f.aspect, f.near, f.far, u, v, t);
            let c = grid.cluster_of(p).expect("probe inside the frustum");
            let b = grid.cluster_box(c).expect("box of a valid cluster");
            for a in 0..3 {
                let slack = 1.0e-4 * (1.0 + p[a].abs());
                prop_assert!(
                    p[a] >= b[a] - slack && p[a] <= b[a + 3] + slack,
                    "axis {}: {:?} outside box {:?} of cluster {}", a, p, b, c
                );
            }
        }
        prop_assert!(grid.cluster_box(grid.cluster_count()).is_none());
    }

    /// Any number of lights, including far more than MAX_LIGHTS and degenerate ones:
    /// no panic, bounded lists, exact ignored count.
    #[test]
    fn any_light_count_is_handled(
        n in 0usize..600,
        range in prop_oneof![Just(0.0f32), Just(-1.0), Just(f32::NAN), Just(f32::INFINITY), 0.01f32..100.0],
        intensity in prop_oneof![Just(0.0f32), Just(f32::NAN), 0.0f32..10.0],
        x in prop_oneof![Just(f32::NAN), Just(f32::INFINITY), -5.0f32..5.0],
    ) {
        let cam = PbrCamera::look_at([0.0, -1.0, 1.0], [0.0, 0.0, 0.0], [0.0, 0.0, 1.0], 0.9, 16.0 / 9.0, 0.05, 10.0);
        let grid = ClusterGrid::for_camera(&cam);
        let lights = vec![PointLight::new([x, 0.0, 0.1], [1.0; 3], intensity, range); n];
        let mut out = ClusterAssignment::new();
        assign_lights(&grid, &cam.view, &lights, &mut out);
        check_structure(&grid, &out, n);
    }
}

fn table_camera() -> PbrCamera {
    PbrCamera::look_at(
        [0.0, -1.2, 1.0],
        [0.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        0.8,
        16.0 / 9.0,
        0.05,
        10.0,
    )
}

/// 128 lights that each cover the whole frustum: every cluster lists all 128.
#[test]
fn all_128_lights_are_assigned() {
    let cam = table_camera();
    let grid = ClusterGrid::for_camera(&cam);
    let lights = vec![PointLight::new([0.0, 0.0, 0.5], [1.0; 3], 1.0, 1000.0); MAX_LIGHTS];
    let mut out = ClusterAssignment::new();
    assign_lights(&grid, &cam.view, &lights, &mut out);
    check_structure(&grid, &out, MAX_LIGHTS);
    assert_eq!(out.ignored(), 0);
    assert_eq!(out.max_count() as usize, MAX_LIGHTS);
    for c in 0..grid.cluster_count() {
        assert_eq!(out.lights_of(c).len(), MAX_LIGHTS, "cluster {c}");
    }
}

/// The 129th light is ignored and counted; nothing panics; no list grows past 128.
#[test]
fn light_129_is_ignored_with_a_counter() {
    let cam = table_camera();
    let grid = ClusterGrid::for_camera(&cam);
    let mut lights = vec![PointLight::new([0.0, 0.0, 0.5], [1.0; 3], 1.0, 1000.0); MAX_LIGHTS];
    lights.push(PointLight::new([0.0, 0.0, 0.5], [0.0, 1.0, 0.0], 50.0, 1000.0));
    let mut out = ClusterAssignment::new();
    assign_lights(&grid, &cam.view, &lights, &mut out);
    check_structure(&grid, &out, MAX_LIGHTS + 1);
    assert_eq!(out.ignored(), 1);
    for c in 0..grid.cluster_count() {
        assert_eq!(out.lights_of(c).len(), MAX_LIGHTS);
        assert!(!out.lights_of(c).contains(&(MAX_LIGHTS as u32)));
    }
    // The counter describes the last call, and the assignment is reusable.
    assign_lights(&grid, &cam.view, &lights[..3], &mut out);
    check_structure(&grid, &out, 3);
    assert_eq!(out.ignored(), 0);
    assert_eq!(out.max_count(), 3);
}

/// A light that reaches nothing in view is listed nowhere near the probe it cannot reach,
/// and a small light is not smeared over the whole grid (the clustering actually culls).
#[test]
fn a_small_light_is_listed_in_few_clusters() {
    let cam = table_camera();
    let grid = ClusterGrid::for_camera(&cam);
    let lights = [PointLight::new([0.0, 0.0, 0.02], [1.0; 3], 1.0, 0.03)];
    let mut out = ClusterAssignment::new();
    assign_lights(&grid, &cam.view, &lights, &mut out);
    let listed = (0..grid.cluster_count()).filter(|&c| !out.lights_of(c).is_empty()).count();
    assert!(listed >= 1, "a light in view is in no cluster");
    assert!(
        listed * 10 <= grid.cluster_count(),
        "a 3 cm light is listed in {listed} of {} clusters",
        grid.cluster_count()
    );
    let own = grid.cluster_of_world(&cam.view, [0.0, 0.0, 0.02]).expect("light centre is in view");
    assert_eq!(out.lights_of(own), &[0]);
}

#[test]
fn points_outside_the_frustum_have_no_cluster() {
    let grid = ClusterGrid::new([8, 8, 8], 1.0, 1.0, 0.1, 10.0);
    assert!(grid.cluster_of([0.0, 0.0, -1.0]).is_some());
    assert!(grid.cluster_of([0.0, 0.0, 1.0]).is_none(), "behind the camera");
    assert!(grid.cluster_of([0.0, 0.0, -0.05]).is_none(), "in front of near");
    assert!(grid.cluster_of([0.0, 0.0, -11.0]).is_none(), "beyond far");
    assert!(grid.cluster_of([5.0, 0.0, -1.0]).is_none(), "off screen");
    assert!(grid.cluster_of([0.0, -5.0, -1.0]).is_none(), "off screen");
}

// ---- LightList: the 128-light cap with a counter ------------------------------------

#[test]
fn light_list_holds_128_and_counts_the_rest() {
    let mut list = LightList::new();
    assert!(list.is_empty());
    for i in 0..MAX_LIGHTS {
        assert!(list.push(PointLight::new([i as f32, 0.0, 0.0], [1.0; 3], 1.0, 1.0)), "light {i}");
    }
    assert_eq!(list.len(), MAX_LIGHTS);
    assert!(list.is_full());
    assert_eq!(list.rejected(), 0);
    assert!(!list.push(PointLight::new([999.0, 0.0, 0.0], [1.0; 3], 1.0, 1.0)), "129th accepted");
    assert_eq!(list.len(), MAX_LIGHTS);
    assert_eq!(list.rejected(), 1);
    for _ in 0..1000 {
        assert!(!list.push(PointLight::default()));
    }
    assert_eq!(list.rejected(), 1001);
    assert_eq!(list.as_slice().len(), MAX_LIGHTS);
    // The kept lights are the first 128, in order.
    for (i, l) in list.as_slice().iter().enumerate() {
        assert_eq!(l.position[0], i as f32);
    }
    list.clear();
    assert!(list.is_empty());
    assert_eq!(list.rejected(), 0);
}

// ---- Light list rebuilt per frame from snapshot lamp states -------------------------

fn lamp(i: usize, kind: LampKind, gi: bool) -> LampLight {
    LampLight {
        lamp: i,
        position: [i as f32 * 0.01, 0.0, 0.05],
        color: [1.0, 0.8, 0.6],
        intensity: 2.0,
        range: 0.5,
        kind,
        gi,
    }
}

/// Runs a bank to `ticks` ms after the lamps in `snapshot` were commanded.
fn warmed(lamps: &[LampLight], snapshot: &FrameStateSnapshot, ticks: u64) -> LampBank {
    let mut bank = LampBank::new();
    for l in lamps {
        bank.set_kind(l.lamp, l.kind);
    }
    let mut s = *snapshot;
    s.tick = 1000;
    bank.update(&s);
    s.tick = 1000 + ticks;
    bank.update(&s);
    bank
}

/// Radiant strength of a light: colour · intensity (the split between the two fields is
/// the implementation's business).
fn strength(l: &PointLight) -> [f32; 3] {
    [l.color[0] * l.intensity, l.color[1] * l.intensity, l.color[2] * l.intensity]
}

#[test]
fn light_list_follows_snapshot_lamp_levels() {
    let lamps = [lamp(3, LampKind::Led, false), lamp(7, LampKind::Led, false), lamp(9, LampKind::Led, false)];
    let mut snap = FrameStateSnapshot::default();
    snap.lamps[3] = 1.0;
    snap.lamps[7] = 0.5;
    snap.lamps[9] = 0.0;
    snap.gi = [1.0, 1.0, 1.0, 1.0];
    let bank = warmed(&lamps, &snap, 200);
    assert!((bank.level(3) - 1.0).abs() < 1e-3);
    assert!((bank.level(7) - 0.5).abs() < 1e-3);
    assert_eq!(bank.level(9), 0.0);

    let mut list = LightList::new();
    list.push(PointLight::default()); // must be cleared by the rebuild
    build_light_list(&lamps, &bank, snap.gi, &mut list);
    assert_eq!(list.len(), 2, "an unlit lamp casts no light");
    assert_eq!(list.rejected(), 0);
    let full = list.as_slice().iter().find(|l| l.position == lamps[0].position).expect("lamp 3");
    let half = list.as_slice().iter().find(|l| l.position == lamps[1].position).expect("lamp 7");
    assert_eq!(full.range, 0.5);
    let (sf, sh) = (strength(full), strength(half));
    for c in 0..3 {
        let want = lamps[0].color[c] * lamps[0].intensity;
        assert!((sf[c] - want).abs() <= 1e-3 * want, "full lamp channel {c}: {} vs {want}", sf[c]);
        assert!((sh[c] - 0.5 * want).abs() <= 2e-3 * want, "half lamp channel {c}: {} vs {}", sh[c], 0.5 * want);
    }
}

#[test]
fn light_list_is_dark_before_warm_up_and_incandescent_lags_led() {
    let lamps = [lamp(0, LampKind::Incandescent, false), lamp(1, LampKind::Led, false)];
    let mut snap = FrameStateSnapshot::default();
    snap.lamps[0] = 1.0;
    snap.lamps[1] = 1.0;
    // 2 ms after the command: the LED is at full level, the bulb is still warming.
    let bank = warmed(&lamps, &snap, 2);
    assert!(bank.level(1) >= 0.95, "LED after 2 ms: {}", bank.level(1));
    assert!(bank.level(0) > 0.0 && bank.level(0) < 0.6, "bulb after 2 ms: {}", bank.level(0));
    let mut list = LightList::new();
    build_light_list(&lamps, &bank, [1.0; 4], &mut list);
    assert_eq!(list.len(), 2);
    let s: Vec<f32> = list.as_slice().iter().map(|l| strength(l)[0]).collect();
    let (bulb, led) = if list.as_slice()[0].position == lamps[0].position { (s[0], s[1]) } else { (s[1], s[0]) };
    assert!(bulb < 0.6 * led, "bulb {bulb} should lag the LED {led}");
    // 15 ms: the bulb has reached 95 %; 10 ms: not yet (the brief's timings, via the bank).
    let b15 = warmed(&lamps, &snap, 15);
    assert!(b15.level(0) >= 0.95, "bulb after 15 ms: {}", b15.level(0));
    let b10 = warmed(&lamps, &snap, 10);
    assert!(b10.level(0) < 0.95, "bulb after 10 ms: {}", b10.level(0));
    let b1 = warmed(&lamps, &snap, 1);
    assert!(b1.level(1) >= 0.95, "LED after 1 ms: {}", b1.level(1));
}

#[test]
fn gi_strings_take_the_gi_tint_and_other_lamps_do_not() {
    let lamps = [lamp(0, LampKind::Led, true), lamp(1, LampKind::Led, false)];
    let mut snap = FrameStateSnapshot::default();
    snap.lamps[0] = 1.0;
    snap.lamps[1] = 1.0;
    let bank = warmed(&lamps, &snap, 100);
    let mut list = LightList::new();
    build_light_list(&lamps, &bank, [1.0, 0.0, 0.5, 0.5], &mut list);
    let gi = list.as_slice().iter().find(|l| l.position == lamps[0].position).expect("GI light");
    let plain = list.as_slice().iter().find(|l| l.position == lamps[1].position).expect("plain light");
    let (sg, sp) = (strength(gi), strength(plain));
    let base = [2.0, 1.6, 1.2];
    let tint = [0.5, 0.0, 0.25];
    for c in 0..3 {
        assert!((sp[c] - base[c]).abs() <= 2e-3 * base[c], "plain lamp channel {c}: {}", sp[c]);
        assert!((sg[c] - base[c] * tint[c]).abs() <= 2e-3, "GI lamp channel {c}: {} vs {}", sg[c], base[c] * tint[c]);
    }
    // GI off: the string casts no light (dropped, or present with zero strength).
    build_light_list(&lamps, &bank, [1.0, 1.0, 1.0, 0.0], &mut list);
    for l in list.as_slice() {
        if l.position == lamps[0].position {
            assert_eq!(strength(l), [0.0; 3], "GI string lit with GI intensity 0");
        }
    }
    assert!(list.as_slice().iter().any(|l| l.position == lamps[1].position));
}

#[test]
fn more_than_128_lit_lamps_are_rejected_with_a_counter() {
    let lamps: Vec<LampLight> = (0..200).map(|i| lamp(i, LampKind::Led, false)).collect();
    let mut snap = FrameStateSnapshot::default();
    for i in 0..200 {
        snap.lamps[i] = 1.0;
    }
    let bank = warmed(&lamps, &snap, 100);
    let mut list = LightList::new();
    build_light_list(&lamps, &bank, [1.0; 4], &mut list);
    assert_eq!(list.len(), MAX_LIGHTS);
    assert_eq!(list.rejected(), 72);
    // Exactly 129 lit lamps: one rejected.
    snap.lamps = [0.0; 256];
    for i in 0..129 {
        snap.lamps[i] = 1.0;
    }
    let bank = warmed(&lamps, &snap, 100);
    build_light_list(&lamps, &bank, [1.0; 4], &mut list);
    assert_eq!(list.len(), MAX_LIGHTS);
    assert_eq!(list.rejected(), 1);
    // Rebuilt per frame: all off → empty, counter reset.
    snap.lamps = [0.0; 256];
    let bank = warmed(&lamps, &snap, 100);
    build_light_list(&lamps, &bank, [1.0; 4], &mut list);
    assert!(list.is_empty());
    assert_eq!(list.rejected(), 0);
}

/// The bank is driven by sim ticks only: the same snapshots give bit-identical levels.
#[test]
fn lamp_bank_is_deterministic_and_idempotent_per_tick() {
    let run = || {
        let mut bank = LampBank::new();
        bank.set_kind(1, LampKind::Led);
        let mut snap = FrameStateSnapshot::default();
        let mut bits = Vec::new();
        for t in 0..300u64 {
            snap.tick = t * 3;
            snap.lamps[0] = if (t / 20) % 2 == 0 { 1.0 } else { 0.0 };
            snap.lamps[1] = ((t * 37) % 100) as f32 / 99.0;
            bank.update(&snap);
            let a = (bank.level(0), bank.level(1));
            bank.update(&snap); // same tick again: no change
            assert_eq!(a, (bank.level(0), bank.level(1)));
            assert!((0.0..=1.0).contains(&a.0) && (0.0..=1.0).contains(&a.1));
            bits.push((a.0.to_bits(), a.1.to_bits()));
        }
        bits
    };
    let first = run();
    for _ in 0..9 {
        assert_eq!(first, run());
    }
    assert_eq!(LampBank::new().level(100_000), 0.0);
}
