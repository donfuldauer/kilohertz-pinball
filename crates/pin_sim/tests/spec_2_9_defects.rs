//! Subphase 2.9 spec tests (Independent Tester), T1: planted defects are
//! each caught and named, in the report struct and in its Markdown and JSON.
//!
//! Fixtures (tests/fixtures/spec_2_9, see README.md): `trap` (a pocket under
//! the serve), `gap` (outer wall open at the drain), `hidden` (switch
//! `hidden_opto` inside a closed wall box), clean geometry with the
//! `mode_error` / `flood` rules, and the clean table plus `impossible_shot`.

#[path = "fixtures/spec_2_9/support.rs"]
mod support;

#[path = "fixtures/spec_2_9/soak_support.rs"]
mod soak_support;

use pin_sim::autoplay::SkillLevel;
use soak_support::*;
use support::*;

/// Two Standard games, 30 s of simulated time at most each.
fn defect_soak(bundle: &std::path::Path) -> pin_sim::autoplay::SoakReport {
    soak(bundle, cfg(2, 11, SkillLevel::Standard, 2, 30 * SECOND))
}

#[test]
fn ball_trap_is_a_stuck_ball_naming_the_pocket() {
    let b = variant("trap", "clean");
    // The bot's nudges reset the 1 cm/s stuck clock, so StuckBall appears
    // about 51 s in: this test gets a 90 s game cap.
    let r = soak(&b, cfg(2, 11, SkillLevel::Standard, 2, 90 * SECOND));
    let stuck = of_kind(&r, "StuckBall");
    assert!(!stuck.is_empty(), "no StuckBall:{}", describe(&r));
    let named: Vec<_> = stuck.iter().filter(|f| names_pocket(&f.subject)).collect();
    assert!(
        !named.is_empty(),
        "no StuckBall subject names the pocket ({POCKET_ELEMENT} or a position near \
         {POCKET_MM:?} mm):{}",
        describe(&r)
    );
    for f in named {
        assert!(
            f.tick > 10 * SECOND,
            "stuck reported at tick {} (< 10 s)",
            f.tick
        );
        assert_in_text(&r, f);
    }
}

#[test]
fn wall_gap_is_an_escaped_ball() {
    let b = variant("gap", "clean");
    // fix-2.9-bot-timing: the calibrated flip timing keeps the ball up
    // longer, and in both 30 s games it never reached the open drain wall
    // (two EndlessBall at the cap, no EscapedBall), so this test gets the
    // 90 s cap the trap test uses.
    let r = soak(&b, cfg(2, 11, SkillLevel::Standard, 2, 90 * SECOND));
    let esc = of_kind(&r, "EscapedBall");
    assert!(!esc.is_empty(), "no EscapedBall:{}", describe(&r));
    for f in esc {
        assert!(!f.subject.is_empty(), "EscapedBall with an empty subject");
        assert_in_text(&r, f);
    }
}

#[test]
fn hidden_switch_is_unreachable_by_exact_id() {
    let b = variant("hidden", "clean");
    let r = defect_soak(&b);
    let un = of_kind(&r, "UnreachableSwitch");
    let hidden: Vec<_> = un.iter().filter(|f| f.subject == "hidden_opto").collect();
    assert!(
        !hidden.is_empty(),
        "no UnreachableSwitch hidden_opto:{}",
        describe(&r)
    );
    assert_in_text(&r, hidden[0]);
    // Cabinet inputs are not switch-coverage subjects.
    for cab in [
        "left_flipper",
        "right_flipper",
        "left_upper",
        "right_upper",
        "action",
        "start",
        "launch",
        "tilt",
    ] {
        assert!(
            un.iter().all(|f| f.subject != cab),
            "cabinet input {cab} reported unreachable:{}",
            describe(&r)
        );
    }
}

#[test]
fn script_error_on_mode_start_is_caught_and_the_mode_is_uncovered() {
    let b = variant("clean", "mode_error");
    let r = defect_soak(&b);
    let errs = of_kind(&r, "ScriptError");
    assert!(!errs.is_empty(), "no ScriptError:{}", describe(&r));
    for f in &errs {
        assert_in_text(&r, f);
    }
    assert!(
        errs.iter()
            .any(|f| f.detail.contains("planted defect") || f.subject.contains("planted defect")),
        "no ScriptError carries the Lua error message:{}",
        describe(&r)
    );
    let modes = of_kind(&r, "ModeCoverage");
    let frenzy: Vec<_> = modes.iter().filter(|f| f.subject == "frenzy").collect();
    assert!(
        !frenzy.is_empty(),
        "no ModeCoverage frenzy:{}",
        describe(&r)
    );
    assert_in_text(&r, frenzy[0]);
}

#[test]
fn command_queue_flood_is_a_queue_overflow() {
    let b = variant("clean", "flood");
    let r = defect_soak(&b);
    let ov = of_kind(&r, "QueueOverflow");
    assert!(!ov.is_empty(), "no QueueOverflow:{}", describe(&r));
    for f in ov {
        assert_in_text(&r, f);
    }
    assert!(
        of_kind(&r, "ScriptError").is_empty(),
        "flooding is not a script error"
    );
}

#[test]
fn impossible_shot_is_uncovered_by_exact_id() {
    let b = bad_shot_bundle();
    let r = defect_soak(&b);
    let shots = of_kind(&r, "ShotCoverage");
    let bad: Vec<_> = shots
        .iter()
        .filter(|f| f.subject == "impossible_shot")
        .collect();
    assert!(
        !bad.is_empty(),
        "no ShotCoverage impossible_shot:{}",
        describe(&r)
    );
    assert_in_text(&r, bad[0]);
    assert!(
        shots.iter().all(|f| f.subject != "orbit_shot"),
        "orbit_shot is made by every serve:{}",
        describe(&r)
    );
}
