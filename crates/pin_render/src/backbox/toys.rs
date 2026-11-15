//! Backbox toys: score reels and animated mechanisms driven by game state.
//!
//! GPU-free transform maths. A [`ToyRig`] maps named dynamic meshes of the
//! backbox scene to transforms:
//!
//! * **Score reels** ([`ScoreReel`]): a digit wheel shows digit `d` of a
//!   player's score when it is turned by `d · 36°` ([`reel_angle_deg`])
//!   about its axis. A new score sets each wheel's target; the wheel then
//!   turns towards it along the **shortest** way round
//!   ([`shortest_delta_deg`]) at its settle rate, so `9 → 0` is one step
//!   forward, not nine back.
//! * **Mechanisms** ([`MechChannel`]): one rotate or translate channel with
//!   an eased target: `x += (target − x)·(1 − e^(−dt/τ))` (`libm::expf`), the
//!   target clamped to the channel's range.
//!
//! Every transform is finite and bounded for any input: non-finite inputs
//! are ignored, angles stay in `[0°, 360°)`, channel values in their range.

use crate::pbr::math::{IDENTITY, Mat4, mul, translation};

/// Degrees between two adjacent digits of a reel.
pub const DIGIT_DEG: f32 = 36.0;

/// Digits on a reel.
pub const REEL_DIGITS: u32 = 10;

/// Wraps `deg` into `[0, 360)`; non-finite input gives 0.
pub fn wrap_deg(deg: f32) -> f32 {
    if !deg.is_finite() {
        return 0.0;
    }
    let w = libm::fmodf(deg, 360.0);
    let w = if w < 0.0 { w + 360.0 } else { w };
    // `-tiny + 360` can round to 360 itself.
    if w >= 360.0 { 0.0 } else { w }
}

/// The reel angle that shows digit `digit` (taken modulo 10): `digit · 36°`.
pub fn reel_angle_deg(digit: u32) -> f32 {
    (digit % REEL_DIGITS) as f32 * DIGIT_DEG
}

/// Shortest signed rotation (degrees) from `from` to `to`, in `(−180, 180]`.
pub fn shortest_delta_deg(from: f32, to: f32) -> f32 {
    let d = wrap_deg(to - from);
    if d > 180.0 { d - 360.0 } else { d }
}

/// Decimal digit `place` (0 = units) of `score`.
pub fn score_digit(score: u64, place: u32) -> u32 {
    let mut s = score;
    for _ in 0..place {
        s /= 10;
        if s == 0 {
            return 0;
        }
    }
    (s % 10) as u32
}

/// The digit a reel at `angle_deg` shows (the nearest one).
pub fn digit_at(angle_deg: f32) -> u32 {
    let k = libm::roundf(wrap_deg(angle_deg) / DIGIT_DEG) as u32;
    k % REEL_DIGITS
}

/// Rotation of `angle` radians about the unit `axis` through `pivot`, as a
/// column-major matrix. A degenerate axis gives the identity rotation.
pub fn rotation_about(axis: [f32; 3], pivot: [f32; 3], angle: f32) -> Mat4 {
    let len = libm::sqrtf(axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]);
    if !(len.is_finite() && len > 1e-6) || !angle.is_finite() {
        return IDENTITY;
    }
    let (x, y, z) = (axis[0] / len, axis[1] / len, axis[2] / len);
    let (s, c) = (libm::sinf(angle), libm::cosf(angle));
    let t = 1.0 - c;
    let r: Mat4 = [
        [t * x * x + c, t * x * y + s * z, t * x * z - s * y, 0.0],
        [t * x * y - s * z, t * y * y + c, t * y * z + s * x, 0.0],
        [t * x * z + s * y, t * y * z - s * x, t * z * z + c, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    let back = translation([-pivot[0], -pivot[1], -pivot[2]]);
    mul(&translation(pivot), &mul(&r, &back))
}

/// One digit wheel of a score display.
#[derive(Clone, Debug, PartialEq)]
pub struct ScoreReel {
    /// Names of the scene meshes that turn with this wheel (drum and digits).
    pub meshes: Vec<String>,
    /// Player whose score it shows (index into [`ToyRig::set_scores`]).
    pub player: usize,
    /// Decimal place (0 = units).
    pub place: u32,
    /// Rotation axis (unit), scene frame.
    pub axis: [f32; 3],
    /// A point on the axis, scene frame, metres.
    pub pivot: [f32; 3],
    /// Settle speed, degrees per second (> 0).
    pub settle_deg_per_s: f32,
}

/// How a mechanism channel moves its meshes.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum MechMotion {
    /// Rotation by the channel value (degrees) about `axis` through `pivot`.
    Rotate {
        /// Unit axis.
        axis: [f32; 3],
        /// Point on the axis, metres.
        pivot: [f32; 3],
    },
    /// Translation by the channel value (metres) along the unit `axis`.
    Translate {
        /// Unit axis.
        axis: [f32; 3],
    },
}

/// An animated mechanism: one eased channel.
#[derive(Clone, Debug, PartialEq)]
pub struct MechChannel {
    /// Names of the scene meshes it moves.
    pub meshes: Vec<String>,
    /// Motion.
    pub motion: MechMotion,
    /// Value range (degrees or metres); targets are clamped into it.
    pub range: (f32, f32),
    /// Easing time constant, seconds (> 0; 0 or less snaps).
    pub time_constant_s: f32,
}

#[derive(Clone, Debug)]
struct Reel {
    def: ScoreReel,
    angle: f32,
    target: f32,
}

#[derive(Clone, Debug)]
struct Channel {
    def: MechChannel,
    value: f32,
    target: f32,
}

/// Toy state: score reels and mechanism channels, advanced in simulated
/// time.
#[derive(Clone, Debug, Default)]
pub struct ToyRig {
    reels: Vec<Reel>,
    channels: Vec<Channel>,
}

fn sane_range(r: (f32, f32)) -> (f32, f32) {
    let (a, b) = (
        if r.0.is_finite() { r.0 } else { 0.0 },
        if r.1.is_finite() { r.1 } else { 0.0 },
    );
    if a <= b { (a, b) } else { (b, a) }
}

impl ToyRig {
    /// An empty rig.
    pub fn new() -> ToyRig {
        ToyRig::default()
    }

    /// Adds a reel showing 0; returns its index.
    pub fn add_reel(&mut self, reel: ScoreReel) -> usize {
        self.reels.push(Reel {
            def: reel,
            angle: 0.0,
            target: 0.0,
        });
        self.reels.len() - 1
    }

    /// Adds a mechanism channel at the low end of its range; returns its
    /// index.
    pub fn add_channel(&mut self, mut channel: MechChannel) -> usize {
        channel.range = sane_range(channel.range);
        let v = channel.range.0;
        self.channels.push(Channel {
            def: channel,
            value: v,
            target: v,
        });
        self.channels.len() - 1
    }

    /// Number of reels.
    pub fn reel_count(&self) -> usize {
        self.reels.len()
    }

    /// Number of channels.
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    /// Sets every reel's target from the players' scores (a snapshot's score
    /// list; a reel whose player is missing shows 0).
    pub fn set_scores(&mut self, scores: &[u64]) {
        for r in &mut self.reels {
            let score = scores.get(r.def.player).copied().unwrap_or(0);
            r.target = reel_angle_deg(score_digit(score, r.def.place));
        }
    }

    /// Jumps every reel to its target (no settle animation).
    pub fn snap_reels(&mut self) {
        for r in &mut self.reels {
            r.angle = r.target;
        }
    }

    /// Sets channel `i`'s target (clamped into its range; non-finite or an
    /// unknown index is ignored).
    pub fn set_channel(&mut self, i: usize, target: f32) {
        if let Some(c) = self.channels.get_mut(i)
            && target.is_finite()
        {
            c.target = target.clamp(c.def.range.0, c.def.range.1);
        }
    }

    /// Advances the animations by `dt` seconds of simulated time (a
    /// non-finite or negative `dt` does nothing).
    pub fn advance(&mut self, dt: f32) {
        if !(dt.is_finite() && dt > 0.0) {
            return;
        }
        for r in &mut self.reels {
            let delta = shortest_delta_deg(r.angle, r.target);
            let rate = r.def.settle_deg_per_s;
            let step = if rate.is_finite() && rate > 0.0 {
                rate * dt
            } else {
                f32::INFINITY
            };
            r.angle = if delta.abs() <= step {
                r.target
            } else {
                wrap_deg(r.angle + step.copysign(delta))
            };
        }
        for c in &mut self.channels {
            let tau = c.def.time_constant_s;
            let k = if tau.is_finite() && tau > 0.0 {
                1.0 - libm::expf(-dt / tau)
            } else {
                1.0
            };
            let v = c.value + (c.target - c.value) * k;
            // Snap once the easing step no longer moves the value in f32
            // (or is within a relative 1e-6), so channels do settle.
            let near = (c.target - v).abs() <= 1.0e-6 * (1.0 + c.target.abs());
            let v = if v == c.value || near { c.target } else { v };
            c.value = v.clamp(c.def.range.0, c.def.range.1);
        }
    }

    /// Whether every reel shows its target and every channel is within
    /// `1e-4` of its target.
    pub fn settled(&self) -> bool {
        self.reels.iter().all(|r| r.angle == r.target)
            && self
                .channels
                .iter()
                .all(|c| (c.value - c.target).abs() <= 1e-4)
    }

    /// Current angle of reel `i`, degrees in `[0, 360)` (0 for an unknown
    /// reel).
    pub fn reel_angle(&self, i: usize) -> f32 {
        self.reels.get(i).map_or(0.0, |r| r.angle)
    }

    /// Current value of channel `i` (0 for an unknown channel).
    pub fn channel_value(&self, i: usize) -> f32 {
        self.channels.get(i).map_or(0.0, |c| c.value)
    }

    /// Transform of reel `i` (identity for an unknown reel).
    pub fn reel_transform(&self, i: usize) -> Mat4 {
        match self.reels.get(i) {
            Some(r) => rotation_about(r.def.axis, r.def.pivot, r.angle.to_radians()),
            None => IDENTITY,
        }
    }

    /// Transform of channel `i` (identity for an unknown channel).
    pub fn channel_transform(&self, i: usize) -> Mat4 {
        match self.channels.get(i) {
            Some(c) => match c.def.motion {
                MechMotion::Rotate { axis, pivot } => {
                    rotation_about(axis, pivot, c.value.to_radians())
                }
                MechMotion::Translate { axis } => {
                    let len =
                        libm::sqrtf(axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]);
                    if len.is_finite() && len > 1e-6 {
                        let k = c.value / len;
                        translation([axis[0] * k, axis[1] * k, axis[2] * k])
                    } else {
                        IDENTITY
                    }
                }
            },
            None => IDENTITY,
        }
    }

    /// Calls `f(mesh name, transform)` for every mesh of every reel, then of
    /// every channel, in insertion order.
    pub fn for_each_transform(&self, mut f: impl FnMut(&str, &Mat4)) {
        for (i, r) in self.reels.iter().enumerate() {
            let m = self.reel_transform(i);
            for name in &r.def.meshes {
                f(name, &m);
            }
        }
        for (i, c) in self.channels.iter().enumerate() {
            let m = self.channel_transform(i);
            for name in &c.def.meshes {
                f(name, &m);
            }
        }
    }
}
