//! 6.0 C4/C5: the grey-box table in the headless simulation.
//!
//! * Every mechanism kind of the table, driven by state injection
//!   (`spawn_ball` / `set_ball_state` / `set_switch` / `advance`), closes its
//!   switch (exact counts where the run is deterministic).
//! * A scripted 3-ball game: attract → start → three balls (ball 2 is the
//!   Powerball, detected at the magnet: the `powerball` mode starts) → game
//!   over → attract; the score equals the rules' points for the switch hits
//!   (doubled in the Powerball mode), with no script fault, queue overflow or
//!   dropped edge.
//! * The rules' display scenes (attract, score, Powerball, game over), read
//!   from the script's command queue (the sim does not expose them).
//! * A recorded input replay of a game reaches the same `state_hash` 10 out
//!   of 10 times (D9), and the scripted game is reproducible.

mod support;

use pin_core::ids::SwitchId;
use pin_core::script::{HostCommandQueue, HostCtx, HostView, SwitchEdge, TableScriptBackend};
use pin_core::snapshot::BallState;
use pin_core::{CabinetInput, InputEvent, Pcg32, SeedableRng, TICK_NS};
use pin_sim::replay::{Recorder, replay};
use pin_sim::{BallHandle, SimConfig, Simulation};
use std::collections::BTreeMap;
use support::{greybox_bundle, on_big_stack};

/// Points per closing edge (rules.lua), switches that score nothing omitted.
const POINTS: [(&str, i64); 14] = [
    ("bumper_1_ring", 100),
    ("bumper_2_ring", 100),
    ("bumper_3_ring", 100),
    ("sling_left_sw", 10),
    ("sling_right_sw", 10),
    ("orbit_entry", 500),
    ("orbit_top", 5000),
    ("ramp_entry", 100),
    ("ramp_made", 10000),
    ("vuk_sw", 2500),
    ("deck_opto", 1500),
    ("magnet_eddy", 250),
    ("newton_sw", 2000),
    ("drop_1_sw", 750),
];

/// Every switch of the table (cabinet switches excluded).
const SWITCHES: [&str; 22] = [
    "trough_1",
    "trough_2",
    "lower_left_eos",
    "lower_right_eos",
    "upper_left_eos",
    "sling_left_sw",
    "sling_right_sw",
    "bumper_1_ring",
    "bumper_2_ring",
    "bumper_3_ring",
    "orbit_entry",
    "orbit_top",
    "ramp_entry",
    "ramp_made",
    "vuk_sw",
    "magnet_eddy",
    "magnet_opto",
    "newton_sw",
    "drop_1_sw",
    "shooter_lane",
    "deck_opto",
    "plunger_tip",
];

// Persist keys (table.toml [persist] order).
const GAMES: u16 = 0;
const HIGH: u16 = 1;
const BALL: u16 = 2;
const STATE: u16 = 3;
const PB_MODES: u16 = 4;

type Hits = BTreeMap<&'static str, u32>;

fn hits(s: &Simulation) -> Hits {
    SWITCHES
        .iter()
        .map(|n| (*n, s.switch_hits(n).expect(n)))
        .collect()
}

fn delta(before: &Hits, after: &Hits, name: &str) -> u32 {
    after[name] - before[name]
}

fn points(before: &Hits, after: &Hits) -> i64 {
    POINTS
        .iter()
        .map(|(n, p)| i64::from(delta(before, after, n)) * p)
        .sum()
}

fn load() -> Simulation {
    Simulation::load(greybox_bundle(), SimConfig::default()).unwrap()
}

fn press(s: &mut Simulation, name: &str, ticks: u64) {
    s.set_switch(name, true).unwrap();
    s.advance(ticks);
    s.release_switch(name).unwrap();
}

fn mm(x: f32, y: f32) -> [f32; 3] {
    [x * 1.0e-3, y * 1.0e-3, 0.0135]
}

fn healthy(s: &Simulation) {
    assert!(!s.script_faulted(), "script fault: {:?}", s.script_error());
    assert_eq!(s.script_queue_overflows(), 0, "script queue overflow");
    let c = s.counters();
    assert_eq!(c.edges_dropped, 0, "{c:?}");
    assert_eq!(c.haptics_dropped, 0, "{c:?}");
    assert_eq!(c.spawns_failed, 0, "{c:?}");
    assert_eq!(c.balls_escaped, 0, "{c:?}");
}

/// Spawns a ball, runs `ticks`, removes it if still there; returns the hit
/// deltas and the ball's layers seen (bit per layer).
fn trial(
    s: &mut Simulation,
    profile: &str,
    p: [f32; 3],
    v: [f32; 3],
    ticks: u64,
) -> (Hits, Hits, u8) {
    let before = hits(s);
    let h = s.spawn_ball(profile, p, v).unwrap();
    let mut layers = 0u8;
    for _ in 0..ticks {
        s.advance(1);
        match s.ball(h) {
            Some(b) => layers |= 1 << b.layer,
            None => break,
        }
    }
    if s.ball(h).is_some() {
        s.remove_ball(h);
    }
    (before, hits(s), layers)
}

/// Starts a game: the first ball lands on the plunger.
fn start_game(s: &mut Simulation) {
    press(s, "start", 5);
    s.advance(1500);
    assert_eq!(s.persist(STATE), 1, "game state");
    assert_eq!(s.persist(BALL), 1);
    assert_eq!(s.balls_in_play(), 1);
    assert_eq!(
        s.switch_closed("plunger_tip"),
        Some(true),
        "ball on the plunger"
    );
}

#[test]
fn every_mechanism_kind_closes_its_switch() {
    on_big_stack(|| {
        let mut s = load();
        s.advance(10);
        start_game(&mut s);
        let served: Vec<BallHandle> = s.ball_handles().to_vec();

        // Flippers (lower left/right, upper left): the button raises the
        // bat; the EOS switch closes again when it returns.
        for (button, flipper, eos) in [
            ("left_flipper", "lower_left", "lower_left_eos"),
            ("right_flipper", "lower_right", "lower_right_eos"),
            ("left_upper", "upper_left", "upper_left_eos"),
        ] {
            let before = hits(&s);
            s.set_switch(button, true).unwrap();
            s.advance(80);
            let (angle, _) = s.flipper(flipper).unwrap();
            assert!(angle.abs() > 0.7, "{flipper} raised: {angle}");
            s.release_switch(button).unwrap();
            s.advance(300);
            let (angle, _) = s.flipper(flipper).unwrap();
            assert!(angle.abs() < 0.05, "{flipper} back at rest: {angle}");
            assert_eq!(delta(&before, &hits(&s), eos), 1, "{eos}");
        }

        // Slingshots: a ball rolling into the rubber; the rules fire the coil.
        let (b, a, _) = trial(
            &mut s,
            "standard_steel",
            mm(149.0, 324.0),
            [-0.8, -0.6, 0.0],
            300,
        );
        assert!(delta(&b, &a, "sling_left_sw") >= 1, "left sling");
        let (b, a, _) = trial(
            &mut s,
            "standard_steel",
            mm(326.0, 324.0),
            [0.8, -0.6, 0.0],
            300,
        );
        assert!(delta(&b, &a, "sling_right_sw") >= 1, "right sling");

        // Pop bumpers.
        for (name, x, y, v) in [
            ("bumper_1_ring", 190.0, 640.0, 1.0),
            ("bumper_2_ring", 324.0, 640.0, 1.0),
            ("bumper_3_ring", 257.0, 700.0, 1.5),
        ] {
            let (b, a, _) = trial(&mut s, "standard_steel", mm(x, y), [0.0, v, 0.0], 250);
            assert!(delta(&b, &a, name) >= 1, "{name}");
        }

        // Orbit: up the left lane and around the top, once each.
        let (b, a, _) = trial(
            &mut s,
            "standard_steel",
            mm(30.0, 430.0),
            [0.0, 2.5, 0.0],
            700,
        );
        assert_eq!(delta(&b, &a, "orbit_entry"), 1);
        assert_eq!(delta(&b, &a, "orbit_top"), 1);

        // Wireform ramp onto the upper deck (layers 0 -> 2 -> 1 -> 0).
        let (b, a, layers) = trial(
            &mut s,
            "standard_steel",
            mm(420.0, 330.0),
            [0.0, 2.3, 0.0],
            1500,
        );
        assert!(delta(&b, &a, "ramp_entry") >= 1);
        assert_eq!(delta(&b, &a, "ramp_made"), 1);
        assert!(delta(&b, &a, "deck_opto") >= 1);
        assert_eq!(layers, 0b111, "ball on layers 0, 1 and 2");

        // VUK: captured, ejected by the rules onto the upper deck.
        let (b, a, layers) = trial(&mut s, "standard_steel", mm(110.0, 800.0), [0.0; 3], 1000);
        assert_eq!(delta(&b, &a, "vuk_sw"), 1);
        assert!(delta(&b, &a, "deck_opto") >= 1);
        assert_eq!(layers & 0b10, 0b10, "ejected onto the deck (layer 1)");

        // Magnet detector: steel closes eddy and opto, the ceramic Powerball
        // only the opto.
        let (b, a, _) = trial(
            &mut s,
            "standard_steel",
            mm(257.0, 620.0),
            [0.0, -0.3, 0.0],
            300,
        );
        assert_eq!(
            (delta(&b, &a, "magnet_eddy"), delta(&b, &a, "magnet_opto")),
            (1, 1)
        );
        let (b, a, _) = trial(
            &mut s,
            "ceramic_powerball",
            mm(257.0, 620.0),
            [0.0, -0.3, 0.0],
            300,
        );
        assert_eq!(
            (delta(&b, &a, "magnet_eddy"), delta(&b, &a, "magnet_opto")),
            (0, 1)
        );

        // Newton ball.
        let (b, a, _) = trial(
            &mut s,
            "standard_steel",
            mm(330.0, 320.0),
            [0.0, 1.5, 0.0],
            300,
        );
        assert!(delta(&b, &a, "newton_sw") >= 1);

        // Drop target: dropped by the hit, reset by the rules a second later.
        let (b, a, _) = trial(
            &mut s,
            "standard_steel",
            mm(200.0, 400.0),
            [0.0, 1.5, 0.0],
            200,
        );
        assert_eq!(delta(&b, &a, "drop_1_sw"), 1);
        assert_eq!(s.switch_closed("drop_1_sw"), Some(true), "target down");
        s.advance(1300);
        assert_eq!(s.switch_closed("drop_1_sw"), Some(false), "target reset");

        // Plunger: the served ball is launched up the shooter lane.
        let before = hits(&s);
        press(&mut s, "launch", 20);
        s.advance(400);
        assert_eq!(delta(&before, &hits(&s), "shooter_lane"), 1);
        assert_eq!(s.switch_closed("plunger_tip"), Some(false));
        for h in served {
            s.remove_ball(h);
        }

        // Trough: a ball between the flippers drains.
        let drained = s.counters().balls_drained;
        let (b, a, _) = trial(
            &mut s,
            "standard_steel",
            mm(257.0, 200.0),
            [0.0, -0.5, 0.0],
            400,
        );
        assert!(delta(&b, &a, "trough_1") + delta(&b, &a, "trough_2") >= 1);
        assert_eq!(s.counters().balls_drained, drained + 1);

        // Tilt switch (cabinet; the tilt bob may already have touched).
        let tilts = s.switch_hits("tilt").unwrap();
        press(&mut s, "tilt", 5);
        assert_eq!(s.switch_hits("tilt"), Some(tilts + 1));

        // Every switch of the table was reached.
        let all = hits(&s);
        for name in SWITCHES {
            assert!(all[name] > 0, "switch {name} never closed");
        }
        healthy(&s);
    });
}

/// Moves every ball into the drain and runs until the rules served the
/// next ball (or ended the game).
fn drain_all(s: &mut Simulation) {
    let ball = s.persist(BALL);
    for h in s.ball_handles() {
        let mut st: BallState = s.ball(h).unwrap();
        st.position_m = mm(257.0, 120.0);
        st.velocity_m_s = [0.0, -0.5, 0.0];
        st.angular_velocity_rad_s = [0.0; 3];
        st.layer = 0;
        assert!(s.set_ball_state(h, &st));
    }
    for _ in 0..2000 {
        s.advance(1);
        if s.persist(BALL) != ball {
            return;
        }
    }
    panic!("ball {ball} did not end");
}

fn launch(s: &mut Simulation) {
    assert_eq!(
        s.switch_closed("plunger_tip"),
        Some(true),
        "ball on the plunger"
    );
    press(s, "launch", 20);
}

/// Result of the scripted game: (final score, expected score, state hash).
fn scripted_game() -> (i64, i64, u64) {
    let mut s = load();
    s.advance(100);
    assert_eq!(s.persist(STATE), 0, "attract");
    assert_eq!(s.score(0), Some(0));
    let start = hits(&s);
    start_game(&mut s);
    let mut expected = 0i64;

    // Ball 1 (steel): launched; it runs the orbit by itself. Then a shot at
    // a bumper and the ramp by state injection, then the drain.
    launch(&mut s);
    s.advance(1500);
    for (p, v) in [
        (mm(190.0, 640.0), [0.0, 1.0, 0.0]),
        (mm(420.0, 330.0), [0.0, 2.3, 0.0]),
    ] {
        let h = s.ball_handles()[0];
        let mut st = s.ball(h).unwrap();
        st.position_m = p;
        st.velocity_m_s = v;
        st.angular_velocity_rad_s = [0.0; 3];
        st.layer = 0;
        assert!(s.set_ball_state(h, &st));
        s.advance(1200);
    }
    drain_all(&mut s);
    assert_eq!(s.persist(BALL), 2);
    assert_eq!(s.persist(PB_MODES), 0);

    // Ball 2 (the Powerball): launched, then rolled across the magnet
    // detector: the opto closes without the eddy sensor, the rules start
    // the `powerball` mode (scores double from the next edge on).
    s.advance(1500);
    let pb = s.ball_handles()[0];
    assert_eq!(
        s.ball(pb).unwrap().profile,
        pin_core::ids::ProfileId(
            pin_core::profiles::BUILTIN_PROFILE_NAMES
                .iter()
                .position(|n| *n == "ceramic_powerball")
                .unwrap() as u16
        )
    );
    launch(&mut s);
    s.advance(1500);
    let mut st = s.ball(pb).unwrap();
    st.position_m = mm(257.0, 620.0);
    st.velocity_m_s = [0.0, -0.3, 0.0];
    st.angular_velocity_rad_s = [0.0; 3];
    st.layer = 0;
    assert!(s.set_ball_state(pb, &st));
    let mut prev = hits(&s);
    let mut detected = None;
    for _ in 0..500 {
        s.advance(1);
        if s.persist(PB_MODES) == 1 {
            detected = Some(prev.clone());
            break;
        }
        prev = hits(&s);
    }
    let at_detection = detected.expect("powerball mode started");
    // Doubled for the rest of the ball: a bumper shot.
    let h = s.ball_handles()[0];
    let mut st = s.ball(h).unwrap();
    st.position_m = mm(324.0, 640.0);
    st.velocity_m_s = [0.0, 1.0, 0.0];
    st.angular_velocity_rad_s = [0.0; 3];
    assert!(s.set_ball_state(h, &st));
    s.advance(300);
    let ball2_doubled_from = at_detection;
    drain_all(&mut s);
    let after_ball2 = hits(&s);
    expected += points(&start, &ball2_doubled_from);
    expected += 2 * points(&ball2_doubled_from, &after_ball2);
    assert!(
        points(&ball2_doubled_from, &after_ball2) > 0,
        "the Powerball scored in its mode"
    );
    assert_eq!(s.persist(BALL), 3);
    assert_eq!(s.persist(PB_MODES), 1);

    // Ball 3 (steel): VUK, Newton ball, drop target by injection.
    s.advance(1500);
    launch(&mut s);
    s.advance(1500);
    for (p, v, t) in [
        (mm(110.0, 800.0), [0.0; 3], 1200),
        (mm(330.0, 320.0), [0.0, 1.5, 0.0], 300),
        (mm(200.0, 400.0), [0.0, 1.5, 0.0], 300),
    ] {
        let h = s.ball_handles()[0];
        let mut st = s.ball(h).unwrap();
        st.position_m = p;
        st.velocity_m_s = v;
        st.angular_velocity_rad_s = [0.0; 3];
        st.layer = 0;
        assert!(s.set_ball_state(h, &st));
        s.advance(t);
    }
    drain_all(&mut s);
    let end = hits(&s);
    expected += points(&after_ball2, &end);

    // Game over.
    s.advance(10);
    assert_eq!(s.persist(STATE), 2, "game over");
    assert_eq!(s.persist(BALL), 0);
    assert_eq!(s.persist(GAMES), 1);
    assert_eq!(s.balls_in_play(), 0);
    let score = s.score(0).unwrap();
    assert_eq!(s.persist(HIGH), score);
    healthy(&s);
    // Attract returns after the game-over scene.
    s.advance(4100);
    assert_eq!(s.persist(STATE), 0, "back to attract");
    // Drains of the game: three, one per ball.
    assert_eq!(s.counters().balls_drained, 3);
    (score, expected, s.state_hash())
}

#[test]
fn scripted_three_ball_game() {
    let (score, expected, hash) = on_big_stack(scripted_game);
    eprintln!("game score {score}, state hash {hash:016x}");
    assert!(score > 0);
    assert_eq!(score, expected, "score = rules' points for the switch hits");
    // Reproducible (D9): the same injected game, the same result.
    let again = on_big_stack(scripted_game);
    assert_eq!(again, (score, expected, hash));
}

// --- Display scenes (from the rules' command queue) ---------------------------

#[test]
fn rules_show_attract_score_powerball_and_game_over_scenes() {
    use pin_assets::{Vfs, load_table};
    use pin_script::lua::{LuaConfig, LuaScriptBackend};
    let dir = greybox_bundle();
    let vfs = Vfs::open(dir).unwrap();
    let table = load_table(&vfs).unwrap();
    let names = pin_sim::load::name_tables(&table.ids);
    let scene = |n: &str| {
        names
            .scenes
            .iter()
            .find(|(s, _)| s == n)
            .map(|(_, id)| *id)
            .unwrap()
    };
    let sw = |n: &str| table.ids.switch(n).unwrap();
    let src = vfs.read("rules.lua").unwrap();
    let mut b = LuaScriptBackend::new(&src, &names, LuaConfig::engine()).unwrap();
    let mut view = HostView::default();
    let mut rng = Pcg32::seed_from_u64(1);
    let mut cmd = HostCommandQueue::default();
    let mut run = |f: &mut dyn FnMut(&mut LuaScriptBackend, &mut HostCtx), view: &HostView| {
        cmd.clear();
        let mut ctx = HostCtx {
            view,
            rng: &mut rng,
            cmd: &mut cmd,
        };
        f(&mut b, &mut ctx);
        (cmd.display_scenes.to_vec(), cmd.ball_spawns.len())
    };
    let (scenes, _) = run(&mut |b, c| b.on_init(c), &view);
    assert_eq!(scenes, [scene("scene_attract")]);
    let edge = |s: SwitchId| {
        move |b: &mut LuaScriptBackend, c: &mut HostCtx| {
            b.on_switch_edge(s, SwitchEdge::Closed, c);
            b.on_tick(c);
        }
    };
    let (scenes, spawns) = run(&mut edge(sw("start")), &view);
    assert_eq!(scenes, [scene("scene_score")]);
    assert_eq!(spawns, 1, "ball 1 served");
    // Ball 1 drains: ball 2 (the Powerball) is served.
    view.tick = 10;
    let (_, spawns) = run(&mut edge(sw("trough_1")), &view);
    assert_eq!(spawns, 1);
    // The Powerball crosses the detector: opto, no eddy.
    view.tick = 20;
    let (_, _) = run(&mut edge(sw("magnet_opto")), &view);
    let mut shown = Vec::new();
    for t in 0..40 {
        view.tick = 21 + t;
        let (s, _) = run(&mut |b, c| b.on_tick(c), &view);
        shown.extend(s);
    }
    assert_eq!(shown, [scene("scene_powerball_mania")]);
    // Balls 2 and 3 drain: game over.
    for t in [100, 200] {
        view.tick = t;
        let (scenes, _) = run(&mut edge(sw("trough_2")), &view);
        if t == 200 {
            assert_eq!(scenes, [scene("scene_game_over")]);
        }
    }
    // Back to attract after the game-over scene.
    let mut shown = Vec::new();
    for t in 0..4100 {
        view.tick = 300 + t;
        let (s, _) = run(&mut |b, c| b.on_tick(c), &view);
        shown.extend(s);
    }
    assert_eq!(shown, [scene("scene_attract")]);
    assert!(!b.is_faulted());
}

// --- Recorded input replay (D9) -----------------------------------------------

/// The inputs of tick `t` of a played game: start, launches, flippers.
fn played_inputs(t: u64, out: &mut Vec<InputEvent>) {
    out.clear();
    let ts = t * TICK_NS;
    let mut b = |i: CabinetInput, p: bool| out.push(InputEvent::button(ts, i, p));
    match t {
        10 => b(CabinetInput::Start, true),
        40 => b(CabinetInput::Start, false),
        _ => {}
    }
    // Launch every 2.5 s (only does something with a ball on the plunger).
    if t % 2500 == 1600 {
        b(CabinetInput::Launch, true);
    }
    if t % 2500 == 1630 {
        b(CabinetInput::Launch, false);
    }
    // Flip both lower flippers and the upper one in a fixed rhythm.
    match t % 700 {
        100 => b(CabinetInput::LeftFlipper, true),
        180 => b(CabinetInput::LeftFlipper, false),
        350 => b(CabinetInput::RightFlipper, true),
        430 => b(CabinetInput::RightFlipper, false),
        500 => b(CabinetInput::LeftUpper, true),
        560 => b(CabinetInput::LeftUpper, false),
        _ => {}
    }
}

const PLAYED_TICKS: u64 = 30_000;

#[test]
fn recorded_game_replays_to_the_same_state_hash_10_times() {
    let dir = greybox_bundle().to_path_buf();
    on_big_stack(move || {
        let mut rec = Recorder::new(Simulation::load(&dir, SimConfig::default()).unwrap());
        let mut buf = Vec::new();
        for t in 1..=PLAYED_TICKS {
            played_inputs(t, &mut buf);
            rec.step(&buf);
        }
        let (sim, recording) = rec.finish();
        healthy(&sim);
        assert!(sim.score(0).is_some_and(|s| s > 0), "the game scored");
        let hash = sim.state_hash();
        // Through the file format too.
        let recording = pin_sim::replay::Recording::from_bytes(&recording.to_bytes()).unwrap();
        for run in 0..10 {
            assert_eq!(
                replay(&dir, &recording).unwrap(),
                hash,
                "replay {run} diverged"
            );
        }
    });
}
