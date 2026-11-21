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

/// App-level rates shown under the profiler lines (REQ-LAT-003): they come
/// from the orchestrator's counters, not from the profiler, so they are
/// passed in rather than read from a [`ProfilerSnapshot`].
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct HudRates {
    /// Ticks dropped by the catch-up cap since the sim thread started
    /// (cumulative, `OrchestratorStats` / `SimClock::dropped_ticks`).
    pub dropped_ticks: u64,
    /// Presented frames per second, in thousandths (`59_940` = 59.94 fps),
    /// over the last refresh period; `None` before a rate is known
    /// ([`FrameRateMeter`]).
    pub presented_fps_milli: Option<u64>,
}

/// `fps_milli` (thousandths of a frame per second) with one decimal,
/// rounded down: `59_940` → `"59.9"`, `0` → `"0.0"`.
#[must_use]
pub fn format_fps(fps_milli: u64) -> String {
    format!("{}.{}", fps_milli / 1_000, (fps_milli / 100) % 10)
}

/// The two rate lines of `r`, in this order:
///
/// 1. `presented fps <fps>` (one decimal, [`format_fps`]; `--` without a
///    rate yet)
/// 2. `dropped ticks <count>`
#[must_use]
pub fn hud_rate_lines(r: &HudRates) -> [String; 2] {
    let fps = r
        .presented_fps_milli
        .map_or_else(|| "--".to_owned(), format_fps);
    [
        format!("presented fps {fps}"),
        format!("dropped ticks {}", r.dropped_ticks),
    ]
}

/// The full profiler HUD of the app (REQ-LAT-003): the four
/// [`hud_lines`] of `s` (unchanged) followed by the two
/// [`hud_rate_lines`] of `r` (presented fps, dropped ticks).
#[must_use]
pub fn hud_lines_with_rates(s: &ProfilerSnapshot, r: &HudRates) -> Vec<String> {
    let mut lines = hud_lines(s);
    lines.extend(hud_rate_lines(r));
    lines
}

/// Presented frames per second between two readings of a cumulative frame
/// counter (the render thread's presented-frame count), in thousandths.
/// HUD thread only; plain integer arithmetic.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct FrameRateMeter {
    last: Option<(u64, u64)>,
}

impl FrameRateMeter {
    /// A meter without a reading yet.
    #[must_use]
    pub const fn new() -> FrameRateMeter {
        FrameRateMeter { last: None }
    }

    /// Takes a reading of `frames_total` at `now_ns` (any monotonic clock)
    /// and returns the rate since the previous reading, in thousandths of a
    /// frame per second (rounded down, saturating). `None` on the first
    /// reading, when no time passed, or when the clock or the counter went
    /// backwards (the reading is still kept as the new base).
    pub fn sample(&mut self, now_ns: u64, frames_total: u64) -> Option<u64> {
        let prev = self.last.replace((now_ns, frames_total));
        let (t0, f0) = prev?;
        if now_ns <= t0 || frames_total < f0 {
            return None;
        }
        let frames = u128::from(frames_total - f0);
        let dt = u128::from(now_ns - t0);
        Some(u64::try_from(frames * 1_000_000_000_000 / dt).unwrap_or(u64::MAX))
    }
}
