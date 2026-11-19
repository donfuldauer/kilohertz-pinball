//! Renderer-agnostic HUD text of a [`ProfilerSnapshot`]: no GPU or window
//! dependency; the overlay draws the strings however it draws text. Built
//! on the HUD thread (it allocates), never on the sim thread.

use super::{HistogramSummary, INPUT_P99_TARGET_NS, ProfilerSnapshot, TICK_P99_9_TARGET_NS};

/// `ns` as milliseconds with three decimals (µs resolution, rounded down),
/// e.g. `350_000` → `"0.350"`, `1_234_567` → `"1.234"`.
#[must_use]
pub fn format_ms(ns: u64) -> String {
    format!("{}.{:03}", ns / 1_000_000, (ns / 1_000) % 1_000)
}

fn verdict(v: Option<bool>) -> &'static str {
    match v {
        Some(true) => "OK",
        Some(false) => "OVER",
        None => "no data",
    }
}

fn value(s: &HistogramSummary, ns: u64) -> String {
    if s.has_samples() {
        format_ms(ns)
    } else {
        "--".to_owned()
    }
}

/// The HUD overlay lines of `s`, in this order:
///
/// 1. `tick p99.9 <ms> ms (target < 0.350 ms) <OK|OVER|no data> | mean <ms> max <ms> n <count>`
/// 2. `input p99 <ms> ms (target <= 1.500 ms) <OK|OVER|no data> | p50 <ms> max <ms> n <count> clamped <count>`
/// 3. `gpu frame p50 <ms> p99 <ms> max <ms> ms | n <count>`
/// 4. `cmd queue overflows <total> (script <n>, input ticks <n>)`
///
/// Values are milliseconds ([`format_ms`]); `--` stands for a metric
/// without samples.
#[must_use]
pub fn hud_lines(s: &ProfilerSnapshot) -> Vec<String> {
    let (t, i, g) = (&s.tick, &s.input, &s.gpu_frame);
    vec![
        format!(
            "tick p99.9 {} ms (target < {} ms) {} | mean {} max {} n {}",
            value(t, t.p99_9),
            format_ms(TICK_P99_9_TARGET_NS),
            verdict(s.tick_target_met()),
            value(t, t.mean),
            value(t, t.max),
            t.count
        ),
        format!(
            "input p99 {} ms (target <= {} ms) {} | p50 {} max {} n {} clamped {}",
            value(i, i.p99),
            format_ms(INPUT_P99_TARGET_NS),
            verdict(s.input_target_met()),
            value(i, i.p50),
            value(i, i.max),
            i.count,
            s.input_clamped
        ),
        format!(
            "gpu frame p50 {} p99 {} max {} ms | n {}",
            value(g, g.p50),
            value(g, g.p99),
            value(g, g.max),
            g.count
        ),
        format!(
            "cmd queue overflows {} (script {}, input ticks {})",
            s.command_queue_overflows(),
            s.script_queue_overflows,
            s.input_overflow_ticks
        ),
    ]
}
