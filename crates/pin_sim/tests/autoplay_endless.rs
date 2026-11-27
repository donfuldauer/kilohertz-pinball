//! Work item A1-endless-detector: the EndlessBall detector, the
//! displacement test of the StuckBall detector, censored ball times, and
//! the before/after proof on the pre-fix grey-box (commit 452eedf).

mod sim_support;
#[path = "fixtures/autoplay/support.rs"]
mod support;

use pin_core::snapshot::BallState;
use pin_sim::autoplay::{
    BotConfig, FindingKind, GameEnd, GameMonitor, ReactionDraw, SkillLevel, SoakConfig, TableInfo,
    play_soak_game, run_soak,
};
use pin_sim::{SimConfig, Simulation};
use sim_support::{ScratchDir, fixture_table_toml, fixture_variant};
use support::{cfg, fixture, on_sim_thread};

const R: f32 = 0.0135;

/// A VUK on the open playfield of the replay fixture whose exit point lies
/// 60 mm uphill of its scoop: the ejected ball rolls straight back into it.
const VUK_EXTRA: &str = r#"
[[coils]]
id = "vuk_coil"
number = 4
kind = "vuk"

[[switches]]
id = "vuk_sw"
number = 20
kind = "leaf"
layer = 0

[[mechs]]
id = "vuk"
kind = "vuk"
coil = "vuk_coil"
switch = "vuk_sw"
position = [400.0, 400.0, 0.0]
angle_deg = 90.0
layer = 0
exit_layer = 0
exit_offset = [0.0, 60.0, 13.5]
eject_speed_m_s = 0.2
kick_speed_m_s = 0.1
"#;

/// With `feed` the scoop kicks the ball out 300 ms after it lands;
/// without, it holds the ball (coil off). No ball is launched.
fn vuk_rules(feed: bool) -> String {
    format!(
        r#"
local feed = {feed}
local due = nil
local now = 0

function on_init()
  pinball.set_score(1, 0)
end

function on_switch(id, closed)
  if id == pinball.sw.vuk_sw and closed and feed then
    due = now + 300
  end
end

function on_tick()
  now = now + 1
  if due ~= nil and now >= due then
    due = nil
    pinball.pulse_coil(pinball.coil.vuk_coil, 20)
  end
end
"#
    )
}

fn vuk_bundle(tag: &str, feed: bool) -> ScratchDir {
    let table = fixture_table_toml() + VUK_EXTRA;
    let rules = vuk_rules(feed);
    fixture_variant(
        tag,
        &[
            ("table.toml", Some(table.as_bytes())),
            ("rules.lua", Some(rules.as_bytes())),
        ],
    )
}

/// Test a: a self-feeding VUK keeps one ball on the table forever. With a
/// 20 s endless limit it is named after the scoop switch within a bounded
/// run, with the switches of the last 60 s in the detail; it is never
/// StuckBall (it leaves the diameter circle every cycle, and the held part
/// of the cycle is exempt).
#[test]
fn a_self_feeding_vuk_is_an_endless_ball() {
    let dir = vuk_bundle("endless_vuk", true);
    let path = dir.to_path_buf();
    let (findings, laps, censored) = on_sim_thread(move || {
        let mut sim = Simulation::load(&path, SimConfig::default()).unwrap();
        let info = TableInfo::load(&path).unwrap();
        sim.spawn_ball("standard_steel", [0.400, 0.400, R], [0.0; 3])
            .unwrap();
        let mut mon = GameMonitor::new(&info, &sim, 7, 10_000).with_endless_ticks(20_000);
        mon.observe(&sim);
        for _ in 0..25_000 {
            sim.advance(1);
            mon.observe(&sim);
        }
        assert_eq!(sim.balls_in_play(), 1, "the loop keeps the ball in play");
        // A second call (as at the cap) does not repeat the finding.
        mon.end_at_cap(&sim);
        let laps = sim.switch_hits("vuk_sw").unwrap();
        let out = mon.finish(&sim);
        (out.findings, laps, out.censored_ball_times_ms)
    });
    drop(dir);
    assert!(laps > 20, "the VUK refed itself only {laps} times");
    assert_eq!(findings.len(), 1, "{findings:?}");
    let f = &findings[0];
    assert_eq!(f.kind, FindingKind::EndlessBall);
    assert_eq!(f.subject, "vuk_sw");
    assert_eq!(f.game, 7);
    assert_eq!(f.tick, 20_001, "{f:?}");
    assert!(
        f.detail.contains("longer than the 20000 ms limit"),
        "{}",
        f.detail
    );
    assert!(
        f.detail.contains("switches hit in the last 60 s: vuk_sw x"),
        "{}",
        f.detail
    );
    // The ball was still in play at the end: one censored ball time.
    assert_eq!(censored, [25_000]);
}

/// Test a, soak path: a game the soak stops at `max_game_ticks` with a
/// ball in play gets an EndlessBall finding, named after the most-hit
/// switch, and the ball is counted as a censored ball time.
#[test]
fn a_game_cut_off_by_the_cap_names_its_ball() {
    let r = on_sim_thread(|| run_soak(&fixture("clean"), &cfg(1, 1, 3_000)).unwrap());
    assert_eq!(r.game_list[0].end, GameEnd::MaxTicks);
    let e: Vec<_> = r.findings_of(FindingKind::EndlessBall).collect();
    assert_eq!(e.len(), 1, "{:?}", r.findings);
    assert_eq!(e[0].tick, 3_000);
    assert!(
        e[0].detail.contains("reached max_game_ticks"),
        "{}",
        e[0].detail
    );
    assert!(!r.passed());
    assert_eq!(r.ball_time_censored, 1);
    assert_eq!(r.ball_time_ms.count, u64::from(r.game_list[0].balls) + 1);
    let v = r.gate_a1(SkillLevel::Standard);
    assert_eq!(
        v.check("no_endless_balls").unwrap().value,
        "1 findings",
        "{}",
        v.lines()
    );
    // The censored count is in the serialised and the text report.
    assert!(
        r.to_json()
            .contains("\"ball_time_censored\": 1, \"game_score\""),
        "{}",
        r.to_json()
    );
    assert!(
        r.to_markdown()
            .contains("| balls censored (in play at game end) | 1 |")
    );
}

/// The per-ball age limit fires before the cap, at the configured age.
#[test]
fn the_endless_age_limit_is_configurable() {
    let mut c = cfg(1, 1, 3_000);
    c.endless_ball_ticks = 1_000;
    let r = on_sim_thread(move || run_soak(&fixture("clean"), &c).unwrap());
    let e: Vec<_> = r.findings_of(FindingKind::EndlessBall).collect();
    assert!(!e.is_empty(), "{:?}", r.findings);
    assert!(e[0].tick < 3_000, "{:?}", e[0]);
    assert!(
        e[0].detail.contains("longer than the 1000 ms limit"),
        "{}",
        e[0].detail
    );
    assert_eq!(SoakConfig::default().endless_ball_ticks, 300_000);
}

/// Test d: the Subphase 2.9 clean fixture, soaked with the default
/// configuration (600 000-tick cap, 300 s endless limit), ends every game
/// by game over and passes with zero findings: no EndlessBall and no
/// displacement StuckBall. (The autoplay `clean` fixture's zero-findings
/// test is `autoplay_fixtures::clean_fixture_passes_with_full_coverage`.)
#[test]
fn the_clean_fixture_has_no_endless_or_stuck_ball() {
    let c = SoakConfig {
        games: 3,
        seed: 2024,
        threads: 3,
        check_tick_budget: false,
        ..SoakConfig::default()
    };
    let bundle =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/spec_2_9/clean");
    let r = on_sim_thread(move || run_soak(&bundle, &c).unwrap());
    assert!(r.findings.is_empty(), "{:?}", r.findings);
    assert!(
        r.game_list.iter().all(|g| g.end == GameEnd::GameOver),
        "{:?}",
        r.game_list
    );
    assert_eq!(r.ball_time_censored, 0);
    assert!(r.passed());
    assert!(
        r.gate_a1(SkillLevel::Standard)
            .check("no_endless_balls")
            .unwrap()
            .status
            == pin_sim::autoplay::CheckStatus::Pass
    );
}

/// Drives one ball through `path(tick)` (position and velocity set before
/// every tick) for `ticks` ticks and returns the findings.
fn driven(
    name: &str,
    ticks: u64,
    stuck_ticks: u64,
    path: impl Fn(u64) -> ([f32; 3], [f32; 3]),
) -> Vec<pin_sim::autoplay::Finding> {
    let b = fixture(name);
    let mut sim = Simulation::load(&b, SimConfig::default()).unwrap();
    let info = TableInfo::load(&b).unwrap();
    let (p0, _) = path(0);
    let h = sim.spawn_ball("standard_steel", p0, [0.0; 3]).unwrap();
    let mut mon = GameMonitor::new(&info, &sim, 0, stuck_ticks);
    for t in 0..ticks {
        let (p, v) = path(t);
        let s = BallState {
            position_m: p,
            velocity_m_s: v,
            ..BallState::default()
        };
        assert!(sim.set_ball_state(h, &s));
        sim.advance(1);
        mon.observe(&sim);
    }
    mon.findings().to_vec()
}

/// Test b: a ball rattling against a firing sling, always faster than
/// 1 cm/s but never more than a few millimetres from one spot, is
/// StuckBall through the displacement test.
#[test]
fn a_rattling_ball_faster_than_1_cm_s_is_stuck() {
    let f = driven("clean", 120, 40, |t| {
        // 8 mm side to side at 0.4 m/s, reversing every 20 ms.
        let phase = (t % 40) as f32;
        let x = if phase < 20.0 {
            0.370 + 0.0004 * phase
        } else {
            0.378 - 0.0004 * (phase - 20.0)
        };
        let vx = if phase < 20.0 { 0.4 } else { -0.4 };
        ([x, 0.790, R], [vx, 0.0, 0.0])
    });
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0].kind, FindingKind::StuckBall);
    assert!(f[0].subject.starts_with("posts[1] @ ("), "{}", f[0].subject);
    assert!(
        f[0].detail.contains("within one ball diameter (27 mm)"),
        "{}",
        f[0].detail
    );
    // Shorter than the stuck time: nothing.
    let f = driven("clean", 40, 40, |_| ([0.374, 0.790, R], [0.4, 0.0, 0.0]));
    assert!(f.is_empty(), "{f:?}");
}

/// Test b: a ball in fast play that keeps leaving the diameter circle is
/// not stuck, however long it plays.
#[test]
fn a_ball_in_fast_play_is_not_stuck() {
    let f = driven("clean", 400, 40, |t| {
        // A 100 mm circle at 1 m/s (one lap per ~628 ms).
        let a = t as f32 * 1.0e-3 / 0.1;
        let (s, c) = libm::sincosf(a);
        ([0.257 + 0.1 * c, 0.450 + 0.1 * s, R], [-s, c, 0.0])
    });
    assert!(f.is_empty(), "{f:?}");
}

/// Test b: re-anchoring. A ball that drifts slowly (faster than 1 cm/s)
/// leaves each circle before the stuck time and is never stuck; the same
/// motion over a shorter span (inside one circle) is.
#[test]
fn a_drifting_ball_reanchors() {
    // 0.5 m/s for 400 ticks = 200 mm of travel; the ball crosses each
    // 27 mm circle in ~54 ms, under the 100-tick stuck time.
    let f = driven("clean", 400, 100, |t| {
        ([0.150 + 0.0005 * t as f32, 0.600, R], [0.5, 0.0, 0.0])
    });
    assert!(f.is_empty(), "{f:?}");
    // 0.02 m/s: 27 mm takes 1350 ticks, longer than the stuck time.
    let f = driven("clean", 400, 100, |t| {
        ([0.150 + 0.00002 * t as f32, 0.600, R], [0.02, 0.0, 0.0])
    });
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0].kind, FindingKind::StuckBall);
}

/// Test b: a ball held in a VUK scoop for far longer than the stuck time
/// stays exempt from both stuck tests.
#[test]
fn a_ball_held_by_the_vuk_is_not_stuck() {
    let dir = vuk_bundle("endless_vuk_hold", false);
    let path = dir.to_path_buf();
    let (findings, held) = on_sim_thread(move || {
        let mut sim = Simulation::load(&path, SimConfig::default()).unwrap();
        let info = TableInfo::load(&path).unwrap();
        sim.spawn_ball("standard_steel", [0.400, 0.400, R], [0.0; 3])
            .unwrap();
        let mut mon = GameMonitor::new(&info, &sim, 0, 200);
        for _ in 0..2_000 {
            sim.advance(1);
            mon.observe(&sim);
        }
        let held = sim.mech_set().vuk_ball(1).is_some();
        (mon.findings().to_vec(), held)
    });
    drop(dir);
    assert!(held, "the scoop holds the ball");
    assert!(findings.is_empty(), "{findings:?}");
}

/// Test c: the soak counts censored balls in the ball-time distribution at
/// their observed age, and reports how many there are.
#[test]
fn censored_balls_count_in_the_median() {
    let r = on_sim_thread(|| run_soak(&fixture("clean"), &cfg(2, 1, 2_000)).unwrap());
    let drained: u64 = r.game_list.iter().map(|g| u64::from(g.balls)).sum();
    assert_eq!(r.ball_time_censored, 2, "{:?}", r.game_list);
    assert_eq!(r.ball_time_ms.count, drained + 2);
    // A ball on the table at the 2000-tick cap has an observed age under
    // 2000 ms; the maximum is such a lower bound.
    assert!(r.ball_time_ms.max <= 2_000 && r.ball_time_ms.max > 0);
    let v = r.gate_a1(SkillLevel::Standard);
    let c = v.check("ball_time_censored").unwrap();
    assert!(c.value.starts_with("2 of "), "{}", c.value);
}

/// The pre-fix grey-box (commit 452eedf, built), pinned as a fixture.
fn greybox() -> std::path::PathBuf {
    fixture("greybox_452eedf")
}

/// Plays game `game` of the Gate A1 standard soak (seed 1) on the pre-fix
/// grey-box with the default detectors, capped at `cap` ticks.
///
/// The bot plays with the flip timing these games were recorded with (the
/// original 2.9 bot: press at the prediction, late-only reaction draw).
/// fix-2.9-bot-timing changed the shared default timing, which replays a
/// different game for every seed; the legacy settings keep the pinned trap
/// games reproducible (docs/decisions/2026-11-18-fix-2.9-bot-timing.md).
fn greybox_game(
    game: u32,
    cap: u64,
) -> (
    pin_sim::autoplay::GameSummary,
    Vec<pin_sim::autoplay::Finding>,
) {
    let c = SoakConfig {
        games: 1000,
        seed: 1,
        skill: SkillLevel::Standard,
        threads: 1,
        max_game_ticks: cap,
        check_tick_budget: false,
        bot: BotConfig {
            flip_delay_ms: (0, 0),
            reaction: ReactionDraw::Late,
            ..BotConfig::default()
        },
        ..SoakConfig::default()
    };
    let (s, o) = play_soak_game(&greybox(), &c, game).unwrap();
    (s, o.findings)
}

/// Test e: before/after proof. One game per loop class from the
/// investigator's Gate A1 standard soak on the 452eedf grey-box (which
/// passed Gate A1 with these games ending at `max_ticks` and no finding):
/// game 6 (steel ball VUK <-> deck loop), game 82 (powerball bumper_1 <->
/// orbit guide ping-pong) and game 756 (powerball wedged against the
/// refiring left sling). History: the sling case moved from game 44 to 334
/// when the receding-captive fix (6cdb26d) changed simulation output; variant B
/// of the guide cast contact fix (fix-wireform-flake, user ruling 2026-11-18)
/// changed guide contact decisions and moved the cases again: game 6's end
/// tick went 72_764 -> 82_935, the bumper loop moved from game 98 to 82 and
/// the sling pocket from game 334 to 763 (the old games 98 and 334 now drain
/// to game_over at t55,812 and t39,400). With the new detectors games 82
/// and 756 are named: the bumper loop as EndlessBall at the 300 s default age
/// limit (before the cap), the sling pocket as StuckBall through the
/// displacement test. Game 6 now pins the VUK eject scatter breaking the
/// VUK <-> deck loop (user ruling A3; DECISIONS "w7pre-vuk-scatter"). The #211
/// Newton end-stop rest rule moved game 6's end tick 82_935 -> 84_586 and the
/// sling pocket from game 763 to 756 (the old game 763 now drains to game_over
/// at tick 46_037 with no findings).
#[test]
fn the_pre_fix_greybox_traps_are_caught_and_named() {
    let runs = [(6u32, 320_000u64), (82, 330_000), (756, 120_000)];
    let handles: Vec<_> = runs
        .iter()
        .map(|&(g, cap)| {
            std::thread::Builder::new()
                .stack_size(pin_sim::cli::SIM_THREAD_STACK_BYTES)
                .spawn(move || (g, greybox_game(g, cap)))
                .unwrap()
        })
        .collect();
    for h in handles {
        let (g, (summary, findings)) = h.join().unwrap();
        for f in &findings {
            println!(
                "game {g}: {} {} @ tick {}: {}",
                f.kind.name(),
                f.subject,
                f.tick,
                f.detail
            );
        }
        println!(
            "game {g}: end {} at tick {}",
            summary.end.name(),
            summary.ticks
        );
        match g {
            6 => {
                // End tick was 72_764 before variant B of the guide cast fix,
                // 82_935 before the #211 Newton end-stop rest rule.
                // Changed by user ruling A3 (VUK eject scatter, DECISIONS
                // "w7pre-vuk-scatter"). Before the scatter, every eject was
                // identical, the steel ball looped VUK <-> deck to the cap
                // (MaxTicks) and was named as one EndlessBall ("vuk_sw x").
                // With the scatter the same seed's ejects differ, the loop
                // breaks, and the game drains to game_over well before the
                // cap with no EndlessBall and nothing naming the VUK.
                assert_eq!(summary.end, GameEnd::GameOver, "game 6: {findings:?}");
                assert_eq!(summary.ticks, 84_586, "game 6 end tick");
                assert!(
                    findings.iter().all(|f| f.kind != FindingKind::EndlessBall),
                    "game 6: {findings:?}"
                );
                assert!(
                    findings
                        .iter()
                        .all(|f| !f.subject.contains("vuk_sw") && !f.detail.contains("vuk_sw")),
                    "game 6: {findings:?}"
                );
            }
            82 => {
                // As before the fix, the game runs on to the cap; now the
                // trapped ball is named on the way.
                assert_eq!(summary.end, GameEnd::MaxTicks);
                let e: Vec<_> = findings
                    .iter()
                    .filter(|f| f.kind == FindingKind::EndlessBall)
                    .collect();
                assert_eq!(e.len(), 1, "game 82: {findings:?}");
                assert!(e[0].detail.contains("longer than the 300000 ms limit"));
                assert_eq!(e[0].subject, "bumper_1_ring");
            }
            756 => {
                let s: Vec<_> = findings
                    .iter()
                    .filter(|f| f.kind == FindingKind::StuckBall)
                    .collect();
                assert_eq!(s.len(), 1, "game 756: {findings:?}");
                assert!(
                    s[0].detail.contains("within one ball diameter"),
                    "{}",
                    s[0].detail
                );
                // Named at the pocket between sling_left and the left
                // inlane wall (`walls[3]`); the investigator found it first
                // at (110, 243) mm; the contact point named is now (107, 241) mm
                // (game 756; it was (112, 247) mm in the old game 763).
                assert_eq!(s[0].subject, "walls[3] @ (107, 241, 14) mm");
                assert_eq!(summary.end, GameEnd::Stuck);
            }
            _ => unreachable!(),
        }
    }
}

// ---------------------------------------------------------------------------
// fix-a2-longball: a ball past the endless limit that later drains naturally
// after broad play in the 60 s window at the limit is a LongBall (reported,
// not failing); every other case stays EndlessBall.
// ---------------------------------------------------------------------------

/// Ten positionless leaf switches (credited to every ball on the table) and
/// a plunger mech whose switch is `launch_sw` (the launch switch counts like
/// any other switch in the window).
const LONG_EXTRA: &str = r#"
[[switches]]
id = "sw_0"
number = 30
kind = "leaf"

[[switches]]
id = "sw_1"
number = 31
kind = "leaf"

[[switches]]
id = "sw_2"
number = 32
kind = "leaf"

[[switches]]
id = "sw_3"
number = 33
kind = "leaf"

[[switches]]
id = "sw_4"
number = 34
kind = "leaf"

[[switches]]
id = "sw_5"
number = 35
kind = "leaf"

[[switches]]
id = "sw_6"
number = 36
kind = "leaf"

[[switches]]
id = "sw_7"
number = 37
kind = "leaf"

[[switches]]
id = "sw_8"
number = 38
kind = "leaf"

[[switches]]
id = "sw_9"
number = 39
kind = "leaf"

[[switches]]
id = "launch_sw"
number = 40
kind = "leaf"

[[coils]]
id = "plunger_coil"
number = 10
kind = "autoplunger"

[[mechs]]
id = "plunger"
kind = "plunger"
coil = "plunger_coil"
switch = "launch_sw"
position = [490.0, 60.0, 0.0]
angle_deg = 90.0
auto_pull = 0.9
"#;

/// Rules that do nothing (no ball is launched).
const QUIET_RULES: &str = r#"
function on_init()
  pinball.set_score(1, 0)
end

function on_switch(id, closed)
end
"#;

/// The endless limit of the long-ball scenarios, ticks.
const LIMIT: u64 = 1_000;

/// Where the scenario ball is held, m.
const HOLD: [f32; 3] = [0.257, 0.300, R];

/// How a scenario ball's story ends.
#[derive(Copy, Clone, Debug)]
enum Fate {
    /// Removed from the table without an escape (a natural drain).
    Drain,
    /// Moved far outside the bounds, so the simulation removes it as
    /// escaped (balls_escaped rises).
    Escape,
    /// Still on the table when the soak stops the game at the cap.
    Cap,
    /// Still on the table when the game ends without the cap.
    Finish,
}

/// One ball held at [`HOLD`] while the switches of `hits` close one after
/// the other (one hit each, every 4 ticks from tick 10, all before the
/// limit), then played on to tick 1 200 and ended by `fate`. Returns all
/// findings of the game.
fn long_ball(tag: &str, hits: &[&'static str], fate: Fate) -> Vec<pin_sim::autoplay::Finding> {
    assert!(
        10 + 4 * hits.len() as u64 + 2 < LIMIT,
        "hits must fit before the limit"
    );
    let table = fixture_table_toml() + LONG_EXTRA;
    let dir = fixture_variant(
        tag,
        &[
            ("table.toml", Some(table.as_bytes())),
            ("rules.lua", Some(QUIET_RULES.as_bytes())),
        ],
    );
    let path = dir.to_path_buf();
    let hits = hits.to_vec();
    let out = on_sim_thread(move || {
        let mut sim = Simulation::load(&path, SimConfig::default()).unwrap();
        let info = TableInfo::load(&path).unwrap();
        let h = sim.spawn_ball("standard_steel", HOLD, [0.0; 3]).unwrap();
        // Stuck detection off: the ball is held still on purpose.
        let mut mon = GameMonitor::new(&info, &sim, 3, u64::MAX / 2).with_endless_ticks(LIMIT);
        let hold = BallState {
            position_m: HOLD,
            ..BallState::default()
        };
        mon.observe(&sim);
        for t in 0..1_200u64 {
            if t >= 10 && (t - 10) % 4 == 0 {
                if let Some(name) = hits.get(((t - 10) / 4) as usize) {
                    sim.set_switch(name, true).unwrap();
                }
            } else if t >= 12
                && (t - 12) % 4 == 0
                && let Some(name) = hits.get(((t - 12) / 4) as usize)
            {
                sim.set_switch(name, false).unwrap();
            }
            assert!(sim.set_ball_state(h, &hold));
            sim.advance(1);
            mon.observe(&sim);
        }
        assert_eq!(sim.balls_in_play(), 1);
        let escaped0 = sim.counters().balls_escaped;
        match fate {
            Fate::Drain => {
                assert!(sim.remove_ball(h));
                for _ in 0..10 {
                    sim.advance(1);
                    mon.observe(&sim);
                }
                assert_eq!(sim.counters().balls_escaped, escaped0);
            }
            Fate::Escape => {
                let out = BallState {
                    position_m: [0.257, 0.300, -5.0],
                    velocity_m_s: [0.0, 0.0, -1.0],
                    ..BallState::default()
                };
                assert!(sim.set_ball_state(h, &out));
                for _ in 0..10 {
                    sim.advance(1);
                    mon.observe(&sim);
                }
                assert_eq!(sim.balls_in_play(), 0, "the escaped ball is removed");
                assert_eq!(sim.counters().balls_escaped, escaped0 + 1);
            }
            Fate::Cap => mon.end_at_cap(&sim),
            Fate::Finish => {}
        }
        mon.finish(&sim).findings
    });
    drop(dir);
    out
}

/// sw_0 x4, sw_1 x3, sw_2..sw_9 x2: 10 switches, 23 hits, top two 7 (30 %).
fn broad_hits() -> Vec<&'static str> {
    let mut v = vec!["sw_0"; 4];
    v.extend(["sw_1"; 3]);
    for s in [
        "sw_2", "sw_3", "sw_4", "sw_5", "sw_6", "sw_7", "sw_8", "sw_9",
    ] {
        v.extend([s, s]);
    }
    v
}

fn kinds(f: &[pin_sim::autoplay::Finding]) -> Vec<FindingKind> {
    f.iter().map(|f| f.kind).collect()
}

/// A soak report holding exactly `findings` (coverage complete).
fn report_with(findings: Vec<pin_sim::autoplay::Finding>) -> pin_sim::autoplay::SoakReport {
    let mut r = on_sim_thread(|| run_soak(&fixture("clean"), &cfg(1, 1, 500)).unwrap());
    r.findings = findings;
    r.timing.findings.clear();
    r
}

/// Test (i): a ball past the limit with broad play in the window, then
/// drained naturally, is exactly one LongBall with the EndlessBall's tick,
/// subject and detail (plus the drain suffix); the report passes.
#[test]
fn a_broad_long_ball_that_drains_is_a_long_ball() {
    let f = long_ball("long_broad_drain", &broad_hits(), Fate::Drain);
    assert_eq!(kinds(&f), [FindingKind::LongBall], "{f:?}");
    let l = &f[0];
    assert_eq!(l.tick, LIMIT + 1, "{l:?}");
    assert_eq!(l.game, 3);
    assert_eq!(l.subject, "sw_0", "most-hit switch over the ball's life");
    assert!(
        l.detail.starts_with(&format!(
            "ball in slot 0 on the table longer than the {LIMIT} ms limit: {} ms on the table",
            LIMIT + 1
        )),
        "{}",
        l.detail
    );
    assert!(
        l.detail
            .contains("switches hit in the last 60 s: sw_0 x4, sw_1 x3, sw_2 x2"),
        "{}",
        l.detail
    );
    assert!(
        l.detail
            .contains("; drained naturally at tick 1201 after 1.201 s on the table"),
        "{}",
        l.detail
    );
    assert!(!FindingKind::LongBall.fails_soak());
    let r = report_with(f);
    assert!(r.passed(), "{:?}", r.findings);
    let v = r.gate_a1(SkillLevel::Standard);
    assert_eq!(v.check("no_endless_balls").unwrap().value, "0 findings");
    assert_eq!(r.findings_of(FindingKind::LongBall).count(), 1);
}

/// Test (ii): the same ball still on the table at the cap is EndlessBall
/// (the finding held at the limit, unchanged), and the report fails.
#[test]
fn a_broad_long_ball_still_on_the_table_at_the_cap_is_endless() {
    let f = long_ball("long_broad_cap", &broad_hits(), Fate::Cap);
    assert_eq!(kinds(&f), [FindingKind::EndlessBall], "{f:?}");
    assert_eq!(f[0].tick, LIMIT + 1);
    assert_eq!(f[0].subject, "sw_0");
    assert!(
        f[0].detail
            .contains(&format!("longer than the {LIMIT} ms limit")),
        "{}",
        f[0].detail
    );
    assert!(!f[0].detail.contains("drained"), "{}", f[0].detail);
    assert!(!report_with(f).passed());
}

/// Test (iii): a narrow window then a natural drain is EndlessBall: too few
/// switches, or the top two holding half the hits or more.
#[test]
fn a_narrow_long_ball_that_drains_is_endless() {
    // Seven switches, evenly hit.
    let few: Vec<&str> = ["sw_0", "sw_1", "sw_2", "sw_3", "sw_4", "sw_5", "sw_6"]
        .iter()
        .flat_map(|&s| [s, s, s])
        .collect();
    let f = long_ball("long_few_drain", &few, Fate::Drain);
    assert_eq!(kinds(&f), [FindingKind::EndlessBall], "{f:?}");
    assert_eq!(f[0].tick, LIMIT + 1);
    assert!(!f[0].detail.contains("drained"), "{}", f[0].detail);
    // Ten switches, but the top two hold 20 of 28 hits (71 %).
    let mut dominated = vec!["sw_0"; 10];
    dominated.extend(["sw_1"; 10]);
    dominated.extend([
        "sw_2", "sw_3", "sw_4", "sw_5", "sw_6", "sw_7", "sw_8", "sw_9",
    ]);
    let f = long_ball("long_dominated_drain", &dominated, Fate::Drain);
    assert_eq!(kinds(&f), [FindingKind::EndlessBall], "{f:?}");
    assert_eq!(f[0].subject, "sw_0");
    assert!(!report_with(f).passed());
}

/// Test (iv): a broad ball past the limit that then escapes keeps its
/// EndlessBall, raised before the EscapedBall, and gets no LongBall.
#[test]
fn a_broad_long_ball_that_escapes_is_endless_and_escaped() {
    let f = long_ball("long_broad_escape", &broad_hits(), Fate::Escape);
    assert_eq!(
        kinds(&f),
        [FindingKind::EndlessBall, FindingKind::EscapedBall],
        "{f:?}"
    );
    assert_eq!(f[0].tick, LIMIT + 1);
}

/// Test (v): the thresholds are strict. Exactly 8 switches with the top
/// two at exactly 50 % is narrow; 8 switches with the top two just under
/// 50 % is broad (so 8 switches alone are enough).
#[test]
fn the_broad_window_thresholds_are_strict() {
    // 8 switches, 16 hits: sw_0 x4 and sw_1 x4 (8 of 16 = 50 %); sw_2..sw_7
    // share the other 8.
    let mut exact = vec!["sw_0"; 4];
    exact.extend(["sw_1"; 4]);
    exact.extend([
        "sw_2", "sw_2", "sw_3", "sw_3", "sw_4", "sw_5", "sw_6", "sw_7",
    ]);
    assert_eq!(exact.len(), 16);
    let f = long_ball("long_exact_drain", &exact, Fate::Drain);
    assert_eq!(kinds(&f), [FindingKind::EndlessBall], "{f:?}");
    // One more hit of a third switch: top two 8 of 17 (47 %) => broad.
    let mut under = exact.clone();
    under.push("sw_7");
    let f = long_ball("long_under_drain", &under, Fate::Drain);
    assert_eq!(kinds(&f), [FindingKind::LongBall], "{f:?}");
    // Seven switches, top two well under 50 %: still narrow (fewer than 8).
    let seven: Vec<&str> = ["sw_0", "sw_1", "sw_2", "sw_3", "sw_4", "sw_5", "sw_6"]
        .iter()
        .flat_map(|&s| [s, s])
        .collect();
    let f = long_ball("long_seven_drain", &seven, Fate::Drain);
    assert_eq!(kinds(&f), [FindingKind::EndlessBall], "{f:?}");
}

/// Test (vi): a game that ends with the held finding unresolved (no cap,
/// no drain) still reports it, as EndlessBall.
#[test]
fn finish_flushes_a_held_long_ball_as_endless() {
    let f = long_ball("long_broad_finish", &broad_hits(), Fate::Finish);
    assert_eq!(kinds(&f), [FindingKind::EndlessBall], "{f:?}");
    assert_eq!(f[0].tick, LIMIT + 1);
}

/// The held finding is resolved in the order the balls' fates are seen and
/// keeps its original tick; repeated runs give identical findings.
#[test]
fn long_ball_findings_are_deterministic() {
    let a = long_ball("long_det_a", &broad_hits(), Fate::Drain);
    let b = long_ball("long_det_b", &broad_hits(), Fate::Drain);
    assert_eq!(a, b);
}
