//! Subphase 2.9 spec tests (Independent Tester), T5: the health detectors on
//! planted ball states, through `run_soak` (the brief fixes no API that runs
//! the detectors on a caller-owned `Simulation`, so the states are planted by
//! fixture rules that dispense a ball at a known place and speed; see
//! DECISIONS.md "2.9 Tester — detectors driven through run_soak").
//!
//! Stuck timing: the `rest` ball (trap geometry, `rest_feed`) settles in the
//! pocket within 800 ms of tick 1 (spec_2_9_fixtures.rs checks this), so a
//! game cut at 9 s cannot hold 10 s of rest and a game run to 11 s must.

#[path = "fixtures/spec_2_9/support.rs"]
mod support;

#[path = "fixtures/spec_2_9/soak_support.rs"]
mod soak_support;

use pin_sim::autoplay::SkillLevel;
use soak_support::*;
use support::*;

fn one_game(bundle: &std::path::Path, max_ticks: u64) -> pin_sim::autoplay::SoakReport {
    soak(bundle, cfg(1, 3, SkillLevel::Standard, 1, max_ticks))
}

#[test]
fn ball_at_rest_is_not_stuck_at_9_s() {
    let b = variant("trap", "rest");
    let r = one_game(&b, 9 * SECOND);
    assert!(
        of_kind(&r, "StuckBall").is_empty(),
        "stuck before 10 s:{}",
        describe(&r)
    );
}

#[test]
fn ball_at_rest_is_stuck_at_11_s() {
    let b = variant("trap", "rest");
    let r = one_game(&b, 11 * SECOND);
    let stuck = of_kind(&r, "StuckBall");
    assert_eq!(
        stuck.len(),
        1,
        "one StuckBall for the one ball:{}",
        describe(&r)
    );
    let f = stuck[0];
    assert!(
        f.tick > 10 * SECOND && f.tick <= 11 * SECOND,
        "reported at tick {} (rest began by tick 800)",
        f.tick
    );
    assert!(
        names_pocket(&f.subject),
        "subject {:?} does not locate the pocket",
        f.subject
    );
    assert!(of_kind(&r, "EscapedBall").is_empty() && of_kind(&r, "NonFinite").is_empty());
}

#[test]
fn ball_at_rest_in_the_trough_is_not_stuck() {
    let b = variant("clean", "trough_rest");
    let r = one_game(&b, 15 * SECOND);
    assert!(of_kind(&r, "StuckBall").is_empty(), "{}", describe(&r));
    assert!(of_kind(&r, "EscapedBall").is_empty(), "{}", describe(&r));
}

#[test]
fn ball_at_rest_on_a_held_flipper_is_not_stuck() {
    let b = variant("clean", "cradle");
    let r = one_game(&b, 15 * SECOND);
    assert!(
        of_kind(&r, "StuckBall").is_empty(),
        "held cradle:{}",
        describe(&r)
    );
}

#[test]
fn ball_thrown_off_the_table_is_escaped() {
    let b = variant("clean", "fly");
    let r = one_game(&b, 3 * SECOND);
    let esc = of_kind(&r, "EscapedBall");
    assert_eq!(esc.len(), 1, "one EscapedBall:{}", describe(&r));
    assert!(
        esc[0].tick <= SECOND,
        "escape reported at tick {}",
        esc[0].tick
    );
    assert!(of_kind(&r, "StuckBall").is_empty());
}

#[test]
fn no_ball_no_findings_beyond_coverage() {
    // Inert rules: no ball ever. Nothing can be stuck, escaped or
    // non-finite; every ball-operated switch is uncovered (the bot may still
    // flip, closing the end-of-stroke switches).
    let b = variant("clean", "inert");
    let r = one_game(&b, 5 * SECOND);
    for k in [
        "StuckBall",
        "EscapedBall",
        "NonFinite",
        "ScriptError",
        "QueueOverflow",
    ] {
        assert!(of_kind(&r, k).is_empty(), "{k}:{}", describe(&r));
    }
    let un = of_kind(&r, "UnreachableSwitch");
    for s in ["top_opto", "mid_opto", "trough_1", "bumper_1_ring"] {
        assert!(
            un.iter().any(|f| f.subject == s),
            "switch {s} not reported uncovered:{}",
            describe(&r)
        );
    }
    assert!(
        of_kind(&r, "ShotCoverage")
            .iter()
            .any(|f| f.subject == "orbit_shot")
    );
    assert!(
        of_kind(&r, "ModeCoverage")
            .iter()
            .any(|f| f.subject == "frenzy")
    );
}
