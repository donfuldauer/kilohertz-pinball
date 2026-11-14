//! Subphase 2.5 integration obligations: the mechs 2.4 leaves inert are wired
//! by the simulation (DECISIONS.md "2.5 — diverters, gates and Newton
//! balls"): a gate's colliders open while its coil is held, a diverter's
//! `<id>` colliders are solid only while it is on, and a `newton_ball`
//! becomes a captive ball whose switch closes while it moves.
//!
//! The fixture's glb names its two posts `posts[0]` and `posts[1]`, so a gate
//! mech named `posts[0]` and a diverter named `posts[1]` bind to them.

mod sim_support;

use pin_sim::{SimConfig, Simulation};
use sim_support::*;

const EXTRA: &str = r#"
[[coils]]
id = "gate_coil"
number = 4
kind = "kicker"

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
"#;

fn bundle(tag: &str, rules: &str) -> ScratchDir {
    let table = fixture_table_toml() + EXTRA;
    fixture_variant(
        tag,
        &[
            ("table.toml", Some(table.as_bytes())),
            ("rules.lua", Some(rules.as_bytes())),
        ],
    )
}

/// Rolls a ball straight at the post at `x` (centre line) and returns its
/// position after 120 ticks.
fn roll_at_post(sim: &mut Simulation, x: f32) -> [f32; 3] {
    let h = sim
        .spawn_ball("standard_steel", [x, 0.80, 0.0135], [0.0, -1.0, 0.0])
        .unwrap();
    sim.advance(120);
    sim.ball(h).expect("ball still on the table").position_m
}

fn passed(p: [f32; 3], x: f32) -> bool {
    p[1] < 0.72 && (p[0] - x).abs() < 0.002
}

#[test]
fn closed_gate_and_off_diverter() {
    let b = bundle("mech_idle", "function on_tick() end\n");
    on_sim_thread(move || {
        let mut sim = Simulation::load(&b, SimConfig::default()).unwrap();
        assert_eq!(sim.switch_closed("gate_sw"), Some(false));
        // The gate's post is solid: the ball bounces off it.
        let p = roll_at_post(&mut sim, 0.140);
        assert!(!passed(p, 0.140), "closed gate let the ball through: {p:?}");
        // The diverter is off, so its `<id>` collider is disabled.
        let p = roll_at_post(&mut sim, 0.374);
        assert!(passed(p, 0.374), "off diverter blocked the ball: {p:?}");
    });
}

#[test]
fn held_gate_opens_and_diverter_on_blocks() {
    let rules = r#"
function on_init()
  pinball.hold_coil(pinball.coil.gate_coil, true)
  pinball.set_diverter(pinball.diverter["posts[1]"], true)
end
function on_tick() end
"#;
    let b = bundle("mech_driven", rules);
    on_sim_thread(move || {
        let mut sim = Simulation::load(&b, SimConfig::default()).unwrap();
        sim.advance(1);
        assert!(!sim.script_faulted(), "{:?}", sim.script_error());
        assert_eq!(
            sim.switch_closed("gate_sw"),
            Some(true),
            "gate switch reads open"
        );
        let p = roll_at_post(&mut sim, 0.140);
        assert!(passed(p, 0.140), "open gate blocked the ball: {p:?}");
        let p = roll_at_post(&mut sim, 0.374);
        assert!(!passed(p, 0.374), "diverter on let the ball through: {p:?}");
    });
}

#[test]
fn gate_pulse_opens_for_its_duration() {
    let rules = r#"
function on_tick()
  if pinball.tick() == 2 then pinball.pulse_coil(pinball.coil.gate_coil, 30) end
end
"#;
    let b = bundle("mech_pulse", rules);
    on_sim_thread(move || {
        let mut sim = Simulation::load(&b, SimConfig::default()).unwrap();
        sim.advance(2);
        assert_eq!(sim.switch_closed("gate_sw"), Some(true));
        assert!(
            !sim.haptic_events().is_empty(),
            "a coil pulse is a haptic event"
        );
        sim.advance(30);
        assert_eq!(sim.switch_closed("gate_sw"), Some(false));
        assert_eq!(sim.switch_hits("gate_sw"), Some(1));
    });
}

#[test]
fn newton_ball_switch_closes_when_struck() {
    let b = bundle("mech_newton", "function on_tick() end\n");
    on_sim_thread(move || {
        let mut sim = Simulation::load(&b, SimConfig::default()).unwrap();
        sim.advance(20);
        assert_eq!(
            sim.switch_hits("newton_sw"),
            Some(0),
            "captive ball at rest"
        );
        // Strike the captive ball (path from y = 300 mm up the table) from below.
        sim.spawn_ball("standard_steel", [0.257, 0.24, 0.0135], [0.0, 1.5, 0.0])
            .unwrap();
        sim.advance(200);
        assert!(sim.switch_hits("newton_sw").unwrap() >= 1);
        assert_eq!(sim.solver().newton_chain_count(), 1);
    });
}

/// A trough on layer 1 behind a 0 -> 1 layer transition. A ball that enters
/// both in one tick drains, and the trough switch still closes in that tick
/// (the drain uses the ball's final layer; so does the trough switch).
const UPPER_TROUGH: &str = r#"
[[switches]]
id = "upper_trough"
number = 22
kind = "trough"
layer = 1
position = [440.0, 600.0, 0.0]
radius = 20.0

[[layer_transitions]]
id = "to_upper"
from_layer = 0
to_layer = 1
zone = { min = [420.0, 580.0, -50.0], max = [460.0, 620.0, 100.0] }
"#;

#[test]
fn trough_closes_when_a_ball_drains_across_a_layer_transition() {
    let table = fixture_table_toml() + UPPER_TROUGH;
    let dir = fixture_variant("upper_trough", &[("table.toml", Some(table.as_bytes()))]);
    let path = dir.to_path_buf();
    on_sim_thread(move || {
        let mut sim = Simulation::load(&path, SimConfig::default()).unwrap();
        let drained = sim.counters().balls_drained;
        // 0.5 mm short of the zone, 1 m/s up the table (1 mm per tick).
        let h = sim
            .spawn_ball("standard_steel", [0.440, 0.5795, 0.0135], [0.0, 1.0, 0.0])
            .unwrap();
        assert_eq!(sim.switch_closed("upper_trough"), Some(false));
        sim.advance(1);
        assert!(sim.ball(h).is_none(), "the ball drained");
        assert_eq!(sim.counters().balls_drained, drained + 1);
        assert_eq!(sim.switch_closed("upper_trough"), Some(true));
        assert_eq!(sim.switch_hits("upper_trough"), Some(1));
        sim.advance(1);
        assert_eq!(sim.switch_closed("upper_trough"), Some(false));
    });
    drop(dir);
}
