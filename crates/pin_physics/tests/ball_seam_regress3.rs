//! Seam task 2.2-S, fix round 3: two balls on one finite-mass body. A ball
//! resting on a body that another ball knocks into it mid-tick is pushed
//! away, not passed through (the Reviewer's R3 repro, a rotor sweep and a
//! translating slider), and a starving pinch does not let an unrelated ball
//! through a flipper (per-ball solve budget).
#[path = "ball_seam_ref.rs"]
mod reference;
#[path = "ball_util.rs"]
mod util;
use glam::Vec3;
use pin_physics::ball::{BallEvents, Geometry, MovingBodies};
use reference::*;
use util::*;

/// A free reference bat (no drive, no stops, no gravity). Ball A rests on its
/// top at 60 mm; ball B hits its underside at 45 mm in the middle of a tick.
/// The bat is knocked into A, which must be pushed away, not passed through.
/// Returns A's deepest penetration and whether it ended up under the bat.
fn knock(inertia: f32, vb: f32) -> (f32, bool) {
    let world = empty_world();
    let geo = Geometry::new(&world);
    let mut s = solver_no_gravity();
    let r = radius(STEEL);
    let mut set = Rotors::one(RotorBody::new(v(0.0, 0.0, r), 0.076, 0.009, inertia, 0.0));
    let a = s
        .add_ball(ball_at(STEEL, v(0.06, 0.009 + r + 2.0e-6, r), Vec3::ZERO))
        .unwrap();
    s.add_ball(ball_at(
        STEEL,
        v(0.045, -(0.009 + r) - vb * 0.003 - 0.004, r),
        v(0.0, vb, 0.0),
    ))
    .unwrap();
    let mut ev = BallEvents::new();
    let (mut deepest, mut through, mut prev) = (0.0f32, false, 1.0f32);
    for _ in 0..30 {
        ev.clear();
        s.step_with_bodies(&geo, &mut [&mut set], DT, &mut ev);
        let ball = s.ball(a).unwrap();
        // `max` drops a NaN operand: check before reducing.
        assert!(
            ball.position.is_finite() && ball.velocity.is_finite(),
            "ball state is not finite: {ball:?}"
        );
        let bat = set.bodies[0];
        deepest = deepest.max(r - bat.closest(0.0, ball.position).distance);
        let dir = RotorBody::direction(bat.angle);
        let rel = ball.position - bat.pivot;
        let (along, side) = (rel.dot(dir), rel.dot(Vec3::Z.cross(dir)));
        through |= along > 0.0 && along < 0.076 && side < 0.0 && prev > 0.0;
        prev = side;
    }
    (deepest, through)
}

#[test]
fn r3_bat_knocked_into_a_resting_ball_pushes_it() {
    let (deep, _) = knock(2.5e-4, 10.0);
    assert!(
        deep < 1.0e-3,
        "10 m/s: resting ball sank {deep} m into the bat"
    );
    let (deep, through) = knock(2.5e-4, 30.0);
    assert!(
        !through && deep < 3.0e-3,
        "30 m/s: sank {deep} m, tunnelled {through}"
    );
}

/// Inertias and speeds of the knock sweep: deepest sink stays well inside
/// the 9 mm capsule radius (under 1 mm) and the ball is never passed through.
#[test]
fn knocked_rotor_sweep() {
    for inertia in [1.0e-4f32, 2.5e-4, 1.0e-3] {
        for vb in [2.0f32, 5.0, 10.0, 20.0, 30.0] {
            let (deep, through) = knock(inertia, vb);
            println!(
                "rotor I {inertia} vb {vb}: deepest {:.3} mm through {through}",
                deep * 1e3
            );
            assert!(!through, "I {inertia} vb {vb}: tunnelled");
            assert!(deep < 1.0e-3, "I {inertia} vb {vb}: sank {deep} m");
        }
    }
}

/// The same with a translating slider (a 4 mm thick plate of `mass` kg on
/// the y axis): ball A rests on top, ball B hits the underside.
fn knock_slider(mass: f32, vb: f32) -> (f32, bool) {
    let world = empty_world();
    let geo = Geometry::new(&world);
    let mut s = solver_no_gravity();
    let r = radius(STEEL);
    let half = v(0.05, 0.002, 0.02);
    let mut set = Sliders::default();
    set.bodies
        .push(SliderBody::new(v(0.0, 0.0, r), half, Vec3::Y, mass));
    let a = s
        .add_ball(ball_at(STEEL, v(0.01, half.y + r + 2.0e-6, r), Vec3::ZERO))
        .unwrap();
    s.add_ball(ball_at(
        STEEL,
        v(-0.01, -(half.y + r) - vb * 0.003 - 0.004, r),
        v(0.0, vb, 0.0),
    ))
    .unwrap();
    let mut ev = BallEvents::new();
    let (mut deepest, mut through) = (0.0f32, false);
    for _ in 0..30 {
        ev.clear();
        s.step_with_bodies(&geo, &mut [&mut set], DT, &mut ev);
        let ball = s.ball(a).unwrap();
        // `max` drops a NaN operand: check before reducing.
        assert!(
            ball.position.is_finite() && ball.velocity.is_finite(),
            "ball state is not finite: {ball:?}"
        );
        let plate = set.bodies[0];
        deepest = deepest.max(r - plate.closest(0.0, ball.position).distance);
        through |= ball.position.y < plate.center.y;
    }
    (deepest, through)
}

#[test]
fn knocked_slider_sweep() {
    for mass in [0.02f32, 0.05, 0.2] {
        for vb in [2.0f32, 10.0, 30.0] {
            let (deep, through) = knock_slider(mass, vb);
            println!(
                "slider m {mass} vb {vb}: deepest {:.3} mm through {through}",
                deep * 1e3
            );
            assert!(!through, "m {mass} vb {vb}: tunnelled");
            assert!(deep < 1.0e-3, "m {mass} vb {vb}: sank {deep} m");
        }
    }
}

// --------------------------------------------- per-ball budget (N1)

/// A broken owner: a plate that never moves but reports a surface velocity
/// of 100 m/s into the ball (kinematic). A ball pressed between it and a
/// wall can never be satisfied, so it uses up its body-solve budget every
/// tick.
struct Phantom {
    inner: Slab,
}

impl MovingBodies for Phantom {
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
    fn material(&self, body: usize) -> pin_core::Material {
        self.inner.material(body)
    }
    fn swept_bounds(&self, body: usize) -> (Vec3, Vec3) {
        self.inner.swept_bounds(body)
    }
    fn max_surface_speed(&self, _body: usize) -> f32 {
        100.0
    }
    fn closest_point(&self, body: usize, t: f32, p: Vec3) -> Option<pin_physics::ball::BodyPoint> {
        self.inner.closest_point(body, t, p)
    }
    fn velocity_at(&self, _body: usize, _t: f32, _p: Vec3) -> Vec3 {
        Vec3::X * 100.0
    }
    fn inverse_mass_at(&self, _body: usize, _t: f32, _p: Vec3, _d: Vec3) -> f32 {
        0.0
    }
    fn apply_impulse(&mut self, body: usize, t: f32, p: Vec3, j: Vec3) {
        self.inner.apply_impulse(body, t, p, j);
    }
}

/// Runs the shot (ball shot at 20 m/s at an honest flipper) with or without
/// a pinched ball starving at a phantom plate elsewhere; returns the shot's
/// final state, its deepest sink into the flipper, its flipper hits and the
/// number of ticks the pinched ball was starved.
fn starving_scene(with_pinch: bool) -> (pin_physics::ball::Ball, f32, usize, usize) {
    let r = radius(STEEL);
    let mut wb = pin_physics::world::StaticWorldBuilder::new();
    // A wall the phantom plate presses ball 1 against, far from the flipper.
    wb.add_collider(
        cuboid(v(0.5, 0.0, r), v(0.01, 0.05, 0.05)),
        pin_physics::world::layers::ALL,
        true,
    );
    let world = wb.build();
    let geo = Geometry::new(&world);
    let mut s = solver_no_gravity();
    let mut phantom = Phantom {
        inner: Slab::one(SlabBody::new(
            v(0.49 - 2.0 * r - 0.002, 0.0, r),
            v(0.002, 0.05, 0.05),
            Vec3::ZERO,
        )),
    };
    let pinched = with_pinch.then(|| {
        s.add_ball(ball_at(STEEL, v(0.49 - r - 1.0e-6, 0.0, r), Vec3::ZERO))
            .unwrap()
    });
    let mut flipper = Rotors::one(RotorBody::new(v(0.0, 0.0, r), 0.076, 0.009, 2.5e-4, 0.0));
    let shot = s
        .add_ball(ball_at(STEEL, v(0.04, 0.12, r), v(0.0, -20.0, 0.0)))
        .unwrap();
    let mut ev = BallEvents::new();
    let (mut starved, mut hits, mut deepest) = (0, 0, 0.0f32);
    for _ in 0..30 {
        ev.clear();
        s.step_with_bodies(&geo, &mut [&mut phantom, &mut flipper], DT, &mut ev);
        let used = s.body_solves_last_step();
        if let Some(p) = pinched
            && used[usize::from(p.0)] >= pin_physics::ball::MAX_BODY_SOLVES_PER_BALL
        {
            starved += 1;
        }
        assert!(used[usize::from(shot.0)] < pin_physics::ball::MAX_BODY_SOLVES_PER_BALL);
        let b = s.ball(shot).unwrap();
        deepest = deepest.max(r - flipper.bodies[0].closest(0.0, b.position).distance);
        hits += ev
            .as_slice()
            .iter()
            .filter(|e| {
                matches!(e, pin_physics::ball::BallEvent::Impact { ball, other: pin_physics::ball::ImpactTarget::Body { set: 1, .. }, .. } if *ball == shot)
            })
            .count();
    }
    (*s.ball(shot).unwrap(), deepest, hits, starved)
}

#[test]
fn a_starving_pinch_does_not_open_an_unrelated_flipper() {
    let (alone, _, alone_hits, _) = starving_scene(false);
    let (shot, deepest, hits, starved) = starving_scene(true);
    println!("pinched ball starved in {starved} of 30 ticks; shot sank {deepest} m");
    // The pinched ball really was starved, every tick ...
    assert_eq!(starved, 30);
    // ... and the shot hit the flipper exactly as without the pinch.
    assert!(
        deepest < 1.0e-3,
        "the shot sank {deepest} m into the flipper"
    );
    assert!(hits >= 1 && hits == alone_hits, "{hits} vs {alone_hits}");
    assert!(
        (shot.velocity - alone.velocity).length() < 1.0e-3,
        "{:?} vs {:?}",
        shot.velocity,
        alone.velocity
    );
    assert!((shot.position - alone.position).length() < 1.0e-4);
}

// ------------------------------------------------- flip sweep depth (N8)

/// A ball in free flight (no gravity) comes down on a flipper resting on
/// its lower stop that flips (`torque`) after `delay` ticks. Returns the
/// ball's deepest penetration into the bat.
fn flip_depth(d: f32, speed: f32, delay: u32, torque: f32, inertia: f32) -> f32 {
    let world = empty_world();
    let geo = Geometry::new(&world);
    let mut s = solver_no_gravity();
    let r = radius(STEEL);
    let (l, br) = (0.076f32, 0.009f32);
    let mut bat = RotorBody::new(v(0.0, 0.0, r), l, br, inertia, -0.5);
    bat.limits = (-0.5, 0.5);
    bat.torque = -0.05;
    let mut set = Rotors::one(bat);
    let yc = d * libm::tanf(-0.5) + (br + r) / libm::cosf(0.5);
    let start = v(d, yc + speed * 0.020 + 0.0005, r);
    let h = s
        .add_ball(ball_at(STEEL, start, v(0.0, -speed, 0.0)))
        .unwrap();
    let mut ev = BallEvents::new();
    let mut deepest = 0.0f32;
    for tick in 0..160u32 {
        if tick == delay {
            set.bodies[0].torque = torque;
        }
        ev.clear();
        s.step_with_bodies(&geo, &mut [&mut set], DT, &mut ev);
        let b = s.ball(h).unwrap();
        // `max` drops a NaN operand: check before reducing.
        assert!(
            b.position.is_finite() && b.velocity.is_finite(),
            "ball state is not finite: {b:?}"
        );
        deepest = deepest.max(r - set.bodies[0].closest(0.0, b.position).distance);
    }
    deepest
}

#[test]
fn flip_sweep_penetration_stays_small() {
    for (torque, inertia) in [(1.2f32, 1.0e-4f32), (1.2, 2.5e-4), (3.0, 1.0e-4)] {
        let mut worst = (0.0f32, String::new());
        for d in [0.01f32, 0.025, 0.04, 0.055, 0.07, 0.078, 0.084] {
            for speed in [0.1f32, 0.3, 1.0, 2.0, 4.0, 7.0, 10.0] {
                for delay in [0u32, 4, 8, 10, 12, 14, 16, 17, 18, 19, 20, 21, 22, 24, 5000] {
                    let deep = flip_depth(d, speed, delay, torque, inertia);
                    if deep > worst.0 {
                        worst = (deep, format!("d {d} v {speed} delay {delay}"));
                    }
                }
            }
        }
        println!(
            "flip sweep torque {torque} I {inertia}: deepest {:.4} mm at {}",
            worst.0 * 1e3,
            worst.1
        );
        assert!(
            worst.0 < 1.0e-4,
            "torque {torque} I {inertia}: {} m at {}",
            worst.0,
            worst.1
        );
    }
}
