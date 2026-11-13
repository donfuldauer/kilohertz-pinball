//! The SSF voice mixer: haptic events → interleaved 7.1 `f32`.

use super::{CH_BL, CH_BR, CH_SL, CH_SR, CHANNELS, SsfPanner, mix_stereo_front};
use crate::mixer::Sample;
use pin_core::haptics::{HapticEvent, HapticKind};
use pin_core::snapshot::MAX_BALLS;

/// Roll voices: one per ball slot.
pub const MAX_ROLL_VOICES: usize = MAX_BALLS;
/// Impulse voices playing at once (the oldest is stolen beyond this).
pub const MAX_IMPULSE_VOICES: usize = 16;

/// Samples in one cycle of the built-in sine table.
const TABLE_LEN: usize = 1024;
/// Impulse voices stop below this amplitude (−80 dB).
const SILENCE: f32 = 1e-4;
/// Exciter channels in [`SsfPanner::corner_gains`] order.
const CORNERS: [usize; 4] = [CH_SL, CH_SR, CH_BL, CH_BR];

/// SSF mixer settings (see [`Default`] for the planned values).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SsfConfig {
    /// Output rate in Hz (48 000; the same as the 3.3 mixer).
    pub sample_rate: u32,
    /// Playfield rectangle `[x_min, x_max, y_min, y_max]` in mm
    /// (`[−257, 257, 0, 1070]`).
    pub playfield_mm: [f32; 4],
    /// Gain applied to the exciter sum (1.0).
    pub master_gain: f32,
    /// Roll loop gain at full speed and reference mass (0.6).
    pub roll_gain: f32,
    /// Speed at which the roll loop reaches full gain and top pitch (3 m/s).
    pub roll_full_speed_m_s: f32,
    /// Mass at which the mass factor is 1 (0.0806 kg, a 1-1/16" steel ball).
    pub roll_ref_mass_kg: f32,
    /// Frequency of the built-in roll loop at pitch 1 (35 Hz).
    pub roll_base_hz: f32,
    /// Roll pitch ratio at rest (0.6).
    pub roll_pitch_min: f32,
    /// Roll pitch ratio at full speed (1.8).
    pub roll_pitch_max: f32,
    /// A roll voice fades out when its ball sent no roll event for this
    /// long (40 ms).
    pub roll_hold_ms: f32,
    /// Ramp length of every roll gain/pitch/pan change (10 ms).
    pub roll_ramp_ms: f32,
    /// Frequency of an impulse thump (45 Hz).
    pub impulse_hz: f32,
    /// Exponential decay time constant of an impulse (60 ms).
    pub impulse_decay_ms: f32,
    /// Peak gain of a coil fire at strength 1 (0.8).
    pub coil_gain: f32,
    /// Peak gain of an EOS hit (0.5).
    pub eos_gain: f32,
    /// Peak gain of a bumper (1.0).
    pub bumper_gain: f32,
    /// Peak gain of a ball drop (0.9).
    pub drop_gain: f32,
    /// Peak gain of a generic impulse (0.7).
    pub impulse_gain: f32,
}

impl Default for SsfConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            playfield_mm: [-257.0, 257.0, 0.0, 1070.0],
            master_gain: 1.0,
            roll_gain: 0.6,
            roll_full_speed_m_s: 3.0,
            roll_ref_mass_kg: 0.0806,
            roll_base_hz: 35.0,
            roll_pitch_min: 0.6,
            roll_pitch_max: 1.8,
            roll_hold_ms: 40.0,
            roll_ramp_ms: 10.0,
            impulse_hz: 45.0,
            impulse_decay_ms: 60.0,
            coil_gain: 0.8,
            eos_gain: 0.5,
            bumper_gain: 1.0,
            drop_gain: 0.9,
            impulse_gain: 0.7,
        }
    }
}

/// `0..=1`, NaN → 0.
#[inline]
fn unit(v: f32) -> f32 {
    if v > 0.0 { v.min(1.0) } else { 0.0 }
}

/// Finite and `≥ 0`, else `fallback`.
#[inline]
fn non_neg(v: f32, fallback: f32) -> f32 {
    if v.is_finite() && v >= 0.0 {
        v
    } else {
        fallback
    }
}

/// Milliseconds → whole frames (≥ 1).
fn ms_frames(ms: f32, rate: u32) -> u32 {
    let f = f64::from(non_neg(ms, 0.0)) * f64::from(rate) / 1000.0;
    // Saturating float → int cast; at least one frame.
    (f.round() as u32).max(1)
}

/// A value that moves linearly to a target over a fixed number of frames.
#[derive(Copy, Clone, Debug, Default)]
struct Ramp {
    cur: f64,
    target: f64,
    step: f64,
    left: u32,
}

impl Ramp {
    fn set(&mut self, target: f64, frames: u32) {
        self.target = target;
        self.left = frames;
        self.step = (target - self.cur) / f64::from(frames);
    }

    fn jump(&mut self, v: f64) {
        *self = Ramp {
            cur: v,
            target: v,
            step: 0.0,
            left: 0,
        };
    }

    #[inline]
    fn tick(&mut self) -> f64 {
        let v = self.cur;
        if self.left > 0 {
            self.left -= 1;
            self.cur = if self.left == 0 {
                self.target
            } else {
                self.cur + self.step
            };
        }
        v
    }

    fn settled_at(&self, v: f64) -> bool {
        self.left == 0 && self.cur == v
    }
}

#[derive(Copy, Clone, Debug, Default)]
struct RollVoice {
    active: bool,
    phase: f64,
    gain: Ramp,
    inc: Ramp,
    pan: [Ramp; 4],
    /// Frames since the last roll event.
    since: u64,
    fading: bool,
}

#[derive(Copy, Clone, Debug, Default)]
struct ImpulseVoice {
    active: bool,
    phase: f64,
    amp: f32,
    pan: [f32; 4],
    seq: u64,
}

/// Renders haptic events into interleaved 7.1 buffers for playfield
/// exciters.
///
/// # Voice model
/// * **Ball roll** ([`HapticKind::BallRoll`]): one looping voice per ball
///   slot (`ball < MAX_ROLL_VOICES`; others count in
///   [`ignored_events`](Self::ignored_events)). With
///   `s = clamp(speed / roll_full_speed, 0, 1)` and
///   `m = clamp(mass / roll_ref_mass, 0, 2)`, the loop's gain is
///   `roll_gain · s · m` and its pitch ratio
///   `roll_pitch_min + (roll_pitch_max − roll_pitch_min) · s` (the event
///   strength is not used: speed and mass carry the information). The loop
///   is a sine at `roll_base_hz` or, after [`set_roll_loop`](Self::set_roll_loop),
///   a mono sample played at the pitch ratio. It is panned with
///   [`SsfPanner`] at the ball's `(x, y)`. Gain, pitch and pan move
///   linearly to each new value over `roll_ramp_ms` (a new voice starts at
///   its pan and pitch, its gain ramps up from 0). With no event for
///   `roll_hold_ms` the gain ramps to 0 and the voice stops.
/// * **Impulses** (coil fire, EOS hit, bumper, ball drop, generic impulse):
///   a sine thump at `impulse_hz` starting at phase 0 with amplitude
///   `class_gain · clamp(strength, 0, 1)`, decaying by
///   `exp(−1 / (impulse_decay_ms · rate / 1000))` per frame and stopping
///   below −80 dB. Ball drops are panned at their position; other kinds at
///   the position registered with [`set_source_position`](Self::set_source_position)
///   for their [`HapticEvent::source`], or at the playfield centre. When all
///   [`MAX_IMPULSE_VOICES`] play, the oldest is stolen.
/// * Events take effect at frame 0 of the next [`render`](Self::render).
///   FL, FR, FC and LFE stay silent; the exciter channels are scaled by
///   `master_gain` and hard-clipped to `[−1, 1]` (NaN → 0).
///
/// [`feed`](Self::feed) and the render calls never allocate; the
/// constructor, [`set_roll_loop`](Self::set_roll_loop) and
/// [`set_source_position`](Self::set_source_position) do (load time only).
pub struct SsfMixer {
    cfg: SsfConfig,
    panner: SsfPanner,
    /// One loop cycle (or the loaded loop) plus one guard sample.
    table: Vec<f32>,
    /// Table increment per frame at pitch 1.
    base_inc: f64,
    sine: Vec<f32>,
    sine_inc: f64,
    rolls: [RollVoice; MAX_ROLL_VOICES],
    impulses: [ImpulseVoice; MAX_IMPULSE_VOICES],
    positions: Vec<(u16, [f32; 2])>,
    ramp_frames: u32,
    hold_frames: u64,
    decay: f32,
    next_seq: u64,
    stolen: u64,
    ignored: u64,
    clock: u64,
}

fn sine_table() -> Vec<f32> {
    (0..=TABLE_LEN)
        .map(|i| libm::sin(core::f64::consts::TAU * i as f64 / TABLE_LEN as f64) as f32)
        .collect()
}

#[inline]
fn lookup(table: &[f32], phase: f64) -> f32 {
    // phase ∈ [0, table.len() − 1); the guard sample makes i + 1 valid.
    let i = phase as usize;
    let f = (phase - i as f64) as f32;
    let a = table[i];
    let b = table[i + 1];
    a + (b - a) * f
}

impl SsfMixer {
    /// Builds a mixer (allocates the wave table and position map).
    pub fn new(cfg: SsfConfig) -> Self {
        let rate = cfg.sample_rate.max(1);
        let [x0, x1, y0, y1] = cfg.playfield_mm;
        let sine = sine_table();
        let sine_inc = f64::from(non_neg(cfg.impulse_hz, 0.0)) * TABLE_LEN as f64 / f64::from(rate);
        let base_inc =
            f64::from(non_neg(cfg.roll_base_hz, 0.0)) * TABLE_LEN as f64 / f64::from(rate);
        let tau = f64::from(ms_frames(cfg.impulse_decay_ms, rate));
        Self {
            panner: SsfPanner::new(x0, x1, y0, y1),
            table: sine.clone(),
            base_inc,
            sine,
            sine_inc,
            rolls: [RollVoice::default(); MAX_ROLL_VOICES],
            impulses: [ImpulseVoice::default(); MAX_IMPULSE_VOICES],
            positions: Vec::new(),
            ramp_frames: ms_frames(cfg.roll_ramp_ms, rate),
            hold_frames: u64::from(ms_frames(cfg.roll_hold_ms, rate)),
            decay: libm::exp(-1.0 / tau) as f32,
            next_seq: 0,
            stolen: 0,
            ignored: 0,
            clock: 0,
            cfg,
        }
    }

    /// The configuration.
    pub fn config(&self) -> &SsfConfig {
        &self.cfg
    }

    /// The panner built from the configured playfield.
    pub fn panner(&self) -> &SsfPanner {
        &self.panner
    }

    /// Uses `sample` (mixed to mono, assumed at the mixer rate) as the ball
    /// roll loop, played at the pitch ratio; an empty sample restores the
    /// built-in sine. Load time only: allocates.
    pub fn set_roll_loop(&mut self, sample: &Sample) {
        if sample.frames() == 0 {
            self.table = self.sine.clone();
            self.base_inc = f64::from(non_neg(self.cfg.roll_base_hz, 0.0)) * TABLE_LEN as f64
                / f64::from(self.cfg.sample_rate.max(1));
        } else {
            let mut t: Vec<f32> = sample
                .data()
                .chunks_exact(2)
                .map(|lr| 0.5 * (lr[0] + lr[1]))
                .collect();
            t.push(t[0]);
            self.table = t;
            self.base_inc = 1.0;
        }
        for v in &mut self.rolls {
            v.phase = 0.0;
        }
    }

    /// Registers the playfield position (mm) of a haptic source id (a coil,
    /// flipper or bumper; [`HapticEvent::source`]). Load time only.
    pub fn set_source_position(&mut self, source: u16, x_mm: f32, y_mm: f32) {
        match self.positions.binary_search_by_key(&source, |p| p.0) {
            Ok(i) => self.positions[i].1 = [x_mm, y_mm],
            Err(i) => self.positions.insert(i, (source, [x_mm, y_mm])),
        }
    }

    /// Removes every registered source position.
    pub fn clear_source_positions(&mut self) {
        self.positions.clear();
    }

    fn source_corners(&self, e: &HapticEvent) -> [f32; 4] {
        if let Some([x, y, _]) = e.position_mm() {
            return self.panner.corner_gains(x, y);
        }
        match self.positions.binary_search_by_key(&e.source(), |p| p.0) {
            Ok(i) => {
                let [x, y] = self.positions[i].1;
                self.panner.corner_gains(x, y)
            }
            Err(_) => self.panner.corner_gains(f32::NAN, f32::NAN),
        }
    }

    /// Applies one event (effective from frame 0 of the next render).
    /// Never allocates.
    pub fn feed(&mut self, e: HapticEvent) {
        let class_gain = match e.kind {
            HapticKind::BallRoll {
                ball,
                position_mm,
                mass_kg,
                speed_m_s,
            } => {
                self.feed_roll(ball, position_mm, mass_kg, speed_m_s);
                return;
            }
            HapticKind::CoilFire { .. } => self.cfg.coil_gain,
            HapticKind::EosHit { .. } => self.cfg.eos_gain,
            HapticKind::Bumper { .. } => self.cfg.bumper_gain,
            HapticKind::BallDrop { .. } => self.cfg.drop_gain,
            _ => self.cfg.impulse_gain,
        };
        let amp = non_neg(class_gain, 0.0).min(16.0) * unit(e.strength);
        if amp < SILENCE {
            return;
        }
        let pan = self.source_corners(&e);
        let slot = match self.impulses.iter().position(|v| !v.active) {
            Some(i) => i,
            None => {
                self.stolen += 1;
                let mut oldest = 0;
                for (i, v) in self.impulses.iter().enumerate() {
                    if v.seq < self.impulses[oldest].seq {
                        oldest = i;
                    }
                }
                oldest
            }
        };
        self.impulses[slot] = ImpulseVoice {
            active: true,
            phase: 0.0,
            amp,
            pan,
            seq: self.next_seq,
        };
        self.next_seq += 1;
    }

    fn feed_roll(&mut self, ball: u16, pos: [f32; 3], mass_kg: f32, speed_m_s: f32) {
        let Some(v) = self.rolls.get_mut(usize::from(ball)) else {
            self.ignored += 1;
            return;
        };
        let full = non_neg(self.cfg.roll_full_speed_m_s, 0.0);
        let s = if full > 0.0 {
            unit(speed_m_s / full)
        } else {
            unit(speed_m_s)
        };
        let ref_mass = non_neg(self.cfg.roll_ref_mass_kg, 0.0);
        let m = if ref_mass > 0.0 {
            let r = mass_kg / ref_mass;
            if r > 0.0 { r.min(2.0) } else { 0.0 }
        } else {
            1.0
        };
        let gain = f64::from(non_neg(self.cfg.roll_gain, 0.0).min(16.0) * s * m);
        let (p0, p1) = (
            non_neg(self.cfg.roll_pitch_min, 0.0),
            non_neg(self.cfg.roll_pitch_max, 0.0),
        );
        let inc = self.base_inc * f64::from(p0 + (p1 - p0) * s);
        // Keep the increment inside the table.
        let inc = inc.min((self.table.len() - 1) as f64 * 0.5);
        let corners = self.panner.corner_gains(pos[0], pos[1]);
        let n = self.ramp_frames;
        if !v.active {
            *v = RollVoice {
                active: true,
                ..RollVoice::default()
            };
            v.inc.jump(inc);
            for (r, g) in v.pan.iter_mut().zip(corners) {
                r.jump(f64::from(g));
            }
        } else {
            v.inc.set(inc, n);
            for (r, g) in v.pan.iter_mut().zip(corners) {
                r.set(f64::from(g), n);
            }
        }
        v.gain.set(gain, n);
        v.since = 0;
        v.fading = false;
    }

    /// Renders the exciter channels × master into `out` (overwritten),
    /// unclipped. Returns the number of frames.
    fn render_raw(&mut self, out: &mut [f32]) -> usize {
        out.fill(0.0);
        let frames = out.len() / CHANNELS;
        let body = &mut out[..frames * CHANNELS];
        let len = (self.table.len() - 1) as f64;
        for v in self.rolls.iter_mut().filter(|v| v.active) {
            for f in body.chunks_exact_mut(CHANNELS) {
                if !v.fading && v.since >= self.hold_frames {
                    v.fading = true;
                    v.gain.set(0.0, self.ramp_frames);
                }
                v.since += 1;
                let g = v.gain.tick();
                let s = lookup(&self.table, v.phase) as f64 * g;
                for (ch, p) in CORNERS.iter().zip(v.pan.iter_mut()) {
                    f[*ch] += (s * p.tick()) as f32;
                }
                v.phase += v.inc.tick();
                while v.phase >= len {
                    v.phase -= len;
                }
                if v.fading && v.gain.settled_at(0.0) {
                    v.active = false;
                    break;
                }
            }
        }
        let slen = (self.sine.len() - 1) as f64;
        for v in self.impulses.iter_mut().filter(|v| v.active) {
            for f in body.chunks_exact_mut(CHANNELS) {
                let s = lookup(&self.sine, v.phase) * v.amp;
                for (ch, p) in CORNERS.iter().zip(v.pan) {
                    f[*ch] += s * p;
                }
                v.phase += self.sine_inc;
                while v.phase >= slen {
                    v.phase -= slen;
                }
                v.amp *= self.decay;
                if v.amp < SILENCE {
                    v.active = false;
                    break;
                }
            }
        }
        let master = non_neg(self.cfg.master_gain, 0.0);
        if master != 1.0 {
            for f in body.chunks_exact_mut(CHANNELS) {
                for ch in CORNERS {
                    f[ch] *= master;
                }
            }
        }
        self.clock += frames as u64;
        frames
    }

    /// Renders `out.len() / 8` frames of interleaved 7.1 into `out`
    /// (overwritten; a trailing partial frame is zeroed). Never allocates.
    pub fn render(&mut self, out: &mut [f32]) {
        self.render_raw(out);
        clip(out);
    }

    /// Like [`render`](Self::render), then adds the 3.3 mixer's interleaved
    /// stereo `stereo` onto FL/FR ([`mix_stereo_front`]) before clipping.
    pub fn render_with_stereo(&mut self, stereo: &[f32], out: &mut [f32]) {
        self.render_raw(out);
        mix_stereo_front(stereo, out);
        clip(out);
    }

    /// Roll voices playing.
    pub fn active_roll_voices(&self) -> usize {
        self.rolls.iter().filter(|v| v.active).count()
    }

    /// Impulse voices playing.
    pub fn active_impulse_voices(&self) -> usize {
        self.impulses.iter().filter(|v| v.active).count()
    }

    /// Impulse voices cut off by a newer impulse.
    pub fn stolen_voices(&self) -> u64 {
        self.stolen
    }

    /// Roll events for ball slots without a voice.
    pub fn ignored_events(&self) -> u64 {
        self.ignored
    }

    /// Frames rendered since construction.
    pub fn frames_rendered(&self) -> u64 {
        self.clock
    }
}

fn clip(out: &mut [f32]) {
    for s in out {
        *s = if s.is_nan() { 0.0 } else { s.clamp(-1.0, 1.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssf::{CH_FC, CH_FL, CH_FR, CH_LFE};
    use pin_core::{CoilId, FlipperId};

    fn roll(ball: u16, x: f32, y: f32, speed: f32) -> HapticEvent {
        HapticEvent::new(
            0,
            HapticKind::BallRoll {
                ball,
                position_mm: [x, y, 0.0],
                mass_kg: 0.0806,
                speed_m_s: speed,
            },
            1.0,
        )
    }

    fn energy(buf: &[f32], ch: usize) -> f64 {
        buf.chunks_exact(CHANNELS)
            .map(|f| f64::from(f[ch]).powi(2))
            .sum()
    }

    #[test]
    fn silence_without_events() {
        let mut m = SsfMixer::new(SsfConfig::default());
        let mut out = vec![1.0f32; 8 * 64 + 3];
        m.render(&mut out);
        assert!(out.iter().all(|&s| s == 0.0));
        assert_eq!(m.frames_rendered(), 64);
    }

    #[test]
    fn roll_pans_and_fades() {
        let mut m = SsfMixer::new(SsfConfig::default());
        m.feed(roll(0, -250.0, 10.0, 3.0));
        let mut out = vec![0.0f32; 8 * 960];
        m.render(&mut out);
        assert_eq!(m.active_roll_voices(), 1);
        let sl = energy(&out, CH_SL);
        assert!(sl > 1.0);
        assert!(energy(&out, CH_BR) < sl * 1e-3);
        for ch in [CH_FL, CH_FR, CH_FC, CH_LFE] {
            assert_eq!(energy(&out, ch), 0.0);
        }
        // Moving update keeps it alive, then it fades after the hold time.
        m.feed(roll(0, 250.0, 1000.0, 1.0));
        m.render(&mut out);
        assert!(energy(&out[8 * 900..], CH_BR) > energy(&out[8 * 900..], CH_SL));
        m.render(&mut out);
        m.render(&mut out);
        assert_eq!(m.active_roll_voices(), 0);
        m.feed(roll(99, 0.0, 0.0, 1.0));
        assert_eq!(m.ignored_events(), 1);
    }

    #[test]
    fn impulses_decay_steal_and_position() {
        let cfg = SsfConfig {
            master_gain: 0.5,
            ..SsfConfig::default()
        };
        let mut m = SsfMixer::new(cfg);
        m.set_source_position(
            HapticEvent::new(
                0,
                HapticKind::EosHit {
                    flipper: FlipperId(0),
                },
                1.0,
            )
            .source(),
            257.0,
            1070.0,
        );
        m.set_source_position(0x1000, 257.0, 1070.0);
        m.feed(HapticEvent::new(
            0,
            HapticKind::EosHit {
                flipper: FlipperId(0),
            },
            1.0,
        ));
        let mut out = vec![0.0f32; 8 * 4800];
        m.render(&mut out);
        assert!(energy(&out, CH_BR) > 1.0);
        assert!(energy(&out, CH_SL) < 1e-9);
        for _ in 0..6 {
            m.render(&mut out);
        }
        assert_eq!(m.active_impulse_voices(), 0);
        for i in 0..20 {
            m.feed(HapticEvent::new(
                0,
                HapticKind::CoilFire { coil: CoilId(i) },
                1.0,
            ));
        }
        m.feed(HapticEvent::new(
            0,
            HapticKind::Bumper { mech: 0 },
            0.0,
        ));
        assert_eq!(m.stolen_voices(), 4);
        assert_eq!(m.active_impulse_voices(), MAX_IMPULSE_VOICES);
        m.render(&mut out);
        assert!(out.iter().all(|s| (-1.0..=1.0).contains(s)));
        m.clear_source_positions();
        m.feed(HapticEvent::new(
            0,
            HapticKind::BallDrop {
                ball: 0,
                position_mm: [0.0, 0.0, 0.0],
            },
            1.0,
        ));
        m.feed(HapticEvent::new(0, HapticKind::Impulse { source: 1 }, 1.0));
    }

    #[test]
    fn custom_loop_and_stereo_front() {
        let mut m = SsfMixer::new(SsfConfig::default());
        m.set_roll_loop(&Sample::constant(100, 0.5, 0.5));
        m.feed(roll(1, 0.0, 535.0, 3.0));
        let stereo = vec![0.25f32; 2 * 480];
        let mut out = vec![0.0f32; 8 * 480];
        m.render_with_stereo(&stereo, &mut out);
        assert!(out.chunks_exact(8).all(|f| f[CH_FL] == 0.25 && f[CH_FR] == 0.25));
        // Constant loop × ramping gain: positive on all four corners.
        let last = &out[8 * 479..];
        for ch in CORNERS {
            assert!(last[ch] > 0.0);
        }
        m.set_roll_loop(&Sample::constant(0, 0.0, 0.0));
        assert_eq!(m.config().sample_rate, 48_000);
        assert_eq!(m.panner().bounds(), [-257.0, 257.0, 0.0, 1070.0]);
    }
}
