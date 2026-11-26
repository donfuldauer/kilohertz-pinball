//! #103 implementer tests: the simulation publishes mech motion
//! (REQ-VIS-002, SPEC Appendix A "Dynamic mesh slot convention").
//!
//! `FrameStateSnapshot::dynamic_meshes[i]` is the slot of the i-th
//! `[[mechs]]` entry. The fixture table already has one mech (`bumper`,
//! slot 0); [`EXTRA`] adds one of every other kind. Rules map "action"
//! switches the test forces to coil and diverter commands, so every moving
//! part can be driven through the public test API.

mod sim_support;

use glam::Vec3;
use pin_core::fmath::{max_f32, max_f64};
use pin_core::snapshot::{IDENTITY_QUAT, MeshTransform, NEWTON_SWING_RANGE_RAD};
use pin_sim::tick::{DIVERTER_ON_ANGLE_RAD, GATE_OPEN_ANGLE_RAD, newton_swing_rad};
use pin_sim::{SimConfig, Simulation};
use sim_support::*;

// Slot (= mech) indices in the all-kinds table.
const BUMPER: usize = 0;
const GATE: usize = 1;
const DIVERTER: usize = 2;
const NEWTON: usize = 3;
const DROP: usize = 4;
const DROP_DEFAULT: usize = 5;
const SPINNER: usize = 6;
const SLING: usize = 7;
const KICKBACK: usize = 8;
const PLUNGER: usize = 9;
const VUK: usize = 10;
const MECH_COUNT: usize = 11;

const EXTRA: &str = r#"
[[coils]]
id = "gate_coil"
number = 4
kind = "kicker"

[[coils]]
id = "drop_reset"
number = 5
kind = "drop_reset"

[[coils]]
id = "sling_coil"
number = 6
kind = "sling"

[[coils]]
id = "kick_coil"
number = 7
kind = "kicker"

[[coils]]
id = "plunger_coil"
number = 8
kind = "autoplunger"

[[coils]]
id = "vuk_coil"
number = 9
kind = "vuk"

[[switches]]
id = "gate_sw"
number = 20
kind = "leaf"
layer = 0

[[switches]]
id = "newton_sw"
number = 21
kind = "target"
layer = 0

[[switches]]
id = "drop_sw"
number = 22
kind = "target"
layer = 0

[[switches]]
id = "spin_sw"
number = 23
kind = "spinner"
layer = 0

[[switches]]
id = "act_gate"
number = 30
kind = "leaf"
layer = 0

[[switches]]
id = "act_div"
number = 31
kind = "leaf"
layer = 0

[[switches]]
id = "act_kick"
number = 32
kind = "leaf"
layer = 0

[[switches]]
id = "act_plunger"
number = 33
kind = "leaf"
layer = 0

[[switches]]
id = "act_reset"
number = 34
kind = "leaf"
layer = 0

[[mechs]]
id = "posts[0]"
kind = "gate"
coil = "gate_coil"
switch = "gate_sw"
position = [140.0, 760.0, 0.0]

[[mechs]]
id = "posts[1]"
kind = "diverter"
position = [374.0, 760.0, 0.0]

[[mechs]]
id = "newton"
kind = "newton_ball"
switch = "newton_sw"
position = [257.0, 300.0, 0.0]
angle_deg = 90.0
length = 50.0

[[mechs]]
id = "drop"
kind = "drop_target"
coil = "drop_reset"
switch = "drop_sw"
position = [420.0, 450.0, 0.0]
height = 30.0

[[mechs]]
id = "drop_default"
kind = "drop_target"
coil = "drop_reset"
position = [90.0, 450.0, 0.0]

[[mechs]]
id = "spinner"
kind = "spinner"
switch = "spin_sw"
position = [300.0, 600.0, 0.0]

[[mechs]]
id = "sling"
kind = "sling"
coil = "sling_coil"
position = [80.0, 250.0, 0.0]
angle_deg = 20.0

[[mechs]]
id = "kickback"
kind = "kickback"
coil = "kick_coil"
position = [30.0, 200.0, 0.0]

[[mechs]]
id = "plunger"
kind = "plunger"
coil = "plunger_coil"
position = [490.0, 60.0, 0.0]
angle_deg = 90.0
auto_pull = 0.9

[[mechs]]
id = "vuk"
kind = "vuk"
coil = "vuk_coil"
position = [450.0, 850.0, 0.0]
"#;

const RULES: &str = r#"
function on_switch(id, closed)
  if id == pinball.sw.act_gate then
    pinball.hold_coil(pinball.coil.gate_coil, closed)
  elseif id == pinball.sw.act_div then
    pinball.set_diverter(pinball.diverter["posts[1]"], closed)
  elseif id == pinball.sw.act_kick and closed then
    pinball.pulse_coil(pinball.coil.sling_coil, 30)
    pinball.pulse_coil(pinball.coil.kick_coil, 30)
  elseif id == pinball.sw.act_plunger then
    pinball.hold_coil(pinball.coil.plunger_coil, closed)
  elseif id == pinball.sw.act_reset and closed then
    pinball.pulse_coil(pinball.coil.drop_reset, 300)
  end
end
function on_tick() end
"#;

/// The all-kinds bundle.
fn bundle(tag: &str) -> ScratchDir {
    let table = fixture_table_toml() + EXTRA;
    fixture_variant(
        tag,
        &[
            ("table.toml", Some(table.as_bytes())),
            ("rules.lua", Some(RULES.as_bytes())),
        ],
    )
}

/// The fixture with its only `[[mechs]]` entry (the bumper) removed.
fn no_mechs_bundle() -> ScratchDir {
    let block = "[[mechs]]\nid = \"bumper\"\nkind = \"bumper\"\ncoil = \"bumper_coil\"\n\
                 switch = \"bumper_ring\"\nposition = [257.0, 700.0, 0.0]\n";
    let toml = fixture_table_toml();
    assert!(toml.contains(block), "fixture bumper block");
    let toml = toml.replace(block, "");
    fixture_variant(
        "kp023_no_mechs",
        &[
            ("table.toml", Some(toml.as_bytes())),
            ("rules.lua", Some(b"function on_tick() end\n")),
        ],
    )
}

/// The fixture plus `n` extra bumpers (fixture bumper + n mechs).
fn many_bumpers_bundle(n: usize) -> ScratchDir {
    let mut toml = fixture_table_toml();
    for i in 0..n {
        let x = 60.0 + (i % 10) as f32 * 40.0;
        let y = 150.0 + (i / 10) as f32 * 60.0;
        toml.push_str(&format!(
            "\n[[mechs]]\nid = \"extra_bumper_{i}\"\nkind = \"bumper\"\nposition = [{x:.1}, {y:.1}, 0.0]\n"
        ));
    }
    fixture_variant(
        &format!("kp023_bumpers_{n}"),
        &[
            ("table.toml", Some(toml.as_bytes())),
            ("rules.lua", Some(b"function on_tick() end\n")),
        ],
    )
}

fn slot(sim: &Simulation, i: usize) -> MeshTransform {
    sim.snapshot().dynamic_meshes()[i]
}

fn pos(t: &MeshTransform) -> Vec3 {
    Vec3::from_array(t.position_m)
}

fn close(a: Vec3, b: Vec3, tol: f32) -> bool {
    let d = (a - b).abs();
    d.x <= tol && d.y <= tol && d.z <= tol
}

/// Unit direction of `angle_deg` in the playfield plane.
fn dir(angle_deg: f32) -> Vec3 {
    let a = angle_deg.to_radians();
    Vec3::new(libm::cosf(a), libm::sinf(a), 0.0)
}

/// Hinge axis `−(z × u)`.
fn hinge(angle_deg: f32) -> Vec3 {
    let u = dir(angle_deg);
    Vec3::new(u.y, -u.x, 0.0)
}

/// Signed rotation angle (rad, `(−π, π]`) of `q` about `axis`, plus the
/// norm of the rotation axis component orthogonal to `axis`.
fn angle_about(q: [f32; 4], axis: Vec3) -> (f64, f64) {
    let v = Vec3::new(q[0], q[1], q[2]);
    let along = f64::from(v.dot(axis));
    let off = f64::from((v - axis * v.dot(axis)).length());
    let a = 2.0 * libm::atan2(along, f64::from(q[3]));
    (wrap(a), off)
}

fn wrap(a: f64) -> f64 {
    let t = core::f64::consts::TAU;
    let r = a - t * libm::floor(a / t);
    if r > core::f64::consts::PI { r - t } else { r }
}

fn quat_norm(q: [f32; 4]) -> f64 {
    q.iter()
        .map(|&c| f64::from(c) * f64::from(c))
        .sum::<f64>()
        .sqrt()
}

/// Every published slot is finite with a unit quaternion.
fn assert_slots_sane(sim: &Simulation) {
    for (i, m) in sim.snapshot().dynamic_meshes().iter().enumerate() {
        assert!(
            m.position_m.iter().all(|c| c.is_finite()),
            "tick {} slot {i}: {m:?}",
            sim.tick()
        );
        assert!(m.orientation.iter().all(|c| c.is_finite()), "slot {i}");
        assert!(
            (quat_norm(m.orientation) - 1.0).abs() <= 1e-6,
            "tick {} slot {i}: |q| = {}",
            sim.tick(),
            quat_norm(m.orientation)
        );
    }
}

/// REQ-VIS-002 / Appendix A: one slot per mech in `table.toml` order
/// (count == mechs; bumper, VUK identity; kinds in order as the mech set
/// sees them), and the load snapshot already has the slots.
#[test]
fn all_kinds_count_and_order() {
    let b = bundle("kp023_count");
    on_sim_thread(move || {
        let mut sim = Simulation::load(&b, SimConfig::default()).unwrap();
        assert_eq!(sim.mech_set().len(), MECH_COUNT);
        assert_eq!(usize::from(sim.snapshot().dynamic_mesh_count), MECH_COUNT);
        sim.advance(5);
        assert!(!sim.script_faulted(), "{:?}", sim.script_error());
        let s = sim.snapshot();
        assert_eq!(usize::from(s.dynamic_mesh_count), MECH_COUNT);
        assert_eq!(s.dynamic_meshes().len(), MECH_COUNT);
        use pin_core::schema::table::MechKind as K;
        let kinds = [
            K::Bumper,
            K::Gate,
            K::Diverter,
            K::NewtonBall,
            K::DropTarget,
            K::DropTarget,
            K::Spinner,
            K::Sling,
            K::Kickback,
            K::Plunger,
            K::Vuk,
        ];
        for (i, k) in kinds.iter().enumerate() {
            assert_eq!(sim.mech_set().kind(i), Some(*k), "mech {i}");
        }
        // Bumper ring and VUK: zero offset, identity.
        for i in [BUMPER, VUK] {
            assert_eq!(slot(&sim, i), MeshTransform::default(), "slot {i}");
        }
        // Absolute-position kinds at their hinge / pivot / cradle.
        assert!(close(
            pos(&slot(&sim, GATE)),
            Vec3::new(0.140, 0.760, 0.0),
            1e-6
        ));
        assert!(close(
            pos(&slot(&sim, DIVERTER)),
            Vec3::new(0.374, 0.760, 0.0),
            1e-6
        ));
        assert!(close(
            pos(&slot(&sim, NEWTON)),
            Vec3::new(0.257, 0.300, 0.0),
            1e-6
        ));
        assert!(close(
            pos(&slot(&sim, SPINNER)),
            Vec3::new(0.300, 0.600, 0.042),
            1e-6
        ));
        // Offset kinds at rest.
        for i in [DROP, DROP_DEFAULT, SLING, KICKBACK, PLUNGER] {
            assert_eq!(slot(&sim, i), MeshTransform::default(), "slot {i}");
        }
        assert_slots_sane(&sim);
    });
}

/// REQ-VIS-002 / Appendix A: a table with no mechs publishes count 0.
#[test]
fn no_mechs_publishes_zero() {
    let b = no_mechs_bundle();
    on_sim_thread(move || {
        let mut sim = Simulation::load(&b, SimConfig::default()).unwrap();
        assert_eq!(sim.mech_set().len(), 0);
        sim.advance(3);
        assert_eq!(sim.snapshot().dynamic_mesh_count, 0);
        assert!(sim.snapshot().dynamic_meshes().is_empty());
    });
}

/// REQ-VIS-002 / Appendix A: 70 mechs publish 64 slots (the 65th..70th
/// are unpublished); 64 mechs publish all 64.
#[test]
fn more_than_64_mechs_publish_64() {
    for (extra, want) in [(69usize, 64usize), (63, 64), (10, 11)] {
        let b = many_bumpers_bundle(extra);
        on_sim_thread(move || {
            let mut sim = Simulation::load(&b, SimConfig::default()).unwrap();
            assert_eq!(sim.mech_set().len(), extra + 1);
            sim.advance(2);
            let s = sim.snapshot();
            assert_eq!(usize::from(s.dynamic_mesh_count), want, "{extra}");
            assert!(
                s.dynamic_meshes()
                    .iter()
                    .all(|m| *m == MeshTransform::default())
            );
        });
    }
}

/// REQ-VIS-002 acceptance "A drop target hit lowers its slot by the target
/// height within 1 tick of the drop": the slot z is 0 while up, −height
/// (30 mm override) on the tick `drop_target_down` turns true, and rises
/// monotonically to 0 (never below −height) while the reset raises it.
/// The default-height target (32 mm) is checked the same way.
#[test]
fn drop_target_slot_drops_by_height_and_rises() {
    let b = bundle("kp023_drop");
    on_sim_thread(move || {
        let mut sim = Simulation::load(&b, SimConfig::default()).unwrap();
        for (idx, x, height) in [(DROP, 0.420f32, 0.030f32), (DROP_DEFAULT, 0.090, 0.032)] {
            assert_eq!(pos(&slot(&sim, idx)), Vec3::ZERO);
            let h = sim
                .spawn_ball("standard_steel", [x, 0.52, 0.0135], [0.0, -1.5, 0.0])
                .unwrap();
            let mut dropped = false;
            for _ in 0..300 {
                sim.advance(1);
                let s = slot(&sim, idx);
                assert_eq!(s.orientation, IDENTITY_QUAT);
                assert_eq!(s.position_m[0], 0.0);
                assert_eq!(s.position_m[1], 0.0);
                let z = s.position_m[2];
                if sim.mech_set().drop_target_down(idx).unwrap() {
                    assert!((z + height).abs() <= 1e-6, "dropped z {z} vs -{height}");
                    dropped = true;
                    break;
                }
                assert_eq!(z, 0.0, "up target has z 0");
            }
            assert!(dropped, "the hit dropped target {idx}");
            sim.remove_ball(h);
            sim.advance(5);
            assert!((slot(&sim, idx).position_m[2] + height).abs() <= 1e-6);
            sim.set_switch("act_reset", true).unwrap();
            sim.advance(1);
            sim.set_switch("act_reset", false).unwrap();
            let mut prev = slot(&sim, idx).position_m[2];
            let mut between = false;
            for _ in 0..200 {
                sim.advance(1);
                let z = slot(&sim, idx).position_m[2];
                assert!(z >= -height - 1e-7 && z <= 0.0, "{z}");
                assert!(z >= prev, "monotone rise {prev} -> {z}");
                between |= z > -height && z < 0.0;
                prev = z;
            }
            assert!(between, "seen while rising");
            assert!(!sim.mech_set().drop_target_down(idx).unwrap());
            assert_eq!(slot(&sim, idx).position_m[2], 0.0);
            // Let the 300 ms reset pulse end before the next target drops.
            sim.advance(200);
        }
    });
}

/// REQ-VIS-002 acceptance "the spinner angle matches its state within
/// 1e-5": a ball shot through the spinner spins it; on every tick of the
/// spin-down the slot's rotation about the spinner axis equals
/// `spinner_state` (mod 2π) within 1e-5, it is a pure rotation about that
/// axis, and the position is the pivot.
#[test]
fn spinner_slot_matches_state() {
    let b = bundle("kp023_spinner");
    on_sim_thread(move || {
        let mut sim = Simulation::load(&b, SimConfig::default()).unwrap();
        let (pivot, axis) = sim.mech_set().spinner_pivot_axis(SPINNER).unwrap();
        assert!(close(axis, hinge(90.0), 1e-6));
        sim.spawn_ball("standard_steel", [0.300, 0.50, 0.0135], [0.0, 3.0, 0.0])
            .unwrap();
        let mut max_angle = 0.0f32;
        for _ in 0..3000 {
            sim.advance(1);
            let s = slot(&sim, SPINNER);
            assert!(close(pos(&s), pivot, 1e-6));
            let (a, _) = sim.mech_set().spinner_state(SPINNER).unwrap();
            let (got, off) = angle_about(s.orientation, axis);
            let err = wrap(got - f64::from(a)).abs();
            assert!(err <= 1e-5, "tick {}: {got} vs {a} ({err})", sim.tick());
            assert!(off <= 1e-6, "rotation axis is the spinner axis");
            max_angle = max_f32(max_angle, a.abs());
        }
        assert!(max_angle > 1.0, "the ball spun the spinner: {max_angle}");
        assert!(sim.mech_set().spinner_pulses(SPINNER) > 0);
    });
}

/// REQ-VIS-002 / Appendix A (sling, kickback: arm translation along the
/// kick axis; plunger: rod translation along the launch axis, negative =
/// pulled back): the slot is `u · slider_state.0` within 1e-6 on every
/// tick of a kick and of a pull and release, with identity orientation.
#[test]
fn slider_slots_follow_the_slider() {
    let b = bundle("kp023_slider");
    on_sim_thread(move || {
        let mut sim = Simulation::load(&b, SimConfig::default()).unwrap();
        let cases = [
            (SLING, dir(20.0)),
            (KICKBACK, dir(90.0)),
            (PLUNGER, dir(90.0)),
        ];
        let check = |sim: &Simulation, seen: &mut [f32; 3]| {
            for (k, (i, u)) in cases.iter().enumerate() {
                let x = sim.mech_set().slider_state(*i).unwrap().0;
                let s = slot(sim, *i);
                assert!(close(pos(&s), *u * x, 1e-6), "slot {i}: {s:?} vs {x}");
                assert_eq!(s.orientation, IDENTITY_QUAT);
                if x.abs() > seen[k].abs() {
                    seen[k] = x;
                }
            }
        };
        let mut seen = [0.0f32; 3];
        sim.set_switch("act_kick", true).unwrap();
        sim.set_switch("act_plunger", true).unwrap();
        for _ in 0..200 {
            sim.advance(1);
            check(&sim, &mut seen);
        }
        sim.set_switch("act_kick", false).unwrap();
        sim.set_switch("act_plunger", false).unwrap();
        for _ in 0..300 {
            sim.advance(1);
            check(&sim, &mut seen);
        }
        assert!(seen[0] > 0.005, "sling kicked: {}", seen[0]);
        assert!(seen[1] > 0.005, "kickback kicked: {}", seen[1]);
        assert!(
            seen[2] < -0.02,
            "plunger pulled back (negative): {}",
            seen[2]
        );
        // The pulled-back plunger slot points backward along the launch
        // axis (−y here).
        assert!(seen[2] * dir(90.0).y < 0.0);
    });
}

/// REQ-VIS-002 / Appendix A (gate and diverter: angle about the hinge,
/// translation = hinge): identity while closed / off, the documented angle
/// about `−(z × u)` while open / on, position = mech position.
#[test]
fn gate_and_diverter_slots() {
    let b = bundle("kp023_gate");
    on_sim_thread(move || {
        let mut sim = Simulation::load(&b, SimConfig::default()).unwrap();
        let axis = hinge(90.0);
        let check = |sim: &Simulation, open: bool| {
            for (i, p, a) in [
                (GATE, Vec3::new(0.140, 0.760, 0.0), GATE_OPEN_ANGLE_RAD),
                (
                    DIVERTER,
                    Vec3::new(0.374, 0.760, 0.0),
                    DIVERTER_ON_ANGLE_RAD,
                ),
            ] {
                let s = slot(sim, i);
                assert!(close(pos(&s), p, 1e-6), "slot {i}");
                if open {
                    let (got, off) = angle_about(s.orientation, axis);
                    assert!((got - f64::from(a)).abs() <= 1e-6, "slot {i}: {got}");
                    assert!(off <= 1e-6);
                } else {
                    assert_eq!(s.orientation, IDENTITY_QUAT, "slot {i}");
                }
            }
        };
        sim.advance(3);
        check(&sim, false);
        sim.set_switch("act_gate", true).unwrap();
        sim.set_switch("act_div", true).unwrap();
        sim.advance(2);
        assert_eq!(sim.switch_closed("gate_sw"), Some(true), "gate open");
        for _ in 0..50 {
            sim.advance(1);
            check(&sim, true);
        }
        sim.set_switch("act_gate", false).unwrap();
        sim.set_switch("act_div", false).unwrap();
        sim.advance(2);
        for _ in 0..20 {
            sim.advance(1);
            check(&sim, false);
        }
        assert!((GATE_OPEN_ANGLE_RAD - core::f32::consts::FRAC_PI_3).abs() < 1e-7);
        assert!((DIVERTER_ON_ANGLE_RAD - core::f32::consts::FRAC_PI_6).abs() < 1e-7);
    });
}

/// REQ-VIS-002 / Appendix A (newton_ball: the cradle frame, swing within
/// ±NEWTON_SWING_RANGE_RAD): 0 swing at rest (the load snapshot, ball on
/// its rest point); on every tick the swing is `newton_swing_rad` of the
/// captive ball's displacement along the path, about the hinge axis and in
/// range; after a free ball strikes the captive ball it is clearly
/// nonzero; position = mech position.
#[test]
fn newton_ball_slot_swings_in_range() {
    let b = bundle("kp023_newton");
    on_sim_thread(move || {
        let mut sim = Simulation::load(&b, SimConfig::default()).unwrap();
        let axis = hinge(90.0);
        let p = Vec3::new(0.257, 0.300, 0.0);
        let start = p + Vec3::Z * 0.0135;
        let s = slot(&sim, NEWTON);
        assert!(close(pos(&s), p, 1e-6));
        assert_eq!(s.orientation, IDENTITY_QUAT, "rest swing");
        let follows = |sim: &Simulation| {
            let c = sim.solver().newton_chain(0);
            let off = (c.ball_position(0) - start).dot(dir(90.0));
            let want = f64::from(newton_swing_rad(off, 0.050));
            let (a, _) = angle_about(slot(sim, NEWTON).orientation, axis);
            assert!((a - want).abs() <= 1e-6, "swing {a} vs {want}");
        };
        for _ in 0..20 {
            sim.advance(1);
            follows(&sim);
        }
        sim.spawn_ball("standard_steel", [0.257, 0.24, 0.0135], [0.0, 1.5, 0.0])
            .unwrap();
        let mut max_swing = 0.0f64;
        for _ in 0..400 {
            sim.advance(1);
            let s = slot(&sim, NEWTON);
            assert!(close(pos(&s), p, 1e-6));
            let (a, off) = angle_about(s.orientation, axis);
            follows(&sim);
            assert!(off <= 1e-6);
            assert!(a.abs() <= f64::from(NEWTON_SWING_RANGE_RAD) + 1e-6, "{a}");
            max_swing = max_f64(max_swing, a.abs());
        }
        assert!(sim.switch_hits("newton_sw").unwrap() >= 1, "struck");
        assert!(max_swing > 1e-3, "the cradle swung: {max_swing}");
    });
}

/// REQ-VIS-002 (R4/R3 safety): every published quaternion is finite and
/// unit length within 1e-6, and every position finite, on every tick of a
/// 4 000-tick soak of the all-kinds table with every mech driven.
#[test]
fn soak_slots_stay_finite_and_unit() {
    let b = bundle("kp023_soak");
    on_sim_thread(move || {
        let mut sim = Simulation::load(&b, SimConfig::default()).unwrap();
        drive(&mut sim, 4000, assert_slots_sane);
        assert_eq!(usize::from(sim.snapshot().dynamic_mesh_count), MECH_COUNT);
    });
}

/// Steps `ticks` ticks of a fixed schedule that moves every mech (balls
/// into the drop target, spinner and Newton ball; coils, diverter and
/// plunger toggled), calling `each` after every tick.
fn drive(sim: &mut Simulation, ticks: u64, mut each: impl FnMut(&Simulation)) {
    for k in 0..ticks {
        match k % 1000 {
            10 => {
                let _ = sim.spawn_ball("standard_steel", [0.300, 0.50, 0.0135], [0.0, 3.0, 0.0]);
                let _ = sim.spawn_ball("standard_steel", [0.420, 0.52, 0.0135], [0.0, -1.5, 0.0]);
                let _ = sim.spawn_ball("standard_steel", [0.257, 0.24, 0.0135], [0.0, 1.5, 0.0]);
            }
            100 => {
                for s in ["act_gate", "act_div", "act_kick", "act_plunger"] {
                    sim.set_switch(s, true).unwrap();
                }
            }
            400 => {
                for s in ["act_gate", "act_div", "act_kick", "act_plunger"] {
                    sim.set_switch(s, false).unwrap();
                }
                sim.set_switch("act_reset", true).unwrap();
            }
            420 => sim.set_switch("act_reset", false).unwrap(),
            _ => {}
        }
        sim.advance(1);
        each(sim);
    }
}

/// R3: two runs of the same schedule publish bit-identical slots on every
/// tick and end with the same `state_hash`.
#[test]
fn slots_are_deterministic() {
    let run = || {
        let b = bundle("kp023_det");
        on_sim_thread(move || {
            let mut sim = Simulation::load(&b, SimConfig::default()).unwrap();
            let mut trace: Vec<u32> = Vec::new();
            drive(&mut sim, 2500, |sim| {
                for m in sim.snapshot().dynamic_meshes() {
                    trace.extend(m.position_m.iter().map(|c| c.to_bits()));
                    trace.extend(m.orientation.iter().map(|c| c.to_bits()));
                }
            });
            (trace, sim.state_hash())
        })
    };
    let (a, ha) = run();
    let (b, hb) = run();
    assert_eq!(ha, hb);
    assert!(a == b, "published slots differ between runs");
}

/// R3 / digests: publishing the slots writes no simulation state, so the
/// replay fixture's golden `state_hash` (10 000 ticks of the CLI input
/// pattern) is unchanged.
#[test]
fn replay_fixture_hash_unchanged() {
    let h = on_sim_thread(|| {
        let sim = Simulation::load(&fixture(), SimConfig::default()).unwrap();
        assert_eq!(sim.snapshot().dynamic_mesh_count, 1, "the fixture bumper");
        let sim = pin_sim::cli::run_pattern(sim, 10_000, false);
        assert_eq!(sim.snapshot().dynamic_mesh_count, 1);
        sim.state_hash()
    });
    assert_eq!(pin_sim::cli::hash_line(h), "state_hash=25097c21c2c3adc1");
}
