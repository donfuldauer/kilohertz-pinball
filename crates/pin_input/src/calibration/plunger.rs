//! Analog plunger filter.
//!
//! # Model
//!
//! * **Position** is measured from the tracked rest: `(raw − rest − dz) /
//!   (1 − rest − dz)` clamped to `0..=1`, 0 within the deadzone or below rest.
//! * **Pull.** A pull starts when `raw − rest > deadzone`. It can end in a
//!   release only if it *qualifies*: it started after the plunger had been at
//!   rest (within ±deadzone) for [`ARM_REST_MS`], it reached
//!   `max(PULL_QUALIFY_DEADZONES · deadzone, MIN_PULL_DEPTH)`, and it lasted
//!   at least [`MIN_PULL_TICKS`] updates. The release is reported on the tick
//!   the plunger is back within the deadzone (overshoot past rest counts),
//!   with the peak forward speed of the last `release_window_ms`, if that peak
//!   is at least `release_min_speed`. After a release no pull starts for
//!   [`RELEASE_HOLDOFF_MS`].
//! * **Rest** (only while `rest_tracking_per_s > 0`; with 0 the rest is fixed
//!   at 0):
//!   - tracking: while not pulled and within ±deadzone, rest follows the
//!     reading at `rest_tracking_per_s`; when a pull qualifies, the tracking
//!     of the [`REST_LOOKBACK_TICKS`] updates before it started is undone;
//!   - re-acquisition: a reading that stays within a band one deadzone wide
//!     for [`STABLE_MS`] becomes the rest when it lies more than a deadzone
//!     *above* rest and is either a small offset (less than the qualifying
//!     depth), or a *known-good* rest, or at most [`MAX_REST_OFFSET`] during
//!     the power-on window ([`ACQUIRE_WINDOW_MS`] after the first update,
//!     before the first release); or when it lies more than a deadzone
//!     *below* rest and is known-good. Known-good rests: nominal 0, the last
//!     rest a release returned to or that was acquired upward, and the rest
//!     the last downward re-acquisition moved away from (tolerance
//!     `max(2·deadzone, 0.01)`). Any other level below rest is the knob held
//!     pushed in: it reads 0 and becomes the rest only after
//!     [`PUSH_IN_REACQUIRE_MS`]; the previous rest is remembered and restored
//!     after [`STABLE_MS`] back there. Levels beyond
//!     `±MAX_REST_OFFSET` never become the rest. Re-acquisition cancels a
//!     pull in progress without a release;
//!   - rest always stays within `±MAX_REST_OFFSET`.
//!
//! A rest that is too *high* never blocks a launch (the return still comes
//! back within the deadzone); a rest that is too low would, which is why
//! lowering it needs a long push-in and raising it back to a known-good rest
//! needs only [`STABLE_MS`].

/// Longest supported release window; larger configured windows are clamped.
pub const MAX_RELEASE_WINDOW_MS: u32 = 1024;

const RING: usize = MAX_RELEASE_WINDOW_MS as usize;

/// When a pull qualifies, rest tracking of the last this-many updates before
/// it started is undone.
pub const REST_LOOKBACK_TICKS: usize = 128;

/// Qualifying depth factor; see the module docs.
pub const PULL_QUALIFY_DEADZONES: f32 = 2.5;

/// Minimum qualifying depth (full-scale); see the module docs.
pub const MIN_PULL_DEPTH: f32 = 0.05;

/// A qualified pull must have lasted this many updates (outside the
/// deadzone) before its return is a release: one- and two-sample spikes are
/// noise, a 3 ms tap is the fastest real pull.
pub const MIN_PULL_TICKS: u32 = 3;

/// A pull can qualify only if the plunger was at rest this long (ms) before.
pub const ARM_REST_MS: u64 = 50;

/// After a release no new pull starts for this long (ms): the spring bounce.
pub const RELEASE_HOLDOFF_MS: u64 = 200;

/// The rest position stays within `±MAX_REST_OFFSET` of nominal 0.
pub const MAX_REST_OFFSET: f32 = 0.25;

/// A reading held within ±deadzone of one level this long (ms) is stable.
pub const STABLE_MS: u64 = 1000;

/// A stable reading below rest that is not a known-good rest (the knob held
/// pushed in) becomes the rest only after this long (ms).
pub const PUSH_IN_REACQUIRE_MS: u64 = 5000;

/// Power-on acquisition window (ms after the first update, before the first
/// release).
pub const ACQUIRE_WINDOW_MS: u64 = 3000;

/// Time constant (s) of the level average used by re-acquisition.
const LEVEL_TAU_S: f32 = 0.05;

/// Raw readings are clamped to `±RAW_LIMIT` full-scale.
pub const RAW_LIMIT: f32 = 16.0;

/// Plunger calibration settings. Positions are in full-scale units (the
/// normalised `CabinetAxis::Plunger` value, 0 = nominal rest, 1 = full pull).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PlungerConfig {
    /// Displacement from the tracked rest position treated as "at rest".
    pub deadzone: f32,
    /// Rest tracking rate (1/s). 0 disables all rest adaptation.
    pub rest_tracking_per_s: f32,
    /// Minimum peak forward speed (full-scale/s) of the return stroke that
    /// counts as a release; slower returns report nothing.
    pub release_min_speed: f32,
    /// How far back (ms) from the tick the plunger re-enters the deadzone the
    /// peak forward speed is taken. Clamped to [`MAX_RELEASE_WINDOW_MS`].
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
    /// Pull, 0 at rest .. 1 full pull, with the deadzone removed.
    pub position: f32,
    /// On the single tick a qualified pull ends with a fast return: the peak
    /// forward speed of the return stroke (full-scale/s).
    pub release: Option<f32>,
}

/// Analog plunger conditioning (see the module docs). Call
/// [`PlungerFilter::update`] once per 1 ms sim tick.
#[derive(Clone, Debug)]
pub struct PlungerFilter {
    cfg: PlungerConfig,
    rest: f32,
    last_raw: f32,
    last_tick: Option<u64>,
    first_tick: Option<u64>,
    released_once: bool,
    // Pull state.
    pulled: bool,
    armed: bool,
    qualified: bool,
    pull_ticks: u32,
    undo_rest: f32,
    holdoff_until: u64,
    /// Tick since which the plunger has been continuously at rest.
    rest_since: Option<u64>,
    // Known-good rests.
    anchor: f32,
    before_push_in: Option<f32>,
    // Stable-signal detector.
    band_ready: bool,
    band_min: f32,
    band_max: f32,
    band_start: u64,
    /// Short exponential average of the reading (the stable level).
    level: f32,
    // Forward speeds of the current pull: `(tick, speed)` ring.
    speeds: [(u64, f32); RING],
    head: usize,
    len: usize,
    // Rest before each of the last updates.
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
            first_tick: None,
            released_once: false,
            pulled: false,
            armed: false,
            qualified: false,
            pull_ticks: 0,
            undo_rest: 0.0,
            holdoff_until: 0,
            rest_since: None,
            anchor: 0.0,
            before_push_in: None,
            band_ready: false,
            band_min: 0.0,
            band_max: 0.0,
            band_start: 0,
            level: 0.0,
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

    /// The rest position (raw units); see the module docs for how it moves.
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
        let rate = finite_or(self.cfg.rest_tracking_per_s, 0.0).max(0.0);
        let qualify_depth = (PULL_QUALIFY_DEADZONES * deadzone).max(MIN_PULL_DEPTH);
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

        self.rest_hist[self.rest_head] = self.rest;
        self.rest_head = (self.rest_head + 1) % REST_LOOKBACK_TICKS;
        self.rest_len = (self.rest_len + 1).min(REST_LOOKBACK_TICKS);

        if rate > 0.0 {
            let (level, held_ms) = self.stable_level(raw, tick_ms, dt_s, deadzone);
            let power_on =
                !self.released_once && tick_ms.saturating_sub(first_tick) <= ACQUIRE_WINDOW_MS;
            self.reacquire(level, held_ms, tick_ms, deadzone, qualify_depth, power_on);
        }

        let mut rel = raw - self.rest;
        let mut release = None;
        if !self.pulled && rel > deadzone && tick_ms >= self.holdoff_until {
            self.pulled = true;
            self.qualified = false;
            self.pull_ticks = 0;
            self.len = 0;
            self.armed = self
                .rest_since
                .is_some_and(|s| tick_ms.saturating_sub(s) >= ARM_REST_MS);
            self.rest_since = None;
            let oldest =
                (self.rest_head + REST_LOOKBACK_TICKS - self.rest_len) % REST_LOOKBACK_TICKS;
            self.undo_rest = self.rest_hist[oldest];
        }
        if self.pulled {
            self.push_speed(tick_ms, speed);
            if !self.qualified && self.armed && rel >= qualify_depth {
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
                if self.qualified
                    && self.pull_ticks >= MIN_PULL_TICKS
                    && peak >= self.cfg.release_min_speed
                {
                    release = Some(peak);
                    self.released_once = true;
                    self.anchor = self.rest;
                    self.before_push_in = None;
                    self.holdoff_until = tick_ms.saturating_add(RELEASE_HOLDOFF_MS);
                }
                self.qualified = false;
            } else {
                self.pull_ticks = self.pull_ticks.saturating_add(1);
            }
        }
        if !self.pulled {
            if rel.abs() <= deadzone {
                self.rest_since.get_or_insert(tick_ms);
                let k = (rate * dt_s).clamp(0.0, 1.0);
                self.rest =
                    (self.rest + (raw - self.rest) * k).clamp(-MAX_REST_OFFSET, MAX_REST_OFFSET);
            } else {
                self.rest_since = None;
            }
        }

        let position = if rel > deadzone {
            let span = (1.0 - self.rest - deadzone).max(1.0e-3);
            ((rel - deadzone) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        PlungerOutput { position, release }
    }

    /// Moves the rest to a stable `level` when the rules in the module docs
    /// allow it.
    fn reacquire(
        &mut self,
        level: f32,
        held_ms: u64,
        tick: u64,
        deadzone: f32,
        qualify_depth: f32,
        power_on: bool,
    ) {
        if held_ms < STABLE_MS || level.abs() > MAX_REST_OFFSET {
            return;
        }
        let above = level > self.rest + deadzone;
        let below = level < self.rest - deadzone;
        if !above && !below {
            return;
        }
        let tol = (2.0 * deadzone).max(0.01);
        let known_good = |x: f32| {
            x.abs() <= tol
                || (x - self.anchor).abs() <= tol
                || self.before_push_in.is_some_and(|r| (x - r).abs() <= tol)
        };
        let level_good = known_good(level);
        let down = below && (level_good || held_ms >= PUSH_IN_REACQUIRE_MS);
        let up = above && (level_good || level - self.rest < qualify_depth || power_on);
        if down {
            // Remember the rest it moved down from, so it can be restored.
            self.before_push_in = Some(self.rest);
        } else if up {
            self.anchor = level;
            self.before_push_in = None;
        } else {
            return;
        }
        self.rest = level;
        self.rest_len = 0;
        self.pulled = false;
        self.qualified = false;
        self.len = 0;
        self.rest_since = Some(tick);
    }

    /// Feeds the stable-signal detector: the current level (a 50 ms
    /// exponential average) and how long (ms) every reading has stayed within
    /// a band one deadzone wide.
    fn stable_level(&mut self, raw: f32, tick: u64, dt_s: f32, deadzone: f32) -> (f32, u64) {
        if !self.band_ready {
            self.band_ready = true;
            self.level = raw;
            self.band_min = raw;
            self.band_max = raw;
            self.band_start = tick;
        }
        self.level += (raw - self.level) * (dt_s / LEVEL_TAU_S).min(1.0);
        let lo = self.band_min.min(raw);
        let hi = self.band_max.max(raw);
        if hi - lo > deadzone {
            self.band_min = raw;
            self.band_max = raw;
            self.band_start = tick;
        } else {
            self.band_min = lo;
            self.band_max = hi;
        }
        (self.level, tick.saturating_sub(self.band_start))
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
        let mut trace = vec![0.0; 50];
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
        let o = f.update(0.0, 3); // backwards tick: dt counts as 1 ms
        assert!(o.position == 0.0 && o.release.is_none_or(f32::is_finite));
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
        let mut trace = vec![0.0; 50];
        trace.extend(vec![1.0; 20]);
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
        let mut trace = vec![0.0; 50];
        trace.extend([0.5, 1.0]);
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
        f.update(f32::MAX, 1042);
        let o = f.update(0.0, 1043);
        let r = o.release.unwrap();
        assert!((r - RAW_LIMIT * 1000.0).abs() < 0.01, "{r}");
    }
}
