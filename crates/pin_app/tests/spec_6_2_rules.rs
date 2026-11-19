//! Subphase 6.2 spec tests (Independent Tester): the "Sky Deck" rules
//! (brief C5), observed through the public `pin_sim::Simulation` API:
//! persist keys by their position in `[persist] keys`, `score(0)`,
//! `snapshot().gi`, `display_scenes()`, the balls on the table, and
//! `script_faulted()` / `script_queue_overflows()`.
//!
//! Inputs go through the real cabinet path (`SimStep::step_1ms`). The
//! Newton ball and the drop bank are completed by physics contact (balls
//! rolled into them); forced switch pulses are used only as rule events
//! (scoring switches, a `vuk_sw` closure), never to fake a mechanism the
//! test then asserts on. Night Overdrive is swept over 8 simulation seeds.

#[path = "fixtures/spec_6_2/common.rs"]
mod common;

use common::{
    GLO_ID, STEEL_ID, assert_healthy, ball_into_vuk, big_stack, cabinet, drain_ball, game_seeded,
    glo_balls, hits, load, persist, press, pulse, run_until, start_game, steel_ball, strike_drop,
    strike_newton, wait_for_ball, wait_hit,
};
use pin_core::input::CabinetInput;
use pin_sim::{BallHandle, Simulation};

/// Sky Bridge duration, ticks (8.0 s).
const BRIDGE_TICKS: u64 = 8_000;
/// Bound for the Glo-Ball to be on the upper deck after the mode starts
/// (from the tick the mode counter first rises to the tick a `uv_globall`
/// ball first has `layer == 1`), ticks; exactly this many is accepted, one
/// more is not. The table's rules lift it within about 2 s of the start
/// (the VUK holds a ball only 0.5 s until the Glo-Ball is up); 3 s leaves
/// a 1 s margin.
const GLO_BOUND_TICKS: u64 = 3_000;
/// Rule-event latency, ticks: an edge collected in tick N reaches the
/// script in tick N+1, whose commands apply in that tick (DECISIONS 2.4);
/// a couple of ticks of slack for scripts that act on the next callback.
const LATENCY: u64 = 5;

fn scene_shown(sim: &Simulation, name: &str) -> bool {
    let id = sim.ids().scene(name).expect("declared scene");
    sim.display_scenes().contains(&id)
}

fn is_uv(gi: [f32; 4]) -> bool {
    gi[2] > gi[0] && gi[2] > gi[1] && gi[3] > 0.3
}

fn is_white(gi: [f32; 4]) -> bool {
    gi.iter().all(|&c| (c - 1.0).abs() <= 1e-6)
}

/// Completes the Newton ball and the drop bank by contact and asserts the
/// arming sequence: `overdrive_ready` stays 0 until both are done.
fn arm(sim: &mut Simulation, newton_first: bool) {
    let ready = |s: &Simulation| persist(s, "overdrive_ready");
    if newton_first {
        assert!(strike_newton(sim, 1.5).is_some(), "Newton strike");
        sim.advance(LATENCY);
        assert_eq!(ready(sim), 0, "not armed by the Newton ball alone");
    }
    for (i, d) in ["drop_1", "drop_2", "drop_3"].iter().enumerate() {
        assert!(strike_drop(sim, d, 1.2).is_some(), "{d} struck");
        sim.advance(LATENCY);
        if i < 2 || !newton_first {
            assert_eq!(
                ready(sim),
                0,
                "not armed after {d} (newton first: {newton_first})"
            );
        }
    }
    if !newton_first {
        assert!(strike_newton(sim, 1.5).is_some(), "Newton strike");
    }
    let ok = run_until(sim, 50, |s| ready(s) == 1);
    assert!(
        ok,
        "overdrive_ready = 1 once the Newton ball and all drops are done"
    );
}

// ---------------------------------------------------------------------------
// Game flow
// ---------------------------------------------------------------------------

/// `start` begins a 3-ball game; scoring switches raise `score(0)`; each
/// trough drain moves to the next ball; the game ends after the third
/// (`game_state` 2, no ball on the table, none served for 10 s); the table
/// returns to attract (`scene_attract` shown within 30 s) and a new start
/// begins a new game.
#[test]
fn c5_three_ball_game() {
    big_stack(|| {
        let mut sim = load();
        sim.advance(2_000);
        assert_eq!(sim.balls_in_play(), 0, "no ball in attract");
        assert_ne!(persist(&sim, "game_state"), 1, "not in a game before start");
        start_game(&mut sim);
        for ball in 1..=3i64 {
            if ball > 1 {
                let ok = run_until(&mut sim, 10_000, |s| {
                    persist(s, "ball") == ball && s.balls_in_play() > 0
                });
                assert!(
                    ok,
                    "ball {ball} served (persist ball {})",
                    persist(&sim, "ball")
                );
            }
            assert_eq!(persist(&sim, "game_state"), 1);
            for sw in ["sling_left_sw", "sling_right_sw", "drop_1_sw", "newton_sw"] {
                let before = sim.score(0).unwrap_or(0);
                pulse(&mut sim, sw, 20);
                sim.advance(20);
                let after = sim.score(0).unwrap_or(0);
                assert!(
                    after > before,
                    "ball {ball}: {sw} scores ({before} -> {after})"
                );
            }
            let handles: Vec<BallHandle> = sim.ball_handles().iter().copied().collect();
            assert_eq!(handles.len(), 1, "one ball in play");
            drain_ball(&mut sim, handles[0]);
            assert_healthy(&sim, &format!("ball {ball}"));
        }
        let over = run_until(&mut sim, 1_000, |s| persist(s, "game_state") == 2);
        assert!(over, "game_state 2 after the third drain");
        let mut attract = false;
        for i in 0..30_000u64 {
            sim.advance(1);
            if i < 10_000 {
                assert_eq!(sim.balls_in_play(), 0, "no ball after the third drain");
            }
            attract |= scene_shown(&sim, "scene_attract");
            if attract && i >= 10_000 {
                break;
            }
        }
        assert!(attract, "back to attract");
        assert!(sim.score(0).unwrap_or(0) > 0, "final score");
        start_game(&mut sim);
        assert_healthy(&sim, "second game");
    });
}

// ---------------------------------------------------------------------------
// Sky Bridge
// ---------------------------------------------------------------------------

/// Watches `bridge_open` tick by tick: every tick (an input tick or a
/// plain one) is followed by a sample.
#[derive(Default)]
struct Bridge {
    /// First tick after which `bridge_open` read 1.
    opened: Option<u64>,
    /// First tick after `opened` after which it read 0.
    closed: Option<u64>,
}

impl Bridge {
    fn sample(&mut self, sim: &Simulation) {
        let v = persist(sim, "bridge_open");
        assert!(v == 0 || v == 1, "bridge_open is 0 or 1, got {v}");
        if v == 1 && self.opened.is_none() {
            self.opened = Some(sim.tick());
        }
        if v == 0 && self.opened.is_some() && self.closed.is_none() {
            self.closed = Some(sim.tick());
        }
    }

    fn input(&mut self, sim: &mut Simulation, i: CabinetInput, pressed: bool) {
        cabinet(sim, i, pressed);
        self.sample(sim);
    }

    fn advance(&mut self, sim: &mut Simulation, n: u64) {
        for _ in 0..n {
            sim.advance(1);
            self.sample(sim);
        }
    }

    /// Ticks it read 1.
    fn open_ticks(&self) -> u64 {
        self.closed.expect("closed") - self.opened.expect("opened")
    }
}

/// The action button opens the Sky Bridge for exactly 8000 ticks in a
/// running game (the number of consecutive ticks after which
/// `bridge_open` reads 1); a second press 3 s into the open time does not
/// extend it; after it closes, a press opens it again for 8000 ticks.
#[test]
fn c5_sky_bridge_8_seconds() {
    big_stack(|| {
        let mut sim = game_seeded(11);
        sim.advance(500);
        assert_eq!(persist(&sim, "bridge_open"), 0, "closed at the start");
        let mut b = Bridge::default();
        b.input(&mut sim, CabinetInput::Action, true);
        let press = sim.tick();
        b.advance(&mut sim, 49);
        b.input(&mut sim, CabinetInput::Action, false);
        b.advance(&mut sim, 3_000 - 51);
        b.input(&mut sim, CabinetInput::Action, true);
        b.advance(&mut sim, 49);
        b.input(&mut sim, CabinetInput::Action, false);
        b.advance(&mut sim, 7_000);
        println!(
            "bridge: press at tick {press}, open after tick {:?}, closed after tick {:?}",
            b.opened, b.closed
        );
        let opened = b.opened.expect("the bridge opened");
        assert!(opened - press <= LATENCY, "opens on the press");
        assert_eq!(
            b.open_ticks(),
            BRIDGE_TICKS,
            "open for exactly 8.0 s, not extended"
        );
        assert_eq!(persist(&sim, "bridge_open"), 0, "stays closed");
        // Again.
        let mut b = Bridge::default();
        b.input(&mut sim, CabinetInput::Action, true);
        b.advance(&mut sim, 49);
        b.input(&mut sim, CabinetInput::Action, false);
        b.advance(&mut sim, 9_000);
        assert_eq!(b.open_ticks(), BRIDGE_TICKS, "reopens for 8.0 s");
        assert_eq!(
            persist(&sim, "mode:night_overdrive"),
            0,
            "no mode from the bridge"
        );
        assert_healthy(&sim, "bridge");
    });
}

/// In attract the action button does nothing to the bridge.
#[test]
fn c5_sky_bridge_not_in_attract() {
    big_stack(|| {
        let mut sim = load();
        sim.advance(1_000);
        press(&mut sim, CabinetInput::Action, 50);
        for _ in 0..9_000 {
            sim.advance(1);
            assert_eq!(persist(&sim, "bridge_open"), 0, "bridge in attract");
        }
        assert_eq!(persist(&sim, "mode:night_overdrive"), 0);
        assert_healthy(&sim, "attract action");
    });
}

// ---------------------------------------------------------------------------
// Night Overdrive: arming
// ---------------------------------------------------------------------------

/// Arming needs both the Newton ball and the whole drop bank (either
/// order); before arming, an action press only opens the bridge and a
/// `vuk_sw` closure starts nothing.
#[test]
fn c5_overdrive_arming() {
    big_stack(|| {
        for newton_first in [true, false] {
            let mut sim = game_seeded(21 + u64::from(newton_first));
            sim.advance(300);
            let mode0 = persist(&sim, "mode:night_overdrive");
            assert_eq!(
                persist(&sim, "overdrive_ready"),
                0,
                "not armed at ball start"
            );
            // Not armed: action opens the bridge, no mode.
            press(&mut sim, CabinetInput::Action, 30);
            sim.advance(LATENCY);
            assert_eq!(
                persist(&sim, "bridge_open"),
                1,
                "unarmed action opens the bridge"
            );
            assert_eq!(persist(&sim, "mode:night_overdrive"), mode0);
            // Not armed: a VUK closure starts nothing.
            pulse(&mut sim, "vuk_sw", 20);
            sim.advance(100);
            assert_eq!(persist(&sim, "mode:night_overdrive"), mode0);
            sim.advance(BRIDGE_TICKS);
            arm(&mut sim, newton_first);
            assert_eq!(
                persist(&sim, "mode:night_overdrive"),
                mode0,
                "arming alone starts nothing"
            );
            assert_healthy(&sim, "arming");
        }
    });
}

/// Progress does not carry over: the Newton ball struck in ball 1 does not
/// count in ball 2.
#[test]
fn c5_overdrive_arming_is_per_ball() {
    big_stack(|| {
        let mut sim = game_seeded(23);
        sim.advance(300);
        assert!(strike_newton(&mut sim, 1.5).is_some(), "Newton strike");
        let h = steel_ball(&sim);
        drain_ball(&mut sim, h);
        let ok = run_until(&mut sim, 10_000, |s| {
            persist(s, "ball") == 2 && s.balls_in_play() > 0
        });
        assert!(ok, "ball 2 served");
        sim.advance(300);
        // Drops whose targets are up again (the bank may still be resetting).
        let ok = run_until(&mut sim, 5_000, |s| {
            ["drop_1", "drop_2", "drop_3"]
                .iter()
                .all(|d| s.mech_set().drop_target_down(common::mech_index(d)) == Some(false))
        });
        assert!(ok, "the drop bank is up in ball 2");
        for d in ["drop_1", "drop_2", "drop_3"] {
            assert!(strike_drop(&mut sim, d, 1.2).is_some(), "{d}");
        }
        sim.advance(100);
        assert_eq!(
            persist(&sim, "overdrive_ready"),
            0,
            "ball 1's Newton strike does not arm ball 2"
        );
        assert!(strike_newton(&mut sim, 1.5).is_some(), "Newton strike");
        let ok = run_until(&mut sim, 50, |s| persist(s, "overdrive_ready") == 1);
        assert!(ok, "armed in ball 2 once both are done in ball 2");
        assert_healthy(&sim, "per ball");
    });
}

// ---------------------------------------------------------------------------
// Night Overdrive: start, Glo-Ball, end
// ---------------------------------------------------------------------------

/// What one Night Overdrive start looked like.
struct Start {
    tick: u64,
    glo: BallHandle,
}

impl Start {
    /// Whether the Glo-Ball is still on the table.
    fn glo_present(&self, sim: &Simulation) -> bool {
        sim.ball(self.glo).is_some()
    }
}

/// After the trigger at tick `trigger`, watches the start: the mode count
/// rises by exactly 1 no later than `trigger + LATENCY` (checked on the
/// tick it first rises), `scene_night_overdrive` is shown within `LATENCY`
/// ticks of it, the GI is UV from the tick after the start, and a
/// `uv_globall` ball reaches layer 1 no later than `GLO_BOUND_TICKS` after
/// the start, and by then there is a tick with it on layer 1 while the
/// steel ball `below` is on layer 0 (the ball the rules lift through the
/// VUK, on that path, has to be back on the main deck).
fn watch_start(
    sim: &mut Simulation,
    trigger: u64,
    mode0: i64,
    below: BallHandle,
    what: &str,
) -> Start {
    let mut start: Option<u64> = None;
    let mut scene = Vec::new();
    let mut glo = None;
    let mut together = None;
    // The first pass observes the trigger tick itself (already run).
    let mut first = true;
    loop {
        if !first {
            sim.advance(1);
        }
        first = false;
        let now = sim.tick();
        let m = persist(sim, "mode:night_overdrive");
        assert!(m <= mode0 + 1, "{what}: one start, not {}", m - mode0);
        if m == mode0 + 1 && start.is_none() {
            assert!(
                start_in_time(trigger, now),
                "{what}: started at {now}, more than {LATENCY} ticks after the trigger {trigger}"
            );
            start = Some(now);
        }
        if scene_shown(sim, "scene_night_overdrive") {
            scene.push(now);
        }
        if let Some(s) = start {
            if now > s {
                let gi = sim.snapshot().gi;
                assert!(is_uv(gi), "{what}: UV GI while active, got {gi:?}");
            }
            if glo.is_none() {
                for h in glo_balls(sim) {
                    if sim.ball(h).is_some_and(|b| b.layer == 1) {
                        glo = Some((h, now));
                    }
                }
            }
            // The Glo-Ball on the deck alongside the steel ball `below` on
            // layer 0 (the VUK may lift the steel ball first; it must be
            // back below while the Glo-Ball is up).
            if let Some((g, _)) = glo
                && together.is_none()
                && sim.ball(g).is_some_and(|b| b.layer == 1)
            {
                let st = sim.ball(below).expect("ball below still on the table");
                assert_eq!(st.profile.0, STEEL_ID);
                if st.layer == 0 {
                    together = Some(now);
                }
            }
            if glo.is_some() && together.is_some() && !scene.is_empty() {
                break;
            }
            if now >= s + GLO_BOUND_TICKS {
                break;
            }
        } else if now > trigger + LATENCY {
            panic!("{what}: no start within {LATENCY} ticks of the trigger");
        }
    }
    let start = start.unwrap_or_else(|| panic!("{what}: never started"));
    assert!(
        scene.iter().any(|&t| t.abs_diff(start) <= LATENCY),
        "{what}: scene_night_overdrive at start {start} (shown at {scene:?})"
    );
    let (glo, arrival) = glo.unwrap_or_else(|| {
        panic!("{what}: no Glo-Ball on layer 1 within {GLO_BOUND_TICKS} ticks of the start")
    });
    assert!(
        glo_in_time(start, arrival),
        "{what}: Glo-Ball on layer 1 at {arrival}, more than {GLO_BOUND_TICKS} ticks after the \
         start {start}"
    );
    let together = together.unwrap_or_else(|| {
        panic!(
            "{what}: the steel ball was never below (layer 0) while the Glo-Ball was on the \
             deck within {GLO_BOUND_TICKS} ticks of the start"
        )
    });
    assert!(glo_in_time(start, together));
    let b = sim.ball(glo).expect("glo");
    assert_eq!(b.profile.0, GLO_ID);
    println!(
        "{what}: started at {start} ({} ticks after the trigger), Glo-Ball on layer 1 after {} \
         ticks",
        start - trigger,
        arrival - start
    );
    Start { tick: start, glo }
}

/// The mode counter rose in time: no later than `trigger + LATENCY`.
fn start_in_time(trigger: u64, rose_at: u64) -> bool {
    rose_at <= trigger + LATENCY
}

/// The Glo-Ball arrived in time: no later than `start + GLO_BOUND_TICKS`.
fn glo_in_time(start: u64, arrival: u64) -> bool {
    arrival <= start + GLO_BOUND_TICKS
}

/// The bounds `watch_start` applies, at their edges.
#[test]
fn c5_start_and_glo_bounds() {
    assert!(start_in_time(100, 100 + LATENCY));
    assert!(!start_in_time(100, 100 + LATENCY + 1));
    assert!(glo_in_time(1_000, 1_000 + 3_000));
    assert!(!glo_in_time(1_000, 1_000 + 3_001));
}

/// Night Overdrive started by the action button, swept over 8 simulation
/// seeds (each also shifting the start time by 97 ticks), with both drain
/// orders (odd seeds: Glo-Ball first; even seeds:
/// steel ball first):
///
/// * the mode ends at the first drain (GI back to white within LATENCY);
/// * the ball ends only after both balls drained (`ball` unchanged while
///   one is on the table; 2 once both are gone);
/// * no second start in the same ball (action, VUK closure);
/// * the game still has exactly 3 balls.
#[test]
fn c5_night_overdrive_by_action_seed_sweep() {
    big_stack(|| {
        let mut orders = [0u32; 2];
        for seed in 1..=8u64 {
            let what = format!("seed {seed}");
            let mut sim = game_seeded(seed);
            // The seed also staggers the hand-off's timing, so the Glo-Ball
            // meets the table in a different state each time.
            sim.advance(300 + 97 * seed);
            let steel = steel_ball(&sim);
            arm(&mut sim, true);
            let mode0 = persist(&sim, "mode:night_overdrive");
            cabinet(&mut sim, CabinetInput::Action, true);
            let trigger = sim.tick();
            let st = watch_start(&mut sim, trigger, mode0, steel, &what);
            // The release runs one tick: UV on it too (no ball has drained).
            cabinet(&mut sim, CabinetInput::Action, false);
            if st.glo_present(&sim) && sim.ball(steel).is_some() {
                let gi = sim.snapshot().gi;
                assert!(is_uv(gi), "{what}: UV after the release, got {gi:?}");
            }
            assert!(st.tick >= trigger);
            let glo_first = seed % 2 == 1;
            // UV stays on on every tick until the first drain (a natural
            // Glo-Ball drain counts as the Glo-Ball first).
            let mut glo_gone = false;
            for _ in 0..200 {
                sim.advance(1);
                if sim.ball(st.glo).is_none() {
                    glo_gone = true;
                    break;
                }
                if sim.ball(steel).is_none() {
                    break;
                }
                let gi = sim.snapshot().gi;
                assert!(is_uv(gi), "{what}: UV while active, got {gi:?}");
            }
            let ball0 = persist(&sim, "ball");
            assert_eq!(ball0, 1, "{what}: still ball 1");
            let glo_first = glo_first || glo_gone;
            if glo_first {
                orders[0] += 1;
                if !glo_gone {
                    drain_ball(&mut sim, st.glo);
                }
                let ok = run_until(&mut sim, LATENCY, |s| is_white(s.snapshot().gi));
                assert!(
                    ok,
                    "{what}: GI white after the Glo-Ball drained: {:?}",
                    sim.snapshot().gi
                );
                sim.advance(2_000);
                assert_eq!(
                    persist(&sim, "ball"),
                    1,
                    "{what}: steel still in play, same ball"
                );
                assert!(sim.ball(steel).is_some(), "{what}: steel on the table");
                assert!(is_white(sim.snapshot().gi), "{what}: mode over");
                // No second start in this ball.
                let m = persist(&sim, "mode:night_overdrive");
                press(&mut sim, CabinetInput::Action, 30);
                pulse(&mut sim, "vuk_sw", 20);
                sim.advance(500);
                assert_eq!(
                    persist(&sim, "mode:night_overdrive"),
                    m,
                    "{what}: one start per ball"
                );
                assert!(glo_balls(&sim).is_empty(), "{what}: no new Glo-Ball");
                drain_ball(&mut sim, steel);
            } else {
                orders[1] += 1;
                if sim.ball(steel).is_some() {
                    drain_ball(&mut sim, steel);
                }
                let ok = run_until(&mut sim, LATENCY, |s| is_white(s.snapshot().gi));
                assert!(
                    ok,
                    "{what}: GI white after the steel ball drained: {:?}",
                    sim.snapshot().gi
                );
                // The ball goes on while the Glo-Ball is on the table.
                let mut left = 0u64;
                while sim.ball(st.glo).is_some() && left < 20_000 {
                    sim.advance(1);
                    left += 1;
                    if sim.ball(st.glo).is_some() {
                        assert_eq!(persist(&sim, "ball"), 1, "{what}: Glo-Ball still in play");
                        assert_eq!(persist(&sim, "game_state"), 1);
                    }
                }
                if sim.ball(st.glo).is_some() {
                    drain_ball(&mut sim, st.glo);
                }
            }
            let ok = run_until(&mut sim, 10_000, |s| {
                persist(s, "ball") == 2 && s.balls_in_play() > 0
            });
            assert!(
                ok,
                "{what}: ball 2 after both drains (ball {})",
                persist(&sim, "ball")
            );
            assert!(is_white(sim.snapshot().gi), "{what}: no UV in ball 2");
            // The Glo-Ball did not count: two more balls end the game.
            for ball in 2..=3i64 {
                let ok = run_until(&mut sim, 10_000, |s| {
                    persist(s, "ball") == ball && s.balls_in_play() > 0
                });
                assert!(ok, "{what}: ball {ball} served");
                let hs: Vec<BallHandle> = sim.ball_handles().iter().copied().collect();
                for h in hs {
                    drain_ball(&mut sim, h);
                }
            }
            let over = run_until(&mut sim, 2_000, |s| persist(s, "game_state") == 2);
            assert!(over, "{what}: game over after the third ball");
            sim.advance(3_000);
            assert_eq!(sim.balls_in_play(), 0, "{what}: no fourth ball");
            assert_healthy(&sim, &what);
        }
        println!(
            "drain orders: Glo-Ball first {}, steel first {}",
            orders[0], orders[1]
        );
        assert!(
            orders[0] >= 1 && orders[1] >= 1,
            "both drain orders tested: {orders:?}"
        );
    });
}

/// Night Overdrive started by the next `vuk_sw` closure (a ball rolled
/// into the VUK) once armed, 3 seeds; the mode starts within LATENCY ticks
/// of the physics capture.
#[test]
fn c5_night_overdrive_by_vuk() {
    big_stack(|| {
        for seed in [31u64, 32, 33] {
            let what = format!("vuk seed {seed}");
            let mut sim = game_seeded(seed);
            sim.advance(300);
            let _served = steel_ball(&sim);
            arm(&mut sim, false);
            let mode0 = persist(&sim, "mode:night_overdrive");
            let v = ball_into_vuk(&mut sim);
            let before = hits(&sim, "vuk_sw");
            let hit = wait_hit(&mut sim, "vuk_sw", 2_000).expect("vuk_sw by contact");
            assert!(hits(&sim, "vuk_sw") > before);
            // `v` is the ball the rules lift: the "below" check covers it.
            let st = watch_start(&mut sim, hit, mode0, v, &what);
            assert!(st.tick >= hit);
            let _ = sim.remove_ball(v);
            assert_healthy(&sim, &what);
        }
    });
}

/// Night Overdrive in ball 2 after it ran in ball 1: one start per ball,
/// not one per game (the count rises by exactly 1 again).
#[test]
fn c5_night_overdrive_again_next_ball() {
    big_stack(|| {
        let mut sim = game_seeded(41);
        sim.advance(300);
        for ball in 1..=2i64 {
            let what = format!("ball {ball}");
            let steel = steel_ball(&sim);
            let ok = run_until(&mut sim, 5_000, |s| {
                ["drop_1", "drop_2", "drop_3"]
                    .iter()
                    .all(|d| s.mech_set().drop_target_down(common::mech_index(d)) == Some(false))
            });
            assert!(ok, "{what}: drop bank up");
            arm(&mut sim, true);
            let mode0 = persist(&sim, "mode:night_overdrive");
            cabinet(&mut sim, CabinetInput::Action, true);
            let trigger = sim.tick();
            let st = watch_start(&mut sim, trigger, mode0, steel, &what);
            cabinet(&mut sim, CabinetInput::Action, false);
            assert_eq!(persist(&sim, "mode:night_overdrive"), mode0 + 1);
            if sim.ball(st.glo).is_some() {
                drain_ball(&mut sim, st.glo);
            }
            let hs: Vec<BallHandle> = sim.ball_handles().iter().copied().collect();
            for h in hs {
                drain_ball(&mut sim, h);
            }
            if ball == 1 {
                let ok = run_until(&mut sim, 10_000, |s| {
                    persist(s, "ball") == 2 && s.balls_in_play() > 0
                });
                assert!(ok, "ball 2 served");
                assert!(wait_for_ball(&mut sim, 10));
                sim.advance(300);
            }
            assert_healthy(&sim, &what);
        }
    });
}
