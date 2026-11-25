//! Subphase 5.5 spec tests (Independent Tester): flippers on the vertical
//! (90°) backbox plane with the existing staging modes, and determinism of
//! a backbox scene.
//!
//! * The three staging modes of the brief (dedicated buttons, parallel
//!   linked, dual-stage at 30 % / 85 %) actuate a vertical-plane flipper on
//!   the expected tick: the coil is energised (non-zero coil torque) after
//!   the step of the first tick whose routed input asks for it, never
//!   before, and the bat leaves its rest stop on the same tick as the same
//!   flipper on the flat playfield.
//! * Same seed, same inputs: identical ball and flipper state hash over 10
//!   runs.
//!
//! `pin_physics` does not depend on `pin_input`; the cabinet frame is routed
//! by the test-local router of `spec_5_5_common.rs` (the documented
//! semantics) and fed to `FlipperSet::set_energised`.

#[path = "spec_5_5_common.rs"]
mod bb;

use bb::c23::{DT, R};
use bb::{BACKBOX_LAYER, Frame, Mount, Stage, dual_stage, route};
use glam::Vec3;
use pin_core::fmath::min_f32;
use pin_core::ids::FlipperId;
use pin_core::{CabinetInput, FlipperConfig, FlipperStagingMode};
use pin_physics::ball::{BallEvents, Geometry};
use pin_physics::flipper::FlipperSet;
use pin_physics::world::{LayerTransition, StaticWorld, TransitionSet, layers};

const LEFT: FlipperId = FlipperId(0);
const RIGHT: FlipperId = FlipperId(1);

/// Left and right flippers 120 mm apart on `mount`, with staging `left` /
/// `right`.
fn pair(mount: Mount, left: FlipperStagingMode, right: FlipperStagingMode) -> [FlipperConfig; 2] {
    let mut l = bb::left_flipper_at(0, mount, 1.0, Vec3::new(-0.06, 0.0, 0.0));
    let mut r = bb::right_flipper_at(1, mount, Vec3::new(0.06, 0.0, 0.0));
    l.staging = left;
    r.staging = right;
    [l, r]
}

/// Per-flipper ticks: first tick after whose step the coil torque is
/// non-zero, and first tick after whose step the bat is off its rest angle.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct Actuation {
    coil: Option<u64>,
    motion: Option<u64>,
}

/// Runs `cfgs` on `mount` (with the mounting plane in the world, no ball)
/// for `ticks`, routing `frame_at(t)` before each tick. `stages` gives the
/// dual-stage stage of each flipper.
fn actuation(
    mount: Mount,
    cfgs: &[FlipperConfig; 2],
    stages: [Stage; 2],
    ticks: u64,
    frame_at: impl Fn(u64) -> Frame,
) -> [Actuation; 2] {
    let world = bb::plane_world(mount.rot(), layers::layer_bit(mount.layer()));
    let mut set = FlipperSet::new(cfgs).unwrap();
    let mut sol = bb::c23::solver(mount.gravity());
    let mut ev = BallEvents::new();
    let mut out = [Actuation {
        coil: None,
        motion: None,
    }; 2];
    for t in 0..ticks {
        let f = frame_at(t);
        for (i, id) in [LEFT, RIGHT].into_iter().enumerate() {
            set.set_energised(id, route(&cfgs[i], stages[i], &f));
        }
        set.step_tick(t);
        ev.clear();
        sol.step_with_bodies(&Geometry::new(&world), &mut [&mut set], DT, &mut ev);
        for (i, id) in [LEFT, RIGHT].into_iter().enumerate() {
            if out[i].coil.is_none() && set.coil_torque_nm(id) > 0.0 {
                out[i].coil = Some(t);
            }
            let off = (set.angle_deg(id) - cfgs[i].rest_angle_deg).abs();
            if out[i].motion.is_none() && off > 0.01 {
                out[i].motion = Some(t);
            }
        }
    }
    out
}

/// Checks one staging scenario on the vertical plane against the expected
/// coil ticks, and the motion ticks against the flat playfield's.
fn check(
    name: &str,
    left: FlipperStagingMode,
    right: FlipperStagingMode,
    stages: [Stage; 2],
    frame_at: impl Fn(u64) -> Frame + Copy,
    expect_coil: [Option<u64>; 2],
) {
    let v = pair(Mount::Vertical, left, right);
    let h = pair(Mount::Horizontal, left, right);
    assert_eq!(v[0].collision_layer, BACKBOX_LAYER);
    let av = actuation(Mount::Vertical, &v, stages, 200, frame_at);
    let ah = actuation(Mount::Horizontal, &h, stages, 200, frame_at);
    for i in 0..2 {
        assert_eq!(
            av[i].coil, expect_coil[i],
            "{name}: flipper {i} coil tick (vertical)"
        );
        assert_eq!(
            ah[i].coil, expect_coil[i],
            "{name}: flipper {i} coil tick (flat)"
        );
        assert_eq!(
            av[i].motion, ah[i].motion,
            "{name}: flipper {i} leaves its stop on a different tick on the vertical plane"
        );
        match (expect_coil[i], av[i].motion) {
            (Some(c), Some(m)) => assert!(
                m >= c && m <= c + 10,
                "{name}: flipper {i} coil at {c}, motion at {m}"
            ),
            (None, None) => {}
            (c, m) => panic!("{name}: flipper {i} coil {c:?} but motion {m:?}"),
        }
    }
}

#[test]
fn dedicated_buttons_actuate_on_the_press_tick() {
    let left = FlipperStagingMode::DedicatedButton(CabinetInput::LeftFlipper);
    let right = FlipperStagingMode::DedicatedButton(CabinetInput::RightFlipper);
    for (tl, tr) in [(0u64, 0u64), (10, 37), (55, 3), (120, 121)] {
        check(
            "dedicated",
            left,
            right,
            [Stage::Lower; 2],
            move |t| Frame {
                left: t >= tl,
                right: t >= tr,
                axis: 0.0,
            },
            [Some(tl), Some(tr)],
        );
    }
    // Only the pressed button's flipper moves.
    check(
        "dedicated, left only",
        left,
        right,
        [Stage::Lower; 2],
        |t| Frame {
            left: t >= 17,
            right: false,
            axis: 0.0,
        },
        [Some(17), None],
    );
}

#[test]
fn parallel_linked_flippers_actuate_together() {
    let linked = FlipperStagingMode::ParallelLinked(CabinetInput::LeftFlipper);
    for tp in [0u64, 9, 64] {
        check(
            "parallel",
            linked,
            linked,
            [Stage::Lower; 2],
            move |t| Frame {
                left: t >= tp,
                right: false,
                axis: 0.0,
            },
            [Some(tp), Some(tp)],
        );
    }
    // The other button does nothing to flippers linked to the left one.
    check(
        "parallel, other button",
        linked,
        linked,
        [Stage::Lower; 2],
        |t| Frame {
            left: false,
            right: t >= 5,
            axis: 0.0,
        },
        [None, None],
    );
}

#[test]
fn dual_stage_actuates_lower_at_30_and_upper_at_85_percent() {
    let ds = dual_stage();
    // Axis ramps 0 → 1 over `ramp` ticks starting at tick 5.
    for ramp in [20u64, 50, 100] {
        let axis_at = move |t: u64| min_f32((t.saturating_sub(5)) as f32 / ramp as f32, 1.0);
        let first = |thr: f32| (0..200u64).find(|&t| axis_at(t) >= thr);
        let lower = first(0.30);
        let upper = first(0.85);
        assert!(lower < upper, "ramp {ramp}: {lower:?} {upper:?}");
        check(
            "dual-stage",
            ds,
            ds,
            [Stage::Lower, Stage::Upper],
            move |t| Frame {
                left: false,
                right: false,
                axis: axis_at(t),
            },
            [lower, upper],
        );
    }
    // A half press (60 %) fires only the lower stage.
    check(
        "dual-stage, half press",
        ds,
        ds,
        [Stage::Lower, Stage::Upper],
        |t| Frame {
            left: false,
            right: false,
            axis: if t >= 12 { 0.6 } else { 0.0 },
        },
        [Some(12), None],
    );
}

#[test]
fn unloaded_stroke_on_the_vertical_plane_matches_the_flat_one() {
    // Rest to EOS (EOS switch opens) on both mountings, default power:
    // same tick, within the Appendix C stroke time (20–35 ms).
    let mut ticks = [0u64; 2];
    for (k, mount) in [Mount::Horizontal, Mount::Vertical].into_iter().enumerate() {
        let cfg = bb::left_flipper(0, mount, 1.0);
        let mut set = FlipperSet::new(std::slice::from_ref(&cfg)).unwrap();
        set.set_energised(LEFT, true);
        let mut t = 0;
        while set.eos_closed(LEFT) && t < 200 {
            set.step_tick(t);
            set.advance(DT);
            t += 1;
        }
        ticks[k] = t;
    }
    assert_eq!(
        ticks[0], ticks[1],
        "stroke ticks flat {} vs vertical {}",
        ticks[0], ticks[1]
    );
    assert!((20..=35).contains(&ticks[1]), "stroke {} ms", ticks[1]);
}

// ---------------------------------------------------------------------------
// Determinism.

/// Minimal PCG32 (XSH RR), seeded: the input script generator.
struct Pcg(u64);

impl Pcg {
    fn next(&mut self) -> u32 {
        let old = self.0;
        self.0 = old
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let x = (((old >> 18) ^ old) >> 27) as u32;
        x.rotate_right((old >> 59) as u32)
    }
}

/// The backbox scene: the vertical plane (layer 4) plus a main-deck slab
/// below it, two staged flippers on layer 4, three balls (two on the
/// backbox, one entering from the main deck through a 0 → 4 zone), and a
/// 4 → 0 return zone. Inputs come from a PCG script of `seed`; returns the
/// FNV hash of the state bits after every tick.
fn run_scene(seed: u64, ticks: u64) -> u64 {
    let mount = Mount::Vertical;
    let rot = mount.rot();
    let cfgs = pair(
        mount,
        FlipperStagingMode::DedicatedButton(CabinetInput::LeftFlipper),
        FlipperStagingMode::DedicatedButton(CabinetInput::RightFlipper),
    );
    let world: StaticWorld = {
        let mut b = pin_physics::world::StaticWorldBuilder::new();
        b.add_collider(
            pin_physics::world::StaticShape::Cuboid {
                center: rot * Vec3::new(0.0, 0.0, -0.05),
                half_extents: Vec3::new(0.3, 0.4, 0.05),
                rotation: rot,
            },
            layers::BACKBOX,
            true,
        );
        // Side walls and a floor of the backbox playfield (layer 4).
        for (c, he) in [
            (Vec3::new(-0.2, 0.0, 0.03), Vec3::new(0.01, 0.4, 0.03)),
            (Vec3::new(0.2, 0.0, 0.03), Vec3::new(0.01, 0.4, 0.03)),
            (Vec3::new(0.0, -0.12, 0.03), Vec3::new(0.04, 0.01, 0.03)),
        ] {
            b.add_collider(
                pin_physics::world::StaticShape::Cuboid {
                    center: rot * c,
                    half_extents: he,
                    rotation: rot,
                },
                layers::BACKBOX,
                true,
            );
        }
        // A main-deck floor crossing the backbox volume (layer 0 only).
        b.add_collider(
            pin_physics::world::StaticShape::Cuboid {
                center: Vec3::new(0.0, -0.2, -0.05),
                half_extents: Vec3::new(0.5, 0.2, 0.05),
                rotation: glam::Quat::IDENTITY,
            },
            layers::MAIN_DECK,
            true,
        );
        b.build()
    };
    let zones = TransitionSet::new(vec![
        LayerTransition {
            from_layer: 0,
            to_layer: BACKBOX_LAYER,
            min: Vec3::new(-0.2, -0.05, 0.0),
            max: Vec3::new(0.2, 0.0, 0.05),
        },
        LayerTransition {
            from_layer: BACKBOX_LAYER,
            to_layer: 0,
            min: Vec3::new(-0.2, -0.05, -0.2),
            max: Vec3::new(0.2, 0.05, -0.15),
        },
    ]);
    let mut set = FlipperSet::new(&cfgs).unwrap();
    let mut sol = bb::c23::solver(mount.gravity());
    let mut hs = Vec::new();
    for (i, p) in [Vec3::new(-0.03, 0.05, 0.0), Vec3::new(0.04, 0.12, 0.0)]
        .into_iter()
        .enumerate()
    {
        let mut b = bb::c23::ball(
            rot * p + rot * Vec3::Z * R,
            Vec3::new(0.1 * i as f32, 0.0, 0.0),
        );
        b.layer = BACKBOX_LAYER;
        hs.push(sol.add_ball(b).unwrap());
    }
    hs.push(
        sol.add_ball(bb::c23::ball(
            Vec3::new(0.0, -0.1, 0.02),
            Vec3::new(0.0, 0.5, 0.3),
        ))
        .unwrap(),
    );
    let mut rng = Pcg(seed.wrapping_mul(2).wrapping_add(1));
    let mut ev = BallEvents::new();
    let mut frame = Frame::default();
    let mut words = Vec::new();
    let mut hash = 0u64;
    for t in 0..ticks {
        if t % 40 == 0 {
            let r = rng.next();
            frame.left = r & 1 != 0;
            frame.right = r & 2 != 0;
        }
        for (i, id) in [LEFT, RIGHT].into_iter().enumerate() {
            set.set_energised(id, route(&cfgs[i], Stage::Lower, &frame));
        }
        let prev: Vec<_> = hs
            .iter()
            .map(|&h| (h, sol.ball(h).unwrap().position))
            .collect();
        set.step_tick(t);
        ev.clear();
        sol.step_with_bodies(&Geometry::new(&world), &mut [&mut set], DT, &mut ev);
        bb::apply_transitions(&mut sol, &zones, &prev);
        words.clear();
        for &h in &hs {
            let b = sol.ball(h).unwrap();
            words.push(u32::from(b.layer));
            for x in b
                .position
                .to_array()
                .into_iter()
                .chain(b.velocity.to_array())
                .chain(b.angular_velocity.to_array())
            {
                words.push(x.to_bits());
            }
        }
        for id in [LEFT, RIGHT] {
            words.push(set.angle_deg(id).to_bits());
            words.push(set.angular_velocity(id).to_bits());
        }
        words.push(hash as u32);
        words.push((hash >> 32) as u32);
        hash = bb::fnv1a(&words);
    }
    for &h in &hs {
        let b = sol.ball(h).unwrap();
        assert!(b.position.is_finite() && b.velocity.is_finite(), "{b:?}");
    }
    hash
}

#[test]
fn same_seed_same_inputs_identical_hash_over_10_runs() {
    for seed in [1u64, 0xDEAD_BEEF] {
        let first = run_scene(seed, 3000);
        for run in 1..10 {
            assert_eq!(
                run_scene(seed, 3000),
                first,
                "seed {seed}: run {run} diverged"
            );
        }
    }
    // The hash does see the inputs.
    assert_ne!(run_scene(1, 3000), run_scene(2, 3000));
}
