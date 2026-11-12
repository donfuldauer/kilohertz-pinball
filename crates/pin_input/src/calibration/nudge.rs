//! Accelerometer nudge filter and the button / thumbstick fallbacks.

use crate::profiles::NudgeDirection;

/// Accelerometer nudge settings.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct NudgeConfig {
    /// High-pass cut-off (Hz). Constant accelerations (gravity on a tilted
    /// sensor, mounting offsets) decay with time constant `1 / (2π·hz)`.
    /// `<= 0` or non-finite disables the high-pass.
    pub high_pass_hz: f32,
    /// Gain of the output curve.
    pub gain: f32,
    /// Exponent of the output curve (1 = linear).
    pub curve_exponent: f32,
    /// High-passed magnitudes at or below this are zeroed (per axis).
    pub deadzone: f32,
    /// Output clamp.
    pub max: f32,
}

impl Default for NudgeConfig {
    fn default() -> Self {
        Self {
            high_pass_hz: 2.0,
            gain: 1.0,
            curve_exponent: 1.0,
            deadzone: 0.01,
            max: 1.0,
        }
    }
}

/// Accelerometer nudge: per-axis first-order high-pass, then deadzone, then
/// the gain curve `out = sign(x)·min(max, gain·|x|^curve_exponent)`.
///
/// The high-pass is the bilinear-transform (Tustin) discretisation of
/// `H(s) = s / (s + ωc)`: `y = a·y' + b·(x − x')` with
/// `a = (2RC − dt)/(2RC + dt)`, `b = 2RC/(2RC + dt)`, `RC = 1/(2π·hz)`. Its
/// step response decays at least as fast as `e^(−t/RC)`, and its pass-band
/// gain stays within 1 % of the analog filter well past 10× the cut-off. The
/// filter starts from zero input, so a constant reading is a step at the
/// first update.
#[derive(Clone, Debug)]
pub struct NudgeFilter {
    cfg: NudgeConfig,
    x_prev: [f32; 2],
    y: [f32; 2],
    out: [f32; 2],
}

impl NudgeFilter {
    /// A filter at rest (zero input history).
    pub fn new(cfg: NudgeConfig) -> Self {
        Self {
            cfg,
            x_prev: [0.0; 2],
            y: [0.0; 2],
            out: [0.0; 2],
        }
    }

    /// The configuration.
    pub fn config(&self) -> &NudgeConfig {
        &self.cfg
    }

    /// The high-passed signal before deadzone and curve (last update).
    pub fn high_passed(&self) -> [f32; 2] {
        self.y
    }

    /// Feeds one accelerometer sample `[x, y]` taken `dt_s` seconds after the
    /// previous one; returns the nudge in `-max..=max`. A non-finite
    /// component repeats the previous input; a non-finite or non-positive
    /// `dt_s` returns the previous output unchanged.
    pub fn update(&mut self, accel: [f32; 2], dt_s: f32) -> [f32; 2] {
        if !(dt_s.is_finite() && dt_s > 0.0) {
            return self.out;
        }
        let hz = self.cfg.high_pass_hz;
        let (a, b) = if hz.is_finite() && hz > 0.0 {
            let rc2 = 2.0 / (2.0 * core::f32::consts::PI * hz);
            (
                // Clamped at 0: for dt > 2RC (a stall) the bilinear pole goes
                // negative and would ring; 0 just forgets the history.
                ((rc2 - dt_s) / (rc2 + dt_s)).max(0.0),
                rc2 / (rc2 + dt_s),
            )
        } else {
            (1.0, 1.0)
        };
        for (i, &raw) in accel.iter().enumerate() {
            let x = if raw.is_finite() { raw } else { self.x_prev[i] };
            let mut y = a * self.y[i] + b * (x - self.x_prev[i]);
            if !y.is_finite() {
                y = 0.0;
            }
            self.x_prev[i] = x;
            self.y[i] = y;
            self.out[i] = self.curve(y);
        }
        self.out
    }

    /// Deadzone and gain curve for one high-passed value.
    fn curve(&self, x: f32) -> f32 {
        let max = finite_or(self.cfg.max, 1.0).abs();
        let mag = x.abs();
        if mag <= finite_or(self.cfg.deadzone, 0.0) {
            return 0.0;
        }
        let e = finite_or(self.cfg.curve_exponent, 1.0);
        let shaped = if e == 1.0 { mag } else { libm::powf(mag, e) };
        let v = finite_or(self.cfg.gain, 0.0) * shaped;
        let v = if v.is_nan() { 0.0 } else { v.clamp(-max, max) };
        if x < 0.0 { -v } else { v }
    }

    /// Clears the filter history (as after [`NudgeFilter::new`]).
    pub fn reset(&mut self) {
        self.x_prev = [0.0; 2];
        self.y = [0.0; 2];
        self.out = [0.0; 2];
    }
}

fn finite_or(v: f32, d: f32) -> f32 {
    if v.is_finite() { v } else { d }
}

/// Most presses a [`DigitalNudge`] keeps at once; a further press replaces
/// the oldest.
pub const MAX_DIGITAL_PRESSES: usize = 16;

/// Nudge from buttons: each press produces a constant vector
/// (`Left = (−s, 0)`, `Right = (s, 0)`, `Forward = (0, s)`) for `duration_ms`
/// ticks starting at the press tick; overlapping presses add, and each
/// component is clamped to `±1`.
#[derive(Clone, Debug)]
pub struct DigitalNudge {
    strength: f32,
    duration_ms: u32,
    presses: [Option<(NudgeDirection, u64)>; MAX_DIGITAL_PRESSES],
}

impl DigitalNudge {
    /// A fallback with this per-press strength and duration. A non-finite
    /// strength counts as 0.
    pub fn new(strength: f32, duration_ms: u32) -> Self {
        Self {
            strength: finite_or(strength, 0.0),
            duration_ms,
            presses: [None; MAX_DIGITAL_PRESSES],
        }
    }

    /// Registers a press at `tick_ms`.
    pub fn press(&mut self, dir: NudgeDirection, tick_ms: u64) {
        let dur = u64::from(self.duration_ms);
        // Reuse a free or expired slot, else replace the oldest press.
        let mut slot = 0;
        let mut oldest = u64::MAX;
        for (i, p) in self.presses.iter().enumerate() {
            match p {
                None => {
                    slot = i;
                    break;
                }
                Some((_, t)) if t.saturating_add(dur) <= tick_ms => {
                    slot = i;
                    break;
                }
                Some((_, t)) if *t < oldest => {
                    oldest = *t;
                    slot = i;
                }
                Some(_) => {}
            }
        }
        self.presses[slot] = Some((dir, tick_ms));
    }

    /// The nudge vector `[x, y]` at `tick_ms`.
    pub fn sample(&self, tick_ms: u64) -> [f32; 2] {
        let dur = u64::from(self.duration_ms);
        let s = self.strength;
        let mut v = [0.0f32; 2];
        for (dir, t) in self.presses.iter().flatten() {
            if *t <= tick_ms && tick_ms < t.saturating_add(dur) {
                match dir {
                    NudgeDirection::Left => v[0] -= s,
                    NudgeDirection::Right => v[0] += s,
                    NudgeDirection::Forward => v[1] += s,
                }
            }
        }
        [v[0].clamp(-1.0, 1.0), v[1].clamp(-1.0, 1.0)]
    }

    /// Forgets all presses.
    pub fn clear(&mut self) {
        self.presses = [None; MAX_DIGITAL_PRESSES];
    }
}

/// Thumbstick nudge with a radial deadzone: inside radius `deadzone` the
/// result is 0; outside, the radius is rescaled from `deadzone..=1` to `0..=1`
/// keeping the direction (inputs beyond radius 1 are treated as radius 1).
/// NaN components count as 0.
pub fn thumbstick_nudge(x: f32, y: f32, deadzone: f32) -> [f32; 2] {
    let x = if x.is_finite() { x } else { 0.0 };
    let y = if y.is_finite() { y } else { 0.0 };
    let dz = if deadzone.is_finite() {
        deadzone.max(0.0)
    } else {
        0.0
    };
    if dz >= 1.0 {
        return [0.0, 0.0];
    }
    let r = (x * x + y * y).sqrt();
    if r <= dz || r == 0.0 || !r.is_finite() {
        return [0.0, 0.0];
    }
    let scaled = (r.min(1.0) - dz) / (1.0 - dz);
    let k = scaled / r;
    [(x * k).clamp(-1.0, 1.0), (y * k).clamp(-1.0, 1.0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linear() -> NudgeConfig {
        NudgeConfig {
            high_pass_hz: 2.0,
            gain: 1.0,
            curve_exponent: 1.0,
            deadzone: 0.0,
            max: 1.0,
        }
    }

    #[test]
    fn step_decays_within_five_time_constants() {
        for dt in [0.001f32, 0.004, 1.0 / 60.0] {
            let mut f = NudgeFilter::new(linear());
            let first = f.update([0.5, -0.5], dt)[0];
            assert!(first > 0.45, "{first}");
            let tau = 1.0 / (2.0 * core::f32::consts::PI * 2.0);
            let n = (5.0 * tau / dt).ceil() as usize;
            let mut last = [0.0; 2];
            for _ in 0..n {
                last = f.update([0.5, -0.5], dt);
            }
            assert!(last[0].abs() < 0.005 && last[1].abs() < 0.005, "{last:?}");
        }
    }

    #[test]
    fn long_gap_does_not_ring() {
        let mut f = NudgeFilter::new(linear());
        f.update([0.5, 0.0], 0.001);
        let mut prev = f32::INFINITY;
        for _ in 0..10 {
            let o = f.update([0.5, 0.0], 1.0); // stalled for 1 s each time
            assert!(o[0] >= 0.0 && o[0] <= prev, "{o:?}");
            prev = o[0];
        }
        assert_eq!(prev, 0.0);
    }

    #[test]
    fn curve_deadzone_and_clamp() {
        let mut f = NudgeFilter::new(NudgeConfig {
            high_pass_hz: 0.0,
            gain: 2.0,
            curve_exponent: 2.0,
            deadzone: 0.1,
            max: 0.6,
        });
        assert_eq!(f.update([0.05, -0.05], 0.001), [0.0, 0.0]);
        let o = f.update([0.3, -0.3], 0.001);
        assert!(
            (o[0] - 0.18).abs() < 1e-5 && (o[1] + 0.18).abs() < 1e-5,
            "{o:?}"
        );
        assert_eq!(f.update([0.9, -0.9], 0.001), [0.6, -0.6]);
        assert_eq!(f.high_passed(), [0.9, -0.9]);
        // Invalid dt keeps the previous output; NaN input repeats the last.
        assert_eq!(f.update([0.0, 0.0], f32::NAN), [0.6, -0.6]);
        assert_eq!(f.update([0.0, 0.0], 0.0), [0.6, -0.6]);
        assert_eq!(f.update([f32::NAN, f32::INFINITY], 0.001), [0.6, -0.6]);
        f.reset();
        assert_eq!(f.high_passed(), [0.0, 0.0]);
        assert_eq!(f.config().max, 0.6);
    }

    #[test]
    fn non_finite_config_is_tamed() {
        let mut f = NudgeFilter::new(NudgeConfig {
            high_pass_hz: f32::NAN,
            gain: f32::NAN,
            curve_exponent: f32::NAN,
            deadzone: f32::NAN,
            max: f32::NAN,
        });
        assert_eq!(f.update([0.5, -0.5], 0.001), [0.0, 0.0]);
        let mut g = NudgeFilter::new(NudgeConfig {
            gain: f32::MAX,
            ..linear()
        });
        let o = g.update([f32::MAX, -f32::MAX], 0.001);
        assert_eq!(o, [1.0, -1.0]);
        let o = g.update([-f32::MAX, f32::MAX], 0.001);
        assert!(o[0].is_finite() && o[1].is_finite());
    }

    #[test]
    fn digital_slots_recycle() {
        let mut d = DigitalNudge::new(0.1, 5);
        for t in 0..40 {
            d.press(NudgeDirection::Right, t);
        }
        // 5 presses overlap at any tick.
        assert!((d.sample(39)[0] - 0.5).abs() < 1e-6);
        let mut e = DigitalNudge::new(0.1, 1000);
        for _ in 0..(MAX_DIGITAL_PRESSES + 4) {
            e.press(NudgeDirection::Left, 7);
        }
        assert_eq!(e.sample(7), [-1.0, 0.0]);
        e.clear();
        assert_eq!(e.sample(7), [0.0, 0.0]);
        assert_eq!(DigitalNudge::new(f32::NAN, 10).sample(0), [0.0, 0.0]);
    }

    #[test]
    fn thumbstick_edges() {
        assert_eq!(thumbstick_nudge(0.5, 0.5, 1.0), [0.0, 0.0]);
        assert_eq!(thumbstick_nudge(0.0, 0.0, 0.0), [0.0, 0.0]);
        assert_eq!(thumbstick_nudge(f32::NAN, 1.0, f32::NAN), [0.0, 1.0]);
        let o = thumbstick_nudge(3.0, 4.0, 0.2);
        assert!((o[0] - 0.6).abs() < 1e-6 && (o[1] - 0.8).abs() < 1e-6);
    }
}
