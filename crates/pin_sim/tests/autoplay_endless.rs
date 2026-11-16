//! Work item A1-endless-detector: the EndlessBall detector, the
//! displacement test of the StuckBall detector, censored ball times, and
//! the before/after proof on the pre-fix grey-box (commit 452eedf).

mod sim_support;
#[path = "fixtures/autoplay/support.rs"]
mod support;

use pin_core::snapshot::BallState;
use pin_sim::autoplay::{
    FindingKind, GameEnd, GameMonitor, SkillLevel, SoakConfig, TableInfo, play_soak_game, run_soak,
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
        let (s, c) = a.sin_cos();
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
        ..SoakConfig::default()
    };
    let (s, o) = play_soak_game(&greybox(), &c, game).unwrap();
    (s, o.findings)
}

/// Test e: before/after proof. One game per loop class from the
/// investigator's Gate A1 standard soak on the 452eedf grey-box (which
/// passed Gate A1 with these games ending at `max_ticks` and no finding):
/// game 6 (steel ball VUK <-> deck loop), game 98 (powerball bumper_1 <->
/// orbit guide ping-pong) and game 334 (powerball wedged against the
/// refiring left sling; the case moved from game 44 when the receding-captive
/// fix (6cdb26d) changed simulation output). With the new detectors each one
/// is named: the two loops as EndlessBall at the 300 s default age limit
/// (before the cap), the sling pocket as StuckBall through the displacement
/// test.
#[test]
fn the_pre_fix_greybox_traps_are_caught_and_named() {
    let runs = [(6u32, 320_000u64), (98, 330_000), (334, 120_000)];
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
                // As before the fix, the game runs on to the cap; now the
                // trapped ball is named on the way.
                assert_eq!(summary.end, GameEnd::MaxTicks);
                let e: Vec<_> = findings
                    .iter()
                    .filter(|f| f.kind == FindingKind::EndlessBall)
                    .collect();
                assert_eq!(e.len(), 1, "game 6: {findings:?}");
                assert!(e[0].detail.contains("longer than the 300000 ms limit"));
                assert!(
                    ["vuk_sw", "deck_opto", "bumper_2_ring", "bumper_3_ring"]
                        .contains(&e[0].subject.as_str()),
                    "{}",
                    e[0].subject
                );
                assert!(e[0].detail.contains("vuk_sw x"), "{}", e[0].detail);
            }
            98 => {
                // As before the fix, the game runs on to the cap; now the
                // trapped ball is named on the way.
                assert_eq!(summary.end, GameEnd::MaxTicks);
                let e: Vec<_> = findings
                    .iter()
                    .filter(|f| f.kind == FindingKind::EndlessBall)
                    .collect();
                assert_eq!(e.len(), 1, "game 98: {findings:?}");
                assert!(e[0].detail.contains("longer than the 300000 ms limit"));
                assert_eq!(e[0].subject, "bumper_1_ring");
            }
            334 => {
                let s: Vec<_> = findings
                    .iter()
                    .filter(|f| f.kind == FindingKind::StuckBall)
                    .collect();
                assert_eq!(s.len(), 1, "game 334: {findings:?}");
                assert!(
                    s[0].detail.contains("within one ball diameter"),
                    "{}",
                    s[0].detail
                );
                // Named at the pocket between sling_left and the left
                // inlane wall (`walls[3]`), where the investigator found it
                // at (110, 243) mm.
                assert_eq!(s[0].subject, "walls[3] @ (112, 243, 14) mm");
                assert_eq!(summary.end, GameEnd::Stuck);
            }
            _ => unreachable!(),
        }
    }
}
