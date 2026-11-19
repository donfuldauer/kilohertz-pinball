//! 6.4-a2fix Fix 2 (Gate A2 run 1, classes C+D EndlessBall): the
//! shooter-lane gate. Bumper kicks sent balls up the right through the
//! 70 mm gap between the lane wall's top (470, 930) and the corner wall at
//! (470, 1000); they rolled down to the plunger and the bot relaunched the
//! same ball 9..24 times. layout.toml walls[16] closes the gap as the gate
//! mech "walls[16]" on lane_gate_coil (11), opened for 1.5 s with every
//! plunger pulse (rules.lua).
//!
//! * (a) a ball at (454, 973) mm moving (+0.24, -0.42) m/s (and a sweep
//!   around it) never crosses x > 470 mm with no plunge; with the gate held
//!   open the reported ball does cross (control);
//! * (b) a plunge still takes the ball round to `orbit_top`;
//! * (c) the Glo-Ball's own plunge (glo_ball mode) opens the gate too: the
//!   served Glo-Ball reaches the playfield;
//! * (d) ignored (release): a 150-game expert soak, seed 1, through
//!   `play_soak_game` with no failing finding and no ball over 300 s.

mod feature_lab_support;

use feature_lab_support::*;
use pin_sim::autoplay::{FindingKind, SkillLevel, SoakConfig, play_soak_game};
use pin_sim::{BallHandle, SimConfig, Simulation};
use std::path::Path;

/// The shooter-lane wall line (layout.toml walls[1] and walls[16]), m.
const LANE_X: f32 = 0.470;

/// A ball at `p` (mm, layer 0) with velocity `v` (m/s) in a sim of `dir`
/// in attract mode (no game, no plunge); returns the largest x (m) the
/// ball centre reaches in 5 s.
fn max_x(dir: &Path, p: [f32; 2], v: [f32; 2]) -> f32 {
    let mut s = Simulation::load(dir, SimConfig::default()).expect("load");
    s.advance(10);
    let h = s
        .spawn_ball("standard_steel", mm(p[0], p[1]), [v[0], v[1], 0.0])
        .expect("spawn");
    let mut x = f32::MIN;
    for _ in 0..5000 {
        s.advance(1);
        let Some(b) = s.ball(h) else { break };
        x = x.max(b.position_m[0]);
    }
    x
}

#[test]
fn a_kicked_ball_cannot_re_enter_the_shooter_lane_from_the_top() {
    on_big_stack(|| {
        let reported = max_x(bundle(), [454.0, 973.0], [0.24, -0.42]);
        assert!(
            reported <= LANE_X,
            "reported ball crossed into the lane: max x {reported}"
        );
        for y in [940.0f32, 955.0, 973.0, 990.0] {
            for vx in [0.1f32, 0.24, 0.5, 0.8] {
                for vy in [-0.42f32, 0.0, 0.3] {
                    let x = max_x(bundle(), [454.0, y], [vx, vy]);
                    assert!(x <= LANE_X, "y {y} v ({vx}, {vy}): max x {x}");
                }
            }
        }
        // Control: with the gate held open the reported ball does go
        // through the gap (so the case above tests the gate).
        let open = bundle_copy("gate_held_open");
        std::fs::write(
            open.join("rules.lua"),
            "function on_init()\n  pinball.hold_coil(pinball.coil.lane_gate_coil, true)\nend\nfunction on_tick() end\n",
        )
        .unwrap();
        let x = max_x(&open, [454.0, 973.0], [0.24, -0.42]);
        assert!(x > LANE_X, "open gate: the ball crosses, max x {x}");
    });
}

/// A game on ball 1 with the player's ball on the plunger.
fn game() -> Simulation {
    let mut s = load();
    s.advance(10);
    start_game(&mut s);
    s
}

/// Plunges and waits up to `ms` for `orbit_top`; the ticks it took.
fn plunge_to_orbit_top(s: &mut Simulation, ms: u64) -> Option<u64> {
    let before = s.switch_hits("orbit_top").unwrap();
    press(s, "launch", 30);
    for t in 0..ms {
        if s.switch_hits("orbit_top").unwrap() > before {
            return Some(30 + t);
        }
        s.advance(1);
    }
    None
}

#[test]
fn a_plunge_still_reaches_orbit_top_through_the_open_gate() {
    on_big_stack(|| {
        for ball in 1..=3 {
            let mut s = game();
            for _ in 1..ball {
                clear_balls(&mut s);
                s.advance(20);
            }
            assert_eq!(persist(&s, "ball"), ball);
            let t = plunge_to_orbit_top(&mut s, 3000);
            assert!(t.is_some(), "ball {ball}: the plunge reached orbit_top");
            healthy(&s);
        }
    });
}

/// The ball handles in play other than `known`.
fn new_balls(s: &Simulation, known: &[BallHandle]) -> Vec<BallHandle> {
    balls(s)
        .into_iter()
        .filter(|h| !known.contains(h))
        .collect()
}

#[test]
fn the_glo_ball_launch_opens_the_gate_too() {
    on_big_stack(|| {
        let mut s = game();
        select_mode(&mut s, 4);
        // The player's ball goes round first, out of the lane.
        assert!(plunge_to_orbit_top(&mut s, 3000).is_some(), "main plunge");
        s.advance(500);
        let known = balls(&s);
        // The mode's qualifying shot (table.toml "pops": bumper_2_ring
        // then bumper_3_ring) serves the Glo-Ball into the shooter lane;
        // the rules plunge it after their launch delay.
        for sw in ["bumper_2_ring", "bumper_3_ring"] {
            press(&mut s, sw, 5);
            s.advance(50);
        }
        assert_eq!(persist(&s, "mode:glo_ball"), 1, "glo_ball started");
        let mut glo = None;
        for _ in 0..500 {
            if let Some(h) = new_balls(&s, &known).first() {
                glo = Some(*h);
                break;
            }
            s.advance(1);
        }
        let glo = glo.expect("the Glo-Ball is served");
        // It leaves the lane over the top and comes out on the playfield
        // (left of the lane wall, layer 0) within 6 s.
        let mut out = false;
        for _ in 0..6000 {
            s.advance(1);
            let Some(b) = s.ball(glo) else { break };
            if b.layer == 0 && b.position_m[0] < LANE_X - 0.03 && b.position_m[1] > 0.9 {
                out = true;
                break;
            }
        }
        assert!(out, "the Glo-Ball reached the playfield");
        healthy(&s);
    });
}

/// Failing finding kinds (as feature_lab_autoplay.rs).
const FAILING: [FindingKind; 6] = [
    FindingKind::StuckBall,
    FindingKind::EndlessBall,
    FindingKind::EscapedBall,
    FindingKind::NonFinite,
    FindingKind::ScriptError,
    FindingKind::QueueOverflow,
];

#[test]
#[ignore = "150 expert games: several minutes in the test profile; run with --ignored"]
fn expert_soak_150_games_no_ball_over_300_s() {
    let dir = bundle().to_path_buf();
    on_big_stack(move || {
        let games = 150;
        let cfg = SoakConfig {
            games,
            seed: 1,
            skill: SkillLevel::Expert,
            threads: 1,
            measure_timing: false,
            check_tick_budget: false,
            ..SoakConfig::default()
        };
        let (mut bad, mut long) = (Vec::new(), Vec::new());
        let mut max_ball = 0i64;
        for g in 0..games {
            let (_, outcome) = play_soak_game(&dir, &cfg, g).unwrap();
            bad.extend(
                outcome
                    .findings
                    .iter()
                    .filter(|f| FAILING.contains(&f.kind))
                    .map(|f| format!("game {g}: {f:?}")),
            );
            for &t in outcome
                .ball_times_ms
                .iter()
                .chain(&outcome.censored_ball_times_ms)
            {
                max_ball = max_ball.max(t);
                if t > 300_000 {
                    long.push(format!("game {g}: ball {t} ms"));
                }
            }
        }
        println!("expert x{games}: max ball {max_ball} ms");
        assert!(bad.is_empty(), "findings:\n{}", bad.join("\n"));
        assert!(long.is_empty(), "balls over 300 s:\n{}", long.join("\n"));
    });
}
