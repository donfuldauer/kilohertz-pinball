//! Seam task 2.2-S, fix round 3: two balls on one finite-mass body. A ball
//! resting on a body that another ball knocks into it mid-tick is pushed
//! away, not passed through (the Reviewer's R3 repro, a rotor sweep and a
//! translating slider), and a starving pinch does not let an unrelated ball
//! through a flipper (per-ball solve budget).
#[path = "ball_util.rs"]
mod util;
#[path = "ball_seam_ref.rs"]
mod reference;
use glam::Vec3;
use pin_physics::ball::{BallEvents, Geometry};
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
    let a = s.add_ball(ball_at(STEEL, v(0.06, 0.009 + r + 2.0e-6, r), Vec3::ZERO)).unwrap();
    s.add_ball(ball_at(STEEL, v(0.045, -(0.009 + r) - vb * 0.003 - 0.004, r), v(0.0, vb, 0.0)))
        .unwrap();
    let mut ev = BallEvents::new();
    let (mut deepest, mut through, mut prev) = (0.0f32, false, 1.0f32);
    for _ in 0..30 {
        ev.clear();
        s.step_with_bodies(&geo, &mut [&mut set], DT, &mut ev);
        let ball = s.ball(a).unwrap();
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
    assert!(deep < 1.0e-3, "10 m/s: resting ball sank {deep} m into the bat");
    let (deep, through) = knock(2.5e-4, 30.0);
    assert!(!through && deep < 3.0e-3, "30 m/s: sank {deep} m, tunnelled {through}");
}

/// Inertias and speeds of the knock sweep: deepest sink stays well inside
/// the 9 mm capsule radius (under 1 mm) and the ball is never passed through.
#[test]
fn knocked_rotor_sweep() {
    for inertia in [1.0e-4f32, 2.5e-4, 1.0e-3] {
        for vb in [2.0f32, 5.0, 10.0, 20.0, 30.0] {
            let (deep, through) = knock(inertia, vb);
            println!("rotor I {inertia} vb {vb}: deepest {:.3} mm through {through}", deep * 1e3);
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
            println!("slider m {mass} vb {vb}: deepest {:.3} mm through {through}", deep * 1e3);
            assert!(!through, "m {mass} vb {vb}: tunnelled");
            assert!(deep < 1.0e-3, "m {mass} vb {vb}: sank {deep} m");
        }
    }
}

// --------------------------------------------- per-ball budget (N1)

/// A plate that moves *toward* whatever pushes it (a broken owner): every
/// contact solve against it fails to converge, so a ball pressed between it
/// and a wall uses up its body-solve budget every tick.
struct Contrary {
    inner: Sliders,
}

impl MovingBodies for Contrary {
    fn body_count(&self) -> usize {
        self.inner.body_count()
    }
    fn begin_tick(&mut self, dt: f32) {
        self.inner.begin_tick(dt);
    }
    fn end_tick(&mut self, dt: f32) {
        self.inner.end_tick(dt);
        // Keep it near the wall whatever it did.
        let b = &mut self.inner.bodies[0];
        b.speed = b.speed.clamp(-1.0, 1.0);
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
    fn max_surface_speed(&self, body: usize) -> f32 {
        self.inner.max_surface_speed(body)
    }
    fn closest_point(&self, body: usize, t: f32, p: Vec3) -> Option<pin_physics::ball::BodyPoint> {
        self.inner.closest_point(body, t, p)
    }
    fn velocity_at(&self, body: usize, t: f32, p: Vec3) -> Vec3 {
        self.inner.velocity_at(body, t, p)
    }
    fn inverse_mass_at(&self, body: usize, t: f32, p: Vec3, d: Vec3) -> f32 {
        self.inner.inverse_mass_at(body, t, p, d)
    }
    fn apply_impulse(&mut self, body: usize, t: f32, p: Vec3, j: Vec3) {
        self.inner.apply_impulse(body, t, p, -j);
    }
}

#[test]
fn a_starving_pinch_does_not_open_an_unrelated_flipper() {
    let r = radius(STEEL);
    let mut wb = pin_physics::world::StaticWorldBuilder::new();
    // A wall the contrary plate presses ball 1 against, far from the flipper.
    wb.add_collider(
        cuboid(v(0.5, 0.0, r), v(0.01, 0.05, 0.05)),
        pin_physics::world::layers::ALL,
        true,
    );
    let world = wb.build();
    let geo = Geometry::new(&world);
    let mut s = solver_no_gravity();
    let mut plate = Sliders::default();
    plate.bodies.push(SliderBody::new(
        v(0.49 - 2.0 * r - 0.002, 0.0, r),
        v(0.002, 0.05, 0.05),
        Vec3::X,
        0.05,
    ));
    plate.bodies[0].speed = 0.5;
    let mut contrary = Contrary { inner: plate };
    let pinched = s
        .add_ball(ball_at(STEEL, v(0.49 - r - 1.0e-6, 0.0, r), Vec3::ZERO))
        .unwrap();
    // An honest flipper elsewhere, and ball 2 shot at it at 20 m/s.
    let mut flipper = Rotors::one(RotorBody::new(v(0.0, 0.0, r), 0.076, 0.009, 2.5e-4, 0.0));
    let shot = s
        .add_ball(ball_at(STEEL, v(0.04, 0.12, r), v(0.0, -20.0, 0.0)))
        .unwrap();
    let mut ev = BallEvents::new();
    let mut starved_ticks = 0;
    for tick in 0..30 {
        ev.clear();
        s.step_with_bodies(&geo, &mut [&mut contrary, &mut flipper], DT, &mut ev);
        let used = s.body_solves_last_step();
        if used[usize::from(pinched.0)] >= pin_physics::ball::MAX_BODY_SOLVES_PER_BALL {
            starved_ticks += 1;
        }
        assert!(used[usize::from(shot.0)] < pin_physics::ball::MAX_BODY_SOLVES_PER_BALL);
        let b = s.ball(shot).unwrap();
        let bat = flipper.bodies[0];
        let rel = b.position - bat.pivot;
        let dir = RotorBody::direction(bat.angle);
        let along = rel.dot(dir);
        if along > 0.0 && along < 0.076 {
            assert!(
                rel.dot(Vec3::Z.cross(dir)) > 0.0,
                "tick {tick}: the shot passed through the flipper"
            );
        }
        assert!(bat.closest(0.0, b.position).distance > r - 1.0e-3);
    }
    println!("pinched ball starved in {starved_ticks} of 30 ticks");
    assert!(starved_ticks > 5, "the pinch did not starve: {starved_ticks}");
    assert!(s.ball(shot).unwrap().velocity.y > 0.0, "the shot was not returned");
}
