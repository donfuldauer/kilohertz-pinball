//! Banana bat pose, checked geometrically (work item w7pre-banana-pose; it
//! replaces the by-eye check "the bat leaves its pivot along the flipper
//! angle and the tip curls up the table").
//!
//! Everything is derived from the collision query
//! [`BatShape::closest_point`] of the bat the solver carries
//! ([`CurvedBanana::from_config`] with the default bat height), by
//! bisection and ray-marching in the bat frame in `f64`, and placed in the
//! world with the solver's own pose ([`FlipperSet::pivot_m`],
//! [`FlipperSet::bat_direction`], bat `y = normal × direction`). The shape's
//! own helpers (`tip_center`, `centerline_*`, `concave_side`) are used only
//! as cross-checks, never to derive what is checked.
//!
//! Frame: mounting normal +Z, table "up" is world +y. Checks:
//!
//! * (a) the outline leaves the pivot tangent to the flipper angle;
//! * (b) the tip is displaced up-table relative to a straight bat of the same
//!   chord on the same flipper angle, by the chord offset the authored sweep
//!   implies;
//! * (c) the curl is on bat `+y` for the left flipper and bat `−y` for the
//!   right one, flips with a negative `arc_sweep_deg`, and left and right
//!   are mirror images.
//!
//! The checks are a pure function of the probed pose
//! ([`check_pose`]); the negative controls at the end feed it wrong poses
//! (wrong concave side, rotated bat, wrong flipper side, straight bat) built
//! with the same probe and assert that it fails.
//!
//! Every sweep prints one summary line
//! (`cargo test -p pin_physics --test banana_pose -- --nocapture`).

#[path = "banana_util.rs"]
mod butil;

use butil::*;
use glam::{DVec3, Quat, Vec3};
use pin_core::{FlipperConfig, FlipperShape};
use pin_physics::ball::MovingBodies;
use pin_physics::flipper::{BatShape, FlipperSet, FlipperTuning, StraightCapsule};
use pin_physics::flipper_banana::{ConcaveSide, CurvedBanana};
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};

// --- tolerances (docs/decisions/2026-11-18-w7pre-banana-pose.md) ---------

/// World bat direction against the configured angle, degrees.
const TOL_WORLD_DIR_DEG: f64 = 0.01;
/// Outline tangent (pivot normals) against the bat axis, degrees.
const TOL_TANGENT_DEG: f64 = 0.05;
/// Symmetry of the two pivot crossings about the pivot, metres.
const TOL_PIVOT_SYM_M: f64 = 10.0e-6;
/// Farthest-point radius against `length + tube`, metres.
const TOL_TIP_RADIUS_M: f64 = 50.0e-6;
/// Farthest-point direction against `±|arc_sweep_deg| / 2`, degrees.
const TOL_TIP_DIR_DEG: f64 = 0.6;
/// Curl magnitude against `2·L·sin(|arc_sweep_deg| / 4)`, metres.
const TOL_CURL_MAG_M: f64 = 1.0e-3;
/// Left/right mirror agreement of the curl's world-y component, metres.
const TOL_MIRROR_M: f64 = 50.0e-6;

// --- probe parameters ---------------------------------------------------

/// Height at which the outline is queried: mid-bat, clear of the flat caps.
const Z_MID: f32 = 0.015;
/// Start radius of the inward ray march (beyond every bat), metres.
const RAY_START_M: f64 = 0.25;
/// Smallest inward march step, metres (only slivers thinner than this off
/// the farthest direction could be missed; the tip is hit head-on).
const MIN_STEP_M: f64 = 0.2e-3;
/// Bisection iterations on a bracket of at most `MIN_STEP_M`-ish width.
const BISECT_ITERS: usize = 40;
/// Angular scan step for the farthest point, degrees.
const SCAN_STEP_DEG: f64 = 0.1;

/// Signed outline distance of the bat at bat-frame `(x, y)`, mid-height.
fn dist<S: BatShape + ?Sized>(shape: &S, x: f64, y: f64) -> f64 {
    f64::from(
        shape
            .closest_point(Vec3::new(x as f32, y as f32, Z_MID))
            .distance,
    )
}

/// Outward unit normal of the bat at bat-frame `(x, y)`, mid-height.
fn normal_at<S: BatShape + ?Sized>(shape: &S, x: f64, y: f64) -> DVec3 {
    shape
        .closest_point(Vec3::new(x as f32, y as f32, Z_MID))
        .normal
        .as_dvec3()
}

/// Bisects `f` (inside ≤ 0, outside > 0) between `inside` and `outside`;
/// returns the outside end of the final bracket.
fn bisect(f: impl Fn(f64) -> f64, mut inside: f64, mut outside: f64) -> f64 {
    assert!(f(inside) <= 0.0, "bisect: {inside} is not inside");
    assert!(f(outside) > 0.0, "bisect: {outside} is not outside");
    for _ in 0..BISECT_ITERS {
        let mid = 0.5 * (inside + outside);
        if f(mid) <= 0.0 {
            inside = mid;
        } else {
            outside = mid;
        }
    }
    outside
}

/// Outer boundary radius of the bat along the bat-frame ray at `theta`
/// (radians) from the pivot: marched inward from far outside by the
/// (exact, outside) distance, then bisected. The pivot itself is inside, so
/// every ray has one.
fn ray_outer<S: BatShape + ?Sized>(shape: &S, theta: f64) -> f64 {
    let (s, c) = libm::sincos(theta);
    let f = |r: f64| dist(shape, r * c, r * s);
    let mut outside = RAY_START_M;
    let mut r = RAY_START_M;
    loop {
        let d = f(r);
        if d <= 0.0 {
            break;
        }
        outside = r;
        r = (r - d.max(MIN_STEP_M)).max(0.0);
    }
    bisect(f, r, outside)
}

/// What the collision query says about a bat, in its own frame.
#[derive(Copy, Clone, Debug)]
struct LocalProbe {
    /// Outline crossings on the bat-frame y axis (x = 0): above and below
    /// the pivot, metres.
    cross_plus: f64,
    cross_minus: f64,
    /// Outward normals there.
    normal_plus: DVec3,
    normal_minus: DVec3,
    /// Unit outline tangent at the pivot end, from those normals.
    tangent: DVec3,
    /// Farthest outline point from the pivot: radius (m) and direction
    /// (radians from bat +x toward bat +y).
    tip_radius: f64,
    tip_theta: f64,
}

/// Probes `shape` through [`BatShape::closest_point`] only.
fn probe_local<S: BatShape + ?Sized>(shape: &S) -> LocalProbe {
    // (a) The pivot end: crossings on the y axis at x = 0, both directions.
    let up = |y: f64| dist(shape, 0.0, y);
    let down = |y: f64| dist(shape, 0.0, -y);
    let far = (1..=64)
        .map(|k| f64::from(k) * 1.0e-3)
        .find(|&y| up(y) > 0.0 && down(y) > 0.0)
        .expect("the bat ends within 64 mm of its pivot on the y axis");
    let cross_plus = bisect(up, 0.0, far);
    let cross_minus = -bisect(down, 0.0, far);
    let normal_plus = normal_at(shape, 0.0, cross_plus);
    let normal_minus = normal_at(shape, 0.0, cross_minus);
    // The tangent is the normal turned by −90° (upper) or +90° (lower).
    let t_plus = DVec3::new(normal_plus.y, -normal_plus.x, 0.0);
    let t_minus = DVec3::new(-normal_minus.y, normal_minus.x, 0.0);
    let tangent = (t_plus + t_minus).normalize();

    // (b) The farthest point: coarse scan, then golden-section refinement.
    let steps = (360.0 / SCAN_STEP_DEG).round() as i32;
    let (mut best_theta, mut best_r) = (0.0, f64::NEG_INFINITY);
    for i in 0..steps {
        let theta = (-180.0 + f64::from(i) * SCAN_STEP_DEG).to_radians();
        let r = ray_outer(shape, theta);
        if r > best_r {
            (best_theta, best_r) = (theta, r);
        }
    }
    let g = 0.5 * (5.0f64.sqrt() - 1.0);
    let (mut a, mut b) = (
        best_theta - SCAN_STEP_DEG.to_radians(),
        best_theta + SCAN_STEP_DEG.to_radians(),
    );
    for _ in 0..60 {
        let c = b - g * (b - a);
        let d = a + g * (b - a);
        if ray_outer(shape, c) >= ray_outer(shape, d) {
            b = d;
        } else {
            a = c;
        }
    }
    let tip_theta = 0.5 * (a + b);
    let tip_radius = ray_outer(shape, tip_theta).max(best_r);
    LocalProbe {
        cross_plus,
        cross_minus,
        normal_plus,
        normal_minus,
        tangent,
        tip_radius,
        tip_theta,
    }
}

/// A probed bat placed in the world.
#[derive(Copy, Clone, Debug)]
struct Pose {
    /// World bat direction as given (the solver's `bat_direction`).
    dir: DVec3,
    /// World bat y axis, `normal × dir`.
    y_axis: DVec3,
    /// World outline tangent at the pivot.
    tangent: DVec3,
    local: LocalProbe,
    /// Banana tip centre (radius `length` in the farthest direction) minus
    /// the straight tip `pivot + length·dir`, world.
    curl: DVec3,
    /// Its bat-frame y component.
    curl_bat_y: f64,
    /// Banana tip centre, world.
    tip_center: DVec3,
}

/// Places `local` at `pivot` with bat direction `dir` on mounting normal
/// `normal`; `length` is the chord (m).
fn place(local: &LocalProbe, pivot: Vec3, dir: Vec3, normal: Vec3, length: f64) -> Pose {
    let x = dir.as_dvec3();
    let y = normal.as_dvec3().cross(x);
    let pivot = pivot.as_dvec3();
    let to_world = |v: DVec3| x * v.x + y * v.y;
    let tip_local =
        DVec3::new(libm::cos(local.tip_theta), libm::sin(local.tip_theta), 0.0) * length;
    let curl_local = tip_local - DVec3::new(length, 0.0, 0.0);
    Pose {
        dir: x,
        y_axis: y,
        tangent: to_world(local.tangent),
        local: *local,
        curl: to_world(curl_local),
        curl_bat_y: curl_local.y,
        tip_center: pivot + to_world(tip_local),
    }
}

/// What a pose must be.
#[derive(Copy, Clone, Debug)]
struct Expect {
    /// Configured flipper angle (rest or end), degrees.
    angle_deg: f64,
    /// Chord and tube radius, metres.
    length: f64,
    tube: f64,
    /// Authored `arc_sweep_deg`.
    arc_sweep_deg: f64,
    /// `+1` when the curl must be on bat +y, `−1` on bat −y.
    side_bat: f64,
    /// `+1` when the curl must be up the table (world +y), `−1` down.
    side_world: f64,
}

impl Expect {
    /// The expectation for a flipper sweeping by `sweep_deg`, at
    /// `angle_deg`: the curl is on the leading side (the sign of the
    /// sweep) for a positive `arc_sweep_deg` and on the trailing side for a
    /// negative one; up the table in the first case, down in the second.
    fn new(c: &Case, angle_deg: f64) -> Self {
        Expect {
            angle_deg,
            length: c.length_mm * 1.0e-3,
            tube: c.tube_mm * 1.0e-3,
            arc_sweep_deg: c.arc_sweep_deg,
            side_bat: c.sweep_deg.signum() * c.arc_sweep_deg.signum(),
            side_world: c.arc_sweep_deg.signum(),
        }
    }
}

/// Angle between two vectors, degrees.
fn angle_between_deg(a: DVec3, b: DVec3) -> f64 {
    a.angle_between(b).to_degrees()
}

/// Errors of a pose, for the summaries.
#[derive(Copy, Clone, Debug, Default)]
struct Errors {
    tangent_deg: f64,
    tip_dir_deg: f64,
}

fn errors(pose: &Pose, e: &Expect) -> Errors {
    let cfg = DVec3::new(
        libm::cos(e.angle_deg.to_radians()),
        libm::sin(e.angle_deg.to_radians()),
        0.0,
    );
    let l = &pose.local;
    let tangent_deg = [
        angle_between_deg(l.normal_plus, DVec3::Y),
        angle_between_deg(l.normal_minus, DVec3::NEG_Y),
        angle_between_deg(pose.tangent, pose.dir),
        angle_between_deg(pose.tangent, cfg),
    ]
    .into_iter()
    .fold(0.0, f64::max);
    let expected_theta = e.side_bat * 0.5 * e.arc_sweep_deg.abs();
    Errors {
        tangent_deg,
        tip_dir_deg: (l.tip_theta.to_degrees() - expected_theta).abs(),
    }
}

/// Whether `value` is strictly on the side `sign` (±1) of zero (false for
/// NaN and for zero).
fn on_side(sign: f64, value: f64) -> bool {
    sign * value > 0.0
}

/// The pose checks (a), (b), (c) of one pose: `Ok` or every failure.
fn check_pose(pose: &Pose, e: &Expect) -> Result<(), String> {
    let mut fails = Vec::new();
    let cfg = DVec3::new(
        libm::cos(e.angle_deg.to_radians()),
        libm::sin(e.angle_deg.to_radians()),
        0.0,
    );
    let l = &pose.local;

    // (a)(i) The solver's world direction is the configured angle.
    let dir_err = angle_between_deg(pose.dir, cfg);
    if dir_err > TOL_WORLD_DIR_DEG {
        fails.push(format!(
            "world direction: {dir_err:.4}° from the configured {:.3}° (tol {TOL_WORLD_DIR_DEG}°)",
            e.angle_deg
        ));
    }
    // (a)(ii) Pivot end: symmetric crossings at ±tube, normals ±bat y, so
    // the outline tangent there is the bat axis, in world too.
    let sym = (l.cross_plus + l.cross_minus).abs();
    if sym > TOL_PIVOT_SYM_M {
        fails.push(format!(
            "pivot crossings: +{:.6} / {:.6} m are not symmetric (off by {:.2} µm)",
            l.cross_plus,
            l.cross_minus,
            sym * 1.0e6
        ));
    }
    if (l.cross_plus - e.tube).abs() > TOL_PIVOT_SYM_M {
        fails.push(format!(
            "pivot crossing at {:.6} m, not the tube radius {:.6} m",
            l.cross_plus, e.tube
        ));
    }
    let np = angle_between_deg(l.normal_plus, DVec3::Y);
    let nm = angle_between_deg(l.normal_minus, DVec3::NEG_Y);
    if np.max(nm) > TOL_TANGENT_DEG {
        fails.push(format!(
            "pivot normals: {np:.4}° / {nm:.4}° from ±bat y (tol {TOL_TANGENT_DEG}°)"
        ));
    }
    let t_dir = angle_between_deg(pose.tangent, pose.dir);
    let t_cfg = angle_between_deg(pose.tangent, cfg);
    if t_dir > TOL_TANGENT_DEG || t_cfg > TOL_TANGENT_DEG + TOL_WORLD_DIR_DEG {
        fails.push(format!(
            "pivot tangent: {t_dir:.4}° from bat_direction, {t_cfg:.4}° from the configured angle"
        ));
    }

    // (b) The farthest point and the curl against the straight bat.
    let reach = e.length + e.tube;
    if (l.tip_radius - reach).abs() > TOL_TIP_RADIUS_M {
        fails.push(format!(
            "tip radius: {:.6} m, expected length + tube {reach:.6} m",
            l.tip_radius
        ));
    }
    let expected_theta = e.side_bat * 0.5 * e.arc_sweep_deg.abs();
    let theta = l.tip_theta.to_degrees();
    if (theta - expected_theta).abs() > TOL_TIP_DIR_DEG {
        fails.push(format!(
            "tip direction: {theta:.3}° in the bat frame, expected {expected_theta:.3}° (tol {TOL_TIP_DIR_DEG}°)"
        ));
    }
    let offset = 2.0 * e.length * libm::sin(0.25 * e.arc_sweep_deg.abs().to_radians());
    let mag = pose.curl.length();
    if (mag - offset).abs() > TOL_CURL_MAG_M {
        fails.push(format!(
            "curl magnitude: {:.3} mm, expected {:.3} mm from the authored sweep",
            mag * 1.0e3,
            offset * 1.0e3
        ));
    }

    // (c) The side: world up/down the table and bat-frame ±y.
    if !on_side(e.side_world, pose.curl.y) {
        fails.push(format!(
            "curl side (world): world-y {:.3} mm, expected {} the table",
            pose.curl.y * 1.0e3,
            if e.side_world > 0.0 { "up" } else { "down" }
        ));
    }
    if !on_side(e.side_bat, pose.curl_bat_y) {
        fails.push(format!(
            "curl side (bat): bat-y {:.3} mm, expected on bat {}y",
            pose.curl_bat_y * 1.0e3,
            if e.side_bat > 0.0 { "+" } else { "−" }
        ));
    }
    let along_y = pose.curl.dot(pose.y_axis);
    if !on_side(e.side_bat, along_y) {
        fails.push(format!(
            "curl side (bat y in world): {:.3} mm along normal × direction",
            along_y * 1.0e3
        ));
    }
    if fails.is_empty() {
        Ok(())
    } else {
        Err(fails.join("; "))
    }
}

// --- cases ---------------------------------------------------------------

/// One flipper and bat.
#[derive(Copy, Clone, Debug)]
struct Case {
    length_mm: f64,
    tube_mm: f64,
    radius_mm: f64,
    arc_sweep_deg: f64,
    rest_deg: f64,
    sweep_deg: f64,
}

impl Case {
    fn end_deg(&self) -> f64 {
        self.rest_deg + self.sweep_deg
    }
}

/// The arc sweep a chord and radius imply, degrees.
fn implied_sweep_deg(length_mm: f64, radius_mm: f64) -> f64 {
    2.0 * libm::asin(length_mm / (2.0 * radius_mm)).to_degrees()
}

/// Authored `|arc_sweep_deg|`: the implied value rounded to 0.1°, moved by
/// `offset_deg` (|offset| ≤ 0.9°, inside the 1° lint).
fn authored_sweep_deg(length_mm: f64, radius_mm: f64, offset_deg: f64) -> f64 {
    let implied = implied_sweep_deg(length_mm, radius_mm);
    let authored = (implied * 10.0).round() / 10.0 + offset_deg;
    assert!(
        (authored - implied).abs() < 1.0,
        "authored sweep {authored} outside the lint range of {implied}"
    );
    authored
}

/// Pivot of every test flipper, mm.
const BASE_MM: [f32; 3] = [120.0, 250.0, 0.0];

fn config(c: &Case) -> FlipperConfig {
    let base = if c.sweep_deg >= 0.0 {
        left_config(0, BASE_MM)
    } else {
        right_config(0, BASE_MM)
    };
    FlipperConfig {
        rest_angle_deg: c.rest_deg as f32,
        sweep_angle_deg: c.sweep_deg as f32,
        shape: FlipperShape::CurvedBanana {
            length_mm: c.length_mm as f32,
            arc_radius_mm: c.radius_mm as f32,
            arc_sweep_deg: c.arc_sweep_deg as f32,
            tube_radius_mm: c.tube_mm as f32,
        },
        ..base
    }
}

/// The bat the solver builds for `config`.
fn carried_shape(config: &FlipperConfig) -> CurvedBanana {
    CurvedBanana::from_config(config, FlipperTuning::default().bat_height_m).expect("valid banana")
}

/// Energises flipper 0 and steps (simulated 1 ms ticks) until it sits on its
/// end stop.
fn drive_to_end(set: &mut FlipperSet, end_deg: f32) {
    set.set_energised(LEFT, true);
    for tick in 0..2000u64 {
        set.step_tick(tick);
        set.advance(DT);
        if set.angle_deg(LEFT) == end_deg && set.angular_velocity(LEFT) == 0.0 {
            return;
        }
    }
    panic!(
        "flipper never settled on its end stop: {}° at {} rad/s",
        set.angle_deg(LEFT),
        set.angular_velocity(LEFT)
    );
}

/// The pose of flipper 0 of `set` as the solver has it now.
fn solver_pose(set: &FlipperSet, local: &LocalProbe, normal: Vec3, length: f64) -> Pose {
    place(
        local,
        set.pivot_m(LEFT).expect("flipper 0"),
        set.bat_direction(LEFT).expect("flipper 0"),
        normal,
        length,
    )
}

/// The banana's own helpers and the solver's carried shape agree with what
/// the collision query showed (cross-checks, not the checks themselves).
fn cross_check(
    shape: &CurvedBanana,
    local: &LocalProbe,
    pose: &Pose,
    set: &FlipperSet,
    e: &Expect,
) -> Result<(), String> {
    let mut fails = Vec::new();
    let probed = DVec3::new(libm::cos(local.tip_theta), libm::sin(local.tip_theta), 0.0) * e.length;
    let helper = shape.tip_center().as_dvec3();
    if (probed - helper).length() > TOL_TIP_RADIUS_M {
        fails.push(format!(
            "tip_center() {helper:?} is not the probed tip centre {probed:?}"
        ));
    }
    let helper_t = shape.centerline_tangent(0.0).as_dvec3();
    if angle_between_deg(helper_t, local.tangent) > TOL_TANGENT_DEG {
        fails.push(format!(
            "centerline_tangent(0) {helper_t:?} is not the probed tangent {:?}",
            local.tangent
        ));
    }
    // The solver's own world query: the probed farthest point is on the
    // surface of the bat it carries.
    let far = pose.tip_center
        + (pose.tip_center - set.pivot_m(LEFT).unwrap().as_dvec3()).normalize() * e.tube;
    let world = Vec3::new(far.x as f32, far.y as f32, Z_MID);
    let d = set
        .closest_point(0, 0.0, world)
        .expect("a bat surface")
        .distance;
    if f64::from(d).abs() > 20.0e-6 {
        fails.push(format!(
            "the solver's bat is {:.2} µm from the probed tip",
            f64::from(d) * 1.0e6
        ));
    }
    if fails.is_empty() {
        Ok(())
    } else {
        Err(fails.join("; "))
    }
}

/// Running summary of a sweep.
#[derive(Clone, Debug)]
struct Summary {
    /// Flippers checked, and poses (rest and end stop of each).
    cases: usize,
    poses: usize,
    worst_tangent_deg: f64,
    worst_tip_dir_deg: f64,
    /// Smallest world-y curl of the up-table cases, largest (nearest zero)
    /// of the down-table ones, mm.
    min_up_mm: f64,
    max_down_mm: f64,
}

impl Summary {
    fn new() -> Self {
        Summary {
            cases: 0,
            poses: 0,
            worst_tangent_deg: 0.0,
            worst_tip_dir_deg: 0.0,
            min_up_mm: f64::INFINITY,
            max_down_mm: f64::NEG_INFINITY,
        }
    }

    fn add(&mut self, pose: &Pose, e: &Expect) {
        let err = errors(pose, e);
        self.poses += 1;
        self.worst_tangent_deg = self.worst_tangent_deg.max(err.tangent_deg);
        self.worst_tip_dir_deg = self.worst_tip_dir_deg.max(err.tip_dir_deg);
        let y_mm = pose.curl.y * 1.0e3;
        if e.side_world > 0.0 {
            self.min_up_mm = self.min_up_mm.min(y_mm);
        } else {
            self.max_down_mm = self.max_down_mm.max(y_mm);
        }
    }

    fn print(&self, name: &str) {
        println!(
            "banana_pose {name}: {} cases ({} poses), worst tangent error {:.5} deg, worst tip-direction \
             error {:.4} deg, min curl world-y (+arc_sweep) {:.3} mm, max curl world-y \
             (−arc_sweep) {:.3} mm",
            self.cases,
            self.poses,
            self.worst_tangent_deg,
            self.worst_tip_dir_deg,
            self.min_up_mm,
            self.max_down_mm
        );
    }
}

/// Probes one case at rest and on its end stop and checks both; returns the
/// world-y curl at rest and at the end, metres.
fn run_case(c: &Case, local: &LocalProbe, summary: &mut Summary) -> Result<[f64; 2], String> {
    let cfg = config(c);
    let shape = carried_shape(&cfg);
    let normal = Vec3::from_array(cfg.surface_normal);
    let length = c.length_mm * 1.0e-3;
    let mut set = FlipperSet::new(std::slice::from_ref(&cfg)).expect("valid flipper");
    let mut out = [0.0; 2];
    summary.cases += 1;
    for (k, angle) in [c.rest_deg, c.end_deg()].into_iter().enumerate() {
        if k == 1 {
            drive_to_end(&mut set, cfg.end_angle_deg());
            let got = f64::from(set.angle_deg(LEFT));
            if (got - f64::from(cfg.end_angle_deg())).abs() > TOL_WORLD_DIR_DEG {
                return Err(format!(
                    "{c:?}: angle_deg {got} on the end stop, configured {}",
                    cfg.end_angle_deg()
                ));
            }
        }
        let e = Expect::new(c, angle);
        let pose = solver_pose(&set, local, normal, length);
        let which = if k == 0 { "rest" } else { "end" };
        check_pose(&pose, &e).map_err(|m| format!("{c:?} at {which} ({angle}°): {m}"))?;
        cross_check(&shape, local, &pose, &set, &e)
            .map_err(|m| format!("{c:?} at {which}: cross-check: {m}"))?;
        summary.add(&pose, &e);
        out[k] = pose.curl.y;
    }
    Ok(out)
}

/// Range of left-flipper angles (rest and end stop) in which the
/// up-table curl (positive `arc_sweep_deg`) is checked, degrees. The right
/// flipper's is the mirror `[180 − hi, 180 − lo]` = `[150°, 240°]`.
const LEFT_RANGE_UP: (f64, f64) = (-60.0, 30.0);
/// The same for the down-table curl (negative `arc_sweep_deg`). It points
/// at `angle − 90° − φ/4` (left), below the horizontal only for
/// `angle > −90° + φ/4`; the tightest valid bend (R = 1.1·L/2, φ = 130.8°)
/// needs `angle > −57.3°`, so the lower end is −55° (right: up to 235°).
const LEFT_RANGE_DOWN: (f64, f64) = (-55.0, 30.0);

/// The left range for a sign of `arc_sweep_deg`.
fn left_range(arc_sweep_deg: f64) -> (f64, f64) {
    if arc_sweep_deg >= 0.0 {
        LEFT_RANGE_UP
    } else {
        LEFT_RANGE_DOWN
    }
}

/// (rest, sweep) of the left flipper in the grid for a range `(lo, hi)`:
/// from the low end with the largest sweep, the middle, and up to the high
/// end; the right flipper uses the mirror `(180 − rest, −sweep)`. Rest and
/// end stay within the range by construction.
fn left_angles((lo, hi): (f64, f64)) -> [(f64, f64); 3] {
    let wide = (hi - lo).min(60.0);
    [(lo, wide), (-30.0, 50.0), (hi - 30.0, 30.0)]
}
const LENGTHS_MM: [f64; 3] = [60.0, 76.0, 90.0];
const TUBES_MM: [f64; 3] = [6.5, 8.0, 11.0];
/// Arc radii (the first is replaced by 1.1·L/2, the tightest valid bend).
const RADII_MM: [f64; 6] = [0.0, 60.0, 100.0, 150.0, 250.0, 400.0];
/// Authored-sweep offsets from the rounded implied value, cycled.
const OFFSETS_DEG: [f64; 6] = [0.0, 0.0, 0.9, 0.0, -0.9, 0.5];

#[test]
fn grid_pose_left_right_rest_and_end() {
    let mut summary = Summary::new();
    let mut worst_mirror: f64 = 0.0;
    let mut n = 0usize;
    for (il, &length_mm) in LENGTHS_MM.iter().enumerate() {
        for (it, &tube_mm) in TUBES_MM.iter().enumerate() {
            for (ir, &r) in RADII_MM.iter().enumerate() {
                let radius_mm = if ir == 0 { 1.1 * length_mm / 2.0 } else { r };
                let offset = OFFSETS_DEG[(il + it + ir) % OFFSETS_DEG.len()];
                let authored = authored_sweep_deg(length_mm, radius_mm, offset);
                for arc_sign in [1.0, -1.0] {
                    let arc_sweep_deg = arc_sign * authored;
                    // One probe per bat shape (left and right differ in
                    // their concave side); the poses come from the solver.
                    let make = |rest_deg: f64, sweep_deg: f64| Case {
                        length_mm,
                        tube_mm,
                        radius_mm,
                        arc_sweep_deg,
                        rest_deg,
                        sweep_deg,
                    };
                    let angles = left_angles(left_range(arc_sweep_deg));
                    for &(rest, sweep) in &angles {
                        let (lo, hi) = left_range(arc_sweep_deg);
                        assert!(rest >= lo && rest + sweep <= hi && (20.0..=60.0).contains(&sweep));
                    }
                    let (l0, s0) = angles[0];
                    let probe_left = probe_local(&carried_shape(&config(&make(l0, s0))));
                    let probe_right = probe_local(&carried_shape(&config(&make(180.0 - l0, -s0))));
                    for &(rest, sweep) in &angles {
                        let left = make(rest, sweep);
                        let right = make(180.0 - rest, -sweep);
                        let yl = run_case(&left, &probe_left, &mut summary)
                            .unwrap_or_else(|m| panic!("left: {m}"));
                        let yr = run_case(&right, &probe_right, &mut summary)
                            .unwrap_or_else(|m| panic!("right: {m}"));
                        for k in 0..2 {
                            assert!(
                                yl[k].signum() == yr[k].signum(),
                                "mirror: curl world-y {} (left) vs {} (right) for {left:?}",
                                yl[k],
                                yr[k]
                            );
                            let diff = (yl[k] - yr[k]).abs();
                            worst_mirror = worst_mirror.max(diff);
                            assert!(
                                diff <= TOL_MIRROR_M,
                                "mirror: curl world-y {} (left) vs {} (right) differ by {:.2} µm \
                                 for {left:?}",
                                yl[k],
                                yr[k],
                                diff * 1.0e6
                            );
                        }
                        n += 1;
                    }
                }
            }
        }
    }
    assert!(n >= 3 * 3 * 6 * 2 * 3, "grid too small: {n}");
    summary.print("grid");
    println!(
        "banana_pose grid mirror: {n} left/right pairs (rest and end), worst world-y \
         disagreement {:.3} µm",
        worst_mirror * 1.0e6
    );
}

#[test]
fn proptest_pose_random_valid_bats() {
    let summary = std::cell::RefCell::new(Summary::new());
    let cfg = Config {
        cases: 64,
        failure_persistence: None,
        ..Config::default()
    };
    let rng = TestRng::from_seed(RngAlgorithm::ChaCha, &[0x26; 32]);
    let mut runner = TestRunner::new_with_rng(cfg, rng);
    let strat = (
        60.0f64..=90.0,
        6.5f64..=11.0,
        0.0f64..=1.0,
        -0.9f64..=0.9,
        proptest::bool::ANY,
        proptest::bool::ANY,
        20.0f64..=60.0,
        0.0f64..=1.0,
    );
    runner
        .run(
            &strat,
            |(length_mm, tube_mm, rr, offset, right, flipped, sweep, rest_t)| {
                let r_min = 1.1 * length_mm / 2.0;
                let radius_mm = r_min + rr * (400.0 - r_min);
                let authored = authored_sweep_deg(length_mm, radius_mm, offset);
                let arc_sweep_deg = if flipped { -authored } else { authored };
                // Left: rest in [lo, hi − sweep]; right: the mirror,
                // rest in [180 − hi + sweep, 180 − lo]; both stops stay in
                // the range of the sign of the arc sweep.
                let (lo, hi) = left_range(arc_sweep_deg);
                let sweep = sweep.min(hi - lo);
                let left_rest = lo + rest_t * (hi - lo - sweep);
                let (rest_deg, sweep_deg) = if right {
                    (180.0 - left_rest, -sweep)
                } else {
                    (left_rest, sweep)
                };
                let c = Case {
                    length_mm,
                    tube_mm,
                    radius_mm,
                    arc_sweep_deg,
                    rest_deg,
                    sweep_deg,
                };
                let local = probe_local(&carried_shape(&config(&c)));
                run_case(&c, &local, &mut summary.borrow_mut())
                    .map_err(proptest::test_runner::TestCaseError::fail)?;
                Ok(())
            },
        )
        .unwrap();
    summary.borrow().print("proptest");
}

// --- negative controls: the same probe and checker on wrong poses --------

/// The 2.3 left flipper with the reference banana (76 mm, R 100, tube 8,
/// arc sweep 44.7°, concave face leading).
fn reference_left() -> Case {
    Case {
        length_mm: 76.0,
        tube_mm: 8.0,
        radius_mm: 100.0,
        arc_sweep_deg: authored_sweep_deg(76.0, 100.0, 0.0),
        rest_deg: -30.0,
        sweep_deg: 50.0,
    }
}

/// The rest pose of `c` with `shape` probed and placed by the solver.
fn rest_pose<S: BatShape>(c: &Case, shape: &S, rotate_deg: f32) -> (Pose, Expect) {
    let cfg = config(c);
    let set = FlipperSet::new(std::slice::from_ref(&cfg)).expect("valid flipper");
    let normal = Vec3::from_array(cfg.surface_normal);
    let dir =
        Quat::from_axis_angle(normal, rotate_deg.to_radians()) * set.bat_direction(LEFT).unwrap();
    let pose = place(
        &probe_local(shape),
        set.pivot_m(LEFT).unwrap(),
        dir,
        normal,
        c.length_mm * 1.0e-3,
    );
    (pose, Expect::new(c, c.rest_deg))
}

/// The correct pose passes (so the controls below fail for their reason).
#[test]
fn control_reference_pose_passes() {
    let c = reference_left();
    let (pose, e) = rest_pose(&c, &carried_shape(&config(&c)), 0.0);
    check_pose(&pose, &e).unwrap();
}

/// (i) A banana with the wrong concave side on the left flipper.
#[test]
fn control_wrong_concave_side_fails() {
    let c = reference_left();
    let right_shape = carried_shape(&config(&c));
    assert_eq!(right_shape.concave_side(), ConcaveSide::PlusY);
    let wrong = CurvedBanana::new(0.076, 0.100, 0.008, ConcaveSide::MinusY, 0.0, 0.030).unwrap();
    let (pose, e) = rest_pose(&c, &wrong, 0.0);
    let err = check_pose(&pose, &e).expect_err("a MinusY bat on the left flipper must fail");
    assert!(err.contains("curl side (world)"), "{err}");
    assert!(err.contains("curl side (bat)"), "{err}");
    assert!(err.contains("tip direction"), "{err}");
}

/// (ii) A bat direction 3° off the configured angle.
#[test]
fn control_rotated_bat_fails() {
    let c = reference_left();
    for rot in [3.0, -3.0] {
        let (pose, e) = rest_pose(&c, &carried_shape(&config(&c)), rot);
        let err = check_pose(&pose, &e).expect_err("a bat 3° off must fail");
        assert!(err.contains("world direction"), "{err}");
        assert!(err.contains("pivot tangent"), "{err}");
    }
}

/// (iii) A left-flipper pose checked against the right flipper's side.
#[test]
fn control_left_pose_against_right_side_fails() {
    let c = reference_left();
    let (pose, mut e) = rest_pose(&c, &carried_shape(&config(&c)), 0.0);
    let right = Case {
        rest_deg: 210.0,
        sweep_deg: -50.0,
        ..c
    };
    e.side_bat = Expect::new(&right, 210.0).side_bat;
    assert_eq!(e.side_bat, -1.0);
    let err = check_pose(&pose, &e).expect_err("the left curl is not on the right's side");
    assert!(err.contains("curl side (bat)"), "{err}");
    assert!(err.contains("tip direction"), "{err}");
}

/// (iv) A straight bat of the same chord has no curl.
#[test]
fn control_straight_bat_fails() {
    let c = reference_left();
    let straight = StraightCapsule::new(0.076, 0.008, 0.008, 0.0, 0.030).unwrap();
    let (pose, e) = rest_pose(&c, &straight, 0.0);
    assert!(pose.curl.length() < 1.0e-4, "straight curl {:?}", pose.curl);
    let err = check_pose(&pose, &e).expect_err("a straight bat must fail");
    assert!(err.contains("curl magnitude"), "{err}");
    assert!(err.contains("tip direction"), "{err}");
}

/// The negative-sweep pose of the reference fails the positive expectation
/// and passes its own (the flip is seen by the checker, both ways).
#[test]
fn control_flipped_sweep_is_distinguished() {
    let c = reference_left();
    let flipped = Case {
        arc_sweep_deg: -c.arc_sweep_deg,
        ..c
    };
    let (pose, e_flipped) = rest_pose(&flipped, &carried_shape(&config(&flipped)), 0.0);
    check_pose(&pose, &e_flipped).unwrap();
    let e_pos = Expect::new(&c, c.rest_deg);
    let err = check_pose(&pose, &e_pos).expect_err("the flipped curl is not up-table");
    assert!(err.contains("curl side (world)"), "{err}");
}
