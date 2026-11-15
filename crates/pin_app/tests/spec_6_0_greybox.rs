//! Subphase 6.0 spec tests (Independent Tester): the grey-box table
//! (`tables/greybox`, built with `tests/support::build_greybox_bundle`),
//! brief 6.0 items 4 and 5 and verification criterion "headless scripted
//! run exercises every mechanism and asserts switch counts; replay
//! deterministic".
//!
//! Everything is black-box: structure through `pin_assets::load_table`,
//! behaviour through the public `pin_sim::Simulation` API (state injection,
//! switch hit counts, scores) on simulated time.

mod support;

#[path = "fixtures/spec_6_0/common.rs"]
mod common;

use common::{STEEL, greybox_bundle, greybox_table, steel_radius_m};
use pin_app::orchestrator::load_validated;
use pin_assets::FlipperStageHint;
use pin_core::input::{CabinetInput, InputEvent};
use pin_core::schema::table::{MechKind, SplineKind, SwitchKind};
use pin_core::sim::{SimStep, TICK_NS};
use pin_sim::replay::Recorder;
use pin_sim::{BallHandle, SimConfig, Simulation};

/// Runs `f` on a thread with a generous stack (script backends recurse).
fn big_stack<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    std::thread::Builder::new()
        .name("spec-6.0-sim".into())
        .stack_size(64 << 20)
        .spawn(f)
        .expect("spawn")
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e))
}

fn load() -> Simulation {
    load_validated(greybox_bundle(), SimConfig::default()).expect("grey-box loads validated")
}

fn assert_healthy(sim: &Simulation, what: &str) {
    assert!(
        !sim.script_faulted(),
        "{what}: script faulted: {:?}",
        sim.script_error()
    );
    assert_eq!(
        sim.script_queue_overflows(),
        0,
        "{what}: script queue overflow"
    );
    assert_eq!(sim.counters().balls_escaped, 0, "{what}: a ball escaped");
    for h in sim.ball_handles() {
        let b = sim.ball(h).expect("live handle");
        let finite = b.position_m.iter().all(|v| v.is_finite())
            && b.velocity_m_s.iter().all(|v| v.is_finite())
            && b.angular_velocity_rad_s.iter().all(|v| v.is_finite());
        assert!(finite, "{what}: non-finite ball {b:?}");
    }
}

/// Closes switch `name` for `hold` ticks, opens it and hands it back to
/// physics. A forced pulse is not a physics hit: it is used only by the
/// Lua rule-event test (`full_three_ball_game_by_injection`), which checks
/// the rules' response to switch events; mechanism contact is proven by the
/// `physics_contact_*` tests.
fn pulse(sim: &mut Simulation, name: &str, hold: u64) {
    sim.set_switch(name, true).expect("known switch");
    sim.advance(hold);
    sim.set_switch(name, false).expect("known switch");
    sim.advance(5);
    sim.release_switch(name).expect("known switch");
}

fn hits(sim: &Simulation, name: &str) -> u32 {
    sim.switch_hits(name).expect("known switch")
}

// ---------------------------------------------------------------------------
// Structure (brief item 4)
// ---------------------------------------------------------------------------

#[test]
fn greybox_flippers_two_lower_one_upper() {
    let t = greybox_table();
    assert_eq!(t.manifest.flippers.len(), 3, "exactly 3 flippers");
    assert_eq!(t.flipper_stages.len(), 3);
    let lower: Vec<usize> = (0..3)
        .filter(|&i| t.flipper_stages[i] == FlipperStageHint::Lower)
        .collect();
    let upper = 3 - lower.len();
    assert_eq!(lower.len(), 2, "two lower flippers");
    assert_eq!(upper, 1, "one upper flipper");
    // The lower pair is a left and a right flipper.
    let xs: Vec<f32> = lower
        .iter()
        .map(|&i| t.manifest.flippers[i].base_position[0])
        .collect();
    assert!(xs[0] != xs[1], "lower flippers at different x");
}

#[test]
fn greybox_slings_bumpers_vuk_newton() {
    let t = greybox_table();
    let count = |k: MechKind| t.manifest.mechs.iter().filter(|m| m.kind == k).count();
    assert_eq!(count(MechKind::Sling), 2, "2 slings");
    assert_eq!(count(MechKind::Bumper), 3, "3 bumpers");
    assert_eq!(count(MechKind::Vuk), 1, "one VUK");
    assert_eq!(count(MechKind::NewtonBall), 1, "one newton_ball mech");
    let vuk = t
        .manifest
        .mechs
        .iter()
        .find(|m| m.kind == MechKind::Vuk)
        .expect("VUK");
    assert_eq!(
        vuk.exit_layer,
        Some(1),
        "the VUK delivers to the upper deck"
    );
}

#[test]
fn greybox_orbit_and_wireform_ramp() {
    let t = greybox_table();
    assert!(
        t.manifest
            .splines
            .iter()
            .any(|s| s.kind == SplineKind::Wireform),
        "at least one wireform ramp"
    );
    // An orbit: a guide lane, switch or shot named as one (the tablegen
    // lane ids are generic, so any named element counts).
    let orbit = |s: &str| s.to_ascii_lowercase().contains("orbit");
    let named = t.manifest.splines.iter().any(|s| orbit(&s.id))
        || t.manifest.switches.iter().any(|s| orbit(&s.id))
        || t.manifest.shots.iter().any(|s| orbit(&s.id))
        || t.manifest.mechs.iter().any(|m| orbit(&m.id));
    assert!(named, "at least one orbit");
    assert!(
        t.manifest
            .splines
            .iter()
            .any(|s| s.kind == SplineKind::Guide),
        "the orbit has guide rails"
    );
}

#[test]
fn greybox_magnet_eddy_dispenser_trough() {
    let t = greybox_table();
    assert_eq!(t.manifest.magnets.len(), 1, "one magnet");
    assert!(
        t.manifest
            .switches
            .iter()
            .any(|s| s.kind == SwitchKind::Eddy),
        "an eddy sensor"
    );
    let powerball = t
        .manifest
        .dispensers
        .iter()
        .filter(|d| d.contents.iter().any(|p| p == "ceramic_powerball"))
        .count();
    assert_eq!(powerball, 1, "one dispenser holds ceramic_powerball");
    assert!(
        t.manifest
            .switches
            .iter()
            .any(|s| s.kind == SwitchKind::Trough),
        "trough switches"
    );
}

#[test]
fn greybox_shots_modes_cabinet_switches() {
    let t = greybox_table();
    assert!(t.manifest.shots.len() >= 2, "[[shots]] >= 2");
    assert!(!t.manifest.modes.is_empty(), "[[modes]] >= 1");
    let cab = t.manifest.cabinet_switches;
    let mut numbers: Vec<u16> = CabinetInput::ALL.iter().map(|&i| cab.number(i)).collect();
    numbers.sort_unstable();
    numbers.dedup();
    assert_eq!(numbers.len(), 8, "8 distinct cabinet switch numbers");
    big_stack(|| {
        let sim = load();
        for key in [
            "left_flipper",
            "right_flipper",
            "left_upper",
            "right_upper",
            "action",
            "start",
            "launch",
            "tilt",
        ] {
            assert!(sim.switch_id(key).is_some(), "cabinet switch {key}");
        }
    });
}

// ---------------------------------------------------------------------------
// Physics-contact runs: every switch-bearing mechanism (verification
// criterion "headless scripted run exercises every mechanism")
// ---------------------------------------------------------------------------
//
// No switch is forced here: a ball is injected with `spawn_ball_on_layer`
// on a trajectory through the mechanism's contact path, the sim steps
// simulated ticks up to a bounded limit, and the test requires (a) a closing
// edge generated by the physics (the mech set's own edge list, or for the
// Newton ball the captive ball's motion, which is what drives its switch)
// counted by `switch_hits`, and (b) the mechanism's effect: score, coil
// fire (haptic impulse whose source is the coil), and ball / mech state.
// The game is started through the cabinet `start` input, not a forced
// switch.

/// Scores of the rules script (`tables/greybox/rules.lua` header).
const SLING_POINTS: i64 = 10;
const BUMPER_POINTS: i64 = 100;
const VUK_POINTS: i64 = 2_500;
const NEWTON_POINTS: i64 = 2_000;
const DROP_POINTS: i64 = 750;
/// rules.lua `VUK_HOLD` and `DROP_RESET_DELAY` (ticks).
const VUK_HOLD_TICKS: u64 = 400;
const DROP_RESET_TICKS: u64 = 1_000;
/// Speed above which a captive Newton ball closes its switch (pin_sim
/// `NEWTON_SWITCH_SPEED`, m/s).
const NEWTON_SWITCH_SPEED: f32 = 0.05;

/// One tick with cabinet input `input` pressed or released, through the
/// real input path (`SimStep::step_1ms`).
fn cabinet(sim: &mut Simulation, input: CabinetInput, pressed: bool) {
    let mut out = *sim.snapshot();
    let t = sim.tick() + 1;
    let ev = InputEvent::button(t * TICK_NS - TICK_NS / 2, input, pressed);
    sim.step_1ms(t, &[ev], &mut out);
}

/// Presses cabinet input `input` for `hold` ticks and releases it.
fn press(sim: &mut Simulation, input: CabinetInput, hold: u64) {
    cabinet(sim, input, true);
    sim.advance(hold);
    cabinet(sim, input, false);
}

/// Starts a game with the cabinet `start` button, waits for the served ball
/// and clears the table so that each contact run has a single ball.
fn start_game_by_input(sim: &mut Simulation) {
    press(sim, CabinetInput::Start, 50);
    assert!(wait_for_ball(sim, 10_000), "start serves a ball");
    clear_table(sim);
    sim.advance(50);
    assert!(sim.score(0).is_some(), "the game reported a score");
}

/// Removes every ball (no drain: the game goes on).
fn clear_table(sim: &mut Simulation) {
    let handles: Vec<BallHandle> = sim.ball_handles().iter().copied().collect();
    for h in handles {
        assert!(sim.remove_ball(h));
    }
}

/// What the physics did to a mechanism's switch, observed tick by tick.
struct Watch {
    switch: String,
    switch_id: pin_core::SwitchId,
    /// Index of the Newton chain driving the switch (Newton ball only).
    newton_chain: Option<usize>,
    /// Tick of the first physics-generated closing edge.
    physics_close: Option<u64>,
    /// `switch_hits` before the run.
    hits_before: u32,
    /// Every haptic impulse seen: (tick, source coil id).
    haptics: Vec<(u64, u16)>,
}

impl Watch {
    fn new(sim: &Simulation, switch: &str, newton_chain: Option<usize>) -> Watch {
        Watch {
            switch: switch.to_owned(),
            switch_id: sim.switch_id(switch).expect("known switch"),
            newton_chain,
            physics_close: None,
            hits_before: hits(sim, switch),
            haptics: Vec::new(),
        }
    }

    /// Steps one tick and records what the physics did.
    fn tick(&mut self, sim: &mut Simulation) {
        let was_closed = sim.switch_closed(&self.switch).expect("known switch");
        let hits_was = hits(sim, &self.switch);
        sim.advance(1);
        let physics_edge = match self.newton_chain {
            None => sim
                .mech_set()
                .edges()
                .iter()
                .any(|&(id, e)| id == self.switch_id && e == pin_core::SwitchEdge::Closed),
            Some(c) => {
                let chain = sim.solver().newton_chain(c);
                let moving =
                    (0..chain.len()).any(|i| chain.ball_speed(i).abs() > NEWTON_SWITCH_SPEED);
                moving && !was_closed && sim.switch_closed(&self.switch) == Some(true)
            }
        };
        if physics_edge && self.physics_close.is_none() {
            assert!(
                hits(sim, &self.switch) > hits_was,
                "{}: the physics edge is counted by switch_hits",
                self.switch
            );
            self.physics_close = Some(sim.tick());
        }
        for e in sim.haptic_events() {
            self.haptics.push((e.tick, e.source));
        }
    }

    /// One tick with cabinet input `input` pressed or released; records
    /// the haptic impulses of that tick (the script may fire a coil on it).
    fn cabinet(&mut self, sim: &mut Simulation, input: CabinetInput, pressed: bool) {
        cabinet(sim, input, pressed);
        for e in sim.haptic_events() {
            self.haptics.push((e.tick, e.source));
        }
    }

    /// Steps until `done` holds (checked after each tick); `false` after
    /// `max` ticks.
    fn run_until(
        &mut self,
        sim: &mut Simulation,
        max: u64,
        mut done: impl FnMut(&Simulation, &Watch) -> bool,
    ) -> bool {
        for _ in 0..max {
            self.tick(sim);
            if done(sim, self) {
                return true;
            }
        }
        false
    }

    /// Steps until the physics closes the switch (≤ `max` ticks) and
    /// asserts it did.
    fn expect_physics_hit(&mut self, sim: &mut Simulation, max: u64, what: &str) -> u64 {
        let ok = self.run_until(sim, max, |_, w| w.physics_close.is_some());
        assert!(
            ok,
            "{what}: no physics-generated closing edge of {} within {max} ticks",
            self.switch
        );
        assert!(hits(sim, &self.switch) > self.hits_before);
        self.physics_close.expect("edge recorded")
    }

    fn coil_fired(&self, coil: u16, from_tick: u64) -> bool {
        self.haptics
            .iter()
            .any(|&(t, s)| s == coil && t >= from_tick)
    }
}

fn vec_dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn vec_len(a: [f32; 3]) -> f32 {
    vec_dot(a, a).sqrt()
}

/// Unit vector of `angle_deg` in the playfield plane (90° = up the table).
fn dir(angle_deg: f32) -> [f32; 3] {
    let a = angle_deg.to_radians();
    [a.cos(), a.sin(), 0.0]
}

/// The ball centre at playfield point `p` (m, z = surface) offset by `d`
/// metres along `u`.
fn at(p: [f32; 3], u: [f32; 3], d: f32) -> [f32; 3] {
    [p[0] + u[0] * d, p[1] + u[1] * d, p[2] + steel_radius_m()]
}

fn scale(u: [f32; 3], k: f32) -> [f32; 3] {
    [u[0] * k, u[1] * k, u[2] * k]
}

/// Asserts the score rises by at least `points` within `max` ticks after
/// the physics edge (the script scores the edge on the next tick).
fn expect_score(sim: &mut Simulation, w: &mut Watch, before: i64, points: i64, what: &str) {
    let ok = w.run_until(sim, 50, |s, _| s.score(0).unwrap_or(0) - before >= points);
    assert!(
        ok,
        "{what}: score {} -> {:?}, expected +{points}",
        before,
        sim.score(0)
    );
}

/// Drives mechanism `name` by ball contact on a game in progress and
/// asserts the physics hit and its effect. Panics for a switch-bearing kind
/// without a driver (a new mechanism must get one).
fn drive_mech(sim: &mut Simulation, name: &str) {
    let t = greybox_table();
    let index = t
        .mechs
        .iter()
        .position(|m| m.name == name)
        .unwrap_or_else(|| panic!("mech {name} in the loaded table"));
    let mech = &t.mechs[index];
    let def = t
        .manifest
        .mechs
        .iter()
        .find(|d| d.id == name)
        .expect("manifest entry");
    let sw = def
        .switch
        .as_deref()
        .unwrap_or_else(|| panic!("mech {name} has a switch"));
    let p = mech
        .position_m
        .unwrap_or_else(|| panic!("mech {name} has a position"));
    let layer = def.layer.unwrap_or(0);
    let u = dir(def.angle_deg.unwrap_or(90.0));
    let coil = mech.coil.map(|c| c.0);
    clear_table(sim);
    sim.advance(20);
    let before = sim.score(0).unwrap_or(0);
    let what = format!("{name} ({:?})", def.kind);
    match def.kind {
        MechKind::Sling => {
            // Rolls into the kicker face against its kick direction.
            let h = sim
                .spawn_ball_on_layer(STEEL, layer, at(p, u, 0.04), scale(u, -1.0))
                .expect("spawn");
            let mut w = Watch::new(sim, sw, None);
            let edge = w.expect_physics_hit(sim, 300, &what);
            expect_score(sim, &mut w, before, SLING_POINTS, &what);
            let mut kick = 0.0f32;
            w.run_until(sim, 150, |s, _| {
                if let Some(b) = s.ball(h) {
                    kick = kick.max(vec_dot(b.velocity_m_s, u));
                }
                false
            });
            let coil = coil.expect("sling coil");
            assert!(w.coil_fired(coil, edge), "{what}: sling coil fired");
            assert!(
                kick >= 1.5,
                "{what}: the kicker drove the ball out along its kick direction ({kick} m/s)"
            );
        }
        MechKind::Bumper => {
            // Rolls up the table into the ring from below.
            let up = dir(90.0);
            let h = sim
                .spawn_ball_on_layer(STEEL, layer, at(p, up, -0.065), scale(up, 1.0))
                .expect("spawn");
            let mut w = Watch::new(sim, sw, None);
            let edge = w.expect_physics_hit(sim, 300, &what);
            expect_score(sim, &mut w, before, BUMPER_POINTS, &what);
            let mut out = 0.0f32;
            w.run_until(sim, 100, |s, _| {
                if let Some(b) = s.ball(h) {
                    let d = [b.position_m[0] - p[0], b.position_m[1] - p[1], 0.0];
                    let n = vec_len(d).max(1e-6);
                    out = out.max(vec_dot(b.velocity_m_s, scale(d, 1.0 / n)));
                }
                false
            });
            let coil = coil.expect("bumper coil");
            assert!(w.coil_fired(coil, edge), "{what}: bumper coil fired");
            // A 1 m/s approach rebounds passively at < 1 m/s; the kick
            // throws it out at >= the ring speed (1.7 m/s).
            assert!(
                out >= 1.5,
                "{what}: the ring kicked the ball out radially ({out} m/s)"
            );
        }
        MechKind::Vuk => {
            // Rolls down the table into the scoop.
            let up = dir(90.0);
            let h = sim
                .spawn_ball_on_layer(STEEL, layer, at(p, up, 0.04), scale(up, -0.3))
                .expect("spawn");
            let mut w = Watch::new(sim, sw, None);
            let edge = w.expect_physics_hit(sim, 1_000, &what);
            assert_eq!(
                sim.mech_set().vuk_ball(index).map(|b| b.0),
                Some(h.slot()),
                "{what}: the scoop holds the ball"
            );
            expect_score(sim, &mut w, before, VUK_POINTS, &what);
            let exit = def.exit_layer.expect("VUK exit layer");
            let coil = coil.expect("VUK coil");
            let ok = w.run_until(sim, VUK_HOLD_TICKS + 500, |s, w| {
                w.coil_fired(coil, edge) && s.ball(h).is_some_and(|b| b.layer == exit)
            });
            assert!(
                ok,
                "{what}: VUK coil fired and the ball reached layer {exit}"
            );
        }
        MechKind::Plunger => {
            // Rolls down the shooter lane onto the plunger tip.
            let h = sim
                .spawn_ball_on_layer(STEEL, layer, at(p, u, 0.07), scale(u, -0.3))
                .expect("spawn");
            let mut w = Watch::new(sim, sw, None);
            let edge = w.expect_physics_hit(sim, 1_000, &what);
            w.run_until(sim, 300, |_, _| false);
            // The launch button (rules: pulse the auto-plunger coil).
            let lane_before = hits(sim, "shooter_lane");
            w.cabinet(sim, CabinetInput::Launch, true);
            let mut launch = 0.0f32;
            let ok = w.run_until(sim, 2_000, |s, _| {
                if let Some(b) = s.ball(h) {
                    launch = launch.max(vec_dot(b.velocity_m_s, u));
                }
                hits(s, "shooter_lane") > lane_before
            });
            w.cabinet(sim, CabinetInput::Launch, false);
            let coil = coil.expect("plunger coil");
            assert!(w.coil_fired(coil, edge), "{what}: auto-plunger coil fired");
            assert!(ok, "{what}: the launched ball passed the shooter-lane opto");
            assert!(launch >= 1.0, "{what}: launch speed {launch} m/s");
        }
        MechKind::NewtonBall => {
            assert_eq!(sim.solver().newton_chain_count(), 1, "one Newton chain");
            let start = sim.solver().newton_chain(0).ball_position(0);
            // Rolls along the captive ball's path into it.
            sim.spawn_ball_on_layer(STEEL, layer, at(p, u, -0.045), scale(u, 1.0))
                .expect("spawn");
            let mut w = Watch::new(sim, sw, Some(0));
            w.expect_physics_hit(sim, 300, &what);
            expect_score(sim, &mut w, before, NEWTON_POINTS, &what);
            let mut moved = 0.0f32;
            w.run_until(sim, 100, |s, _| {
                let q = s.solver().newton_chain(0).ball_position(0);
                moved = moved.max((q - start).length());
                false
            });
            assert!(
                moved > 0.002,
                "{what}: the captive ball moved along its path ({moved} m)"
            );
        }
        MechKind::DropTarget => {
            // Rolls into the target face (`angle_deg` = face normal).
            sim.spawn_ball_on_layer(STEEL, layer, at(p, u, 0.04), scale(u, -1.0))
                .expect("spawn");
            let mut w = Watch::new(sim, sw, None);
            let edge = w.expect_physics_hit(sim, 300, &what);
            assert_eq!(
                sim.mech_set().drop_target_down(index),
                Some(true),
                "{what}: the impact dropped the target"
            );
            expect_score(sim, &mut w, before, DROP_POINTS, &what);
            let coil = coil.expect("drop reset coil");
            let ok = w.run_until(sim, DROP_RESET_TICKS + 1_000, |s, w| {
                w.coil_fired(coil, edge)
                    && s.mech_set().drop_target_down(index) == Some(false)
                    && s.switch_closed(sw) == Some(false)
            });
            assert!(ok, "{what}: reset coil fired and the target rose again");
        }
        k => panic!("{what}: switch-bearing kind {k:?} has no physics-contact driver"),
    }
    assert_healthy(sim, &what);
}

/// A game in progress, started by the cabinet `start` input.
fn game() -> Simulation {
    let mut sim = load();
    sim.advance(200);
    start_game_by_input(&mut sim);
    sim
}

/// The switch-bearing `[[mechs]]` of the table, manifest order.
fn switch_mechs() -> Vec<String> {
    greybox_table()
        .manifest
        .mechs
        .iter()
        .filter(|m| m.switch.is_some())
        .map(|m| m.id.clone())
        .collect()
}

/// The mechanisms with a dedicated `physics_contact_*` test below.
const CONTACT_TESTS: [&str; 9] = [
    "sling_left",
    "sling_right",
    "bumper_1",
    "bumper_2",
    "bumper_3",
    "vuk",
    "plunger",
    "newton",
    "drop_1",
];

/// One `physics_contact_*` test per switch-bearing mechanism, and no test
/// for a mechanism the table does not have.
#[test]
fn every_switch_mech_has_a_physics_contact_test() {
    let mechs = switch_mechs();
    for m in &mechs {
        assert!(
            CONTACT_TESTS.contains(&m.as_str()),
            "switch-bearing mech {m} has no physics_contact_* test"
        );
    }
    for c in CONTACT_TESTS {
        assert!(
            mechs.iter().any(|m| m == c),
            "physics_contact test for {c}, which is not a switch-bearing mech"
        );
    }
}

macro_rules! contact_test {
    ($test:ident, $mech:literal) => {
        #[test]
        fn $test() {
            big_stack(|| {
                let mut sim = game();
                drive_mech(&mut sim, $mech);
            });
        }
    };
}

contact_test!(physics_contact_sling_left, "sling_left");
contact_test!(physics_contact_sling_right, "sling_right");
contact_test!(physics_contact_bumper_1, "bumper_1");
contact_test!(physics_contact_bumper_2, "bumper_2");
contact_test!(physics_contact_bumper_3, "bumper_3");
contact_test!(physics_contact_vuk, "vuk");
contact_test!(physics_contact_plunger, "plunger");
contact_test!(physics_contact_newton, "newton");
contact_test!(physics_contact_drop_1, "drop_1");

/// Every `[[mechs]]` entry on one game: a switch-bearing mech is hit by a
/// ball (physics edge + effect, as in the `physics_contact_*` tests); a
/// switch-less mech gets a ball spawned onto it. No switch is forced; no
/// ball is lost or non-finite.
#[test]
fn every_mech_is_exercised() {
    big_stack(|| {
        let t = greybox_table();
        let mut sim = game();
        let r = steel_radius_m();
        for m in &t.manifest.mechs {
            match &m.switch {
                Some(sw) => {
                    let before = hits(&sim, sw);
                    drive_mech(&mut sim, &m.id);
                    assert!(
                        hits(&sim, sw) > before,
                        "mech {} ({:?}): switch {sw} hit count did not increase",
                        m.id,
                        m.kind
                    );
                }
                None => {
                    let p = m
                        .position
                        .unwrap_or_else(|| panic!("switch-less mech {} has a position", m.id));
                    let layer = m.layer.unwrap_or(0);
                    let pos = [p[0] * 1e-3, p[1] * 1e-3, p[2] * 1e-3 + r];
                    sim.spawn_ball_on_layer(STEEL, layer, pos, [0.0; 3])
                        .unwrap_or_else(|e| panic!("spawn onto {}: {e}", m.id));
                    sim.advance(500);
                }
            }
            assert_healthy(&sim, &m.id);
        }
        clear_table(&mut sim);
        sim.advance(2_000);
        assert_healthy(&sim, "after all mechs");
    });
}

/// Physically: a steel (conductive) ball on the eddy sensor closes it.
#[test]
fn eddy_sensor_sees_a_steel_ball() {
    big_stack(|| {
        let t = greybox_table();
        let eddy = t
            .switches
            .iter()
            .find(|s| s.kind == SwitchKind::Eddy)
            .expect("eddy switch");
        let p = eddy
            .position_m
            .unwrap_or_else(|| panic!("eddy switch {} has a position", eddy.name));
        let mut sim = load();
        sim.advance(100);
        let before = hits(&sim, &eddy.name);
        sim.spawn_ball_on_layer(
            STEEL,
            eddy.layer,
            [p[0], p[1], p[2] + steel_radius_m()],
            [0.0; 3],
        )
        .expect("spawn on the eddy sensor");
        sim.advance(3);
        assert!(
            hits(&sim, &eddy.name) > before,
            "steel ball over the eddy sensor"
        );
        assert_healthy(&sim, "eddy");
    });
}

/// Presses the cabinet `start` button (real input path) and waits (≤ 10 s
/// simulated) for a ball to be served.
fn start_game(sim: &mut Simulation) {
    press(sim, CabinetInput::Start, 50);
    assert!(wait_for_ball(sim, 10_000), "start serves a ball");
}

/// Steps one tick at a time until a ball is on the table; `false` after
/// `max` ticks.
fn wait_for_ball(sim: &mut Simulation, max: u64) -> bool {
    for _ in 0..max {
        if sim.balls_in_play() > 0 {
            return true;
        }
        sim.advance(1);
    }
    sim.balls_in_play() > 0
}

/// The VUK delivers a ball to the upper deck (layer 1).
#[test]
fn vuk_delivers_to_layer_1() {
    big_stack(|| {
        let t = greybox_table();
        let vuk = t
            .manifest
            .mechs
            .iter()
            .find(|m| m.kind == MechKind::Vuk)
            .expect("VUK");
        let p = vuk.position.expect("VUK position");
        let layer = vuk.layer.unwrap_or(0);
        let mut sim = load();
        start_game(&mut sim);
        let r = steel_radius_m();
        let h = sim
            .spawn_ball_on_layer(
                STEEL,
                layer,
                [p[0] * 1e-3, p[1] * 1e-3, p[2] * 1e-3 + r],
                [0.0; 3],
            )
            .expect("spawn into the scoop");
        let vuk_hits = vuk.switch.as_ref().map(|s| (s.clone(), hits(&sim, s)));
        let mut delivered = false;
        for _ in 0..5_000 {
            sim.advance(1);
            if sim.ball(h).is_some_and(|b| b.layer == 1) {
                delivered = true;
                break;
            }
        }
        assert!(delivered, "the VUK put the ball on layer 1");
        if let Some((sw, before)) = vuk_hits {
            assert!(
                hits(&sim, &sw) > before,
                "VUK switch {sw} counted the capture"
            );
        }
        assert_healthy(&sim, "vuk");
    });
}

// ---------------------------------------------------------------------------
// Full 3-ball game (brief item 5)
// ---------------------------------------------------------------------------

/// Drains every ball on the table through the first trough switch: moved
/// onto it (when it has a position) or removed with the switch pulsed.
fn drain_all(sim: &mut Simulation, trough: &pin_assets::SwitchSi) {
    let handles: Vec<BallHandle> = sim.ball_handles().iter().copied().collect();
    for h in handles {
        let Some(mut b) = sim.ball(h) else { continue };
        match trough.position_m {
            Some(p) => {
                b.position_m = [p[0], p[1], p[2] + steel_radius_m()];
                b.velocity_m_s = [0.0; 3];
                b.angular_velocity_rad_s = [0.0; 3];
                b.layer = trough.layer;
                assert!(sim.set_ball_state(h, &b), "move the ball onto the trough");
                sim.advance(3);
                assert!(sim.ball(h).is_none(), "the ball drained into the trough");
            }
            None => {
                assert!(sim.remove_ball(h));
                pulse(sim, &trough.name, 5);
            }
        }
    }
}

/// Lua rule-event test (forced switch pulses): the rules' 3-ball game flow
/// and scoring, driven by state injection.
#[test]
fn full_three_ball_game_by_injection() {
    big_stack(|| {
        let t = greybox_table();
        let trough = t
            .switches
            .iter()
            .find(|s| s.kind == SwitchKind::Trough && s.position_m.is_some())
            .or_else(|| t.switches.iter().find(|s| s.kind == SwitchKind::Trough))
            .expect("trough switch")
            .clone();
        let scorers: Vec<String> = t
            .manifest
            .mechs
            .iter()
            .filter(|m| matches!(m.kind, MechKind::Bumper | MechKind::Sling))
            .filter_map(|m| m.switch.clone())
            .collect();
        assert_eq!(scorers.len(), 5, "3 bumpers + 2 slings have switches");

        let mut sim = load();
        sim.advance(2_000);
        assert_eq!(sim.balls_in_play(), 0, "no ball before start");
        start_game(&mut sim);

        for ball in 1..=3u32 {
            if ball > 1 {
                assert!(
                    wait_for_ball(&mut sim, 10_000),
                    "ball {ball} is served after the previous drain"
                );
            }
            press(&mut sim, CabinetInput::Launch, 50);
            // Lua rule events: forced scorer pulses check the scoring rules
            // (physics contact of each scorer: `physics_contact_*`).
            for sw in &scorers {
                let before = sim.score(0).unwrap_or(0);
                pulse(&mut sim, sw, 20);
                sim.advance(20);
                let after = sim.score(0).unwrap_or(0);
                assert!(
                    after > before,
                    "ball {ball}: {sw} scores ({before} -> {after})"
                );
            }
            let drained = sim.counters().balls_drained;
            let trough_hits = hits(&sim, &trough.name);
            drain_all(&mut sim, &trough);
            assert!(
                hits(&sim, &trough.name) > trough_hits,
                "trough switch counted"
            );
            if trough.position_m.is_some() {
                assert!(sim.counters().balls_drained > drained);
            }
            assert_healthy(&sim, &format!("ball {ball}"));
        }

        // Game over: no fourth ball within 10 s.
        for _ in 0..10_000 {
            sim.advance(1);
            assert_eq!(sim.balls_in_play(), 0, "no ball after the 3rd drain");
        }
        let score = sim.score(0).expect("score reported");
        assert!(score > 0, "final score {score} > 0");
        assert_healthy(&sim, "game over");
    });
}

// ---------------------------------------------------------------------------
// Replay determinism (verification criterion; gate 6)
// ---------------------------------------------------------------------------

/// A recorded input stream (start, launch, flipper presses) replays to the
/// same `state_hash` 10 times.
#[test]
fn recorded_inputs_replay_identically_10_times() {
    big_stack(|| {
        let sim = Simulation::load(greybox_bundle(), SimConfig::default()).expect("loads");
        let mut rec = Recorder::new(sim);
        let ticks = 12_000u64;
        let schedule: Vec<(u64, CabinetInput, bool)> = {
            let mut v = vec![
                (100, CabinetInput::Start, true),
                (180, CabinetInput::Start, false),
                (1_500, CabinetInput::Launch, true),
                (1_600, CabinetInput::Launch, false),
            ];
            let mut k = 2_000;
            let mut left = true;
            while k < ticks - 500 {
                let i = if left {
                    CabinetInput::LeftFlipper
                } else {
                    CabinetInput::RightFlipper
                };
                v.push((k, i, true));
                v.push((k + 120, i, false));
                if k % 3 == 0 {
                    v.push((k + 40, CabinetInput::RightUpper, true));
                    v.push((k + 90, CabinetInput::RightUpper, false));
                }
                k += 377;
                left = !left;
            }
            v.sort_by_key(|e| e.0);
            v
        };
        let mut next = 0;
        let mut evs: Vec<InputEvent> = Vec::new();
        for tick in 1..=ticks {
            evs.clear();
            while next < schedule.len() && schedule[next].0 == tick {
                let (_, i, p) = schedule[next];
                evs.push(InputEvent::button(tick * TICK_NS - TICK_NS / 2, i, p));
                next += 1;
            }
            rec.step(&evs);
        }
        let (sim, recording) = rec.finish();
        assert!(sim.switch_hits("start").expect("start") >= 1);
        assert!(sim.switch_hits("left_flipper").expect("lf") >= 1);
        assert!(!sim.script_faulted());
        let expected = sim.state_hash();
        for run in 0..10 {
            let got = pin_sim::replay::replay(greybox_bundle(), &recording).expect("replays");
            assert_eq!(got, expected, "run {run}: replay diverged");
        }
        // The same stream through SimStep directly, on fresh loads.
        for run in 0..2 {
            let mut s = Simulation::load(greybox_bundle(), SimConfig::default()).expect("loads");
            assert_eq!(
                Recorder::play(&mut s, &recording),
                expected,
                "play run {run}"
            );
        }
    });
}
