//! The real-time mixer: buses, voices, voice stealing, ducking and the master
//! stage. Pure Rust; [`Mixer::render`] fills any interleaved stereo buffer, so
//! tests render offline and [`SdlOutput`](super::SdlOutput) only forwards.

use std::sync::Arc;

use pin_core::SoundId;
use pin_core::schema::table::SoundBus;

use super::decode::Sample;
use super::ring::{SoundReceiver, SoundTrigger};

/// Frames mixed per internal block (sizes the duck-curve scratch buffer).
const BLOCK_FRAMES: usize = 256;

/// A mixer bus. Indexes [`MixerConfig::max_voices`] as `bus as usize`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Bus {
    /// Background music; ducked under callouts.
    Music = 0,
    /// Voice callouts; never ducked, and they duck the other two buses.
    Callout = 1,
    /// Sound effects; ducked under callouts.
    Sfx = 2,
}

impl Bus {
    /// All buses in index order.
    pub const ALL: [Bus; 3] = [Bus::Music, Bus::Callout, Bus::Sfx];

    /// Parses the `table.toml` `[[sounds]] bus` name (`"music"`, `"callout"`,
    /// `"sfx"`; case-sensitive).
    pub fn from_name(name: &str) -> Option<Bus> {
        match name {
            "music" => Some(Bus::Music),
            "callout" => Some(Bus::Callout),
            "sfx" => Some(Bus::Sfx),
            _ => None,
        }
    }

    /// The `table.toml` name.
    pub fn name(self) -> &'static str {
        match self {
            Bus::Music => "music",
            Bus::Callout => "callout",
            Bus::Sfx => "sfx",
        }
    }

    /// Index into per-bus arrays.
    pub fn index(self) -> usize {
        self as usize
    }
}

impl From<SoundBus> for Bus {
    fn from(b: SoundBus) -> Self {
        match b {
            SoundBus::Music => Bus::Music,
            SoundBus::Callout => Bus::Callout,
            SoundBus::Sfx => Bus::Sfx,
        }
    }
}

/// Mixer settings (see [`Default`] for the planned values).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MixerConfig {
    /// Output rate in Hz (48 000).
    pub sample_rate: u32,
    /// Frames per audio buffer (256).
    pub buffer_frames: usize,
    /// Voice limit per bus `[music, callout, sfx]` (`[2, 4, 24]`).
    pub max_voices: [usize; 3],
    /// Music and sfx multiplier while a callout plays (0.35).
    pub duck_gain: f32,
    /// Ramp length from 1.0 down to `duck_gain`, in ms (30).
    pub duck_attack_ms: f32,
    /// Ramp length from `duck_gain` back to 1.0, in ms (250).
    pub duck_release_ms: f32,
    /// Gain applied to the sum before clipping (1.0).
    pub master_gain: f32,
}

impl Default for MixerConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            buffer_frames: 256,
            max_voices: [2, 4, 24],
            duck_gain: 0.35,
            duck_attack_ms: 30.0,
            duck_release_ms: 250.0,
            master_gain: 1.0,
        }
    }
}

struct Registered {
    sample: Arc<Sample>,
    bus: Bus,
}

#[derive(Copy, Clone, Debug, Default)]
struct Voice {
    active: bool,
    /// Index into `Mixer::sounds`.
    sound: u16,
    /// Next frame of the sample to play.
    pos: usize,
    gain: f32,
    /// Mixer frame clock when the voice started (its age).
    start: u64,
}

/// The audio-thread mixer (see the module docs for the exact semantics).
pub struct Mixer {
    cfg: MixerConfig,
    triggers: SoundReceiver,
    sounds: Vec<Option<Registered>>,
    /// One fixed-size slot array per bus (`cfg.max_voices`).
    voices: [Vec<Voice>; 3],
    duck: f64,
    duck_target: f64,
    attack_step: f64,
    release_step: f64,
    duck_curve: Vec<f32>,
    clock: u64,
    stolen: u64,
    unknown: u64,
}

fn ramp_step(span: f64, ms: f32, rate: u32) -> f64 {
    let frames = f64::from(ms) * f64::from(rate) / 1000.0;
    if frames > 0.0 && frames.is_finite() {
        span / frames
    } else {
        f64::INFINITY
    }
}

impl Mixer {
    /// Builds a mixer; all voice storage and scratch buffers are allocated
    /// here, so [`render`](Self::render) never allocates.
    pub fn new(cfg: MixerConfig, triggers: SoundReceiver) -> Self {
        let duck_target = if cfg.duck_gain.is_finite() {
            f64::from(cfg.duck_gain.max(0.0))
        } else {
            1.0
        };
        let span = (1.0 - duck_target).abs();
        Self {
            triggers,
            sounds: Vec::new(),
            voices: cfg.max_voices.map(|n| vec![Voice::default(); n]),
            duck: 1.0,
            duck_target,
            attack_step: ramp_step(span, cfg.duck_attack_ms, cfg.sample_rate),
            release_step: ramp_step(span, cfg.duck_release_ms, cfg.sample_rate),
            duck_curve: vec![1.0; BLOCK_FRAMES],
            clock: 0,
            stolen: 0,
            unknown: 0,
            cfg,
        }
    }

    /// The configuration the mixer was built with.
    pub fn config(&self) -> &MixerConfig {
        &self.cfg
    }

    /// Registers (or replaces) sound `id` on `bus`. Load time only: may
    /// allocate. Replacing a sound stops the voices playing its old sample.
    pub fn register(&mut self, id: SoundId, sample: Arc<Sample>, bus: Bus) {
        let i = usize::from(id.0);
        if self.sounds.len() <= i {
            self.sounds.resize_with(i + 1, || None);
        } else if self.sounds[i].is_some() {
            for v in self.voices.iter_mut().flatten() {
                if v.active && v.sound == id.0 {
                    v.active = false;
                }
            }
        }
        self.sounds[i] = Some(Registered { sample, bus });
    }

    /// Whether `id` has been registered.
    pub fn is_registered(&self, id: SoundId) -> bool {
        matches!(self.sounds.get(usize::from(id.0)), Some(Some(_)))
    }

    /// Mixes into `out` (interleaved stereo; an odd trailing value is set to
    /// 0). Drains every pending trigger first, so a trigger's first frame is
    /// frame 0 of this call. Never allocates, locks or blocks.
    pub fn render(&mut self, out: &mut [f32]) {
        self.drain();
        let frames = out.len() / 2;
        if out.len() % 2 == 1 {
            out[out.len() - 1] = 0.0;
        }
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(BLOCK_FRAMES);
            self.render_block(&mut out[2 * done..2 * (done + n)]);
            done += n;
        }
        self.clock += frames as u64;
    }

    /// Voices currently playing on `bus`.
    pub fn active_voices(&self, bus: Bus) -> usize {
        self.voices[bus.index()].iter().filter(|v| v.active).count()
    }

    /// Voices cut off by a newer trigger on a full bus.
    pub fn stolen_voices(&self) -> u64 {
        self.stolen
    }

    /// Triggers for sound ids that were never registered.
    pub fn unknown_sounds(&self) -> u64 {
        self.unknown
    }

    /// Current music/sfx duck multiplier (1.0 = no duck).
    pub fn duck_level(&self) -> f32 {
        self.duck as f32
    }

    /// Frames rendered since construction.
    pub fn frames_rendered(&self) -> u64 {
        self.clock
    }

    fn drain(&mut self) {
        // Bounded so a producer pushing concurrently cannot keep us here.
        for _ in 0..self.triggers.capacity() {
            match self.triggers.pop() {
                Some(t) => self.start(t),
                None => break,
            }
        }
    }

    fn start(&mut self, t: SoundTrigger) {
        let Some(Some(reg)) = self.sounds.get(usize::from(t.sound.0)) else {
            self.unknown += 1;
            return;
        };
        if reg.sample.frames() == 0 {
            return;
        }
        let voices = &mut self.voices[reg.bus.index()];
        let slot = match voices.iter().position(|v| !v.active) {
            Some(free) => free,
            None => {
                // Oldest = earliest start; ties go to the lowest slot.
                let Some((oldest, _)) = voices
                    .iter()
                    .enumerate()
                    .min_by_key(|(i, v)| (v.start, *i))
                else {
                    return; // max_voices == 0: the bus is muted.
                };
                self.stolen += 1;
                oldest
            }
        };
        voices[slot] = Voice {
            active: true,
            sound: t.sound.0,
            pos: 0,
            gain: if t.gain > 0.0 { t.gain } else { 0.0 },
            start: self.clock,
        };
    }

    fn render_block(&mut self, out: &mut [f32]) {
        let n = out.len() / 2;
        out.fill(0.0);

        // Callouts only end during a block (none start), so "a callout is
        // active at frame k" is "k < the longest remaining callout".
        let callout_left = self.voices[Bus::Callout.index()]
            .iter()
            .filter(|v| v.active)
            .filter_map(|v| self.sounds[usize::from(v.sound)].as_ref().map(|r| r.sample.frames() - v.pos))
            .max()
            .unwrap_or(0);
        // Frame k is mixed with the multiplier as it stands, then the
        // multiplier takes one ramp step (attack if a callout played frame k).
        for (k, d) in self.duck_curve[..n].iter_mut().enumerate() {
            *d = self.duck as f32;
            self.duck = if k < callout_left {
                if self.duck > self.duck_target {
                    (self.duck - self.attack_step).max(self.duck_target)
                } else {
                    (self.duck + self.attack_step).min(self.duck_target)
                }
            } else if self.duck < 1.0 {
                (self.duck + self.release_step).min(1.0)
            } else {
                (self.duck - self.release_step).max(1.0)
            };
        }

        for bus in Bus::ALL {
            let ducked = bus != Bus::Callout;
            for v in self.voices[bus.index()].iter_mut().filter(|v| v.active) {
                let Some(reg) = self.sounds[usize::from(v.sound)].as_ref() else {
                    v.active = false;
                    continue;
                };
                let data = reg.sample.data();
                let avail = (reg.sample.frames() - v.pos).min(n);
                let src = &data[2 * v.pos..2 * (v.pos + avail)];
                let g = v.gain;
                if ducked {
                    for ((o, s), d) in out.chunks_exact_mut(2).zip(src.chunks_exact(2)).zip(&self.duck_curve) {
                        o[0] += s[0] * g * d;
                        o[1] += s[1] * g * d;
                    }
                } else {
                    for (o, s) in out.chunks_exact_mut(2).zip(src.chunks_exact(2)) {
                        o[0] += s[0] * g;
                        o[1] += s[1] * g;
                    }
                }
                v.pos += avail;
                if v.pos >= reg.sample.frames() {
                    v.active = false;
                }
            }
        }

        // Master gain, then a hard clip to [-1, 1] (NaN → 0).
        let m = self.cfg.master_gain;
        for o in out.iter_mut() {
            let v = *o * m;
            *o = if v.is_nan() { 0.0 } else { v.clamp(-1.0, 1.0) };
        }
    }
}
