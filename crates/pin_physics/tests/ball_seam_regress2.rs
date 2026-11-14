//! Seam task 2.2-S, fix round 2: the Reviewer's round-2 repros (R1 kick-off
//! energy from a stop, R2 a non-speculative owner with spin and high
//! friction), kick-off energy and angular-momentum tests, a sweep of
//! owners that answer wrongly (lying owners), a body-supplied restitution,
//! and stacks resting on kinematic bodies (early-out regression).
#[path = "ball_seam_ref.rs"]
mod reference;
#[path = "ball_util.rs"]
mod util;
use glam::Vec3;
use pin_core::Material;
use pin_core::PhysicsDefaults;
use pin_physics::ball::{
    BODY_SPEED_LIMIT, BallEvents, BallSolver, BallSolverConfig, BodyPoint, Geometry, MovingBodies,
};
use reference::*;
use util::*;

/// R1: an UNDRIVEN reference bat (I = 1e-4) on its lower stop, moving away
/// from it at 12 rad/s, touches a steel ball at 75 mm that drifts away at
/// 0.05 m/s. Nothing drives the system: kinetic energy must not grow.
#[test]
fn r1_kickoff_from_a_stop_creates_no_energy() {
    let world = empty_world();
    let geo = Geometry::new(&world);
    let mut s = solver_no_gravity();
    let r = radius(STEEL);
    let mut bat = RotorBody::new(v(0.0, 0.0, r), 0.076, 0.009, 1.0e-4, -0.5);
    bat.limits = (-0.5, 0.5);
    bat.omega = 12.0;
    let dir = RotorBody::direction(-0.5);
    let nrm = Vec3::Z.cross(dir);
    let pivot = bat.pivot;
    let mut set = Rotors::one(bat);
    let h = s
        .add_ball(ball_at(
            STEEL,
            pivot + dir * 0.075 + nrm * (0.009 + r + 2.0e-6),
            nrm * 0.05,
        ))
        .unwrap();
    let e0 = kinetic_energy(&s) + set.bodies[0].energy();
    let mut ev = BallEvents::new();
    for tick in 0..8 {
        ev.clear();
        s.step_with_bodies(&geo, &mut [&mut set], DT, &mut ev);
        let e = kinetic_energy(&s) + set.bodies[0].energy();
        assert!(
            e <= e0 * 1.001,
            "tick {tick}: energy {e0:.4e} -> {e:.4e} J, ball {:?}, bat omega {}",
            s.ball(h).unwrap().velocity,
            set.bodies[0].omega
        );
    }
}

/// R2: the same owner without obligation 3 (`speculative_stops = false`):
/// the guard is documented to keep the solve bounded.
#[test]
fn r2_guard_keeps_a_non_speculative_owner_bounded() {
    let world = empty_world();
    let geo = Geometry::new(&world);
    let mut cfg = BallSolverConfig::from_defaults(&PhysicsDefaults::builtin());
    cfg.gravity = Vec3::ZERO;
    for p in &mut cfg.pairs {
        p.friction = 0.9;
    }
    let mut s = BallSolver::new(cfg, &profiles());
    let r = radius(STEEL);
    let mut bat = RotorBody::new(v(0.0, 0.0, r), 0.076, 0.009, 1.0e-4, -0.5);
    bat.limits = (-0.5, 0.5);
    bat.torque = -0.05;
    bat.speculative_stops = false;
    let mut set = Rotors::one(bat);
    let mut b = ball_at(STEEL, v(0.08, 0.06, r), v(0.0, -7.0, 0.0));
    b.angular_velocity = v(0.0, 0.0, 400.0);
    let h = s.add_ball(b).unwrap();
    let mut ev = BallEvents::new();
    for tick in 0..80 {
        if tick == 9 {
            set.bodies[0].torque = 1.2;
        }
        ev.clear();
        s.step_with_bodies(&geo, &mut [&mut set], DT, &mut ev);
        let v = s.ball(h).unwrap().velocity;
        assert!(
            v.abs().max_element() < 60.0,
            "tick {tick}: ball velocity {v:?}"
        );
    }
}

// ------------------------------------------------------- lying owners

#[derive(Copy, Clone, Debug, PartialEq)]
enum Lie {
    Honest,
    Scale(f32),
    Nan,
    Inf,
    Neg,
    /// 0 for every push into the ball side, honest otherwise.
    OneSided,
    /// Applies the impulse with the wrong sign.
    Reversed,
    /// Reports its velocity at the start of the tick, whatever happened.
    Stale,
    /// Velocity NaN after the first impulse of a tick.
    VelNan,
    /// Inverse mass from a fixed pseudo-random sequence (0..5 × honest).
    Random,
}

struct Liar {
    inner: Rotors,
    lie: Lie,
    stale: std::cell::Cell<Option<f32>>,
    impulsed: bool,
    rng: std::cell::Cell<u32>,
    /// Largest surface speed this body reported.
    fastest: std::cell::Cell<f32>,
}

impl Liar {
    fn new(inner: Rotors, lie: Lie) -> Self {
        Self {
            inner,
            lie,
            stale: std::cell::Cell::new(None),
            impulsed: false,
            rng: std::cell::Cell::new(12345),
            fastest: std::cell::Cell::new(0.0),
        }
    }

    fn reported_velocity(&self, body: usize, time: f32, point: Vec3) -> Vec3 {
        let b = &self.inner.bodies[body];
        match self.lie {
            Lie::Stale => {
                let w = self.stale.get().unwrap_or_else(|| {
                    let w = b.omega_at(time);
                    self.stale.set(Some(w));
                    w
                });
                Vec3::Z.cross(point - b.pivot) * w
            }
            Lie::VelNan if self.impulsed => Vec3::splat(f32::NAN),
            _ => self.inner.velocity_at(body, time, point),
        }
    }
}

impl MovingBodies for Liar {
    fn body_count(&self) -> usize {
        self.inner.body_count()
    }
    fn begin_tick(&mut self, dt: f32) {
        self.inner.begin_tick(dt);
        self.stale.set(None);
        self.impulsed = false;
    }
    fn end_tick(&mut self, dt: f32) {
        self.inner.end_tick(dt);
    }
    fn layer_mask(&self, body: usize) -> u8 {
        self.inner.layer_mask(body)
    }
    fn material(&self, body: usize) -> Material {
        self.inner.material(body)
    }
    fn swept_bounds(&self, body: usize) -> (Vec3, Vec3) {
        self.inner.swept_bounds(body)
    }
    fn max_surface_speed(&self, body: usize) -> f32 {
        self.inner.max_surface_speed(body)
    }
    fn closest_point(&self, body: usize, time: f32, point: Vec3) -> Option<BodyPoint> {
        self.inner.closest_point(body, time, point)
    }
    fn velocity_at(&self, body: usize, time: f32, point: Vec3) -> Vec3 {
        let v = self.reported_velocity(body, time, point);
        let speed = v.length();
        if speed.is_finite() && speed > self.fastest.get() {
            self.fastest.set(speed);
        }
        v
    }
    fn inverse_mass_at(&self, body: usize, time: f32, point: Vec3, dir: Vec3) -> f32 {
        let honest = self.inner.inverse_mass_at(body, time, point, dir);
        match self.lie {
            Lie::Scale(f) => honest * f,
            Lie::Nan => f32::NAN,
            Lie::Inf => f32::INFINITY,
            Lie::Neg => -honest - 1.0,
            Lie::OneSided => {
                let b = &self.inner.bodies[body];
                if (point - b.pivot).cross(dir).z < 0.0 {
                    0.0
                } else {
                    honest
                }
            }
            Lie::Random => {
                let x = self
                    .rng
                    .get()
                    .wrapping_mul(1_103_515_245)
                    .wrapping_add(12345);
                self.rng.set(x);
                honest * ((x >> 16) % 6) as f32
            }
            _ => honest,
        }
    }
    fn apply_impulse(&mut self, body: usize, time: f32, point: Vec3, impulse: Vec3) {
        self.impulsed = true;
        let j = if self.lie == Lie::Reversed {
            -impulse
        } else {
            impulse
        };
        self.inner.apply_impulse(body, time, point, j);
    }
}

thread_local! {
    /// Slowest step of the lying-owner sweep (report only: tests never
    /// assert on wall-clock time).
    static SLOWEST: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
}

const LIES: [Lie; 15] = [
    Lie::Honest,
    Lie::Scale(0.0),
    Lie::Scale(0.01),
    Lie::Scale(0.3),
    Lie::Scale(0.6),
    Lie::Scale(3.0),
    Lie::Scale(100.0),
    Lie::Nan,
    Lie::Inf,
    Lie::Neg,
    Lie::OneSided,
    Lie::Reversed,
    Lie::Stale,
    Lie::VelNan,
    Lie::Random,
];

/// A ball (with spin, under friction) lands on a bat lying on its stop;
/// the flip starts at tick 9. Returns (largest ball speed, largest bat tip
/// speed, incoming speed).
#[allow(clippy::too_many_arguments)]
fn lying_flip(
    lie: Lie,
    speculative: bool,
    inertia: f32,
    x: f32,
    speed: f32,
    spin: f32,
    friction: Option<f32>,
) -> (f32, f32) {
    let world = empty_world();
    let geo = Geometry::new(&world);
    let mut cfg = BallSolverConfig::from_defaults(&PhysicsDefaults::builtin());
    cfg.gravity = Vec3::ZERO;
    if let Some(mu) = friction {
        for p in &mut cfg.pairs {
            p.friction = mu;
        }
    }
    let mut s = BallSolver::new(cfg, &profiles());
    let r = radius(STEEL);
    let mut bat = RotorBody::new(v(0.0, 0.0, r), 0.076, 0.009, inertia, -0.5);
    bat.limits = (-0.5, 0.5);
    bat.torque = -0.05;
    bat.speculative_stops = speculative;
    let mut set = Liar::new(Rotors::one(bat), lie);
    let mut b = ball_at(STEEL, v(x, 0.06, r), v(0.0, -speed, 0.0));
    b.angular_velocity = v(0.0, 0.0, spin);
    let h = s.add_ball(b).unwrap();
    let mut ev = BallEvents::new();
    let (mut top, mut tip) = (0.0f32, 0.0f32);
    for tick in 0..60 {
        if tick == 9 {
            set.inner.bodies[0].torque = 1.2;
        }
        ev.clear();
        let t0 = std::time::Instant::now();
        s.step_with_bodies(&geo, &mut [&mut set], DT, &mut ev);
        let el = t0.elapsed().as_secs_f64();
        if el > 1e-3 && std::env::var_os("DBG").is_some() {
            println!(
                "slow {:.0} µs: {lie:?} spec {speculative} I {inertia} x {x} v {speed} spin {spin} fr {friction:?} tick {tick}",
                el * 1e6
            );
        }
        SLOWEST.with(|c| c.set(c.get().max(el)));
        let b = s.ball(h).unwrap();
        assert!(
            b.velocity.is_finite() && b.angular_velocity.is_finite() && b.position.is_finite(),
            "{lie:?} spec {speculative} I {inertia} x {x} v {speed} spin {spin} fr {friction:?}: tick {tick} {b:?}"
        );
        top = top.max(b.velocity.length());
        let o = set.inner.bodies[0].omega;
        if o.is_finite() {
            tip = tip.max(o.abs() * set.inner.bodies[0].reach());
        }
    }
    // The body's surface speed: the fastest it committed or reported.
    (top, tip.max(set.fastest.get()))
}

#[test]
fn lying_owners_stay_finite_and_bounded() {
    let mut report = Vec::new();
    for lie in LIES {
        for speculative in [true, false] {
            let mut worst = (0.0f32, 0.0f32, String::new());
            for inertia in [3.0e-5f32, 1.0e-4] {
                for x in [0.02f32, 0.055, 0.07, 0.08] {
                    for speed in [0.5f32, 3.0, 7.0, 12.0] {
                        for spin in [0.0f32, 200.0, -400.0, 400.0] {
                            for friction in [None, Some(0.0), Some(0.9)] {
                                let (top, tip) =
                                    lying_flip(lie, speculative, inertia, x, speed, spin, friction);
                                // The documented absolute bound, and the
                                // relative one: no faster than (1 + e) times
                                // the faster of the incoming ball and the
                                // body's surface, plus the cap headroom.
                                let what = format!(
                                    "{lie:?} spec {speculative} I {inertia} x {x} v {speed} spin {spin} fr {friction:?}"
                                );
                                assert!(
                                    top <= BODY_SPEED_LIMIT.max(speed) * 1.0001,
                                    "{what}: {top}"
                                );
                                assert!(
                                    top <= 2.0 * speed.max(tip) + 2.0,
                                    "{what}: ball {top}, bat tip {tip}"
                                );
                                if top > worst.0 {
                                    worst = (
                                        top,
                                        tip,
                                        format!(
                                            "I {inertia} x {x} v {speed} spin {spin} fr {friction:?}"
                                        ),
                                    );
                                }
                            }
                        }
                    }
                }
            }
            report.push(format!(
                "{lie:?} spec {speculative}: top ball {:.2} m/s (bat tip {:.2} m/s) at {}",
                worst.0, worst.1, worst.2
            ));
        }
    }
    for line in &report {
        println!("{line}");
    }
    println!(
        "slowest step: {:.0} µs",
        SLOWEST.with(std::cell::Cell::get) * 1e6
    );
}

// ---------------------------------------------- kick-off from a stop (R1)

/// An undriven bat leaving its lower stop at `omega` touches a ball at `d`
/// from the pivot (frictionless pair, no rolling resistance). Returns the
/// energy and the angular momentum about the pivot before and after.
fn kickoff(omega: f32, d: f32, drift: f32, inertia: f32) -> (f64, f64, f64, f64, bool) {
    let world = empty_world();
    let geo = Geometry::new(&world);
    let mut cfg = BallSolverConfig::from_defaults(&PhysicsDefaults::builtin());
    cfg.gravity = Vec3::ZERO;
    cfg.rolling_resistance = 0.0;
    for p in &mut cfg.pairs {
        p.friction = 0.0;
    }
    let mut s = BallSolver::new(cfg, &profiles());
    let r = radius(STEEL);
    let mut bat = RotorBody::new(v(0.0, 0.0, r), 0.076, 0.009, inertia, -0.5);
    bat.limits = (-0.5, 0.5);
    bat.omega = omega;
    let dir = RotorBody::direction(-0.5);
    let nrm = Vec3::Z.cross(dir);
    let pivot = bat.pivot;
    let mut set = Rotors::one(bat);
    let h = s
        .add_ball(ball_at(
            STEEL,
            pivot + dir * d + nrm * (0.009 + r + 2.0e-6),
            nrm * drift,
        ))
        .unwrap();
    let m = f64::from(profiles()[0].mass_kg);
    let ib = f64::from(profiles()[0].moment_of_inertia());
    let l = |s: &BallSolver, set: &Rotors| {
        let b = s.ball(h).unwrap();
        let p = (b.position - pivot).as_dvec3();
        let vv = b.velocity.as_dvec3();
        f64::from(set.bodies[0].inertia) * f64::from(set.bodies[0].omega)
            + m * (p.x * vv.y - p.y * vv.x)
            + ib * f64::from(b.angular_velocity.z)
    };
    let e0 = kinetic_energy(&s) + set.bodies[0].energy();
    let l0 = l(&s, &set);
    let mut ev = BallEvents::new();
    let mut free = true;
    for _ in 0..8 {
        ev.clear();
        s.step_with_bodies(&geo, &mut [&mut set], DT, &mut ev);
        // The stop is an external constraint: L is only conserved while
        // the bat stays off it.
        free &= set.bodies[0].angle > -0.5 + 1e-6 || set.bodies[0].omega > 0.0;
    }
    let e1 = kinetic_energy(&s) + set.bodies[0].energy();
    (e0, e1, l0, l(&s, &set), free)
}

#[test]
fn kickoff_from_a_stop_conserves_energy_and_angular_momentum() {
    for (omega, d, drift, inertia) in [
        (12.0f32, 0.075f32, 0.05f32, 1.0e-4f32),
        (12.0, 0.04, 0.0, 1.0e-4),
        (30.0, 0.06, 0.2, 2.5e-4),
        (3.0, 0.07, 0.01, 3.0e-5),
    ] {
        let (e0, e1, l0, l1, free) = kickoff(omega, d, drift, inertia);
        assert!(e1 <= e0 * 1.001, "ω {omega} d {d}: energy {e0:e} → {e1:e}");
        if free {
            assert!(
                (l1 - l0).abs() <= 1e-3 * l0.abs(),
                "ω {omega} d {d}: L {l0:e} → {l1:e}"
            );
        }
    }
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(96))]

    #[test]
    fn kickoff_never_creates_energy(
        omega in 0.5f32..40.0, d in 0.015f32..0.08, drift in 0.0f32..0.5,
        inertia in 3.0e-5f32..5.0e-4,
    ) {
        let (e0, e1, l0, l1, free) = kickoff(omega, d, drift, inertia);
        proptest::prop_assert!(e1 <= e0 * 1.001 + 1e-12, "energy {} → {}", e0, e1);
        if free {
            proptest::prop_assert!((l1 - l0).abs() <= 1e-3 * l0.abs() + 1e-12, "L {} → {}", l0, l1);
        }
    }
}

// ---------------------------------------------- body-supplied restitution

/// A kinematic slab that may give its own restitution.
struct Bouncy {
    inner: Slab,
    e: Option<f32>,
}

impl MovingBodies for Bouncy {
    fn body_count(&self) -> usize {
        self.inner.body_count()
    }
    fn begin_tick(&mut self, dt: f32) {
        self.inner.begin_tick(dt);
    }
    fn end_tick(&mut self, dt: f32) {
        self.inner.end_tick(dt);
    }
    fn layer_mask(&self, body: usize) -> u8 {
        self.inner.layer_mask(body)
    }
    fn material(&self, body: usize) -> Material {
        self.inner.material(body)
    }
    fn swept_bounds(&self, body: usize) -> (Vec3, Vec3) {
        self.inner.swept_bounds(body)
    }
    fn max_surface_speed(&self, body: usize) -> f32 {
        self.inner.max_surface_speed(body)
    }
    fn closest_point(&self, body: usize, time: f32, point: Vec3) -> Option<BodyPoint> {
        self.inner.closest_point(body, time, point)
    }
    fn velocity_at(&self, body: usize, time: f32, point: Vec3) -> Vec3 {
        self.inner.velocity_at(body, time, point)
    }
    fn inverse_mass_at(&self, body: usize, time: f32, point: Vec3, dir: Vec3) -> f32 {
        self.inner.inverse_mass_at(body, time, point, dir)
    }
    fn apply_impulse(&mut self, body: usize, time: f32, point: Vec3, impulse: Vec3) {
        self.inner.apply_impulse(body, time, point, impulse);
    }
    fn restitution(&self, _body: usize, _time: f32, _point: Vec3, _approach: f32) -> Option<f32> {
        self.e
    }
}

/// A ball at 2 m/s hits a kinematic slab at rest head-on (frictionless):
/// rebound speed and the final state bits.
fn bounce(e: Option<Option<f32>>) -> (f32, Vec<u32>) {
    let world = empty_world();
    let geo = Geometry::new(&world);
    let mut cfg = BallSolverConfig::from_defaults(&PhysicsDefaults::builtin());
    cfg.gravity = Vec3::ZERO;
    for p in &mut cfg.pairs {
        p.friction = 0.0;
    }
    let mut s = BallSolver::new(cfg, &profiles());
    let slab = Slab::one(SlabBody::new(
        v(-0.05, 0.0, 0.0),
        v(0.01, 0.2, 0.2),
        Vec3::ZERO,
    ));
    let h = s
        .add_ball(ball_at(STEEL, Vec3::ZERO, v(-2.0, 0.0, 0.0)))
        .unwrap();
    let mut ev = BallEvents::new();
    match e {
        None => {
            let mut set = slab;
            for _ in 0..40 {
                s.step_with_bodies(&geo, &mut [&mut set], DT, &mut ev);
            }
        }
        Some(e) => {
            let mut set = Bouncy { inner: slab, e };
            for _ in 0..40 {
                s.step_with_bodies(&geo, &mut [&mut set], DT, &mut ev);
            }
        }
    }
    let b = s.ball(h).unwrap();
    let bits = b
        .position
        .to_array()
        .into_iter()
        .chain(b.velocity.to_array())
        .chain(b.angular_velocity.to_array())
        .map(f32::to_bits)
        .collect();
    (b.velocity.x, bits)
}

#[test]
fn body_restitution_overrides_the_pair_table() {
    let (vx, _) = bounce(Some(Some(0.3)));
    assert!((vx - 0.6).abs() < 1e-4, "{vx}");
    // Clamped to [0, 1]; NaN and None fall back to the pair table.
    let (vx, _) = bounce(Some(Some(1.7)));
    assert!((vx - 2.0).abs() < 1e-4, "{vx}");
    let (vx, _) = bounce(Some(Some(-0.5)));
    assert!(vx.abs() < 1e-4, "{vx}");
    let (_, plain) = bounce(None);
    assert_eq!(bounce(Some(None)).1, plain);
    assert_eq!(bounce(Some(Some(f32::NAN))).1, plain);
}

// ------------------------------------- stacks on kinematic bodies (N1)

/// A column of `n` steel balls resting on a floor: static geometry
/// (`kinematic = false`) or a kinematic slab (optionally with kinematic
/// walls around the column, optionally moving up at `lift` m/s). Returns the
/// largest speed relative to the floor over the last 0.5 s and the deepest
/// sink of the bottom ball into the floor.
fn column(n: usize, kinematic: bool, walls: bool, lift: f32) -> (f32, f32) {
    let r = radius(STEEL);
    let mut s = solver();
    let mut wb = pin_physics::world::StaticWorldBuilder::new();
    if !kinematic {
        wb.add_collider(
            cuboid(v(0.0, 0.0, -0.05), v(0.5, 0.5, 0.05)),
            pin_physics::world::layers::ALL,
            true,
        );
    }
    let world = wb.build();
    let geo = Geometry::new(&world);
    let mut set = Slab::default();
    if kinematic {
        set.bodies.push(SlabBody::new(
            v(0.0, 0.0, -0.05),
            v(0.5, 0.5, 0.05),
            v(0.0, 0.0, lift),
        ));
        if walls {
            let gap = r + 5.0e-6; // touching (within the contact skin)
            for (c, h) in [
                (v(gap + 0.01, 0.0, 0.1), v(0.01, 0.05, 0.2)),
                (v(-gap - 0.01, 0.0, 0.1), v(0.01, 0.05, 0.2)),
                (v(0.0, gap + 0.01, 0.1), v(0.05, 0.01, 0.2)),
                (v(0.0, -gap - 0.01, 0.1), v(0.05, 0.01, 0.2)),
            ] {
                set.bodies.push(SlabBody::new(c, h, v(0.0, 0.0, lift)));
            }
        }
    }
    for k in 0..n {
        s.add_ball(ball_at(
            STEEL,
            v(0.0, 0.0, r + 2.0 * r * k as f32 + 1.0e-6 * k as f32),
            Vec3::ZERO,
        ))
        .unwrap();
    }
    let mut ev = BallEvents::new();
    let (mut rest, mut sink) = (0.0f32, 0.0f32);
    for tick in 0..2000 {
        ev.clear();
        if kinematic {
            s.step_with_bodies(&geo, &mut [&mut set], DT, &mut ev);
        } else {
            s.step(&geo, DT, &mut ev);
        }
        let floor_top = if kinematic {
            set.bodies[0].center.z + 0.05
        } else {
            0.0
        };
        // `min`/`max` drop a NaN operand: check before reducing.
        for (h, b) in s.balls() {
            assert!(
                b.position.is_finite() && b.velocity.is_finite(),
                "tick {tick}: ball {h:?} is not finite: {b:?}"
            );
        }
        let bottom = s.ball(pin_physics::ball::BallHandle(0)).unwrap();
        sink = sink.min(bottom.position.z - r - floor_top);
        if tick >= 1500 {
            for (_, b) in s.balls() {
                rest = rest.max((b.velocity - v(0.0, 0.0, lift)).length());
            }
        }
    }
    (rest, sink)
}

#[test]
fn stacks_on_kinematic_bodies_rest_like_on_static_geometry() {
    let (base_rest, base_sink) = column(5, false, false, 0.0);
    println!("static floor: rest {base_rest:.2e} m/s, sink {base_sink:.2e} m");
    for (walls, lift) in [(false, 0.0f32), (true, 0.0), (false, 0.2), (true, 0.2)] {
        let (rest, sink) = column(5, true, walls, lift);
        println!(
            "kinematic floor walls {walls} lift {lift}: rest {rest:.2e} m/s, sink {sink:.2e} m"
        );
        assert!(
            rest < 1.0e-4 + 10.0 * base_rest,
            "walls {walls} lift {lift}: rest {rest:e}"
        );
        assert!(
            sink > -2.0e-5 + base_sink.min(0.0),
            "walls {walls} lift {lift}: sink {sink:e}"
        );
    }
}
