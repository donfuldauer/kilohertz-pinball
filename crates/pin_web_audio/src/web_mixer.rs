//! [`WebMixer`]: the safe engine behind the `pwa_*` C ABI.

use std::sync::Arc;

use pin_audio::mixer::DecodeBudget;
use pin_audio::{Bus, Mixer, MixerConfig, Sample, SoundSender, trigger_ring};
use pin_core::SoundId;

use crate::protocol::{At, CMD_BUFFER_BYTES, Command, Scheduler};

/// Lowest accepted sample rate (Hz).
pub const MIN_SAMPLE_RATE: u32 = 8_000;
/// Highest accepted sample rate (Hz).
pub const MAX_SAMPLE_RATE: u32 = 384_000;
/// Largest `buffer_frames` (frames per render call).
pub const MAX_BUFFER_FRAMES: u32 = 4096;

/// Highest valid sound id.
const MAX_SOUND_ID: u32 = 65_535;

/// Why [`WebMixer::register`] refused a sound. [`code`](Self::code) gives
/// the `pwa_register` return code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegisterError {
    /// `sound_id > 65535` (code -1).
    SoundId,
    /// `bus > 2` (code -2).
    Bus,
    /// `channels` is not 1 or 2 (code -3).
    Channels,
    /// `frames == 0`, or `frames × channels` exceeds the staged values
    /// (code -4).
    Frames,
    /// The decode budget cannot hold the sample; nothing was charged or
    /// registered and an existing sound with that id is kept (code -5).
    Budget,
}

impl RegisterError {
    /// The `pwa_register` return code (-1 to -5).
    pub fn code(self) -> i32 {
        match self {
            RegisterError::SoundId => -1,
            RegisterError::Bus => -2,
            RegisterError::Channels => -3,
            RegisterError::Frames => -4,
            RegisterError::Budget => -5,
        }
    }
}

/// Maps a bus code (0 music, 1 callout, 2 sfx) to a [`Bus`].
pub fn bus_from_code(code: u32) -> Option<Bus> {
    match code {
        0 => Some(Bus::Music),
        1 => Some(Bus::Callout),
        2 => Some(Bus::Sfx),
        _ => None,
    }
}

/// `pin_audio::Mixer` for the browser: PCM staging, registration under a
/// [`DecodeBudget`], the trigger protocol ([`crate::protocol`]) and an
/// engine-owned output buffer.
///
/// Everything is allocated in [`new`](Self::new) (and the staging area in
/// [`stage`](Self::stage), load time only), so [`render`](Self::render),
/// [`trigger`](Self::trigger) and [`submit`](Self::submit) never allocate.
pub struct WebMixer {
    mixer: Mixer,
    tx: SoundSender,
    sched: Scheduler,
    budget: DecodeBudget,
    /// The sample registered under each id (for exact budget release).
    samples: Vec<Option<Arc<Sample>>>,
    staging: Vec<f32>,
    staged: usize,
    cmd: Box<[u8; CMD_BUFFER_BYTES]>,
    out: Vec<f32>,
    buffer_frames: usize,
}

impl std::fmt::Debug for WebMixer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebMixer")
            .field("quantum", &self.sched.quantum())
            .field("buffer_frames", &self.buffer_frames)
            .field("budget_used", &self.budget.used_bytes())
            .finish_non_exhaustive()
    }
}

impl WebMixer {
    /// A mixer at `sample_rate` Hz rendering at most `buffer_frames` frames
    /// per call, with a trigger ring of `trigger_capacity` and a decode
    /// budget of `budget_mib` MiB. `None` when `sample_rate` is outside
    /// 8000..=384000, `buffer_frames` is 0 or above 4096, or
    /// `trigger_capacity` is 0. The mixer config is
    /// `MixerConfig { sample_rate, buffer_frames, ..MixerConfig::default() }`.
    pub fn new(
        sample_rate: u32,
        buffer_frames: u32,
        trigger_capacity: u32,
        budget_mib: u32,
    ) -> Option<Self> {
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate)
            || buffer_frames == 0
            || buffer_frames > MAX_BUFFER_FRAMES
            || trigger_capacity == 0
        {
            return None;
        }
        let buffer_frames = buffer_frames as usize;
        let (tx, rx) = trigger_ring(trigger_capacity as usize);
        let cfg = MixerConfig {
            sample_rate,
            buffer_frames,
            ..MixerConfig::default()
        };
        Some(Self {
            mixer: Mixer::new(cfg, rx),
            tx,
            sched: Scheduler::new(),
            budget: DecodeBudget::new(u64::from(budget_mib) << 20),
            samples: Vec::new(),
            staging: Vec::new(),
            staged: 0,
            cmd: Box::new([0; CMD_BUFFER_BYTES]),
            out: vec![0.0; buffer_frames * 2],
            buffer_frames,
        })
    }

    /// The staging area for `n_values` PCM values (load time only; may
    /// grow). `None` when `n_values × 4` exceeds the budget limit or the
    /// memory cannot be reserved. The returned slice has exactly `n_values`
    /// values; [`register`](Self::register) reads from it.
    pub fn stage(&mut self, n_values: u32) -> Option<&mut [f32]> {
        let n = n_values as usize;
        if u64::from(n_values) * 4 > self.budget.limit_bytes() {
            return None;
        }
        if self.staging.len() < n {
            self.staging
                .try_reserve_exact(n - self.staging.len())
                .ok()?;
            self.staging.resize(n, 0.0);
        }
        self.staged = n;
        self.staging.get_mut(..n)
    }

    /// Registers (or replaces) `sound_id` on bus code `bus` from the first
    /// `channels × frames` staged values (interleaved when stereo), charging
    /// the budget exactly [`Sample::bytes`]. Replacing an id releases the old
    /// sample's bytes first; when the new one does not fit, the old one stays
    /// registered and charged.
    pub fn register(
        &mut self,
        sound_id: u32,
        bus: u32,
        channels: u32,
        frames: u32,
    ) -> Result<(), RegisterError> {
        if sound_id > MAX_SOUND_ID {
            return Err(RegisterError::SoundId);
        }
        let bus = bus_from_code(bus).ok_or(RegisterError::Bus)?;
        if channels != 1 && channels != 2 {
            return Err(RegisterError::Channels);
        }
        let n = u64::from(channels) * u64::from(frames);
        if frames == 0 || n > self.staged as u64 {
            return Err(RegisterError::Frames);
        }
        let n = n as usize; // <= staged, a usize
        let values = self.staging.get(..n).ok_or(RegisterError::Frames)?.to_vec();
        let sample = if channels == 1 {
            Sample::from_mono_vec(values)
        } else {
            Sample::from_interleaved(values)
        };
        let idx = sound_id as usize;
        let old = self.samples.get(idx).and_then(Option::as_ref);
        if let Some(old) = old {
            // It was charged when registered, so this cannot over-release.
            let _ = self.budget.release(old);
        }
        if !self.budget.try_charge_sample(&sample) {
            if let Some(old) = old {
                // Just released, so it fits again: accounting stays exact.
                let _ = self.budget.try_charge_sample(old);
            }
            return Err(RegisterError::Budget);
        }
        if self.samples.len() <= idx {
            self.samples.resize(idx + 1, None);
        }
        let sample = Arc::new(sample);
        self.mixer
            .register(SoundId(sound_id as u16), Arc::clone(&sample), bus);
        if let Some(slot) = self.samples.get_mut(idx) {
            *slot = Some(sample);
        }
        Ok(())
    }

    /// One immediate trigger (a "next quantum" record); `false` when
    /// `sound_id > 65535` (counted as rejected) or the pending list is full
    /// (counted as dropped).
    pub fn trigger(&mut self, sound_id: u32, gain: f32) -> bool {
        match u16::try_from(sound_id) {
            Ok(id) => self.sched.push(Command {
                sound: SoundId(id),
                gain,
                at: At::Next,
            }),
            Err(_) => {
                self.sched.reject();
                false
            }
        }
    }

    /// The command staging buffer (`CMD_BUFFER_BYTES` bytes).
    pub fn cmd_buffer(&mut self) -> &mut [u8; CMD_BUFFER_BYTES] {
        &mut self.cmd
    }

    /// Parses the first `n_bytes` (clamped to `CMD_BUFFER_BYTES`) of the
    /// command buffer as records; returns how many entered the pending list.
    pub fn submit(&mut self, n_bytes: u32) -> u32 {
        let n = (n_bytes as usize).min(CMD_BUFFER_BYTES);
        let bytes = self.cmd.get(..n).unwrap_or(&[]);
        self.sched.submit(bytes)
    }

    /// Parses `bytes` as records directly (same rules as
    /// [`submit`](Self::submit), no length clamp).
    pub fn submit_bytes(&mut self, bytes: &[u8]) -> u32 {
        self.sched.submit(bytes)
    }

    /// Renders `frames` stereo frames (interleaved L, R): releases the due
    /// pending triggers into the mixer's ring, mixes, then advances the
    /// quantum index. `None` when `frames` is 0 or above `buffer_frames`.
    pub fn render(&mut self, frames: u32) -> Option<&[f32]> {
        let frames = frames as usize;
        if frames == 0 || frames > self.buffer_frames {
            return None;
        }
        let tx = &mut self.tx;
        self.sched.release_due(|t| tx.send(t));
        let out = self.out.get_mut(..frames * 2)?;
        self.mixer.render(out);
        self.sched.advance();
        Some(out)
    }

    /// Triggers dropped (ring full or pending list full) — stat 0.
    pub fn dropped(&self) -> u64 {
        self.sched.dropped()
    }

    /// Command records rejected — stat 1.
    pub fn rejected(&self) -> u64 {
        self.sched.rejected()
    }

    /// The quantum index (successful renders) — stat 5.
    pub fn quantum(&self) -> u64 {
        self.sched.quantum()
    }

    /// Triggers waiting in the pending list.
    pub fn pending(&self) -> usize {
        self.sched.pending_len()
    }

    /// The decode budget.
    pub fn budget(&self) -> &DecodeBudget {
        &self.budget
    }

    /// The underlying mixer.
    pub fn mixer(&self) -> &Mixer {
        &self.mixer
    }

    /// Voices playing on all buses — stat 8.
    pub fn active_voices(&self) -> usize {
        Bus::ALL.iter().map(|&b| self.mixer.active_voices(b)).sum()
    }

    /// The `pwa_stat` value for `which` (0 dropped, 1 rejected, 2 unknown
    /// sounds, 3 stolen voices, 4 frames rendered, 5 quantum index, 6 budget
    /// used bytes, 7 budget limit bytes, 8 active voices); NaN otherwise.
    pub fn stat(&self, which: u32) -> f64 {
        match which {
            0 => self.dropped() as f64,
            1 => self.rejected() as f64,
            2 => self.mixer.unknown_sounds() as f64,
            3 => self.mixer.stolen_voices() as f64,
            4 => self.mixer.frames_rendered() as f64,
            5 => self.quantum() as f64,
            6 => self.budget.used_bytes() as f64,
            7 => self.budget.limit_bytes() as f64,
            8 => self.active_voices() as f64,
            _ => f64::NAN,
        }
    }
}
