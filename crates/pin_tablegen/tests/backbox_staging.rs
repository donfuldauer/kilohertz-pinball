//! Subphase 5.5 test (c): the three staging modes (dedicated, parallel
//! linked, dual-stage) actuate the generated vertical backbox flippers on
//! the expected tick. The flippers come from the generated `table.toml`
//! through the runtime loader; input goes through `pin_input`'s ingest and
//! `FlipperStagingRouter` (the engine's path); the bats are stepped with the
//! physics solver on the generated backbox colliders.

#[path = "backbox_common.rs"]
mod common;

use common::*;
use pin_assets::FlipperStageHint;
use pin_core::PhysicsDefaults;
use pin_core::input::{CabinetAxis, CabinetInput, InputEvent};
use pin_input::events::{IngestConfig, InputIngest, NS_PER_TICK};
use pin_input::staging::{FlipperStage, FlipperStagingRouter, StagingEntry};
use pin_physics::ball::{BallEvents, BallSolver, BallSolverConfig, Geometry};
use pin_physics::flipper::FlipperSet;
use pin_physics::world::StaticWorldBuilder;
use pin_tablegen::backbox::BACKBOX_PREFIX;

#[test]
fn staging_modes_actuate_the_backbox_flippers_on_their_tick() {
    let (dir, table, g) = load("staging");
    let _ = std::fs::remove_dir_all(dir);
    let ids: Vec<usize> = table
        .manifest
        .flippers
        .iter()
        .enumerate()
        .filter(|(_, f)| f.id.starts_with(BACKBOX_PREFIX))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(ids.len(), 3);
    // Backbox flippers are not on layer 0: the loader stages them Upper.
    for &i in &ids {
        assert_eq!(table.flipper_stages[i], FlipperStageHint::Upper);
    }
    let configs: Vec<_> = ids.iter().map(|&i| table.flippers[i].clone()).collect();
    let mut flippers = FlipperSet::new(&configs).expect("valid backbox flippers");
    let entries: Vec<StagingEntry> = configs
        .iter()
        .map(|c| StagingEntry {
            flipper: c.id,
            mode: c.staging,
            stage: FlipperStage::Upper,
        })
        .collect();
    let mut router = FlipperStagingRouter::new(&entries).unwrap();
    let mut world = StaticWorldBuilder::new();
    for c in &g.colliders {
        world.add_collider(c.shape.clone(), c.layer_mask, true);
    }
    let world = world.build();
    let mut cfg = BallSolverConfig::from_defaults(&PhysicsDefaults::builtin());
    let p = table.pitch_rad;
    cfg.gravity = glam::Vec3::new(0.0, -9.81 * libm::sinf(p), -9.81 * libm::cosf(p));
    let mut solver = BallSolver::new(cfg, &[]);
    let mut ingest = InputIngest::new(IngestConfig::default());
    // Dedicated (left_upper) at tick 20; parallel linked (right_flipper) at
    // 50; the dual-stage axis passes 0.30 at 80 (a lower-stage flipper would
    // fire; this Upper one must not) and 0.85 at 110.
    let ms = NS_PER_TICK;
    let events = [
        InputEvent::button(20 * ms, CabinetInput::LeftUpper, true),
        InputEvent::button(50 * ms, CabinetInput::RightFlipper, true),
        InputEvent::axis(80 * ms, CabinetAxis::LeftFlipper, 0.5),
        InputEvent::axis(110 * ms, CabinetAxis::LeftFlipper, 0.9),
    ];
    for e in events {
        ingest.push(e).unwrap();
    }
    let want = [20u64, 50, 110];
    let rest: Vec<f32> = configs.iter().map(|c| flippers.angle_deg(c.id)).collect();
    let mut first_move = [None::<u64>; 3];
    let mut ball_events = BallEvents::new();
    for t in 0..200u64 {
        let frame = ingest.advance(t);
        // The router's fixed-capacity change list (type from `update`).
        let mut out = Default::default();
        router.update(&frame, &mut out);
        for (id, on) in out.iter() {
            flippers.set_energised(*id, *on);
        }
        flippers.step_tick(t);
        ball_events.clear();
        solver.step_with_bodies(
            &Geometry::new(&world),
            &mut [&mut flippers],
            1.0e-3,
            &mut ball_events,
        );
        for (k, c) in configs.iter().enumerate() {
            if first_move[k].is_none() && flippers.angle_deg(c.id) != rest[k] {
                first_move[k] = Some(t);
            }
        }
    }
    for k in 0..3 {
        assert_eq!(
            first_move[k],
            Some(want[k]),
            "{:?} ({:?}) moves on its input tick",
            configs[k].id,
            configs[k].staging
        );
    }
}
