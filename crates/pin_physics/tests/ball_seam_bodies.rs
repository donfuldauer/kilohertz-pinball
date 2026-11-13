//! Seam task 2.2-S, moving bodies: the reference bodies of
//! `ball_seam_ref.rs` (a kinematic translating slab and a 1-DOF rotating
//! capsule with finite inertia) prove the `MovingBodies` contract: no
//! tunnelling, the two-body impulse, no energy gain at a kinematic body at
//! rest, stable resting and cradling contacts, call order, layers, events
//! and determinism.

#[path = "ball_util.rs"]
mod util;

#[path = "ball_seam_ref.rs"]
mod reference;

use std::cell::RefCell;

use glam::Vec3;
use pin_core::ids::ProfileId;
use pin_core::{Material, PhysicsDefaults};
use pin_physics::ball::{
    BallEvent, BallEvents, BallSolver, BallSolverConfig, BodyCast, BodyPoint, Geometry,
    ImpactTarget, MovingBodies, conservative_cast,
};
use pin_physics::world::layers;
use proptest::prelude::*;
use reference::*;
use util::*;

fn body_impacts(ev: &BallEvents, set: usize) -> usize {
    ev.as_slice()
        .iter()
        .filter(|e| {
            matches!(e, BallEvent::Impact { other: ImpactTarget::Body { set: s, .. }, .. } if *s == set)
        })
        .count()
}

/// Solver without gravity whose pairs with `material` are frictionless.
fn frictionless_with(material: Material) -> BallSolver {
    let mut cfg = BallSolverConfig::from_defaults(&PhysicsDefaults::builtin());
    cfg.gravity = Vec3::ZERO;
    for p in &mut cfg.pairs {
        if p.materials.contains(&material) {
            p.friction = 0.0;
        }
    }
    BallSolver::new(cfg, &profiles())
}

fn pair_restitution(ball: ProfileId, material: Material) -> f32 {
    let d = PhysicsDefaults::builtin();
    let bm = profiles()[ball.index()].material;
    d.material_pairs
        .iter()
        .find(|p| {
            (p.materials[0] == bm && p.materials[1] == material)
                || (p.materials[1] == bm && p.materials[0] == material)
        })
        .map_or(profiles()[ball.index()].restitution, |p| p.restitution)
}

// ---------------------------------------------------------------- tunnelling

/// A plate (thin slab, 1 m wide) moving along +x at `u` and a ball coming
/// the other way at `vb` along `dir`: the ball never gets to the other side
/// of the plate and never sinks into it.
fn plate_run(profile: ProfileId, u: Vec3, vb: f32, dir: Vec3) -> (usize, Vec3) {
    let world = empty_world();
    let geo = Geometry::new(&world);
    let mut s = solver_no_gravity();
    let r = radius(profile);
    let half = v(0.002, 1.0, 1.0);
    let mut plate = Slab::one(SlabBody::new(v(-0.1, 0.0, 0.0), half, u));
    let h = s
        .add_ball(ball_at(profile, v(0.1, 0.0, 0.0), dir * vb))
        .unwrap();
    let mut ev = BallEvents::new();
    let mut impacts = 0;
    let closing = (u.x - dir.x * vb).max(1.0e-3);
    let ticks = ((0.2 / closing) / DT) as u32 + 40;
    for _ in 0..ticks.min(3000) {
        ev.clear();
        s.step_with_bodies(&geo, &mut [&mut plate], DT, &mut ev);
        impacts += body_impacts(&ev, 0);
        let b = s.ball(h).unwrap();
        let slab = &plate.bodies[0];
        assert!(
            b.position.x > slab.center.x,
            "tunnelled: u {u:?} vb {vb} dir {dir:?} ball {:?} plate {:?}",
            b.position,
            slab.center
        );
        let gap = slab.closest(0.0, b.position).distance - r;
        assert!(gap > -1.0e-4, "sank {gap} (u {u:?} vb {vb})");
    }
    (impacts, s.ball(h).unwrap().velocity)
}

#[test]
fn plate_never_tunnels_head_on_sweep() {
    for profile in [STEEL, MINI, JUMBO] {
        for u in [0.0f32, 1.0, 5.0, 10.0, 20.0] {
            for vb in [0.0f32, 0.5, 3.0, 10.0, 30.0, 60.0, 100.0] {
                if u == 0.0 && vb == 0.0 {
                    continue;
                }
                let (impacts, vel) = plate_run(profile, v(u, 0.0, 0.0), vb, v(-1.0, 0.0, 0.0));
                assert_eq!(impacts, 1, "{profile:?} u {u} vb {vb}");
                // The ball leaves at least as fast as the plate.
                assert!(vel.x >= u - 1.0e-3, "{profile:?} u {u} vb {vb}: {vel:?}");
            }
        }
    }
}

#[test]
fn plate_hits_a_ball_at_rest_with_two_body_restitution() {
    // A kinematic plate at 5 m/s hits a resting steel ball: it leaves at
    // (1 + e) · 5 m/s (frictionless, no spin).
    let world = empty_world();
    let geo = Geometry::new(&world);
    let mut s = frictionless_with(Material::Rubber);
    let mut plate = Slab::one(SlabBody::new(
        v(-0.05, 0.0, 0.0),
        v(0.002, 0.1, 0.1),
        v(5.0, 0.0, 0.0),
    ));
    let h = s.add_ball(ball_at(STEEL, v(0.0, 0.0, 0.0), Vec3::ZERO)).unwrap();
    let mut ev = BallEvents::new();
    for _ in 0..30 {
        s.step_with_bodies(&geo, &mut [&mut plate], DT, &mut ev);
    }
    let e = pair_restitution(STEEL, Material::Rubber);
    let vx = s.ball(h).unwrap().velocity.x;
    assert!((vx - (1.0 + e) * 5.0).abs() < 1e-3 * 5.0, "{vx} vs e {e}");
    assert_eq!(body_impacts(&ev, 0), 1);
    // The body was told the impulse it gave: −m·Δv.
    let m = profiles()[0].mass_kg;
    let got = plate.bodies[0].received;
    assert!((got.x + m * vx).abs() < 1e-5, "{got:?}");
}

/// A rotor sweeping from −0.6 rad toward a stop at +1.2 rad at `tip` m/s
/// through a ball at `rho` on the x axis moving toward it at `vb`.
fn rotor_sweep(profile: ProfileId, tip: f32, rho: f32, vb: f32) -> usize {
    let world = empty_world();
    let geo = Geometry::new(&world);
    let mut s = solver_no_gravity();
    let r = radius(profile);
    let mut rotor = RotorBody::new(Vec3::ZERO, 0.07, 0.008, 2.5e-4, -0.6);
    rotor.omega = tip / rotor.reach();
    rotor.limits = (-0.6, 1.2);
    let mut set = Rotors::one(rotor);
    let h = s
        .add_ball(ball_at(profile, v(rho, 0.0, 0.0), v(0.0, -vb, 0.0)))
        .unwrap();
    let mut ev = BallEvents::new();
    let mut impacts = 0;
    let side = |set: &Rotors, p: Vec3| {
        let b = &set.bodies[0];
        RotorBody::direction(b.angle).cross(p - b.pivot).z
    };
    let inside = |set: &Rotors, p: Vec3| (p - set.bodies[0].pivot).length() < set.bodies[0].reach() + r;
    assert!(side(&set, s.ball(h).unwrap().position) > 0.0);
    for _ in 0..80 {
        ev.clear();
        s.step_with_bodies(&geo, &mut [&mut set], DT, &mut ev);
        impacts += body_impacts(&ev, 0);
        let p = s.ball(h).unwrap().position;
        if inside(&set, p) {
            assert!(
                side(&set, p) > 0.0,
                "tunnelled: tip {tip} rho {rho} vb {vb}: ball {p:?} bat {}",
                set.bodies[0].angle
            );
            let gap = set.bodies[0].closest(0.0, p).distance - r;
            assert!(gap > -2.0e-4, "sank {gap}: tip {tip} rho {rho} vb {vb}");
        }
    }
    impacts
}

#[test]
fn rotor_never_tunnels_sweep() {
    for profile in [STEEL, MINI, JUMBO] {
        for tip in [0.5f32, 2.0, 5.0, 10.0, 20.0] {
            for rho in [0.015f32, 0.04, 0.07] {
                for vb in [0.0f32, 2.0, 10.0, 40.0] {
                    let impacts = rotor_sweep(profile, tip, rho, vb);
                    assert!(impacts >= 1, "{profile:?} tip {tip} rho {rho} vb {vb}");
                }
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    #[test]
    fn plate_never_tunnels_oblique(
        ux in -5.0f32..20.0, uy in -5.0f32..5.0, uz in -5.0f32..5.0,
        vb in 0.0f32..80.0, dy in -1.0f32..1.0, dz in -1.0f32..1.0,
        p in 0usize..6,
    ) {
        let dir = v(-1.0, dy, dz).normalize();
        let profile = ProfileId(p as u16);
        // The ball must reach the plate within the run.
        prop_assume!(ux - dir.x * vb > 0.5);
        let (impacts, _) = plate_run(profile, v(ux, uy, uz), vb, dir);
        prop_assert!(impacts >= 1);
    }

    #[test]
    fn rotor_never_tunnels_random(
        tip in 0.2f32..25.0, rho in 0.01f32..0.075, vb in 0.0f32..50.0, p in 0usize..6,
    ) {
        rotor_sweep(ProfileId(p as u16), tip, rho, vb);
    }
}

// ----------------------------------------------------- two-body impulse

/// A free ball hits a free-pivot rod (rotor at rest, no torque, no stops)
/// perpendicular to it at distance `d` from the pivot, frictionless.
/// Returns (ball velocity, rotor ω, expected ball vy, expected ω, L before,
/// L after).
fn rod_hit(profile: ProfileId, d: f32, speed: f32, inertia: f32) -> [f64; 6] {
    let world = empty_world();
    let geo = Geometry::new(&world);
    let mut s = frictionless_with(Material::Rubber);
    let rotor = RotorBody::new(Vec3::ZERO, 0.08, 0.008, inertia, 0.0);
    let mut set = Rotors::one(rotor);
    let r = radius(profile);
    let m = profiles()[profile.index()].mass_kg;
    let y0 = 0.008 + r + 0.003;
    let h = s
        .add_ball(ball_at(profile, v(d, y0, 0.0), v(0.0, -speed, 0.0)))
        .unwrap();
    let mut ev = BallEvents::new();
    let mut hit = false;
    for _ in 0..200 {
        ev.clear();
        s.step_with_bodies(&geo, &mut [&mut set], DT, &mut ev);
        if body_impacts(&ev, 0) > 0 {
            hit = true;
            break;
        }
    }
    assert!(hit);
    let b = *s.ball(h).unwrap();
    let rot = set.bodies[0];
    let e = f64::from(pair_restitution(profile, Material::Rubber));
    let (m, d, v0, i) = (
        f64::from(m),
        f64::from(d),
        f64::from(speed),
        f64::from(inertia),
    );
    let j = (1.0 + e) * v0 / (1.0 / m + d * d / i);
    let vy = -v0 + j / m;
    let w = -j * d / i;
    let l0 = -m * d * v0;
    let pb = b.position.as_dvec3();
    let vbv = b.velocity.as_dvec3();
    let ib = f64::from(profiles()[profile.index()].moment_of_inertia());
    let l1 = i * f64::from(rot.omega)
        + m * (pb.x * vbv.y - pb.y * vbv.x)
        + ib * f64::from(b.angular_velocity.z);
    [f64::from(b.velocity.y), f64::from(rot.omega), vy, w, l0, l1]
}

#[test]
fn free_ball_on_free_pivot_rod_matches_analytic() {
    for (profile, d, speed, inertia) in [
        (STEEL, 0.05f32, 2.0f32, 2.5e-4f32),
        (STEEL, 0.02, 4.0, 2.5e-4),
        (POWERBALL, 0.07, 1.0, 1.0e-4),
        (GLOBALL, 0.03, 6.0, 5.0e-4),
        (JUMBO, 0.06, 3.0, 2.0e-4),
    ] {
        let [vy, w, evy, ew, l0, l1] = rod_hit(profile, d, speed, inertia);
        let f64speed = f64::from(speed);
        assert!(
            (vy - evy).abs() <= 0.005 * f64speed,
            "{profile:?}: vy {vy} expected {evy}"
        );
        assert!((w - ew).abs() <= 0.005 * ew.abs(), "{profile:?}: ω {w} expected {ew}");
        assert!((l1 - l0).abs() <= 0.005 * l0.abs(), "L {l0} → {l1}");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn two_body_impulse_exchange(
        d in 0.006f32..0.075, speed in 0.3f32..10.0, inertia in 5.0e-5f32..2.0e-3, p in 0usize..6,
    ) {
        let [vy, w, evy, ew, l0, l1] = rod_hit(ProfileId(p as u16), d, speed, inertia);
        prop_assert!((vy - evy).abs() <= 0.005 * f64::from(speed), "vy {} vs {}", vy, evy);
        prop_assert!((w - ew).abs() <= 0.005 * ew.abs(), "ω {} vs {}", w, ew);
        prop_assert!((l1 - l0).abs() <= 0.005 * l0.abs(), "L {} vs {}", l0, l1);
    }

    /// A kinematic body at rest never adds energy (restitution ≤ 1,
    /// friction and spin included).
    #[test]
    fn kinematic_body_at_rest_never_adds_energy(
        vx in -20.0f32..-0.01, vy in -5.0f32..5.0, vz in -5.0f32..5.0,
        wx in -300.0f32..300.0, wy in -300.0f32..300.0, wz in -300.0f32..300.0,
        p in 0usize..6, mat in 0usize..7,
    ) {
        let world = empty_world();
        let geo = Geometry::new(&world);
        let mut s = solver_no_gravity();
        let mut body = SlabBody::new(v(-0.05, 0.0, 0.0), v(0.01, 0.5, 0.5), Vec3::ZERO);
        body.material = Material::ALL[mat];
        let mut set = Slab::one(body);
        let mut b = ball_at(ProfileId(p as u16), v(0.0, 0.0, 0.0), v(vx, vy, vz));
        b.angular_velocity = v(wx, wy, wz);
        s.add_ball(b).unwrap();
        let mut ev = BallEvents::new();
        let mut e0 = kinetic_energy(&s);
        for _ in 0..60 {
            s.step_with_bodies(&geo, &mut [&mut set], DT, &mut ev);
            let e1 = kinetic_energy(&s);
            prop_assert!(e1 <= e0 * (1.0 + 1e-6) + 1e-12, "{} → {}", e0, e1);
            e0 = e1;
        }
    }
}

// ------------------------------------------------------- resting contacts

#[test]
fn ball_rests_stably_on_a_static_registered_body() {
    // No static world at all: the floor is a kinematic body at rest.
    let world = empty_world();
    let geo = Geometry::new(&world);
    let mut floor = Slab::one(SlabBody::new(
        v(0.0, 0.0, -0.05),
        v(1.0, 1.0, 0.05),
        Vec3::ZERO,
    ));
    for p in [STEEL, POWERBALL, MINI, JUMBO] {
        let mut s = solver();
        let r = radius(p);
        let mut b = ball_at(p, v(0.0, 0.0, r + 0.02), v(0.2, 0.0, 0.0));
        b.angular_velocity = v(0.0, -5.0, 0.0);
        let h = s.add_ball(b).unwrap();
        let mut ev = BallEvents::new();
        for _ in 0..3000 {
            ev.clear();
            s.step_with_bodies(&geo, &mut [&mut floor], DT, &mut ev);
        }
        let settled = *s.ball(h).unwrap();
        for _ in 0..2000 {
            ev.clear();
            s.step_with_bodies(&geo, &mut [&mut floor], DT, &mut ev);
            assert_eq!(body_impacts(&ev, 0), 0, "{p:?} bounced");
            let b = s.ball(h).unwrap();
            assert!((b.position.z - r).abs() < 2.0e-5, "{p:?} z {}", b.position.z);
        }
        let end = *s.ball(h).unwrap();
        assert!(end.velocity.z.abs() < 1.0e-4, "{p:?} {:?}", end.velocity);
        assert!(
            (end.position.z - settled.position.z).abs() < 1.0e-6,
            "{p:?} drifts {} → {}",
            settled.position.z,
            end.position.z
        );
    }
}

#[test]
fn ball_rolls_down_a_held_bat_and_stays_cradled() {
    // A pitched table; a bat (finite inertia, hold torque against its upper
    // stop) points up and to the right; a wall post below the pivot closes
    // the cradle. The ball starts near the tip, rolls down the bat and
    // stays in the cradle.
    let pitch = 6.5f32.to_radians();
    let mut cfg = BallSolverConfig::from_defaults(&PhysicsDefaults::builtin());
    cfg.gravity = v(0.0, -9.81 * libm::sinf(pitch), -9.81 * libm::cosf(pitch));
    let mut s = BallSolver::new(cfg, &profiles());
    let r = radius(STEEL);
    let mut wb = pin_physics::world::StaticWorldBuilder::new();
    wb.add_collider(
        cuboid(v(0.0, 0.0, -0.05), v(0.5, 0.5, 0.05)),
        layers::ALL,
        true,
    );
    // A wall just left of the pivot.
    wb.add_collider(
        cuboid(v(-0.02, 0.0, 0.02), v(0.005, 0.1, 0.02)),
        layers::ALL,
        true,
    );
    let world = wb.build();
    let geo = Geometry::new(&world);
    let hold = 0.25f32;
    let mut bat = RotorBody::new(v(0.0, 0.0, r), 0.075, 0.009, 2.5e-4, 0.35);
    bat.limits = (-0.5, 0.35);
    bat.torque = hold;
    let mut set = Rotors::one(bat);
    // On top of the bat near the tip (bat surface + radius along its normal).
    let dir = RotorBody::direction(0.35);
    let nrm = Vec3::Z.cross(dir);
    let start = v(0.0, 0.0, r) + dir * 0.06 + nrm * (0.009 + r + 0.0005);
    let h = s.add_ball(ball_at(STEEL, start, Vec3::ZERO)).unwrap();
    let mut ev = BallEvents::new();
    let mut min_gap = f32::INFINITY;
    let mut along = Vec::new();
    for tick in 0..4000 {
        ev.clear();
        s.step_with_bodies(&geo, &mut [&mut set], DT, &mut ev);
        let b = *s.ball(h).unwrap();
        let gap = set.bodies[0].closest(0.0, b.position).distance - r;
        min_gap = min_gap.min(gap);
        if tick >= 2000 {
            along.push(b.position);
        }
    }
    let b = *s.ball(h).unwrap();
    // Rolled down the bat into the cradle, stays there, on the bat.
    let proj = (b.position - set.bodies[0].pivot).dot(dir);
    assert!(proj < 0.03, "ball still up the bat: {proj}");
    assert!(min_gap > -2.0e-4, "ball sank into the bat by {min_gap}");
    let first = along[0];
    for p in &along {
        assert!((*p - first).length() < 1.0e-3, "not cradled: {first:?} → {p:?}");
    }
    assert!((b.position.z - r).abs() < 1.0e-4);
    // The bat is still held near its stop.
    assert!(set.bodies[0].angle > 0.3, "{}", set.bodies[0].angle);
    assert!(b.velocity.length() < 1.0e-2, "{:?}", b.velocity);
}

#[test]
fn a_flip_launches_a_cradled_ball() {
    // Same idea on the reference table: the left flipper fires under a
    // ball resting against it; the ball leaves fast and the bat is pushed
    // back (it slows down compared with an empty flip).
    let mut t = FlipperTable::new(0);
    let r = radius(STEEL);
    let bat = t.rotors.bodies[0];
    let dir = RotorBody::direction(bat.angle);
    let nrm = Vec3::Z.cross(dir);
    let start = bat.pivot + dir * 0.05 + nrm * (bat.radius + r + 0.0002);
    let h = t
        .solver
        .add_ball(ball_at(STEEL, v(start.x, start.y, r), Vec3::ZERO))
        .unwrap();
    let mut ev = BallEvents::new();
    for _ in 0..200 {
        ev.clear();
        t.step(&mut ev);
    }
    let mut empty = t.rotors.clone();
    empty.bodies[0].torque = FLIP_TORQUE;
    t.rotors.bodies[0].torque = FLIP_TORQUE;
    let mut top = 0.0f32;
    for k in 0..12 {
        ev.clear();
        t.step(&mut ev);
        empty.begin_tick(DT);
        empty.end_tick(DT);
        top = top.max(t.solver.ball(h).unwrap().velocity.length());
        if k == 4 {
            assert!(
                t.rotors.bodies[0].omega < empty.bodies[0].omega,
                "the ball does not push the bat back"
            );
        }
    }
    assert!(top > 2.0, "flip too weak: {top}");
}

// ------------------------------------------------- contract and plumbing

#[derive(Debug, Clone, Copy, PartialEq)]
enum Call {
    Begin(f32),
    Query(f32),
    Impulse(f32),
    End(f32),
}

/// A recording wrapper around a rotor set.
struct Recorder {
    inner: Rotors,
    log: RefCell<Vec<Call>>,
}

impl MovingBodies for Recorder {
    fn body_count(&self) -> usize {
        self.inner.body_count()
    }
    fn begin_tick(&mut self, dt: f32) {
        self.log.borrow_mut().push(Call::Begin(dt));
        self.inner.begin_tick(dt);
    }
    fn end_tick(&mut self, dt: f32) {
        self.log.borrow_mut().push(Call::End(dt));
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
        self.log.borrow_mut().push(Call::Query(time));
        self.inner.closest_point(body, time, point)
    }
    fn velocity_at(&self, body: usize, time: f32, point: Vec3) -> Vec3 {
        self.log.borrow_mut().push(Call::Query(time));
        self.inner.velocity_at(body, time, point)
    }
    fn inverse_mass_at(&self, body: usize, time: f32, point: Vec3, dir: Vec3) -> f32 {
        self.log.borrow_mut().push(Call::Query(time));
        self.inner.inverse_mass_at(body, time, point, dir)
    }
    fn apply_impulse(&mut self, body: usize, time: f32, point: Vec3, impulse: Vec3) {
        self.log.borrow_mut().push(Call::Impulse(time));
        self.inner.apply_impulse(body, time, point, impulse);
    }
}

#[test]
fn call_order_contract() {
    let world = floor_world();
    let geo = Geometry::new(&world);
    let mut s = solver();
    let r = radius(STEEL);
    let mut bat = RotorBody::new(v(0.0, 0.0, r), 0.075, 0.009, 2.5e-4, -0.5);
    bat.limits = (-0.5, 0.6);
    bat.torque = 1.5;
    let mut rec = Recorder {
        inner: Rotors::one(bat),
        log: RefCell::new(Vec::new()),
    };
    for k in 0..3 {
        let p = RotorBody::direction(-0.5) * (0.02 + 0.025 * k as f32)
            + Vec3::Z.cross(RotorBody::direction(-0.5)) * (0.009 + r + 0.001 * k as f32);
        s.add_ball(ball_at(STEEL, v(p.x, p.y, r), v(0.0, -0.3, 0.0)))
            .unwrap();
    }
    let mut ev = BallEvents::new();
    let mut impulses = 0;
    for _ in 0..40 {
        rec.log.borrow_mut().clear();
        ev.clear();
        s.step_with_bodies(&geo, &mut [&mut rec], DT, &mut ev);
        let log = rec.log.borrow();
        assert_eq!(log.first(), Some(&Call::Begin(DT)));
        assert_eq!(log.last(), Some(&Call::End(DT)));
        assert_eq!(log.iter().filter(|c| matches!(c, Call::Begin(_))).count(), 1);
        assert_eq!(log.iter().filter(|c| matches!(c, Call::End(_))).count(), 1);
        let mut last_impulse = 0.0f32;
        for c in &log[1..log.len() - 1] {
            match *c {
                Call::Query(t) => {
                    assert!((0.0..=DT).contains(&t), "query at {t}");
                    assert!(t >= last_impulse, "query at {t} after impulse at {last_impulse}");
                }
                Call::Impulse(t) => {
                    assert!((0.0..=DT).contains(&t));
                    assert!(t >= last_impulse, "impulses out of order");
                    last_impulse = t;
                    impulses += 1;
                }
                _ => panic!("begin/end inside the tick"),
            }
        }
    }
    assert!(impulses > 3, "{impulses}");
    // Bodies are called even without balls; a bad dt calls nothing.
    let mut empty = solver();
    rec.log.borrow_mut().clear();
    empty.step_with_bodies(&geo, &mut [&mut rec], DT, &mut ev);
    assert_eq!(*rec.log.borrow(), vec![Call::Begin(DT), Call::End(DT)]);
    rec.log.borrow_mut().clear();
    empty.step_with_bodies(&geo, &mut [&mut rec], f32::NAN, &mut ev);
    empty.step_with_bodies(&geo, &mut [&mut rec], 0.0, &mut ev);
    assert!(rec.log.borrow().is_empty());
}

#[test]
fn impact_target_names_set_and_body_and_layers_filter() {
    let world = empty_world();
    let geo = Geometry::new(&world);
    let mut s = solver_no_gravity();
    let mut a = Slab::default();
    a.bodies
        .push(SlabBody::new(v(0.0, 1.0, 0.0), v(0.01, 0.01, 0.01), Vec3::ZERO));
    let mut b = Slab::default();
    // Body 0 of set 1 is on layer 1 only: the ball (layer 0) passes it.
    let mut ghost = SlabBody::new(v(-0.05, 0.0, 0.0), v(0.002, 0.2, 0.2), Vec3::ZERO);
    ghost.layer_mask = layers::layer_bit(1);
    b.bodies.push(ghost);
    b.bodies
        .push(SlabBody::new(v(-0.1, 0.0, 0.0), v(0.002, 0.2, 0.2), Vec3::ZERO));
    let h = s
        .add_ball(ball_at(STEEL, Vec3::ZERO, v(-2.0, 0.0, 0.0)))
        .unwrap();
    let mut ev = BallEvents::new();
    let mut seen = Vec::new();
    for _ in 0..100 {
        ev.clear();
        s.step_with_bodies(&geo, &mut [&mut a, &mut b], DT, &mut ev);
        for e in ev.as_slice() {
            if let BallEvent::Impact {
                other, normal_speed, ..
            } = e
            {
                seen.push((*other, *normal_speed));
            }
        }
    }
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].0, ImpactTarget::Body { set: 1, body: 1 });
    assert!((seen[0].1 - 2.0).abs() < 1e-4);
    assert!(s.ball(h).unwrap().velocity.x > 0.0);
    assert_eq!(b.bodies[0].impulses, 0);
    assert!(b.bodies[1].impulses > 0);
}

#[test]
fn moving_body_surface_velocity_drags_a_resting_ball() {
    // A kinematic conveyor slab moving along +x under a ball at rest on it:
    // friction brings the contact point to the slab's speed.
    let world = empty_world();
    let geo = Geometry::new(&world);
    let mut belt = Slab::one(SlabBody::new(
        v(0.0, 0.0, -0.05),
        v(5.0, 1.0, 0.05),
        v(0.3, 0.0, 0.0),
    ));
    let mut s = solver();
    let r = radius(STEEL);
    let h = s.add_ball(ball_at(STEEL, v(0.0, 0.0, r), Vec3::ZERO)).unwrap();
    let mut ev = BallEvents::new();
    for _ in 0..500 {
        s.step_with_bodies(&geo, &mut [&mut belt], DT, &mut ev);
    }
    let b = *s.ball(h).unwrap();
    let slip = b.velocity + b.angular_velocity.cross(v(0.0, 0.0, -r)) - v(0.3, 0.0, 0.0);
    assert!(slip.length() < 1e-3, "{slip:?}");
    assert!(b.velocity.x > 0.05, "{:?}", b.velocity);
}

#[test]
fn conservative_cast_edge_cases() {
    let set = Slab::one(SlabBody::new(Vec3::ZERO, v(0.01, 0.01, 0.01), Vec3::ZERO));
    // No window, nothing moving, touching at rest: no hit.
    assert_eq!(conservative_cast(&set, 0, 0.0, v(0.1, 0.0, 0.0), 0.01, Vec3::ZERO, 0.0), None);
    assert_eq!(conservative_cast(&set, 0, 0.0, v(0.1, 0.0, 0.0), 0.01, Vec3::ZERO, 1.0), None);
    assert_eq!(
        conservative_cast(&set, 0, 0.0, v(0.02, 0.0, 0.0), 0.01, Vec3::X, 1.0),
        None
    );
    // Closing in from a distance: hit at the analytic time.
    match conservative_cast(&set, 0, 0.0, v(0.1, 0.0, 0.0), 0.01, -Vec3::X, 1.0) {
        Some(BodyCast::Hit { time, normal, .. }) => {
            assert!((time - 0.08).abs() < 1e-4, "{time}");
            assert_eq!(normal, Vec3::X);
        }
        other => panic!("{other:?}"),
    }
    // Out of the window.
    assert_eq!(
        conservative_cast(&set, 0, 0.0, v(0.1, 0.0, 0.0), 0.01, -Vec3::X, 0.05),
        None
    );
    // Sliding along a face just outside the skin: no progress toward a
    // contact, so it gives up after the iteration budget with an Advance.
    match conservative_cast(&set, 0, 0.0, v(-0.005, 0.02 + 2e-5, 0.0), 0.01, Vec3::X, 1.0) {
        Some(BodyCast::Advance { time }) => assert!(time > 1e-4 && time < 2e-3, "{time}"),
        other => panic!("{other:?}"),
    }
    // Inside the skin, sliding (not closing in): a resting contact.
    assert_eq!(
        conservative_cast(&set, 0, 0.0, v(-0.005, 0.02 + 5e-6, 0.0), 0.01, Vec3::X, 1.0),
        None
    );
    // Grazing past a corner: the sphere does reach the edge while closing
    // in, a real hit.
    assert!(matches!(
        conservative_cast(&set, 0, 0.0, v(-0.5, 0.02 + 1e-6, 0.0), 0.01, Vec3::X, 1.0),
        Some(BodyCast::Hit { .. })
    ));
    // Non-finite input.
    assert_eq!(
        conservative_cast(&set, 0, 0.0, v(0.1, 0.0, 0.0), 0.01, v(f32::NAN, 0.0, 0.0), 1.0),
        None
    );
}

// ------------------------------------------------------------ determinism

fn table_run() -> (u64, Vec<u32>, usize) {
    let mut t = FlipperTable::new(4);
    let mut ev = BallEvents::new();
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut body_hits = 0;
    for tick in 0..4000u32 {
        t.drive(tick);
        ev.clear();
        t.step(&mut ev);
        body_hits += body_impacts(&ev, 0) + body_impacts(&ev, 1);
        for w in t.state_bits() {
            hash = (hash ^ u64::from(w)).wrapping_mul(0x0000_0100_0000_01b3);
        }
        for e in ev.as_slice() {
            if let BallEvent::Impact {
                ball, normal_speed, ..
            } = e
            {
                hash = (hash ^ u64::from(ball.0)).wrapping_mul(0x0000_0100_0000_01b3);
                hash = (hash ^ u64::from(normal_speed.to_bits()))
                    .wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
    }
    (hash, t.state_bits(), body_hits)
}

#[test]
fn flipper_table_is_bit_identical_over_ten_runs() {
    let first = table_run();
    assert!(first.2 > 10, "the bodies were hardly hit: {}", first.2);
    for _ in 0..9 {
        assert_eq!(table_run(), first);
    }
}

#[test]
fn flipper_table_balls_stay_on_the_table() {
    let mut t = FlipperTable::new(6);
    let mut ev = BallEvents::new();
    for tick in 0..6000u32 {
        t.drive(tick);
        ev.clear();
        t.step(&mut ev);
        for (_, b) in t.solver.balls() {
            assert!(b.position.is_finite() && b.velocity.is_finite());
            assert!(
                b.position.x.abs() < 0.26 && b.position.y > -0.41 && b.position.y < 0.51,
                "tick {tick}: escaped {:?}",
                b.position
            );
            assert!(b.position.z > 0.0 && b.position.z < 0.1, "{:?}", b.position);
        }
    }
}
#[test]
fn dbg_rotor() {
    let world = empty_world();
    let geo = Geometry::new(&world);
    let mut s = solver_no_gravity();
    let r = radius(JUMBO);
    let mut rotor = RotorBody::new(Vec3::ZERO, 0.07, 0.008, 2.5e-4, -0.6);
    rotor.omega = 0.5 / rotor.reach();
    rotor.limits = (-0.6, 1.2);
    let mut set = Rotors::one(rotor);
    let h = s.add_ball(ball_at(JUMBO, v(0.07, 0.0, 0.0), v(0.0, -2.0, 0.0))).unwrap();
    let mut ev = BallEvents::new();
    for t in 0..80 {
        ev.clear();
        s.step_with_bodies(&geo, &mut [&mut set], DT, &mut ev);
        let b = s.ball(h).unwrap();
        let cp = set.bodies[0].closest(0.0, b.position);
        println!("{t} pos {:?} vel {:?} ang {} om {} gap {} n {:?} ev {:?}", b.position, b.velocity, set.bodies[0].angle, set.bodies[0].omega, cp.distance - r, cp.normal, ev.as_slice());
    }
}
