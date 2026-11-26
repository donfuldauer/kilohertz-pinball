//! KP-023 (#103) independent tests: the simulation publishes mech motion in
//! `FrameStateSnapshot::dynamic_meshes`, one slot per `[[mechs]]` entry in
//! `table.toml` order (SPEC Appendix A "Dynamic mesh slot convention",
//! REQ-VIS-002), tick by tick, deterministically, with unit quaternions and
//! without touching the replay state hash (R3) or allocating (R4).
//!
//! Black-box: only the public API is used.

mod spec_kp023_support;

use pin_core::input::{CabinetAxis, InputEvent};
use pin_core::sim::SimStep;
use pin_core::snapshot::{FrameStateSnapshot, MAX_DYNAMIC_MESHES};
use pin_sim::alloc_guard::{CountingAlloc, count_allocs_split};
use pin_sim::cli::record_pattern;
use pin_sim::{SimConfig, Simulation};
use proptest::test_runner::{Config, TestRunner};
use spec_kp023_support::*;

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

/// Committed golden of the replay fixture (10 000 ticks, default seed).
const GOLDEN_HASH: u64 = 0x25097c21c2c3adc1;

fn load(b: &Bundle) -> Simulation {
    Simulation::load(&b.path(), SimConfig::default()).expect("load bundle")
}

/// Runs `ticks` ticks of the all-kinds scenario: balls are launched at the
/// mechs at the start of every 400-tick cycle; `visit` sees the simulation
/// after each tick.
fn drive_all_kinds(sim: &mut Simulation, ticks: u64, mut visit: impl FnMut(&Simulation)) {
    for _ in 0..ticks {
        if sim.tick().is_multiple_of(400) {
            spawn_launches(sim);
        }
        sim.advance(1);
        visit(sim);
    }
}

fn assert_slot_sane(i: usize, t: &pin_core::snapshot::MeshTransform, tick: u64) {
    assert!(finite(t), "tick {tick} slot {i} not finite: {t:?}");
    let l = qlen(t.orientation);
    assert!(
        (l - 1.0).abs() <= 1e-6,
        "tick {tick} slot {i} quaternion length {l}: {t:?}"
    );
}

// ---------------------------------------------------------------------------
// 1. count and order
// ---------------------------------------------------------------------------

/// REQ-VIS-002 (SPEC App. A): one slot per mech, `dynamic_mesh_count` equals
/// the number of `[[mechs]]` after every tick, and slot i belongs to mech i:
/// only the drop targets' slots (indices 2 and 3) show a z offset after a
/// drop.
#[test]
fn req_vis_002_count_and_order_all_kinds() {
    let b = bundle("count", &all_kinds(), ALL_KINDS_RULES);
    on_sim_thread(move || {
        let mut sim = load(&b);
        assert_eq!(sim.mech_set().len(), ALL_KINDS_COUNT);
        assert_eq!(
            usize::from(sim.snapshot().dynamic_mesh_count),
            ALL_KINDS_COUNT,
            "count after load"
        );
        let mut dropped_seen = [false; ALL_KINDS_COUNT];
        drive_all_kinds(&mut sim, 900, |s| {
            let snap = s.snapshot();
            assert_eq!(
                usize::from(snap.dynamic_mesh_count),
                ALL_KINDS_COUNT,
                "tick {}",
                snap.tick
            );
            assert_eq!(snap.dynamic_meshes().len(), ALL_KINDS_COUNT);
            for (i, t) in snap.dynamic_meshes().iter().enumerate() {
                if t.position_m[2] < -1e-4 {
                    dropped_seen[i] = true;
                }
            }
        });
        for (i, seen) in dropped_seen.iter().enumerate() {
            assert_eq!(
                *seen,
                i == 2 || i == 3,
                "only the drop targets (slots 2, 3) show a negative z offset; slot {i}"
            );
        }
    });
}

/// REQ-VIS-002: a table with no mechs publishes no slots.
#[test]
fn req_vis_002_zero_mechs_zero_slots() {
    let b = bundle("zero", &[], QUIET);
    on_sim_thread(move || {
        let mut sim = load(&b);
        assert_eq!(sim.mech_set().len(), 0);
        assert_eq!(sim.snapshot().dynamic_mesh_count, 0);
        for _ in 0..20 {
            sim.advance(1);
            assert_eq!(sim.snapshot().dynamic_mesh_count, 0);
        }
    });
}

/// REQ-VIS-002 (SPEC App. A): more than 64 mechs publish only the first 64.
#[test]
fn req_vis_002_seventy_mechs_publish_sixty_four() {
    let mut mechs = String::new();
    for i in 0..70u32 {
        let (c, r) = (i % 10, i / 10);
        mechs.push_str(&format!(
            "\n[[mechs]]\nid = \"bmp{i}\"\nkind = \"bumper\"\nposition = [{}.0, {}.0, 0.0]\n",
            40 + 45 * c,
            200 + 60 * r
        ));
    }
    let b = bundle("seventy", &[mechs.as_str()], QUIET);
    on_sim_thread(move || {
        let mut sim = load(&b);
        assert_eq!(sim.mech_set().len(), 70);
        assert_eq!(
            usize::from(sim.snapshot().dynamic_mesh_count),
            MAX_DYNAMIC_MESHES
        );
        for _ in 0..30 {
            sim.advance(1);
            assert_eq!(usize::from(sim.snapshot().dynamic_mesh_count), 64);
        }
    });
}

// ---------------------------------------------------------------------------
// 2 and 12. drop target
// ---------------------------------------------------------------------------

/// Drives a drop target hit and its reset and checks the slot tick by tick.
/// `height` is the expected up height in metres.
fn drop_target_scenario(
    tag: &str,
    mech: &'static str,
    switch: &str,
    coil: &str,
    x_m: f32,
    height: f32,
) {
    let rules = format!(
        r#"
local t, at = 0, -1
function on_init() end
function on_switch(id, closed)
  if closed and id == pinball.sw.{switch} then at = t + 60 end
end
function on_tick()
  t = t + 1
  if at >= 0 and t >= at then
    at = -1
    pinball.pulse_coil(pinball.coil.{coil}, 150)
  end
end
"#
    );
    let b = bundle(tag, &[mech], &rules);
    on_sim_thread(move || {
        let mut sim = load(&b);
        assert_eq!(sim.mech_set().len(), 1);
        assert!(is_identity(&slots(&sim)[0]), "identity at load");
        assert_eq!(sim.mech_set().drop_target_down(0), Some(false));
        let h = sim
            .spawn_ball("standard_steel", [x_m, 0.44, R], [0.0, 1.5, 0.0])
            .unwrap();
        let mut first_down: Option<u64> = None;
        let mut removed = false;
        let mut prev_z = 0.0f32;
        let mut rose = false;
        let mut saw_mid = false;
        let mut done_at: Option<u64> = None;
        for _ in 0..1500 {
            sim.advance(1);
            let tick = sim.tick();
            let s = slots(&sim);
            assert_eq!(s.len(), 1);
            let z = s[0].position_m[2];
            assert_eq!(s[0].orientation, [0.0, 0.0, 0.0, 1.0], "tick {tick}");
            assert_eq!(s[0].position_m[0], 0.0, "tick {tick} x");
            assert_eq!(s[0].position_m[1], 0.0, "tick {tick} y");
            assert!(
                z >= -height - 1e-6 && z <= 1e-6,
                "tick {tick}: z {z} outside [-{height}, 0]"
            );
            let down = sim.mech_set().drop_target_down(0).unwrap();
            match first_down {
                None => {
                    if down {
                        first_down = Some(tick);
                        if !removed {
                            sim.remove_ball(h);
                            removed = true;
                        }
                    } else {
                        assert_eq!(z, 0.0, "tick {tick}: up target must publish z = 0");
                    }
                }
                Some(t0) => {
                    if tick > t0 && !rose {
                        // Within one tick of the first `down`: exactly the
                        // target height below the raised position.
                        if tick == t0 + 1 {
                            assert!(
                                (z + height).abs() <= 1e-6,
                                "tick {tick}: dropped slot z {z}, want -{height}"
                            );
                        }
                        if z > -height + 1e-6 {
                            rose = true;
                            prev_z = z;
                        }
                    } else if rose {
                        assert!(
                            z >= prev_z,
                            "tick {tick}: z fell while rising {prev_z} -> {z}"
                        );
                        if z < -1e-6 {
                            saw_mid = true;
                        }
                        prev_z = z;
                        if z == 0.0 && done_at.is_none() {
                            done_at = Some(tick);
                            assert_eq!(sim.mech_set().drop_target_down(0), Some(false));
                        }
                        if done_at.is_some() {
                            // 12: no stale offset after the rise.
                            assert!(
                                is_identity(&s[0]),
                                "tick {tick}: stale slot data {:?}",
                                s[0]
                            );
                        }
                    }
                }
            }
        }
        assert!(first_down.is_some(), "the ball never dropped the target");
        assert!(rose, "the reset coil never raised the target");
        assert!(saw_mid, "no intermediate height while rising");
        assert!(done_at.is_some(), "z never returned to exactly 0");
    });
}

/// REQ-VIS-002 acceptance 1 (and 12): a drop target with `height = 40` mm
/// publishes z = -0.040 within one tick of the drop, rises monotonically and
/// ends at exactly identity.
#[test]
fn req_vis_002_drop_target_explicit_height() {
    drop_target_scenario("drop40", DROP_40, "drop_sw", "drop_coil", 0.200, 0.040);
}

/// REQ-VIS-002 acceptance 1 (and 12): with `height` omitted the drop depth is
/// the 32 mm default of physics_defaults.toml.
#[test]
fn req_vis_002_drop_target_default_height() {
    drop_target_scenario(
        "dropdef",
        DROP_DEFAULT,
        "drop2_sw",
        "drop2_coil",
        0.330,
        0.032,
    );
}

// ---------------------------------------------------------------------------
// 3. spinner
// ---------------------------------------------------------------------------

/// REQ-VIS-002 acceptance 1: on every tick of the spin-down the angle in the
/// quaternion equals `spinner_state(i).0` within 1e-5; the pivot is the mech
/// position (x, y), at or above the playfield, constant.
#[test]
fn req_vis_002_spinner_quaternion_matches_state() {
    let b = bundle("spinner", &[SPINNER], QUIET);
    on_sim_thread(move || {
        let mut sim = load(&b);
        let u = dir(90.0);
        let axis = [u[1], -u[0], 0.0];
        let mut rows: Vec<(f64, f64, f64)> = Vec::new(); // (state, +axis angle, -axis angle)
        let mut pivot: Option<[f32; 3]> = None;
        for k in 0..2500u64 {
            if k % 500 == 0 {
                for _ in 0..3 {
                    let _ = sim.spawn_ball("standard_steel", [0.400, 0.58, R], [0.0, 2.5, 0.0]);
                }
            }
            sim.advance(1);
            let t = slots(&sim)[0];
            assert!(finite(&t));
            assert!((qlen(t.orientation) - 1.0).abs() <= 1e-6);
            // Pivot.
            assert!((f64::from(t.position_m[0]) - 0.400).abs() <= 1e-6, "{t:?}");
            assert!((f64::from(t.position_m[1]) - 0.640).abs() <= 1e-6, "{t:?}");
            assert!(t.position_m[2] >= 0.0, "pivot below the playfield: {t:?}");
            match pivot {
                None => pivot = Some(t.position_m),
                Some(p) => assert_eq!(p, t.position_m, "pivot moved"),
            }
            let state = f64::from(sim.mech_set().spinner_state(0).expect("spinner").0);
            if state.abs() >= 20.0 {
                continue;
            }
            let (ang, ax) = angle_axis(t.orientation);
            let dot = ax[0] * axis[0] + ax[1] * axis[1] + ax[2] * axis[2];
            if ang > 1e-6 {
                assert!(
                    dot.abs() > 0.999_999,
                    "axis {ax:?} not perpendicular to the spinner direction"
                );
                assert!(ax[2].abs() <= 1e-6, "axis not horizontal: {ax:?}");
            }
            let signed = if dot < 0.0 { -ang } else { ang };
            rows.push((state, signed, -signed));
        }
        let max_state = rows.iter().map(|r| r.0.abs()).fold(0.0, fmax);
        assert!(
            max_state > 0.5,
            "the spinner never spun (max |angle| {max_state})"
        );
        assert!(rows.iter().filter(|r| r.0.abs() > 1e-3).count() > 50);
        let worst = |sel: fn(&(f64, f64, f64)) -> f64| {
            rows.iter()
                .map(|r| wrap(sel(r) - r.0).abs())
                .fold(0.0, fmax)
        };
        let plus = worst(|r| r.1);
        let minus = worst(|r| r.2);
        assert!(
            plus <= 1e-5,
            "spinner angle mismatch: worst error {plus:e} (documented axis (u.y, -u.x, 0)); opposite-sign worst error {minus:e}"
        );
    });
}

// ---------------------------------------------------------------------------
// 4. sling, kickback, plunger
// ---------------------------------------------------------------------------

fn assert_slider_slot(sim: &Simulation, i: usize, u: [f64; 3], tick: u64) -> f32 {
    let t = slots(sim)[i];
    let x = f64::from(sim.mech_set().slider_state(i).expect("slider").0);
    for k in 0..3 {
        assert!(
            (f64::from(t.position_m[k]) - u[k] * x).abs() <= 1e-6,
            "tick {tick} slot {i} axis {k}: {:?} vs u*x = {:?}",
            t.position_m,
            [u[0] * x, u[1] * x, u[2] * x]
        );
    }
    assert_eq!(t.orientation, [0.0, 0.0, 0.0, 1.0], "tick {tick}");
    x as f32
}

/// REQ-VIS-002: a sling's slot is `u * x` (u from `angle_deg`) on every tick
/// of a kick and exactly zero at rest.
#[test]
fn req_vis_002_sling_offset_follows_slider() {
    slider_kick("sling", SLING, "sling_coil", 35.0);
}

/// REQ-VIS-002: a kickback's slot is `u * x` on every tick of a kick and
/// exactly zero at rest.
#[test]
fn req_vis_002_kickback_offset_follows_slider() {
    slider_kick("kickback", KICKBACK, "kick_coil", 120.0);
}

fn slider_kick(tag: &str, mech: &'static str, coil: &str, angle: f64) {
    let rules = format!(
        "function on_init() end\nfunction on_tick()\n  local t = pinball.tick()\n  \
         if t == 10 or t == 200 then pinball.pulse_coil(pinball.coil.{coil}, 25) end\nend\n"
    );
    let b = bundle(tag, &[mech], &rules);
    on_sim_thread(move || {
        let mut sim = load(&b);
        let u = dir(angle);
        assert!(is_identity(&slots(&sim)[0]), "rest at load");
        let mut max_x = 0.0f32;
        for _ in 0..500 {
            sim.advance(1);
            let tick = sim.tick();
            if tick < 10 {
                assert!(
                    is_identity(&slots(&sim)[0]),
                    "rest before the kick, tick {tick}"
                );
            }
            let x = assert_slider_slot(&sim, 0, u, tick);
            max_x = fmax32(max_x, x.abs());
        }
        assert!(max_x > 1e-4, "the kick never moved the slider ({max_x})");
        assert!(sim.snapshot().dynamic_meshes()[0].position_m != [0.0; 3] || max_x > 0.0);
    });
}

/// REQ-VIS-002: the plunger slot is `u * x` on every tick of a pull and
/// release (input axis) and of an auto pull (coil), and zero at rest.
#[test]
fn req_vis_002_plunger_pull_and_release() {
    let rules = "function on_init() end\nfunction on_tick()\n  \
                 if pinball.tick() == 600 then pinball.pulse_coil(pinball.coil.plunger_coil, 100) end\nend\n";
    let b = bundle("plunger", &[PLUNGER], rules);
    on_sim_thread(move || {
        let mut sim = load(&b);
        let u = dir(90.0);
        assert!(is_identity(&slots(&sim)[0]), "rest at load");
        let mut out = FrameStateSnapshot::default();
        let mut min_x = 0.0f32;
        let mut max_x = 0.0f32;
        for _ in 0..1000u64 {
            let t = sim.tick() + 1;
            let ev: Vec<InputEvent> = match t {
                20 => vec![InputEvent::axis(t * 1_000_000, CabinetAxis::Plunger, 1.0)],
                60 => vec![InputEvent::axis(t * 1_000_000, CabinetAxis::Plunger, 0.0)],
                _ => Vec::new(),
            };
            sim.step_1ms(t, &ev, &mut out);
            if t < 20 {
                assert!(
                    is_identity(&slots(&sim)[0]),
                    "rest before the pull, tick {t}"
                );
            }
            let x = assert_slider_slot(&sim, 0, u, t);
            min_x = if x < min_x { x } else { min_x };
            max_x = fmax32(max_x, x);
        }
        assert!(
            min_x < -1e-3 || max_x > 1e-3,
            "the plunger never moved (x in [{min_x}, {max_x}])"
        );
    });
}

// ---------------------------------------------------------------------------
// 5. gate and diverter
// ---------------------------------------------------------------------------

fn hinge_scenario(tag: &str, mechs: &[&'static str], idx: usize, pos_mm: [f64; 2]) {
    let rules = r#"
function on_init() end
function on_tick()
  local t = pinball.tick()
  if t == 20 then
    pinball.hold_coil(pinball.coil.gate_coil, true)
    pinball.set_diverter(pinball.diverter["posts[1]"], true)
  elseif t == 400 then
    pinball.hold_coil(pinball.coil.gate_coil, false)
    pinball.set_diverter(pinball.diverter["posts[1]"], false)
  end
end
"#;
    let b = bundle(tag, mechs, rules);
    on_sim_thread(move || {
        let mut sim = load(&b);
        assert!(
            is_identity(&slots(&sim)[idx]) || slots(&sim)[idx].orientation == [0.0, 0.0, 0.0, 1.0]
        );
        let mut max_angle = 0.0f64;
        let mut open_axis_ok = true;
        for _ in 0..1200 {
            sim.advance(1);
            let tick = sim.tick();
            let t = slots(&sim)[idx];
            assert!(finite(&t));
            assert!(
                (qlen(t.orientation) - 1.0).abs() <= 1e-6,
                "tick {tick}: {t:?}"
            );
            assert!(
                (f64::from(t.position_m[0]) - pos_mm[0] / 1000.0).abs() <= 1e-6,
                "tick {tick}: {t:?}"
            );
            assert!(
                (f64::from(t.position_m[1]) - pos_mm[1] / 1000.0).abs() <= 1e-6,
                "tick {tick}: {t:?}"
            );
            assert!(
                t.position_m[2].abs() <= 1e-6 || t.position_m[2] >= 0.0,
                "tick {tick}: {t:?}"
            );
            let (a, ax) = angle_axis(t.orientation);
            if tick < 20 {
                assert_eq!(
                    t.orientation,
                    [0.0, 0.0, 0.0, 1.0],
                    "closed/off at tick {tick}"
                );
            }
            if a > 1e-3 {
                open_axis_ok &= ax[2].abs() <= 1e-3;
            }
            max_angle = fmax(max_angle, a);
        }
        assert!(
            max_angle > 1e-3,
            "never rotated while open/on (max {max_angle})"
        );
        assert!(open_axis_ok, "rotation axis not horizontal");
        let t = slots(&sim)[idx];
        for c in 0..3 {
            assert!(
                t.orientation[c].abs() <= 1e-6,
                "not back to identity: {t:?}"
            );
        }
        assert!(
            (t.orientation[3] - 1.0).abs() <= 1e-6,
            "not back to identity: {t:?}"
        );
    });
}

/// REQ-VIS-002 (and 12): a gate publishes its hinge position, identity while
/// closed, a non-zero rotation about a horizontal axis while its coil is
/// held, and identity again once closed.
#[test]
fn req_vis_002_gate_rotates_when_open() {
    hinge_scenario("gate", &[GATE, DIVERTER], 0, [140.0, 760.0]);
}

/// REQ-VIS-002: a diverter publishes its hinge position, identity while off,
/// a non-zero rotation about a horizontal axis while on, and identity again
/// once off.
#[test]
fn req_vis_002_diverter_rotates_when_on() {
    hinge_scenario("diverter", &[DIVERTER, GATE], 0, [374.0, 760.0]);
}

// ---------------------------------------------------------------------------
// 6, 7, 8. vuk, bumper, newton
// ---------------------------------------------------------------------------

/// REQ-VIS-002: a vuk keeps its slot (index lines up) and publishes identity
/// at rest.
#[test]
fn req_vis_002_vuk_slot_is_identity() {
    let b = bundle("vuk", &[BUMPER, VUK], QUIET);
    on_sim_thread(move || {
        let mut sim = load(&b);
        assert_eq!(sim.snapshot().dynamic_mesh_count, 2);
        for _ in 0..100 {
            assert!(is_identity(&slots(&sim)[1]), "tick {}", sim.tick());
            sim.advance(1);
            assert_eq!(sim.snapshot().dynamic_mesh_count, 2);
        }
        assert!(is_identity(&slots(&sim)[1]));
    });
}

/// REQ-VIS-002: a bumper's slot is identity at rest, and after a kick (coil
/// pulse and a ball strike) the offset stays finite and under 5 mm.
#[test]
fn req_vis_002_bumper_ring_offset_small() {
    let rules = "function on_init() end\nfunction on_tick()\n  \
                 if pinball.tick() == 10 then pinball.pulse_coil(pinball.coil.ring2_coil, 20) end\nend\n";
    let b = bundle("bumper", &[BUMPER], rules);
    on_sim_thread(move || {
        let mut sim = load(&b);
        assert_eq!(sim.snapshot().dynamic_mesh_count, 1);
        assert!(is_identity(&slots(&sim)[0]));
        for k in 0..600u64 {
            if k == 200 {
                let _ = sim.spawn_ball("standard_steel", [0.150, 0.66, R], [0.0, 1.5, 0.0]);
            }
            sim.advance(1);
            let tick = sim.tick();
            let t = slots(&sim)[0];
            assert!(finite(&t));
            assert!((qlen(t.orientation) - 1.0).abs() <= 1e-6);
            let m = t
                .position_m
                .iter()
                .map(|c| f64::from(*c).powi(2))
                .sum::<f64>()
                .sqrt();
            assert!(m < 0.005, "tick {tick}: ring offset {m} m");
            if tick < 10 {
                assert!(is_identity(&t), "rest before the kick, tick {tick}");
            }
        }
    });
}

/// Runs the Newton table for 400 ticks, striking the captive ball at tick 30
/// when `strike`; returns (max swing over all ticks, max swing in the first
/// 20 ticks, swing at load).
fn newton_run(tag: &str, strike: bool) -> (f64, f64, f64) {
    let b = bundle(tag, &[NEWTON], QUIET);
    on_sim_thread(move || {
        let mut sim = load(&b);
        let at_load = angle_of(slots(&sim)[0].orientation);
        assert_eq!(
            slots(&sim)[0].orientation,
            [0.0, 0.0, 0.0, 1.0],
            "identity at load"
        );
        let (mut max_a, mut rest_max) = (0.0f64, 0.0f64);
        for k in 0..400u64 {
            if strike && k == 30 {
                let _ = sim.spawn_ball("standard_steel", [0.257, 0.24, R], [0.0, 1.5, 0.0]);
            }
            sim.advance(1);
            let tick = sim.tick();
            let t = slots(&sim)[0];
            assert!(finite(&t));
            assert!((qlen(t.orientation) - 1.0).abs() <= 1e-6);
            assert!(
                (f64::from(t.position_m[0]) - 0.257).abs() <= 1e-6,
                "tick {tick}: {t:?}"
            );
            assert!(
                (f64::from(t.position_m[1]) - 0.300).abs() <= 1e-6,
                "tick {tick}: {t:?}"
            );
            let a = angle_of(t.orientation);
            assert!(a <= newton_range() + 1e-6, "tick {tick}: swing {a}");
            if tick <= 20 {
                rest_max = fmax(rest_max, a);
            }
            max_a = fmax(max_a, a);
        }
        (max_a, rest_max, at_load)
    })
}

/// REQ-VIS-002: a Newton ball publishes its position (mech position), swing 0
/// at load and (within the 1e-3 rad the captive ball needs to settle under
/// the playfield pitch: observed 6.2e-4) at rest, a swing beyond the idle run
/// by more than 1e-4 rad after being struck, always within
/// `NEWTON_SWING_RANGE_RAD`.
#[test]
fn req_vis_002_newton_swing() {
    let (idle_max, rest_max, at_load) = newton_run("newton_idle", false);
    assert_eq!(at_load, 0.0);
    assert!(rest_max < 1e-3, "swing at rest {rest_max}");
    let (struck_max, _, _) = newton_run("newton_hit", true);
    assert!(
        struck_max > 1e-4 && struck_max > idle_max + 1e-4,
        "the struck cradle never swung beyond rest (struck {struck_max}, idle {idle_max})"
    );
}

// ---------------------------------------------------------------------------
// 9. soak and property
// ---------------------------------------------------------------------------

/// R3 / REQ-VIS-002: over a 3 200-tick soak of the all-kinds table every
/// published component is finite and every quaternion has unit length.
#[test]
fn req_vis_002_soak_finite_and_unit() {
    let b = bundle("soak", &all_kinds(), ALL_KINDS_RULES);
    on_sim_thread(move || {
        let mut sim = load(&b);
        let mut moved = [false; ALL_KINDS_COUNT];
        drive_all_kinds(&mut sim, 3200, |s| {
            let snap = s.snapshot();
            assert_eq!(usize::from(snap.dynamic_mesh_count), ALL_KINDS_COUNT);
            for (i, t) in snap.dynamic_meshes().iter().enumerate() {
                assert_slot_sane(i, t, snap.tick);
                if !is_identity(t) {
                    moved[i] = true;
                }
            }
        });
        // Slots that carry absolute pivots are never identity; the moving
        // offset kinds (sling, kickback, drop targets, plunger) must have moved.
        for i in [0usize, 1, 2, 3, 6] {
            assert!(moved[i], "slot {i} never moved during the soak");
        }
    });
}

/// R3 (property): random launch speed and direction into the all-kinds table
/// keeps every slot finite and unit, and the count equal to the mech count.
#[test]
fn req_vis_002_prop_launch_keeps_slots_valid() {
    let b = bundle("prop", &all_kinds(), ALL_KINDS_RULES);
    let path = b.path();
    let mut runner = TestRunner::new(Config {
        cases: 16,
        ..Config::default()
    });
    runner
        .run(&(0.1f32..4.0f32, 0.0f32..360.0f32), |(speed, deg)| {
            let path = path.clone();
            on_sim_thread(move || {
                let mut sim = Simulation::load(&path, SimConfig::default()).unwrap();
                let a = deg.to_radians();
                let v = [speed * libm::cosf(a), speed * libm::sinf(a), 0.0];
                let _ = sim.spawn_ball("standard_steel", [0.257, 0.5, R], v);
                for _ in 0..450 {
                    sim.advance(1);
                    let snap = sim.snapshot();
                    assert_eq!(usize::from(snap.dynamic_mesh_count), ALL_KINDS_COUNT);
                    for (i, t) in snap.dynamic_meshes().iter().enumerate() {
                        assert_slot_sane(i, t, snap.tick);
                    }
                }
            });
            Ok(())
        })
        .unwrap();
    drop(b);
}

// ---------------------------------------------------------------------------
// 10. determinism and replay digest
// ---------------------------------------------------------------------------

fn slot_bits(sim: &Simulation) -> Vec<[u32; 7]> {
    slots(sim)
        .iter()
        .map(|t| {
            [
                t.position_m[0].to_bits(),
                t.position_m[1].to_bits(),
                t.position_m[2].to_bits(),
                t.orientation[0].to_bits(),
                t.orientation[1].to_bits(),
                t.orientation[2].to_bits(),
                t.orientation[3].to_bits(),
            ]
        })
        .collect()
}

/// R3: two fresh simulations of the same table with the same inputs publish
/// bit-identical slots and the same state hash on every tick.
#[test]
fn req_vis_002_slots_bit_identical_across_runs() {
    let b = bundle("det", &all_kinds(), ALL_KINDS_RULES);
    let path = b.path();
    on_sim_thread(move || {
        let mut a = Simulation::load(&path, SimConfig::default()).unwrap();
        let mut c = Simulation::load(&path, SimConfig::default()).unwrap();
        for _ in 0..1700u64 {
            if a.tick().is_multiple_of(400) {
                spawn_launches(&mut a);
                spawn_launches(&mut c);
            }
            a.advance(1);
            c.advance(1);
            assert_eq!(slot_bits(&a), slot_bits(&c), "tick {}", a.tick());
            assert_eq!(a.state_hash(), c.state_hash(), "tick {}", a.tick());
        }
    });
    drop(b);
}

/// R3: publishing slots leaves the replay state hash at the committed golden
/// (replay fixture, 10 000 ticks of the standard input pattern, default seed).
#[test]
fn req_vis_002_replay_hash_matches_golden() {
    let path = fixture();
    let hash = on_sim_thread(move || {
        let sim = Simulation::load(&path, SimConfig::default()).unwrap();
        let (sim, rec) = record_pattern(sim, 10_000, false);
        assert_eq!(rec.ticks, 10_000);
        sim.state_hash()
    });
    assert_eq!(hash, GOLDEN_HASH, "state_hash {hash:016x}");
}

// ---------------------------------------------------------------------------
// 11. allocation guard
// ---------------------------------------------------------------------------

/// R4: stepping the all-kinds table for 2 000 ticks with moving mechs
/// allocates nothing in `step_1ms` (the slot publishing is part of the tick).
#[test]
fn req_vis_002_step_allocates_nothing() {
    let b = bundle("alloc", &all_kinds(), ALL_KINDS_RULES);
    let path = b.path();
    on_sim_thread(move || {
        let mut sim = Simulation::load(&path, SimConfig::default()).unwrap();
        let mut out = FrameStateSnapshot::default();
        let events: Vec<InputEvent> = Vec::new();
        let mut moved = false;
        for k in 0..2500u64 {
            let t = sim.tick() + 1;
            if sim.tick().is_multiple_of(400) {
                spawn_launches(&mut sim);
            }
            if k < 500 {
                sim.step_1ms(t, &events, &mut out);
                continue;
            }
            let ((), n, _exempt) = count_allocs_split(|| sim.step_1ms(t, &events, &mut out));
            assert_eq!(n, 0, "tick {t}: {n} allocation(s) in step_1ms");
            moved |= out.dynamic_meshes().iter().any(|m| !is_identity(m));
        }
        assert!(moved, "no mech moved during the guarded run");
        assert_eq!(usize::from(out.dynamic_mesh_count), ALL_KINDS_COUNT);
    });
    drop(b);
}
