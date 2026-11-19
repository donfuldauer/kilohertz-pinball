//! 6.4-a2fix Fix 3 (Gate A2 run 1, class B StuckBall): the two
//! magnet-comparison balls can form a two-ball arch at the left inlane
//! mouth (between walls[3] and the free top end of walls[12] at
//! (63.87, 329.4) mm), and the main ball piles on behind them.
//!
//! The sweep releases two standard balls together above the mouth (the
//! investigator's 865-case release sweep: the replay of game 36's arrival
//! plus a 4 x 4 x 3 x 3 x 3 x 2 grid over the left ball's x, the pair's
//! spacing, the release height, the speed down the table, vx and a height
//! offset) and a single-ball sweep onto the free top end of walls[12]
//! (x 60..68, y 335..350 on the run-1 geometry, small vx). A case jams when
//! a ball anywhere on the table (no game runs, so nothing holds a ball:
//! no magnet, lock or plunger) stays slower than the StuckBall speed
//! (`pin_sim::autoplay::monitor::STUCK_SPEED_M_S`, 1 cm/s) for more than
//! 5 s of simulated time without draining or being freed (the monitor's
//! criterion, with 5 s for its 10 s `DEFAULT_STUCK_TICKS`).
//!
//! On the Gate A2 run-1 geometry 33 of the 865 cases jam (32 two-ball
//! arches and one single-ball rest on the guard's top end at (64, 343)),
//! and 5 of the 60 tip cases rest there.

mod feature_lab_support;

use feature_lab_support::*;
use pin_sim::autoplay::monitor::STUCK_SPEED_M_S;
use pin_sim::{SimConfig, Simulation};
use std::path::Path;

/// Slow ticks in the mouth area that count as a jam (5 s).
const JAM_TICKS: u32 = 5_000;
/// Simulated time per case, ticks (12 s: a jam forms within 4 s).
const CASE_TICKS: u32 = 12_000;

/// A ball's start (position m, velocity m/s).
type Start = ([f32; 3], [f32; 3]);

/// The 865 two-ball release cases, in the investigator's order.
fn two_ball_cases() -> Vec<Vec<Start>> {
    let z = R;
    let mut cases = vec![vec![
        ([0.024, 0.334, z], [0.216, 0.085, 0.0]),
        ([0.075, 0.366, z], [-0.308, -0.452, 0.0]),
    ]];
    for x1 in [0.016f32, 0.022, 0.028, 0.034] {
        for dx in [0.028f32, 0.034, 0.040, 0.046] {
            for y0 in [0.360f32, 0.400, 0.450] {
                for v in [0.0f32, 0.3, 0.6] {
                    for vx in [-0.2f32, 0.0, 0.2] {
                        for dy in [0.0f32, 0.010] {
                            cases.push(vec![
                                ([x1, y0, z], [vx, -v, 0.0]),
                                ([x1 + dx, y0 + dy, z], [vx, -v, 0.0]),
                            ]);
                        }
                    }
                }
            }
        }
    }
    cases
}

/// The free top end of the sling guard (layout.toml walls[12], its first
/// point) in the bundle `dir`, mm.
fn guard_tip(dir: &Path) -> [f32; 2] {
    let text = std::fs::read_to_string(dir.join("layout.toml")).expect("layout.toml");
    let t: toml::Table = toml::from_str(&text).expect("layout.toml parses");
    let p = &t["walls"].as_array().expect("walls")[12]["points"][0];
    let c = |k: usize| p[k].as_float().expect("coordinate") as f32;
    [c(0), c(1)]
}

/// The single-ball cases onto the free top end of walls[12]: x from 3.87
/// mm left to 4.13 mm right of it, 5.6..20.6 mm above it (on the Gate A2
/// run-1 geometry, tip (63.87, 329.4): x 60..68, y 335..350), small vx.
fn tip_cases(tip: [f32; 2]) -> Vec<Vec<Start>> {
    let mut cases = Vec::new();
    for dx in [-3.87f32, -1.87, 0.13, 2.13, 4.13] {
        for dy in [5.6f32, 10.6, 15.6, 20.6] {
            for vx in [-0.05f32, 0.0, 0.05] {
                cases.push(vec![(mm(tip[0] + dx, tip[1] + dy), [vx, 0.0, 0.0])]);
            }
        }
    }
    cases
}

/// Where a ball of `case` jams (rest point, mm), if one does.
fn jams(dir: &Path, case: &[Start]) -> Option<[f32; 2]> {
    let mut s = Simulation::load(dir, SimConfig::default()).expect("load");
    let hs: Vec<_> = case
        .iter()
        .map(|(p, v)| s.spawn_ball("standard_steel", *p, *v).expect("spawn"))
        .collect();
    let mut slow = vec![0u32; hs.len()];
    for _ in 0..CASE_TICKS {
        s.advance(1);
        let mut alive = false;
        for (k, h) in hs.iter().enumerate() {
            let Some(b) = s.ball(*h) else {
                slow[k] = 0;
                continue;
            };
            alive = true;
            let [vx, vy, vz] = b.velocity_m_s;
            let speed = (vx * vx + vy * vy + vz * vz).sqrt();
            let [x, y, _] = b.position_m;
            if speed < STUCK_SPEED_M_S {
                slow[k] += 1;
                if slow[k] > JAM_TICKS {
                    return Some([x * 1.0e3, y * 1.0e3]);
                }
            } else {
                slow[k] = 0;
            }
        }
        if !alive {
            break;
        }
    }
    None
}

/// Runs `cases` (index, balls) and returns the jammed ones with the rest
/// point (mm).
fn sweep(dir: &Path, cases: &[(usize, Vec<Start>)]) -> Vec<(usize, [f32; 2])> {
    cases
        .iter()
        .filter_map(|(i, c)| jams(dir, c).map(|p| (*i, p)))
        .collect()
}

/// The best measured bounds (docs/decisions/2026-11-19-6.4-a2fix.md
/// "6.4-a2-inlane"); the target for both is 0. Every residual is a lone
/// ball balanced on the guard's free top end: a ball whose centre is
/// within about 0.9 mm of straight up-table of a free wall end stays there,
/// because the playfield's rolling resistance outweighs the small sideways
/// pull (any free end has this perch, whatever its shape). No two-ball
/// arch is left.
const MAX_TWO_BALL_JAMS: usize = 2;
const MAX_TIP_RESTS: usize = 5;

/// Whether a rest point (mm) is the single-ball perch on the guard tip: the
/// ball touching the tip from straight up-table (within 2 mm).
fn on_tip_perch(p: [f32; 2], tip: [f32; 2]) -> bool {
    (p[0] - tip[0]).abs() < 2.0 && (p[1] - tip[1] - R * 1.0e3).abs() < 2.0
}

#[test]
fn single_ball_never_rests_on_the_free_end_of_the_sling_guard() {
    let dir = bundle().to_path_buf();
    on_big_stack(move || {
        let tip = guard_tip(&dir);
        let cases: Vec<_> = tip_cases(tip).into_iter().enumerate().collect();
        let j = sweep(&dir, &cases);
        println!("tip {tip:?} sweep: {} of {} rest", j.len(), cases.len());
        assert!(j.len() <= MAX_TIP_RESTS, "tip rests: {j:?}");
        assert!(
            j.iter().all(|(_, p)| on_tip_perch(*p, tip)),
            "a rest away from the tip perch: {j:?}"
        );
    });
}

#[test]
fn two_ball_release_sweep_865_cases() {
    let dir = bundle().to_path_buf();
    on_big_stack(move || {
        let cases: Vec<_> = two_ball_cases().into_iter().enumerate().collect();
        assert_eq!(cases.len(), 865);
        let j = sweep(&dir, &cases);
        println!("two-ball sweep: {} of 865 jam: {j:?}", j.len());
        assert!(j.len() <= MAX_TWO_BALL_JAMS, "jams: {j:?}");
        let tip = guard_tip(&dir);
        let arches: Vec<_> = j.iter().filter(|(_, p)| !on_tip_perch(*p, tip)).collect();
        assert!(
            arches.is_empty(),
            "jams other than the tip perch: {arches:?}"
        );
    });
}
