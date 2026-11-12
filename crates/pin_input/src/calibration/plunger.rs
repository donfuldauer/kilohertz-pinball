//! Analog plunger filter.

/// Longest supported release window; larger configured windows are clamped.
pub const MAX_RELEASE_WINDOW_MS: u32 = 1024;

const RING: usize = MAX_RELEASE_WINDOW_MS as usize;

/// When a pull qualifies, rest tracking of the last this-many updates before
/// it started is undone: those samples were the start of the pull moving
/// through the deadzone, not the plunger at rest.
pub const REST_LOOKBACK_TICKS: usize = 128;

/// A pull must reach at least `max(PULL_QUALIFY_DEADZONES · deadzone,
/// MIN_PULL_DEPTH)` beyond the rest position before its return can be a
/// release. Shallower excursions (ADC noise at the deadzone edge, a resting
/// plunger touched) only report a position.
pub const PULL_QUALIFY_DEADZONES: f32 = 2.5;

/// See [`PULL_QUALIFY_DEADZONES`] (full-scale).
pub const MIN_PULL_DEPTH: f32 = 0.05;

/// After a release no new pull arms for this long (ms): the spring bounce.
pub const RELEASE_HOLDOFF_MS: u64 = 200;

/// The tracked rest position stays within `±MAX_REST_OFFSET` of nominal 0.
pub const MAX_REST_OFFSET: f32 = 0.25;

/// A reading that stays within ±deadzone of one level this long (ms) is
/// "stable"; see [`PlungerFilter::rest`] for when that re-acquires the rest.
pub const STABLE_MS: u64 = 1000;

/// Power-on rest acquisition is possible only this long (ms) after the first
/// update, and only before the first release.
pub const ACQUIRE_WINDOW_MS: u64 = 3000;

/// Raw readings are clamped to `±RAW_LIMIT` full-scale (a mis-scaled device
/// must not produce an infinite speed).
pub const RAW_LIMIT: f32 = 16.0;

/// Plunger calibration settings. Positions are in full-scale units (the
/// normalised `CabinetAxis::Plunger` value, 0 = nominal rest, 1 = full pull).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PlungerConfig {
    /// Displacement from the tracked rest position treated as "at rest".
    pub deadzone: f32,
    /// Rest tracking rate (1/s): while within the deadzone the rest position
    /// moves toward the reading by `rate · dt` of the difference per update.
    pub rest_tracking_per_s: f32,
    /// Minimum peak forward speed (full-scale/s) of the return stroke that
    /// counts as a release; slower returns report nothing.
    pub release_min_speed: f32,
    /// How far back (ms) from the tick the plunger re-enters the deadzone the
    /// peak forward speed is taken. Clamped to [`MAX_RELEASE_WINDOW_MS`].
    /// (The post-release hold-off is the separate [`RELEASE_HOLDOFF_MS`].)
    pub release_window_ms: u32,
}

impl Default for PlungerConfig {
    fn default() -> Self {
        Self {
            deadzone: 0.02,
            rest_tracking_per_s: 1.0,
            release_min_speed: 3.0,
            release_window_ms: 100,
        }
    }
}

/// One filtered plunger sample.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PlungerOutput {
    /// Pull, 0 at rest .. 1 full pull, with the deadzone removed (0 whenever
    /// the plunger is within the deadzone of its tracked rest position).
    pub position: f32,
    /// On the single tick a pull ends with a fast return: the peak forward
    /// speed of the return stroke (full-scale/s).
    pub release: Option<f32>,
}

/// Analog plunger conditioning: deadzone, rest tracking and release
/// detection. Call [`PlungerFilter::update`] once per 1 ms sim tick.
#[derive(Clone, Debug)]
pub struct PlungerFilter {
    cfg: PlungerConfig,
    rest: f32,
    last_raw: f32,
    last_tick: Option<u64>,
    pulled: bool,
    /// The current pull reached the qualifying depth.
    qualified: bool,
    /// Rest to restore when the current pull qualifies.
    undo_rest: f32,
    /// First update's tick (power-on acquisition window).
    first_tick: Option<u64>,
    /// A release has been reported (ends power-on acquisition).
    released_once: bool,
    /// Stable-signal detector: anchor level, start tick, running sum/count.
    band_anchor: f32,
    band_start: u64,
    band_sum: f64,
    band_count: u32,
    /// No new pull is armed before this tick (bounce after a release).
    holdoff_until: u64,
    /// Forward speeds of the current pull: `(tick, speed)` ring.
    speeds: [(u64, f32); RING],
    head: usize,
    len: usize,
    /// Rest position before each of the last updates (ring, oldest first at
    /// `rest_head` once full).
    rest_hist: [f32; REST_LOOKBACK_TICKS],
    rest_head: usize,
    rest_len: usize,
}

impl PlungerFilter {
    /// A filter with the rest position at 0.
    pub fn new(cfg: PlungerConfig) -> Self {
        Self {
            cfg,
            rest: 0.0,
            last_raw: 0.0,
            last_tick: None,
            pulled: false,
            qualified: false,
            undo_rest: 0.0,
            first_tick: None,
            released_once: false,
            band_anchor: 0.0,
            band_start: 0,
            band_sum: 0.0,
            band_count: 0,
            holdoff_until: 0,
            speeds: [(0, 0.0); RING],
            head: 0,
            len: 0,
            rest_hist: [0.0; REST_LOOKBACK_TICKS],
            rest_head: 0,
            rest_len: 0,
        }
    }

    /// The configuration.
    pub fn config(&self) -> &PlungerConfig {
        &self.cfg
    }

    /// The tracked rest position (raw units).
    ///
    /// * Tracking: while the reading is within the deadzone of the rest
    ///   position, rest follows it at `rest_tracking_per_s`.
    /// * When a pull qualifies (see [`PULL_QUALIFY_DEADZONES`]), the tracking
    ///   of the [`REST_LOOKBACK_TICKS`] updates before it started is undone.
    /// * Power-on acquisition: within [`ACQUIRE_WINDOW_MS`] of the first
    ///   update and before the first release, a reading stable for
    ///   [`STABLE_MS`] more than a deadzone above rest becomes the rest (the
    ///   pull it looked like is cancelled without a release).
    /// * Downward re-acquisition: at any time, a reading stable for
    ///   [`STABLE_MS`] more than a deadzone below rest becomes the rest.
    /// * Rest always stays within `±`[`MAX_REST_OFFSET`].
    pub fn rest(&self) -> f32 {
        self.rest
    }

    /// `true` while a pull is in progress.
    pub fn is_pulled(&self) -> bool {
        self.pulled
    }

    /// Feeds one raw reading (normalised plunger axis) at sim tick `tick_ms`.
    /// A non-finite reading repeats the previous one.
    pub fn update(&mut self, raw: f32, tick_ms: u64) -> PlungerOutput {
        let raw = if raw.is_finite() {
            raw.clamp(-RAW_LIMIT, RAW_LIMIT)
        } else {
            self.last_raw
        };
        let deadzone = finite_or(self.cfg.deadzone, 0.0).max(0.0);
        let window = u64::from(self.cfg.release_window_ms.min(MAX_RELEASE_WINDOW_MS));
        let dt_ms = match self.last_tick {
            Some(t) if tick_ms > t => tick_ms - t,
            _ => 1,
        };
        let dt_s = dt_ms as f32 / 1000.0;
        // Forward (toward rest) speed; finite because both readings are.
        let speed = if self.last_tick.is_some() {
            (self.last_raw - raw) / dt_s
        } else {
            0.0
        };
        self.last_raw = raw;
        self.last_tick = Some(tick_ms);
        let first_tick = *self.first_tick.get_or_insert(tick_ms);
        let stable_level = self.stable_level(raw, tick_ms, deadzone);
        if let Some(level) = stable_level {
            let acquiring = !self.released_once
                && tick_ms.saturating_sub(first_tick) <= ACQUIRE_WINDOW_MS
                && level <= MAX_REST_OFFSET;
            if level < self.rest - deadzone || (acquiring && level > self.rest + deadzone) {
                self.rest = level.clamp(-MAX_REST_OFFSET, MAX_REST_OFFSET);
                self.rest_len = 0;
                self.pulled = false;
                self.qualified = false;
                self.len = 0;
            }
        }

        self.rest_hist[self.rest_head] = self.rest;
        self.rest_head = (self.rest_head + 1) % REST_LOOKBACK_TICKS;
        self.rest_len = (self.rest_len + 1).min(REST_LOOKBACK_TICKS);

        let qualify_depth = (PULL_QUALIFY_DEADZONES * deadzone).max(MIN_PULL_DEPTH);
        let mut rel = raw - self.rest;
        let mut release = None;
        if !self.pulled && rel > deadzone && tick_ms >= self.holdoff_until {
            self.pulled = true;
            self.qualified = false;
            self.len = 0;
            let oldest =
                (self.rest_head + REST_LOOKBACK_TICKS - self.rest_len) % REST_LOOKBACK_TICKS;
            self.undo_rest = self.rest_hist[oldest];
        }
        if self.pulled {
            self.push_speed(tick_ms, speed);
            if !self.qualified && rel >= qualify_depth {
                // A real pull: undo the tracking that followed its start.
                self.qualified = true;
                self.rest = self.undo_rest;
                self.rest_len = 0;
                rel = raw - self.rest;
            }
            if rel <= deadzone {
                self.pulled = false;
                let peak = self.peak_since(tick_ms.saturating_sub(window));
                self.len = 0;
                if self.qualified && peak >= self.cfg.release_min_speed {
                    release = Some(peak);
                    self.released_once = true;
                    self.holdoff_until = tick_ms.saturating_add(RELEASE_HOLDOFF_MS);
                }
                self.qualified = false;
            }
        }
        if !self.pulled && rel.abs() <= deadzone {
            let k = (finite_or(self.cfg.rest_tracking_per_s, 0.0) * dt_s).clamp(0.0, 1.0);
            self.rest =
                (self.rest + (raw - self.rest) * k).clamp(-MAX_REST_OFFSET, MAX_REST_OFFSET);
        }

        let position = if rel > deadzone {
            let span = (1.0 - self.rest - deadzone).max(1.0e-3);
            ((rel - deadzone) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        PlungerOutput { position, release }
    }

    /// Feeds the stable-signal detector; the band's mean once it has held
    /// within ±deadzone of its first reading for [`STABLE_MS`].
    fn stable_level(&mut self, raw: f32, tick: u64, deadzone: f32) -> Option<f32> {
        if self.band_count == 0 || (raw - self.band_anchor).abs() > deadzone {
            self.band_anchor = raw;
            self.band_start = tick;
            self.band_sum = 0.0;
            self.band_count = 0;
        }
        self.band_sum += f64::from(raw);
        self.band_count = self.band_count.saturating_add(1);
        (tick.saturating_sub(self.band_start) >= STABLE_MS)
            .then(|| (self.band_sum / f64::from(self.band_count)) as f32)
    }

    fn push_speed(&mut self, tick: u64, speed: f32) {
        self.speeds[self.head] = (tick, speed);
        self.head = (self.head + 1) % RING;
        self.len = (self.len + 1).min(RING);
    }

    /// Peak forward speed among the stored samples at or after `since`.
    fn peak_since(&self, since: u64) -> f32 {
        let mut peak = 0.0f32;
        for i in 0..self.len {
            let (t, s) = self.speeds[(self.head + RING - 1 - i) % RING];
            if t < since {
                break;
            }
            peak = peak.max(s);
        }
        peak
    }
}

fn finite_or(v: f32, d: f32) -> f32 {
    if v.is_finite() { v } else { d }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(f: &mut PlungerFilter, trace: &[f32], t0: u64) -> (Vec<PlungerOutput>, u64) {
        let mut out = Vec::new();
        let mut t = t0;
        for &x in trace {
            out.push(f.update(x, t));
            t += 1;
        }
        (out, t)
    }

    #[test]
    fn linear_snap_reports_exact_speed() {
        let mut f = PlungerFilter::new(PlungerConfig::default());
        let mut trace = vec![0.0; 10];
        trace.extend((0..=500).map(|i| i as f32 / 500.0)); // slow pull, 0.5 s
        trace.extend(vec![1.0; 50]);
        trace.extend((0..=20).rev().map(|i| i as f32 / 20.0)); // 20 ms snap: 50/s
        trace.extend(vec![0.0; 200]);
        let (out, _) = run(&mut f, &trace, 0);
        let rel: Vec<f32> = out.iter().filter_map(|o| o.release).collect();
        assert_eq!(rel.len(), 1);
        assert!((rel[0] - 50.0).abs() < 0.5, "{}", rel[0]);
        assert!(out.iter().any(|o| o.position == 1.0));
        assert_eq!(out.last().unwrap().position, 0.0);
    }

    #[test]
    fn slow_return_no_release() {
        let mut f = PlungerFilter::new(PlungerConfig::default());
        let mut trace: Vec<f32> = (0..=500).map(|i| i as f32 / 500.0).collect();
        trace.extend((0..=1000).rev().map(|i| i as f32 / 1000.0)); // 1/s
        let (out, _) = run(&mut f, &trace, 0);
        assert!(out.iter().all(|o| o.release.is_none()));
        assert!(!f.is_pulled());
    }

    #[test]
    fn nan_and_time_gaps() {
        let mut f = PlungerFilter::new(PlungerConfig::default());
        let o = f.update(f32::NAN, 0);
        assert_eq!(o.position, 0.0);
        let o = f.update(0.5, 0); // repeated tick: dt treated as 1 ms
        assert!(o.position > 0.4 && o.position.is_finite());
        let o = f.update(f32::INFINITY, 5);
        assert!(o.position.is_finite());
        let o = f.update(0.0, 3); // backwards tick
        assert!(o.release.is_some());
        let mut bad = PlungerFilter::new(PlungerConfig {
            deadzone: f32::NAN,
            rest_tracking_per_s: f32::NAN,
            release_min_speed: 1.0,
            release_window_ms: u32::MAX,
        });
        for t in 0..10 {
            let o = bad.update(0.0, t);
            assert_eq!(o.position, 0.0);
        }
        assert_eq!(bad.rest(), 0.0);
        assert_eq!(bad.config().release_min_speed, 1.0);
    }

    #[test]
    fn bounce_after_release_is_held_off() {
        let mut f = PlungerFilter::new(PlungerConfig::default());
        let mut trace = vec![1.0; 20];
        trace.extend([0.6, 0.2, -0.05, 0.05, 0.1, 0.0, -0.02, 0.0]);
        let (out, _) = run(&mut f, &trace, 0);
        assert_eq!(out.iter().filter(|o| o.release.is_some()).count(), 1);
    }

    #[test]
    fn ring_wraps_and_window_limits_peak() {
        let cfg = PlungerConfig {
            release_window_ms: 10,
            ..PlungerConfig::default()
        };
        let mut f = PlungerFilter::new(cfg);
        // Fast pull-out early (backward), long hold, then a 100 ms return
        // whose speed (10/s) is all that the 10 ms window sees.
        let mut trace = vec![0.0, 0.5, 1.0];
        trace.extend(vec![1.0; 2000]);
        trace.extend((0..=100).rev().map(|i| i as f32 / 100.0));
        let (out, _) = run(&mut f, &trace, 0);
        let rel: Vec<f32> = out.iter().filter_map(|o| o.release).collect();
        assert_eq!(rel.len(), 1);
        assert!((rel[0] - 10.0).abs() < 0.1, "{}", rel[0]);
    }

    #[test]
    fn pull_start_undoes_recent_tracking_and_huge_raw_is_clamped() {
        let mut f = PlungerFilter::new(PlungerConfig {
            rest_tracking_per_s: 50.0,
            ..PlungerConfig::default()
        });
        for t in 0..1000 {
            f.update(0.0, t);
        }
        // Creep through the deadzone over 40 ms: fast tracking follows it ...
        for k in 0..40u64 {
            f.update(0.0005 * k as f32, 1000 + k);
        }
        assert!(f.rest() > 0.005);
        // ... and is undone when the pull starts.
        let o = f.update(0.5, 1040);
        assert_eq!(f.rest(), 0.0);
        assert!((o.position - 0.48 / 0.98).abs() < 1e-6);
        let o = f.update(f32::MAX, 1041);
        assert_eq!(o.position, 1.0);
        let o = f.update(0.0, 1042);
        let r = o.release.unwrap();
        assert!((r - RAW_LIMIT * 1000.0).abs() < 0.01, "{r}");
    }
}
