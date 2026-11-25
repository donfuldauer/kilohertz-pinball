//! Subphase 5.5: the generated backbox works in the physics solver. A ball
//! on the backbox layer is held between the glass and the back panel, a
//! generated flipper fires it up the vertical plane, the generated drain
//! transition hands it back to layer 0, and main-deck balls pass through the
//! backbox colliders. Gravity is the table's (pitched) gravity: along the
//! backbox plane it is the full `g`.

#[path = "backbox_common.rs"]
mod common;

use common::*;
use glam::Vec3;
use pin_core::PhysicsDefaults;
use pin_core::fmath::max_f32;
use pin_physics::ball::{Ball, BallEvents, BallSolver, BallSolverConfig, Geometry};
use pin_physics::flipper::FlipperSet;
use pin_physics::world::{LayerTransition, StaticWorld, StaticWorldBuilder, TransitionSet};
use pin_tablegen::backbox::{BACKBOX_PREFIX, BackboxFrame};

const R: f32 = 0.0135;

struct Scene {
    world: StaticWorld,
    solver: BallSolver,
    flippers: FlipperSet,
    /// Loader ids of the backbox flippers, in layout order.
    ids: Vec<pin_core::FlipperId>,
    zones: TransitionSet,
    frame: BackboxFrame,
    tick: u64,
}

fn scene() -> Scene {
    let (dir, table, g) = load("physics");
    let _ = std::fs::remove_dir_all(dir);
    let mut b = StaticWorldBuilder::new();
    for c in &g.colliders {
        b.add_collider(c.shape.clone(), c.layer_mask, true);
    }
    let configs: Vec<_> = table
        .manifest
        .flippers
        .iter()
        .zip(&table.flippers)
        .filter(|(d, _)| d.id.starts_with(BACKBOX_PREFIX))
        .map(|(_, c)| c.clone())
        .collect();
    let mut cfg = BallSolverConfig::from_defaults(&PhysicsDefaults::builtin());
    let p = table.pitch_rad;
    cfg.gravity = Vec3::new(0.0, -9.81 * libm::sinf(p), -9.81 * libm::cosf(p));
    let profiles: Vec<_> = pin_core::profiles::builtin_profiles().collect();
    let zones = TransitionSet::new(
        table
            .transitions
            .iter()
            .map(|t| LayerTransition {
                from_layer: t.from_layer,
                to_layer: t.to_layer,
                min: Vec3::from(t.zone_min_m),
                max: Vec3::from(t.zone_max_m),
            })
            .collect(),
    );
    Scene {
        world: b.build(),
        solver: BallSolver::new(cfg, &profiles),
        flippers: FlipperSet::new(&configs).unwrap(),
        ids: configs.iter().map(|c| c.id).collect(),
        zones,
        frame: g.frame,
        tick: 0,
    }
}

impl Scene {
    /// A ball at backbox-frame `local` (mm) on `layer`.
    fn add(&mut self, local_mm: [f32; 3], layer: u8) -> pin_physics::ball::BallHandle {
        let p = self.frame.to_table(Vec3::from(local_mm) * 1e-3);
        let mut ball = Ball::new(pin_core::ProfileId(0), p);
        ball.layer = layer;
        self.solver.add_ball(ball).unwrap()
    }

    fn local(&self, h: pin_physics::ball::BallHandle) -> Vec3 {
        let b = self.solver.ball(h).unwrap();
        self.frame.rotation.inverse() * (b.position - self.frame.origin_m) * 1e3
    }

    fn step(&mut self) {
        let prev: Vec<_> = self.solver.balls().map(|(h, b)| (h, b.position)).collect();
        self.flippers.step_tick(self.tick);
        let mut ev = BallEvents::new();
        self.solver.step_with_bodies(
            &Geometry::new(&self.world),
            &mut [&mut self.flippers],
            1.0e-3,
            &mut ev,
        );
        for (h, p0) in prev {
            let b = self.solver.ball_mut(h).unwrap();
            b.layer = self.zones.update(b.layer, p0, b.position);
            assert!(b.position.is_finite() && b.velocity.is_finite());
        }
        self.tick += 1;
    }
}

#[test]
fn a_backbox_ball_rests_on_the_flipper_and_is_fired_up_the_plane() {
    let mut s = scene();
    // Above the left flipper (pivot u = −70, v = 50; rest −30°), in the slot.
    let h = s.add([-45.0, 35.0 - R * 1e3 - 1.0, 60.0], 4);
    // It drops onto the bat and starts rolling down it.
    for _ in 0..50 {
        s.step();
    }
    let rest = s.local(h);
    println!("on the bat at {rest:?} mm");
    assert!(
        rest.y > 0.0 && rest.y < 35.0,
        "between glass and back: {rest:?}"
    );
    assert!(
        rest.z > 30.0 && rest.z < 60.0,
        "caught by the bat: {rest:?}"
    );
    let left = s.ids[0];
    s.flippers.set_energised(left, true);
    let mut top = rest.z;
    for _ in 0..300 {
        s.step();
        let p = s.local(h);
        assert!(
            p.y > -1.0 && p.y < 36.0,
            "never through the glass or the back: {p:?}"
        );
        assert!(
            p.x > -201.0 && p.x < 201.0 && p.z < 301.0,
            "inside the box: {p:?}"
        );
        top = max_f32(top, p.z);
    }
    println!("highest point {top:.1} mm");
    assert!(
        top > rest.z + 50.0,
        "the flip sends the ball up the backbox"
    );
}

#[test]
fn the_drain_hands_the_ball_back_to_layer_0() {
    let mut s = scene();
    // Above the drain hole (u = 160).
    let h = s.add([160.0, 35.0 - R * 1e3 - 1.0, 40.0], 4);
    let mut layers = vec![4u8];
    for _ in 0..400 {
        s.step();
        let l = s.solver.ball(h).unwrap().layer;
        if *layers.last().unwrap() != l {
            layers.push(l);
        }
    }
    assert_eq!(layers, vec![4, 0], "drained back to the main deck layer");
}

#[test]
fn main_deck_balls_pass_through_the_backbox() {
    let mut s = scene();
    // A layer-0 ball inside the backbox volume falls straight through
    // every backbox collider (shelf included).
    let h = s.add([-45.0, 17.0, 150.0], 0);
    for _ in 0..400 {
        s.step();
    }
    let p = s.local(h);
    assert!(p.z < -100.0, "fell through the shelf: {p:?}");
}
