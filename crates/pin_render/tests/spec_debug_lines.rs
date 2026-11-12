//! Independent spec tests for the CPU side of `pin_render::debug` (Subphase 1.4 FIXED API):
//! `DebugLine`, `DebugLines`, `snapshot_lines`, `box_lines`, `polyline_lines`, `Camera`.
//! No GPU is needed here.

use pin_core::snapshot::{BallState, FlipperState};
use pin_core::{FlipperId, FrameStateSnapshot, ProfileId};
use pin_render::debug::{Camera, DebugLine, DebugLines, box_lines, polyline_lines, snapshot_lines};
use proptest::prelude::*;

const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const TEAL: [f32; 4] = [0.0, 0.5, 0.25, 1.0];

fn line(i: u32) -> DebugLine {
    DebugLine {
        a: [i as f32, 0.0, 0.0],
        b: [i as f32, 1.0, 0.0],
        color: RED,
    }
}

// ---------------------------------------------------------------------------------------
// DebugLine
// ---------------------------------------------------------------------------------------

#[test]
fn debug_line_is_a_40_byte_pod_in_field_order() {
    assert_eq!(std::mem::size_of::<DebugLine>(), 40);
    assert_eq!(std::mem::align_of::<DebugLine>(), 4);
    let l = DebugLine {
        a: [1.0, 2.0, 3.0],
        b: [4.0, 5.0, 6.0],
        color: [7.0, 8.0, 9.0, 10.0],
    };
    let bytes: &[u8] = bytemuck::bytes_of(&l);
    assert_eq!(bytes.len(), 40);
    let floats: &[f32] = bytemuck::cast_slice(bytes);
    assert_eq!(floats, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0]);

    let z: DebugLine = bytemuck::Zeroable::zeroed();
    assert_eq!(
        z,
        DebugLine {
            a: [0.0; 3],
            b: [0.0; 3],
            color: [0.0; 4]
        }
    );
    // Copy, Clone, Debug, PartialEq.
    let copy = l;
    #[allow(clippy::clone_on_copy)]
    let clone = l.clone();
    assert_eq!(copy, clone);
    assert!(format!("{l:?}").contains("DebugLine"));
}

// ---------------------------------------------------------------------------------------
// DebugLines
// ---------------------------------------------------------------------------------------

#[test]
fn debug_lines_starts_empty() {
    let lines = DebugLines::new(8);
    assert!(lines.as_slice().is_empty());
    assert_eq!(lines.dropped() as u64, 0);
}

#[test]
fn push_keeps_order_until_capacity_then_drops_and_counts() {
    let mut lines = DebugLines::new(5);
    for i in 0..5 {
        assert!(lines.push(line(i)), "push {i} fits");
        assert_eq!(lines.as_slice().len(), i as usize + 1);
    }
    assert_eq!(lines.dropped() as u64, 0);
    for extra in 1..=7u64 {
        assert!(!lines.push(line(100)), "push beyond capacity is refused");
        assert_eq!(lines.dropped() as u64, extra);
        assert_eq!(lines.as_slice().len(), 5);
    }
    let want: Vec<DebugLine> = (0..5).map(line).collect();
    assert_eq!(
        lines.as_slice(),
        &want[..],
        "stored lines are untouched by drops"
    );
}

#[test]
fn clear_empties_the_list_and_capacity_is_reusable() {
    let mut lines = DebugLines::new(3);
    for i in 0..3 {
        assert!(lines.push(line(i)));
    }
    assert!(!lines.push(line(3)));
    lines.clear();
    assert!(lines.as_slice().is_empty());
    for i in 10..13 {
        assert!(
            lines.push(line(i)),
            "capacity is available again after clear"
        );
    }
    assert!(!lines.push(line(13)));
    assert_eq!(lines.as_slice()[0], line(10));
    assert_eq!(lines.as_slice()[2], line(12));
}

#[test]
fn zero_capacity_list_drops_everything() {
    let mut lines = DebugLines::new(0);
    assert!(!lines.push(line(0)));
    assert!(!lines.push(line(1)));
    assert!(lines.as_slice().is_empty());
    assert_eq!(lines.dropped() as u64, 2);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn prop_len_is_min_of_pushes_and_capacity(cap in 0usize..200, pushes in 0usize..400) {
        let mut lines = DebugLines::new(cap);
        let mut accepted = 0usize;
        for i in 0..pushes {
            if lines.push(line(i as u32)) {
                accepted += 1;
            }
        }
        prop_assert_eq!(accepted, pushes.min(cap));
        prop_assert_eq!(lines.as_slice().len(), pushes.min(cap));
        prop_assert_eq!(lines.dropped() as u64, pushes.saturating_sub(cap) as u64);
        for (i, l) in lines.as_slice().iter().enumerate() {
            prop_assert_eq!(*l, line(i as u32));
        }
    }
}

// ---------------------------------------------------------------------------------------
// box_lines
// ---------------------------------------------------------------------------------------

fn key(p: [f32; 3]) -> [u32; 3] {
    [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()]
}

fn check_box(min: [f32; 3], max: [f32; 3], lines: &[DebugLine], color: [f32; 4]) {
    assert_eq!(lines.len(), 12, "a box has 12 edges");
    let mut corner_use = std::collections::HashMap::new();
    let mut per_axis = [0usize; 3];
    let mut edges = std::collections::HashSet::new();
    for l in lines {
        assert_eq!(l.color, color);
        for p in [l.a, l.b] {
            for (axis, &v) in p.iter().enumerate() {
                assert!(
                    v == min[axis] || v == max[axis],
                    "endpoint {p:?} is not a corner of {min:?}..{max:?}"
                );
            }
            *corner_use.entry(key(p)).or_insert(0usize) += 1;
        }
        let differing: Vec<usize> = (0..3).filter(|&i| l.a[i] != l.b[i]).collect();
        assert_eq!(
            differing.len(),
            1,
            "edge {l:?} must run along exactly one axis"
        );
        per_axis[differing[0]] += 1;
        let (ka, kb) = (key(l.a), key(l.b));
        assert!(
            edges.insert(if ka <= kb { (ka, kb) } else { (kb, ka) }),
            "duplicate edge {l:?}"
        );
    }
    assert_eq!(per_axis, [4, 4, 4], "four edges along each axis");
    assert_eq!(corner_use.len(), 8, "all eight corners are used");
    assert!(
        corner_use.values().all(|&n| n == 3),
        "three edges meet at each corner"
    );
}

#[test]
fn box_lines_emits_the_twelve_edges() {
    let mut out = DebugLines::new(64);
    box_lines([0.0, 0.0, 0.0], [1.0, 2.0, 3.0], TEAL, &mut out);
    check_box([0.0, 0.0, 0.0], [1.0, 2.0, 3.0], out.as_slice(), TEAL);
    assert_eq!(out.dropped() as u64, 0);
}

#[test]
fn box_lines_overflow_drops_and_counts() {
    let mut out = DebugLines::new(5);
    box_lines([0.0; 3], [1.0; 3], RED, &mut out);
    assert_eq!(out.as_slice().len(), 5);
    assert_eq!(out.dropped() as u64, 7);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn prop_box_lines_are_the_edges_of_the_box(
        min in prop::array::uniform3(-10.0f32..10.0),
        size in prop::array::uniform3(0.001f32..5.0),
        color in prop::array::uniform4(0.0f32..=1.0),
    ) {
        let max = [min[0] + size[0], min[1] + size[1], min[2] + size[2]];
        prop_assume!((0..3).all(|i| max[i] > min[i]));
        let mut out = DebugLines::new(12);
        box_lines(min, max, color, &mut out);
        check_box(min, max, out.as_slice(), color);
        prop_assert_eq!(out.dropped() as u64, 0);
    }
}

// ---------------------------------------------------------------------------------------
// polyline_lines
// ---------------------------------------------------------------------------------------

#[test]
fn polyline_of_fewer_than_two_points_emits_nothing() {
    let mut out = DebugLines::new(8);
    polyline_lines(&[], RED, &mut out);
    polyline_lines(&[[1.0, 2.0, 3.0]], RED, &mut out);
    assert!(out.as_slice().is_empty());
    assert_eq!(out.dropped() as u64, 0);
}

#[test]
fn polyline_overflow_drops_and_counts() {
    let pts: Vec<[f32; 3]> = (0..11).map(|i| [i as f32, 0.0, 0.0]).collect();
    let mut out = DebugLines::new(4);
    polyline_lines(&pts, RED, &mut out);
    assert_eq!(out.as_slice().len(), 4);
    assert_eq!(out.dropped() as u64, 6);
    assert_eq!(out.as_slice()[3].a, [3.0, 0.0, 0.0]);
    assert_eq!(out.as_slice()[3].b, [4.0, 0.0, 0.0]);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn prop_polyline_emits_n_minus_one_connected_segments(
        pts in prop::collection::vec(prop::array::uniform3(-100.0f32..100.0), 0..40),
        color in prop::array::uniform4(0.0f32..=1.0),
    ) {
        let mut out = DebugLines::new(64);
        polyline_lines(&pts, color, &mut out);
        let got = out.as_slice();
        prop_assert_eq!(got.len(), pts.len().saturating_sub(1));
        for (i, l) in got.iter().enumerate() {
            prop_assert_eq!(l.a, pts[i]);
            prop_assert_eq!(l.b, pts[i + 1]);
            prop_assert_eq!(l.color, color);
        }
    }
}

// ---------------------------------------------------------------------------------------
// snapshot_lines
// ---------------------------------------------------------------------------------------

fn ball(x: f32, y: f32, z: f32, profile: u16) -> BallState {
    BallState {
        position_m: [x, y, z],
        profile: ProfileId(profile),
        ..BallState::default()
    }
}

/// Checks that `segs` is one closed 16-gon of circumradius `r` around `c`, in the plane
/// z = c.z, parallel to world XY.
fn check_circle(segs: &[DebugLine], c: [f32; 3], r: f32) {
    assert_eq!(segs.len(), 16, "a ball is a circle of 16 segments");
    let tol = 1e-4 * r.max(1.0) + 1e-6;
    let mut angles = Vec::new();
    for s in segs {
        for p in [s.a, s.b] {
            assert!(
                (p[2] - c[2]).abs() <= tol,
                "circle must lie in the XY plane at ball z"
            );
            let d = ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2)).sqrt();
            assert!(
                (d - r).abs() <= tol,
                "endpoint {p:?} is {d} from the centre, expected radius {r}"
            );
        }
        let chord = ((s.a[0] - s.b[0]).powi(2) + (s.a[1] - s.b[1]).powi(2)).sqrt();
        let want = 2.0 * r * (std::f32::consts::PI / 16.0).sin();
        assert!(
            (chord - want).abs() <= 2.0 * tol,
            "segment length {chord}, expected a regular 16-gon side {want}"
        );
        angles.push((s.a[1] - c[1]).atan2(s.a[0] - c[0]));
    }
    // Closed loop without gaps: every segment end is (within tolerance) another segment's start.
    for s in segs {
        let hits = segs
            .iter()
            .filter(|o| (0..3).all(|i| (o.a[i] - s.b[i]).abs() <= 2.0 * tol))
            .count();
        assert_eq!(
            hits, 1,
            "segment end {:?} must meet exactly one segment start",
            s.b
        );
    }
    // The 16 start points are evenly spread over the full turn.
    angles.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let step = std::f32::consts::TAU / 16.0;
    for w in angles.windows(2) {
        assert!((w[1] - w[0] - step).abs() < 1e-3, "uneven vertex spacing");
    }
}

#[test]
fn an_empty_snapshot_draws_nothing() {
    let mut out = DebugLines::new(64);
    snapshot_lines(&FrameStateSnapshot::default(), &|_| 0.0135, &mut out);
    assert!(out.as_slice().is_empty());
}

#[test]
fn one_ball_is_a_16_segment_circle_at_its_position() {
    let mut s = FrameStateSnapshot::default();
    s.push_ball(ball(0.25, 0.6, 0.0135, 0));
    let mut out = DebugLines::new(64);
    snapshot_lines(&s, &|_| 0.0135, &mut out);
    check_circle(out.as_slice(), [0.25, 0.6, 0.0135], 0.0135);
    assert_eq!(out.dropped() as u64, 0);
}

#[test]
fn each_ball_uses_the_radius_of_its_profile() {
    let mut s = FrameStateSnapshot::default();
    s.push_ball(ball(0.1, 0.2, 0.01, 3));
    s.push_ball(ball(0.4, 0.9, 0.02, 7));
    s.push_ball(ball(-0.3, 0.5, 0.0, 3));
    let radii = |p: ProfileId| match p.0 {
        3 => 0.0135f32,
        7 => 0.05,
        other => panic!("radius asked for a profile not in the snapshot: {other}"),
    };
    let mut out = DebugLines::new(256);
    snapshot_lines(&s, &radii, &mut out);
    let got = out.as_slice();
    assert_eq!(got.len(), 48);
    // Lines are emitted ball by ball, in snapshot order.
    check_circle(&got[0..16], [0.1, 0.2, 0.01], 0.0135);
    check_circle(&got[16..32], [0.4, 0.9, 0.02], 0.05);
    check_circle(&got[32..48], [-0.3, 0.5, 0.0], 0.0135);
}

#[test]
fn only_live_balls_are_drawn_and_flippers_add_nothing() {
    let mut s = FrameStateSnapshot::default();
    // Stale data beyond ball_count must be ignored.
    s.balls[5] = ball(9.0, 9.0, 9.0, 1);
    for i in 0..4 {
        s.push_flipper(FlipperState {
            id: FlipperId(i),
            angle_rad: 0.3 * f32::from(i),
            angular_velocity_rad_s: 1.0,
            zipper_offset_m: 0.0,
        });
    }
    let mut out = DebugLines::new(256);
    snapshot_lines(&s, &|_| 0.0135, &mut out);
    assert!(
        out.as_slice().is_empty(),
        "no balls: flippers carry no position in the snapshot, so nothing is drawn"
    );

    s.push_ball(ball(0.5, 0.5, 0.0135, 0));
    snapshot_lines(&s, &|_| 0.0135, &mut out);
    assert_eq!(out.as_slice().len(), 16);
}

#[test]
fn sixteen_balls_fill_exactly_256_lines_and_overflow_is_counted() {
    let mut s = FrameStateSnapshot::default();
    for i in 0..16 {
        assert!(s.push_ball(ball(0.05 * i as f32, 0.5, 0.0135, 0)));
    }
    let mut out = DebugLines::new(256);
    snapshot_lines(&s, &|_| 0.0135, &mut out);
    assert_eq!(out.as_slice().len(), 256);
    assert_eq!(out.dropped() as u64, 0);

    let mut small = DebugLines::new(20);
    snapshot_lines(&s, &|_| 0.0135, &mut small);
    assert_eq!(small.as_slice().len(), 20);
    assert_eq!(small.dropped() as u64, 236);
    check_circle(&small.as_slice()[0..16], [0.0, 0.5, 0.0135], 0.0135);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn prop_snapshot_circles(
        balls in prop::collection::vec(
            (-1.0f32..1.0, -1.0f32..2.0, 0.0f32..0.1, 0u16..4), 0..=16),
        base_radius in 0.005f32..0.05,
    ) {
        let mut s = FrameStateSnapshot::default();
        for &(x, y, z, p) in &balls {
            s.push_ball(ball(x, y, z, p));
        }
        let radius = |p: ProfileId| base_radius * (1.0 + f32::from(p.0));
        let mut out = DebugLines::new(16 * 16);
        snapshot_lines(&s, &radius, &mut out);
        prop_assert_eq!(out.as_slice().len(), 16 * balls.len());
        for (i, &(x, y, z, p)) in balls.iter().enumerate() {
            check_circle(&out.as_slice()[16 * i..16 * (i + 1)], [x, y, z], radius(ProfileId(p)));
        }
    }
}

// ---------------------------------------------------------------------------------------
// Camera::ortho_top_down
// ---------------------------------------------------------------------------------------

/// Applies `view_proj` to (x, y, z, 1). The brief fixes the field type but not whether the
/// outer index is the column (wgpu/WGSL/glam convention) or the row, so both readings are
/// offered and the tests require ONE of them to hold for every assertion.
fn project(m: &[[f32; 4]; 4], p: [f32; 3], column_major: bool) -> [f32; 4] {
    let v = [p[0], p[1], p[2], 1.0];
    let mut out = [0.0f32; 4];
    for (r, o) in out.iter_mut().enumerate() {
        for (c, vc) in v.iter().enumerate() {
            *o += if column_major { m[c][r] } else { m[r][c] } * vc;
        }
    }
    out
}

fn camera_maps_region(min: [f32; 2], max: [f32; 2], column_major: bool) -> Result<(), String> {
    let cam = Camera::ortho_top_down(min, max);
    let m = &cam.view_proj;
    let mid = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5];
    let cases = [
        ([min[0], min[1]], [-1.0f32, -1.0]), // bottom-left
        ([max[0], max[1]], [1.0, 1.0]),      // top-right: X right, Y up
        ([max[0], min[1]], [1.0, -1.0]),
        ([min[0], max[1]], [-1.0, 1.0]),
        (mid, [0.0, 0.0]),
    ];
    for z in [0.0f32, 0.0135, 0.05] {
        for (world, ndc) in cases {
            let clip = project(m, [world[0], world[1], z], column_major);
            if (clip[3] - 1.0).abs() > 1e-5 {
                return Err(format!(
                    "w = {} for {world:?} (orthographic needs w = 1)",
                    clip[3]
                ));
            }
            for i in 0..2 {
                if (clip[i] - ndc[i]).abs() > 1e-4 {
                    return Err(format!(
                        "world {world:?} z {z} → clip {clip:?}, expected xy {ndc:?}"
                    ));
                }
            }
            if !(0.0..=1.0).contains(&clip[2]) {
                return Err(format!(
                    "world z {z} → clip z {} is outside wgpu's 0..1 depth range",
                    clip[2]
                ));
            }
        }
    }
    Ok(())
}

const REGIONS: [([f32; 2], [f32; 2]); 4] = [
    ([0.0, 0.0], [1.0, 1.0]),
    ([1.0, 2.0], [3.0, 6.0]),
    ([-0.25, -0.5], [0.25, 0.75]),
    ([0.0, 0.0], [0.51, 1.07]),
];

#[test]
fn ortho_top_down_maps_min_to_bottom_left_and_max_to_top_right() {
    let mut errors = Vec::new();
    for column_major in [true, false] {
        match REGIONS
            .iter()
            .try_for_each(|&(min, max)| camera_maps_region(min, max, column_major))
        {
            Ok(()) => return,
            Err(e) => errors.push(format!("column_major={column_major}: {e}")),
        }
    }
    panic!("Camera::ortho_top_down does not map the region onto the viewport: {errors:#?}");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn prop_ortho_top_down_is_affine_and_orientation_preserving(
        min in prop::array::uniform2(-5.0f32..5.0),
        size in prop::array::uniform2(0.05f32..5.0),
        t in prop::array::uniform2(0.0f32..=1.0),
    ) {
        let max = [min[0] + size[0], min[1] + size[1]];
        // Use the convention the fixed regions established.
        let column_major = camera_maps_region([1.0, 2.0], [3.0, 6.0], true).is_ok();
        let cam = Camera::ortho_top_down(min, max);
        let world = [min[0] + t[0] * size[0], min[1] + t[1] * size[1], 0.0];
        let clip = project(&cam.view_proj, world, column_major);
        let want = [2.0 * t[0] - 1.0, 2.0 * t[1] - 1.0];
        prop_assert!((clip[0] - want[0]).abs() < 2e-3, "x: {:?} vs {:?}", clip, want);
        prop_assert!((clip[1] - want[1]).abs() < 2e-3, "y: {:?} vs {:?}", clip, want);
        prop_assert!((clip[3] - 1.0).abs() < 1e-5);
        prop_assert!((0.0..=1.0).contains(&clip[2]));
    }
}
