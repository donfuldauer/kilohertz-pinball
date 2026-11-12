//! Analog plunger filter.

/// Longest supported release window; larger configured windows are clamped.
pub const MAX_RELEASE_WINDOW_MS: u32 = 1024;

const RING: usize = MAX_RELEASE_WINDOW_MS as usize;

/// When a pull starts, rest tracking of the last this-many updates is undone:
/// those samples were the start of the pull moving through the deadzone, not
/// the plunger at rest.
pub const REST_LOOKBACK_TICKS: usize = 128;

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
    /// peak forward speed is taken. Also the hold-off after a release during
    /// which bounces cannot arm a new pull. Clamped to
    /// [`MAX_RELEASE_WINDOW_MS`].
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
    /// Tracking happens while the reading is within the deadzone of the rest
    /// position; when a pull starts, the last [`REST_LOOKBACK_TICKS`] updates
    /// of tracking are undone.
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

        self.rest_hist[self.rest_head] = self.rest;
        self.rest_head = (self.rest_head + 1) % REST_LOOKBACK_TICKS;
        self.rest_len = (self.rest_len + 1).min(REST_LOOKBACK_TICKS);

        let mut rel = raw - self.rest;
        let mut release = None;
        if self.pulled {
            self.push_speed(tick_ms, speed);
            if rel <= deadzone {
                self.pulled = false;
                let peak = self.peak_since(tick_ms.saturating_sub(window));
                self.len = 0;
                if peak >= self.cfg.release_min_speed {
                    release = Some(peak);
                    self.holdoff_until = tick_ms.saturating_add(window);
                }
            }
        } else if rel > deadzone && tick_ms >= self.holdoff_until {
            self.pulled = true;
            self.len = 0;
            self.push_speed(tick_ms, speed);
            // Undo the tracking that followed the start of this pull.
            let oldest =
                (self.rest_head + REST_LOOKBACK_TICKS - self.rest_len) % REST_LOOKBACK_TICKS;
            self.rest = self.rest_hist[oldest];
            self.rest_len = 0;
            rel = raw - self.rest;
        } else if rel.abs() <= deadzone {
            let k = (finite_or(self.cfg.rest_tracking_per_s, 0.0) * dt_s).clamp(0.0, 1.0);
            self.rest += (raw - self.rest) * k;
        }

        let position = if rel > deadzone {
            let span = (1.0 - self.rest - deadzone).max(1.0e-3);
            ((rel - deadzone) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        PlungerOutput { position, release }
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
}
