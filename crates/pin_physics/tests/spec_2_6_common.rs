//! Shared helpers for the Subphase 2.6 (Curved "Banana" flipper) spec tests,
//! included with `#[path]` (an empty test crate on its own).
//!
//! Written black-box from the 2.6 brief and the published contract
//! (DECISIONS "2.6 — Banana flipper public API"). The tests drive a
//! `FlipperSet` built from `FlipperShape::CurvedBanana` configs through the
//! public solver API only (`FlipperSet`, the 2.2-S `MovingBodies` trait,
//! `BallSolver::step_with_bodies`). The 2.3 Tester's rig
//! (`spec_2_3_common.rs`) is reused for the scene, the table and the
//! straight-capsule reference.
//!
//! Geometry: an independent analytic model of the banana centreline
//! ([`ArcModel`], a circular arc in the bat plane) is built here from the
//! config and the conventions the contract fixes; it is used to aim balls,
//! to brute-force the closest point and to detect tunnelling, never taken
//! from the implementation.
#![allow(dead_code)]

#[path = "spec_2_3_common.rs"]
mod c23;

pub use c23::*;

use glam::{Quat, Vec3};
use pin_core::ids::FlipperId;
use pin_core::{FlipperConfig, FlipperShape};
use pin_physics::ball::{BallEvent, BallHandle, ImpactTarget, MovingBodies};
use pin_physics::flipper::FlipperSet;

/// Banana bat dimensions in table units (mm, degrees).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Banana {
    /// Chord, pivot to tip-arc end, mm.
    pub length_mm: f32,
    /// Arc radius of the centreline, mm.
    pub arc_radius_mm: f32,
    /// Tube radius, mm.
    pub tube_radius_mm: f32,
}

impl Banana {
    /// Arc sweep (degrees) consistent with chord and radius:
    /// `2·asin(L / 2R)`.
    pub fn arc_sweep_deg(&self) -> f32 {
        2.0 * libm::asinf((0.5 * self.length_mm / self.arc_radius_mm).min(1.0)).to_degrees()
    }

    pub fn shape(&self) -> FlipperShape {
        FlipperShape::CurvedBanana {
            length_mm: self.length_mm,
            arc_radius_mm: self.arc_radius_mm,
            arc_sweep_deg: self.arc_sweep_deg(),
            tube_radius_mm: self.tube_radius_mm,
        }
    }
}

/// The reference banana: the Appendix C bat length (76 mm chord), a
/// 120 mm arc radius (≈ 37° of arc, 6.3 mm sagitta) and an 8 mm tube.
pub const BANANA: Banana = Banana {
    length_mm: 76.0,
    arc_radius_mm: 120.0,
    tube_radius_mm: 8.0,
};

/// A flipper config with the Appendix C defaults and a banana bat.
pub fn banana_config(
    id: u16,
    base_mm: [f32; 3],
    normal: Vec3,
    rest_deg: f32,
    sweep_deg: f32,
    b: Banana,
) -> FlipperConfig {
    let mut c = config(id, base_mm, normal, rest_deg, sweep_deg);
    c.shape = b.shape();
    c
}

/// The straight capsule "of equal length" to `b`: same chord, both radii
/// equal to the tube radius.
pub fn straight_equiv_config(
    id: u16,
    base_mm: [f32; 3],
    normal: Vec3,
    rest_deg: f32,
    sweep_deg: f32,
    b: Banana,
) -> FlipperConfig {
    let mut c = config(id, base_mm, normal, rest_deg, sweep_deg);
    c.shape = FlipperShape::StraightCapsule {
        length_mm: b.length_mm,
        base_radius_mm: b.tube_radius_mm,
        tip_radius_mm: b.tube_radius_mm,
    };
    c
}

// ------------------------------------------------------------- centreline

/// Bat centreline in the bat frame (metres): x along
/// `FlipperSet::bat_direction`, y = normal × x, origin at the pivot.
#[derive(Copy, Clone, Debug)]
pub enum ArcModel {
    /// Straight segment (0,0) → (len, 0).
    Segment { len: f32, tube: f32 },
    /// Arc `c + rho·(cos φ, sin φ)`, φ from `phi0` to `phi0 + dphi`
    /// (signed), so fraction `f` of the arc length is at `phi0 + f·dphi`.
    Arc {
        c: [f32; 2],
        rho: f32,
        phi0: f32,
        dphi: f32,
        tube: f32,
    },
}

/// The analytic centreline of `b` on a flipper with sweep `sweep_deg`
/// (contract "2.6 — Banana flipper public API"): the arc starts at the
/// pivot tangent to bat-frame +x; its centre of curvature is at
/// `(0, s·arc_radius)` with `s` = sign of the sweep times sign of
/// `arc_sweep_deg` (the concave face leads the swing); the chord
/// `length_mm` runs from the pivot to the tip-cap centre, so the arc angle
/// is `2·asin(L / 2R)`.
pub fn arc_model(b: Banana, sweep_deg: f32) -> ArcModel {
    arc_model_side(b, if sweep_deg >= 0.0 { 1.0 } else { -1.0 })
}

/// [`arc_model`] with the concave side `s` (+1: bat-frame +y) given
/// directly (`s = sign(sweep_angle_deg) · sign(arc_sweep_deg)`).
pub fn arc_model_side(b: Banana, s: f32) -> ArcModel {
    let l = b.length_mm * 1.0e-3;
    let rho = b.arc_radius_mm * 1.0e-3;
    let tube = b.tube_radius_mm * 1.0e-3;
    let phi = 2.0 * libm::asinf((0.5 * l / rho).min(1.0));
    ArcModel::Arc {
        c: [0.0, s * rho],
        rho,
        phi0: -s * std::f32::consts::FRAC_PI_2,
        dphi: s * phi,
        tube,
    }
}

pub fn segment_model(b: Banana) -> ArcModel {
    ArcModel::Segment {
        len: b.length_mm * 1.0e-3,
        tube: b.tube_radius_mm * 1.0e-3,
    }
}

fn wrap(a: f32) -> f32 {
    let mut a = a;
    while a > std::f32::consts::PI {
        a -= std::f32::consts::TAU;
    }
    while a <= -std::f32::consts::PI {
        a += std::f32::consts::TAU;
    }
    a
}

impl ArcModel {
    pub fn tube(&self) -> f32 {
        match *self {
            ArcModel::Segment { tube, .. } | ArcModel::Arc { tube, .. } => tube,
        }
    }

    /// Centreline point at fraction `f` of its length (bat frame, 2D).
    pub fn point(&self, f: f32) -> [f32; 2] {
        match *self {
            ArcModel::Segment { len, .. } => [f * len, 0.0],
            ArcModel::Arc {
                c, rho, phi0, dphi, ..
            } => {
                let p = phi0 + f * dphi;
                [c[0] + rho * libm::cosf(p), c[1] + rho * libm::sinf(p)]
            }
        }
    }

    /// Unit tangent (direction of increasing `f`) at `f`.
    pub fn tangent(&self, f: f32) -> [f32; 2] {
        match *self {
            ArcModel::Segment { .. } => [1.0, 0.0],
            ArcModel::Arc { phi0, dphi, .. } => {
                let p = phi0 + f * dphi;
                let s = dphi.signum();
                [-s * libm::sinf(p), s * libm::cosf(p)]
            }
        }
    }

    /// Unit in-plane normal at `f` on the side `side` (+1: +y side of the
    /// bat frame, i.e. `normal × bat_direction`).
    pub fn side_normal(&self, f: f32, side: f32) -> [f32; 2] {
        let t = self.tangent(f);
        // Left normal of the tangent is (−t.y, t.x) = +y for t = +x.
        [-t[1] * side, t[0] * side]
    }

    /// 2D distance from bat-frame point `q` to the centreline, and the
    /// fraction of the nearest point (brute force: dense sampling refined
    /// by a local golden-section search).
    pub fn distance_2d(&self, q: [f32; 2]) -> (f32, f32) {
        let d = |f: f32| {
            let p = self.point(f);
            libm::hypotf(q[0] - p[0], q[1] - p[1])
        };
        let n = 400;
        let mut best = (f32::INFINITY, 0.0f32);
        for k in 0..=n {
            let f = k as f32 / n as f32;
            let dd = d(f);
            if dd < best.0 {
                best = (dd, f);
            }
        }
        let (mut lo, mut hi) = (
            (best.1 - 1.0 / n as f32).max(0.0),
            (best.1 + 1.0 / n as f32).min(1.0),
        );
        for _ in 0..60 {
            let m1 = lo + (hi - lo) * 0.381_966;
            let m2 = lo + (hi - lo) * 0.618_034;
            if d(m1) < d(m2) {
                hi = m2;
            } else {
                lo = m1;
            }
        }
        let f = 0.5 * (lo + hi);
        let df = d(f);
        if df < best.0 { (df, f) } else { best }
    }

    /// Analytic 2D distance (exact point-to-arc), used where speed matters.
    pub fn distance_2d_fast(&self, q: [f32; 2]) -> f32 {
        match *self {
            ArcModel::Segment { len, .. } => {
                let x = q[0].clamp(0.0, len);
                libm::hypotf(q[0] - x, q[1])
            }
            ArcModel::Arc {
                c, rho, phi0, dphi, ..
            } => {
                let a = libm::atan2f(q[1] - c[1], q[0] - c[0]);
                let rel = wrap(a - phi0) * dphi.signum();
                if rel >= 0.0 && rel <= dphi.abs() {
                    (libm::hypotf(q[0] - c[0], q[1] - c[1]) - rho).abs()
                } else {
                    let p0 = self.point(0.0);
                    let p1 = self.point(1.0);
                    libm::hypotf(q[0] - p0[0], q[1] - p0[1])
                        .min(libm::hypotf(q[0] - p1[0], q[1] - p1[1]))
                }
            }
        }
    }

    /// For an arc: is `q` within the arc's angular span (shrunk by `margin`
    /// radians at both ends), and is it inside the arc circle? `None` for a
    /// point outside the span or a segment model.
    pub fn arc_side(&self, q: [f32; 2], margin: f32) -> Option<bool> {
        match *self {
            ArcModel::Segment { len, .. } => {
                if q[0] > margin * len && q[0] < (1.0 - margin) * len {
                    Some(q[1] > 0.0)
                } else {
                    None
                }
            }
            ArcModel::Arc {
                c, rho, phi0, dphi, ..
            } => {
                let a = libm::atan2f(q[1] - c[1], q[0] - c[0]);
                let rel = wrap(a - phi0) * dphi.signum();
                if rel > margin && rel < dphi.abs() - margin {
                    Some(libm::hypotf(q[0] - c[0], q[1] - c[1]) < rho)
                } else {
                    None
                }
            }
        }
    }

    /// Bat-frame side (+1 = +y) on which the concave face lies, at
    /// mid-arc (the side of the arc centre). +1 for a segment.
    pub fn concave_side(&self) -> f32 {
        match *self {
            ArcModel::Segment { .. } => 1.0,
            ArcModel::Arc { c, .. } => {
                let m = self.point(0.5);
                let n = self.side_normal(0.5, 1.0);
                if (c[0] - m[0]) * n[0] + (c[1] - m[1]) * n[1] > 0.0 {
                    1.0
                } else {
                    -1.0
                }
            }
        }
    }
}

// ------------------------------------------------------------ world frame

/// World frame of a flipper's bat at its current pose.
#[derive(Copy, Clone, Debug)]
pub struct Pose {
    /// Pivot on the bat's mid-plane (world).
    pub o: Vec3,
    /// Bat direction (bat-frame x).
    pub x: Vec3,
    /// normal × x (bat-frame y).
    pub y: Vec3,
    /// Unit normal (bat-frame z).
    pub n: Vec3,
}

impl Pose {
    /// Current pose of flipper `id` in `set`, with the bat mid-plane `lift`
    /// metres above the pivot along `n`.
    pub fn of(set: &FlipperSet, id: FlipperId, n: Vec3, lift: f32) -> Pose {
        let x = set.bat_direction(id).expect("bat direction");
        let o = set.pivot_m(id).expect("pivot") + n * lift;
        Pose {
            o,
            x,
            y: n.cross(x),
            n,
        }
    }

    /// The same pose turned about `n` by `deg` (CCW positive).
    pub fn turned(&self, deg: f32) -> Pose {
        let q = Quat::from_axis_angle(self.n, deg.to_radians());
        let x = q * self.x;
        Pose {
            o: self.o,
            x,
            y: self.n.cross(x),
            n: self.n,
        }
    }

    pub fn to_world(self, p: [f32; 2], z: f32) -> Vec3 {
        self.o + self.x * p[0] + self.y * p[1] + self.n * z
    }

    pub fn vec_to_world(&self, p: [f32; 2]) -> Vec3 {
        self.x * p[0] + self.y * p[1]
    }

    /// (x, y, z) of world point `w` in the bat frame.
    pub fn local(&self, w: Vec3) -> ([f32; 2], f32) {
        let d = w - self.o;
        ([d.dot(self.x), d.dot(self.y)], d.dot(self.n))
    }
}

/// Offset of the bat's mid-plane from the pivot along the normal, measured
/// by probing `closest_point` above and below centreline point `f` of
/// `model` on a set that has run one tick.
pub fn measure_lift(
    set: &FlipperSet,
    body: usize,
    id: FlipperId,
    n: Vec3,
    model: &ArcModel,
) -> f32 {
    let p0 = Pose::of(set, id, n, 0.0);
    let q = p0.to_world(model.point(0.5), 0.0);
    let d_up = set.closest_point(body, 0.0, q + n * 0.1).unwrap().distance;
    let d_dn = set.closest_point(body, 0.0, q - n * 0.1).unwrap().distance;
    0.5 * (d_dn - d_up)
}

/// Lift of a config, measured on a throw-away set (one empty tick).
pub fn lift_of(c: &FlipperConfig, model: &ArcModel) -> f32 {
    let mut s = Scene::new(empty_world(), Vec3::ZERO, std::slice::from_ref(c));
    s.advance();
    let n = Vec3::from_array(c.surface_normal).normalize();
    measure_lift(&s.set, 0, c.id, n, model)
}

/// `rest_angle_deg` that makes `bat_direction` point along `want` at rest
/// for a flat (normal +z) flipper at `base_mm`, measured.
pub fn rest_for_dir(c0: &FlipperConfig, want: Vec3) -> f32 {
    let mut c = c0.clone();
    c.rest_angle_deg = 0.0;
    let mut s = Scene::new(empty_world(), Vec3::ZERO, std::slice::from_ref(&c));
    s.advance();
    let n = Vec3::from_array(c.surface_normal).normalize();
    let e = s.set.bat_direction(c.id).unwrap();
    let mut a = signed_angle_deg(e, want, n);
    if a <= -180.0 {
        a += 360.0;
    }
    a
}

/// A flat flipper (normal +z) whose mid-plane sits at the ball-centre
/// height `R` above the playfield (so in-plane contacts are horizontal),
/// with the bat direction at `dir_deg` from +x at rest. `make` builds the
/// config from (base, rest).
pub fn aligned(
    make: impl Fn([f32; 3], f32) -> FlipperConfig,
    model: &ArcModel,
    base_xy_mm: [f32; 2],
    dir_deg: f32,
) -> (FlipperConfig, f32) {
    let probe = make([base_xy_mm[0], base_xy_mm[1], 0.0], 0.0);
    let lift = lift_of(&probe, model);
    let base = [base_xy_mm[0], base_xy_mm[1], (R - lift) * 1.0e3];
    let c0 = make(base, 0.0);
    let rest = rest_for_dir(&c0, flat_dir(dir_deg));
    let c = make(base, rest);
    let lift = lift_of(&c, model);
    (c, lift)
}

// -------------------------------------------------------------- shot rig

/// Result of one shot (see [`run_shot_arc`]).
#[derive(Copy, Clone, Debug)]
pub struct ArcShot {
    pub impact_tick: Option<u64>,
    pub speed: f32,
    pub velocity: Vec3,
    pub caught: bool,
    pub clear_tick: Option<u64>,
    /// Bat (angle °, ω rad/s) at the start of the first-impact tick.
    pub bat_at_impact: Option<(f32, f32)>,
    pub episodes: u32,
    /// Impact events of the ball with the bat (each counted, however close
    /// together; up to the clearing tick or the window end).
    pub impacts: u32,
}

/// Half the bat height (`FlipperTuning::bat_height_m` = 30 mm, flat top
/// and bottom faces, both shapes).
pub const HALF_HEIGHT: f32 = 0.015;

/// Clearance beyond ball radius + tube radius at which the ball counts as
/// having left the bat (as in the 2.3 rig).
pub const CLEAR_ARC: f32 = 0.015;

/// The 2.3 rebound measurement against a bat with centreline `model`: the
/// ball's speed at the first tick after its first impact with the bat at
/// which its centre is `R + tube + CLEAR_ARC` from the centreline (3D).
#[allow(clippy::too_many_arguments)]
pub fn run_shot_arc(
    scene: &mut Scene,
    h: BallHandle,
    model: &ArcModel,
    n: Vec3,
    lift: f32,
    id: FlipperId,
    body: usize,
    ticks: u64,
    mut drive: impl FnMut(u64, &mut FlipperSet),
) -> ArcShot {
    let tube = model.tube();
    let mut impact = None;
    let mut contact_seen = false;
    let mut bat_at_impact = None;
    let mut episodes = 0u32;
    let mut impacts = 0u32;
    let mut separated = true;
    for _ in 0..ticks {
        drive(scene.tick, &mut scene.set);
        let t = scene.tick;
        let pre = (scene.set.angle_deg(id), scene.set.angular_velocity(id));
        scene.advance();
        let hit = scene.hit(h, body);
        impacts += scene
            .ev
            .as_slice()
            .iter()
            .filter(|e| {
                matches!(e, BallEvent::Impact { ball, other: ImpactTarget::Body { set: 0, body: b }, .. }
                    if *ball == h && *b == body)
            })
            .count() as u32;
        if impact.is_none() && hit {
            impact = Some(t);
            bat_at_impact = Some(pre);
        }
        if hit && separated {
            episodes += 1;
            separated = false;
        }
        let b = scene.ball(h);
        let pose = Pose::of(&scene.set, id, n, lift);
        let (q, z) = pose.local(b.position);
        let d = libm::hypotf(model.distance_2d_fast(q), (z.abs() - HALF_HEIGHT).max(0.0));
        if impact.is_some() {
            if d < R + tube + 0.002 {
                contact_seen = true;
            }
            if d > R + tube + 0.003 {
                separated = true;
            }
            if contact_seen && d >= R + tube + CLEAR_ARC {
                return ArcShot {
                    impact_tick: impact,
                    speed: b.velocity.length(),
                    velocity: b.velocity,
                    caught: false,
                    clear_tick: Some(t),
                    bat_at_impact,
                    episodes,
                    impacts,
                };
            }
        }
    }
    let b = scene.ball(h);
    ArcShot {
        impact_tick: impact,
        speed: b.velocity.length(),
        velocity: b.velocity,
        caught: impact.is_some(),
        clear_tick: None,
        bat_at_impact,
        episodes,
        impacts,
    }
}

/// Ball + bat energy as in the 2.3 rig.
pub fn energy_of(scene: &Scene, gravity: Vec3, inertias: &[f32]) -> f64 {
    energy(scene, gravity, inertias)
}
