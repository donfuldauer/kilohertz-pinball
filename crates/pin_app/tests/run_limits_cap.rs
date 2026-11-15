//! The `--run-ms` cap and run conditions on supplied durations (D8): the
//! pure `RunLimits::check(&stats, elapsed)`, no sleeping, no process.

use pin_app::orchestrator::{RunExit, RunLimits, StatsSnapshot};
use std::time::Duration;

fn stats(frames: u64, ticks: u64) -> StatsSnapshot {
    StatsSnapshot {
        render_frames: frames,
        sim_ticks: ticks,
        ..StatsSnapshot::default()
    }
}

const MS: fn(u64) -> Duration = Duration::from_millis;

fn capped() -> RunLimits {
    RunLimits {
        frames: Some(3),
        ticks: Some(100),
        cap: Some(MS(1000)),
    }
}

#[test]
fn cap_reached_when_conditions_are_unmet_at_the_cap() {
    assert_eq!(
        capped().check(&stats(2, 100), MS(1000)),
        Some(RunExit::CapReached)
    );
    assert_eq!(
        capped().check(&stats(0, 0), MS(60_000)),
        Some(RunExit::CapReached)
    );
}

#[test]
fn no_exit_before_the_cap_while_conditions_are_unmet() {
    assert_eq!(capped().check(&stats(0, 0), MS(0)), None);
    assert_eq!(capped().check(&stats(2, 99), MS(999)), None);
}

#[test]
fn met_conditions_win_over_an_expired_cap() {
    assert_eq!(
        capped().check(&stats(3, 100), MS(5000)),
        Some(RunExit::ConditionMet)
    );
}

#[test]
fn a_cap_alone_is_a_plain_duration() {
    let plain = RunLimits {
        cap: Some(MS(50)),
        ..RunLimits::default()
    };
    assert_eq!(plain.check(&stats(9, 9), MS(49)), None);
    assert_eq!(plain.check(&stats(0, 0), MS(50)), Some(RunExit::Elapsed));
}

#[test]
fn conditions_without_a_cap_never_time_out() {
    let ticks = RunLimits {
        ticks: Some(10),
        ..RunLimits::default()
    };
    assert_eq!(ticks.check(&stats(0, 9), Duration::MAX), None);
    assert_eq!(
        ticks.check(&stats(0, 10), MS(0)),
        Some(RunExit::ConditionMet)
    );
    // --run-ticks parses without --run-ms.
    let l = RunLimits::from_args(&["--run-ticks".to_owned(), "5".to_owned()]).unwrap();
    assert_eq!(l.cap, None);
    assert_eq!(l.ticks, Some(5));
}
