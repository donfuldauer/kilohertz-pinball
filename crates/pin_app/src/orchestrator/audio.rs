//! The audio thread's engine and output (Subphase 6.0 wiring).
//!
//! * [`AudioEngine`]: one block at a time, the 3.3 [`Mixer`] (sound
//!   triggers from the sim's `HostCommandQueue::sounds`, through the
//!   wait-free [`SoundSender`](pin_audio::mixer::SoundSender) ring that
//!   [`TableSim`](super::TableSim) feeds) renders stereo, the 3.4
//!   [`SsfMixer`] renders the exciter channels from the haptic impulses, and
//!   [`SsfMixer::render_with_stereo`] combines both into one interleaved 7.1
//!   block (stereo on FL/FR, exciters on SL/SR/BL/BR).
//! * [`AudioSink`]: where the blocks go. [`NullSink`] discards them,
//!   [`OfflineSink`] records them into memory (tests, D31: audio renders
//!   offline), [`DeviceSink`] pushes them to an SDL3 playback stream
//!   opened on the main thread ([`DeviceStream::open`]; SDL converts 7.1 to
//!   the device's channel layout). No device, or `--headless`, falls back
//!   to [`NullSink`].
//! * [`table_samples`]: decodes the table's `[[sounds]]` from the validated
//!   bundle snapshot for [`Mixer::register`].

use super::table::ValidatedTable;
use pin_assets::IdKind;
use pin_audio::mixer::{
    Bus, DecodeBudget, Mixer, MixerConfig, Sample, SoundReceiver, decode_with_budget, trigger_ring,
};
use pin_audio::ssf::{CHANNELS, SsfConfig, SsfMixer};
use pin_core::SoundId;
use pin_core::haptics::HapticEvent;
use std::sync::{Arc, Mutex, PoisonError};

/// Frames the audio thread renders per block (5.3 ms at 48 kHz).
pub const AUDIO_BLOCK_FRAMES: usize = 256;

/// A sound registered with the mixer: id, decoded sample, bus.
pub type RegisteredSound = (SoundId, Arc<Sample>, Bus);

/// Decodes every `[[sounds]]` entry of `checked` from its bundle snapshot
/// at `sample_rate` (one [`DecodeBudget::DEFAULT_BYTES`] budget for the
/// table). Returns the sounds in id order and one message per sound that
/// could not be read or decoded (that sound stays silent).
pub fn table_samples(
    checked: &ValidatedTable,
    sample_rate: u32,
) -> (Vec<RegisteredSound>, Vec<String>) {
    let budget = DecodeBudget::new(DecodeBudget::DEFAULT_BYTES);
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for def in &checked.table.manifest.sounds {
        let Some(id) = checked
            .table
            .ids
            .entries(IdKind::Sound)
            .iter()
            .find(|(n, _)| *n == def.id)
            .map(|&(_, i)| SoundId(i))
        else {
            errors.push(format!("sound {}: no id", def.id));
            continue;
        };
        let sample = checked
            .snapshot
            .read(&def.file)
            .map_err(|e| e.to_string())
            .and_then(|b| {
                decode_with_budget(&b, sample_rate, &budget).map_err(|e| format!("{e:?}"))
            });
        match sample {
            Ok(s) => out.push((id, Arc::new(s), Bus::from(def.bus))),
            Err(e) => errors.push(format!("sound {} ({}): {e}", def.id, def.file)),
        }
    }
    out.sort_by_key(|(id, _, _)| id.0);
    (out, errors)
}

/// The audio thread's mixing state: the 3.3 mixer and the SSF exciter
/// mixer combined into interleaved 7.1 blocks of [`AUDIO_BLOCK_FRAMES`].
/// All buffers are allocated in [`new`](Self::new);
/// [`render_block`](Self::render_block) never allocates.
pub struct AudioEngine {
    ssf: SsfMixer,
    mixer: Mixer,
    stereo: Vec<f32>,
    block: Vec<f32>,
    peak_voices: usize,
}

impl AudioEngine {
    /// An engine whose mixer takes its triggers from `sounds` (`None`: no
    /// trigger source, the mixer stays silent) and plays `samples`.
    pub fn new(
        ssf: SsfConfig,
        mixer: MixerConfig,
        sounds: Option<SoundReceiver>,
        samples: &[RegisteredSound],
    ) -> AudioEngine {
        let rx = sounds.unwrap_or_else(|| trigger_ring(1).1);
        let mut mixer = Mixer::new(mixer, rx);
        for (id, s, bus) in samples {
            mixer.register(*id, Arc::clone(s), *bus);
        }
        AudioEngine {
            ssf: SsfMixer::new(ssf),
            mixer,
            stereo: vec![0.0; AUDIO_BLOCK_FRAMES * 2],
            block: vec![0.0; AUDIO_BLOCK_FRAMES * CHANNELS],
            peak_voices: 0,
        }
    }

    /// Feeds one haptic impulse to the SSF exciter mixer.
    pub fn feed_haptic(&mut self, e: HapticEvent) {
        self.ssf.feed(e);
    }

    /// Renders the next block: the mixer (draining every pending sound
    /// trigger first) into stereo, then the SSF channels with the stereo on
    /// FL/FR. Returns the interleaved 7.1 block ([`CHANNELS`] per frame).
    pub fn render_block(&mut self) -> &[f32] {
        self.mixer.render(&mut self.stereo);
        self.peak_voices = self.peak_voices.max(self.active_voices());
        self.ssf.render_with_stereo(&self.stereo, &mut self.block);
        &self.block
    }

    /// The last rendered block.
    pub fn block(&self) -> &[f32] {
        &self.block
    }

    /// Voices playing on all mixer buses now.
    pub fn active_voices(&self) -> usize {
        Bus::ALL.iter().map(|&b| self.mixer.active_voices(b)).sum()
    }

    /// Most mixer voices seen still playing at the end of a block (a voice
    /// that starts and ends within one block is not counted).
    pub fn peak_voices(&self) -> usize {
        self.peak_voices
    }

    /// The 3.3 mixer.
    pub fn mixer(&self) -> &Mixer {
        &self.mixer
    }

    /// The SSF exciter mixer.
    pub fn ssf(&self) -> &SsfMixer {
        &self.ssf
    }
}

/// Where the audio thread sends its 7.1 blocks.
pub trait AudioSink: Send {
    /// Short name for logs (`null`, `offline`, `device`).
    fn name(&self) -> &'static str;

    /// `true` for a sink that paces the audio thread (a device): the thread
    /// then renders while [`wants_block`](Self::wants_block) says so.
    /// Unpaced sinks get one block per audio-thread period.
    fn paced(&self) -> bool {
        false
    }

    /// For a paced sink: whether it has room for another block now.
    fn wants_block(&mut self) -> bool {
        true
    }

    /// Takes one interleaved block of `channels` channels.
    fn write(&mut self, block: &[f32], channels: usize);
}

/// Discards every block (headless, or no output device), counting frames.
#[derive(Debug, Default)]
pub struct NullSink {
    frames: u64,
}

impl NullSink {
    /// Frames written so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }
}

impl AudioSink for NullSink {
    fn name(&self) -> &'static str {
        "null"
    }

    fn write(&mut self, block: &[f32], channels: usize) {
        self.frames += (block.len() / channels.max(1)) as u64;
    }
}

/// The shared buffer of an [`OfflineSink`]: read it after the audio thread
/// was joined (or at any time; a mutex guards it — on the audio thread,
/// never on the sim thread).
#[derive(Clone, Debug, Default)]
pub struct OfflineRecording {
    data: Arc<Mutex<Vec<f32>>>,
}

impl OfflineRecording {
    /// A copy of the recorded interleaved samples.
    pub fn samples(&self) -> Vec<f32> {
        self.data
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Largest absolute sample recorded so far.
    pub fn peak(&self) -> f32 {
        self.data
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .fold(0.0f32, |m, &x| m.max(x.abs()))
    }
}

/// Records blocks into memory, up to a fixed number of frames (the rest is
/// dropped). Unpaced.
#[derive(Debug)]
pub struct OfflineSink {
    data: Arc<Mutex<Vec<f32>>>,
    max_samples: usize,
    channels: usize,
}

impl OfflineSink {
    /// A sink keeping at most `max_frames` frames of [`CHANNELS`] channels,
    /// and the handle to read them.
    pub fn new(max_frames: usize) -> (OfflineSink, OfflineRecording) {
        let max_samples = max_frames.saturating_mul(CHANNELS);
        let data = Arc::new(Mutex::new(Vec::with_capacity(max_samples)));
        (
            OfflineSink {
                data: Arc::clone(&data),
                max_samples,
                channels: CHANNELS,
            },
            OfflineRecording { data },
        )
    }
}

impl AudioSink for OfflineSink {
    fn name(&self) -> &'static str {
        "offline"
    }

    fn write(&mut self, block: &[f32], channels: usize) {
        if channels != self.channels {
            return;
        }
        let mut d = self.data.lock().unwrap_or_else(PoisonError::into_inner);
        let room = self.max_samples.saturating_sub(d.len());
        d.extend_from_slice(&block[..block.len().min(room)]);
    }
}

/// Raw SDL stream pointer shared by [`DeviceStream`] (main thread) and
/// [`DeviceSink`] (audio thread).
struct RawStream(*mut sdl3::sys::audio::SDL_AudioStream);

// SAFETY: SDL3 audio streams are thread-safe (every SDL_AudioStream call
// takes the stream's own lock); the pointer is only used while the
// `DeviceStream` that owns it is alive (see `DeviceStream`'s `Drop`).
unsafe impl Send for RawStream {}
unsafe impl Sync for RawStream {}

/// The owner of an SDL3 playback stream (main thread: SDL objects are
/// created and destroyed there). Keep it alive until the audio thread was
/// joined; if its [`DeviceSink`] still exists when it drops, the stream is
/// deliberately leaked instead of destroyed under the sink.
pub struct DeviceStream {
    owner: Option<sdl3::audio::AudioStreamOwner>,
    shared: Arc<RawStream>,
}

/// Queued audio a [`DeviceSink`] keeps in the device stream, in blocks
/// (about 21 ms at 48 kHz).
pub const DEVICE_QUEUE_BLOCKS: usize = 4;

impl DeviceStream {
    /// Opens the default playback device as `f32`, `sample_rate`, 7.1
    /// ([`CHANNELS`]) and starts it. Returns the owner (keep it on this
    /// thread) and the sink for the audio thread.
    ///
    /// # Errors
    /// SDL's message when no device can be opened.
    pub fn open(
        audio: &sdl3::AudioSubsystem,
        sample_rate: u32,
    ) -> Result<(DeviceStream, DeviceSink), String> {
        let spec = sdl3::audio::AudioSpec {
            freq: Some(i32::try_from(sample_rate).map_err(|e| e.to_string())?),
            channels: Some(CHANNELS as i32),
            format: Some(sdl3::audio::AudioFormat::f32_sys()),
        };
        let owner = audio
            .default_playback_device()
            .open_device_stream(Some(&spec))
            .map_err(|e| e.to_string())?;
        owner.resume().map_err(|e| e.to_string())?;
        let shared = Arc::new(RawStream(owner.raw()));
        let sink = DeviceSink {
            stream: Arc::clone(&shared),
            target_bytes: (DEVICE_QUEUE_BLOCKS * AUDIO_BLOCK_FRAMES * CHANNELS * 4) as i32,
        };
        Ok((
            DeviceStream {
                owner: Some(owner),
                shared,
            },
            sink,
        ))
    }
}

impl Drop for DeviceStream {
    fn drop(&mut self) {
        if Arc::strong_count(&self.shared) > 1 {
            // The audio thread may still write: never destroy the stream
            // under it (the OS reclaims it at exit).
            std::mem::forget(self.owner.take());
        }
    }
}

/// The audio-thread end of a [`DeviceStream`]: pushes blocks into the SDL
/// stream, paced to keep about [`DEVICE_QUEUE_BLOCKS`] blocks queued.
pub struct DeviceSink {
    stream: Arc<RawStream>,
    target_bytes: i32,
}

impl AudioSink for DeviceSink {
    fn name(&self) -> &'static str {
        "device"
    }

    fn paced(&self) -> bool {
        true
    }

    fn wants_block(&mut self) -> bool {
        // SAFETY: the stream is alive while this sink exists (DeviceStream
        // leaks it otherwise); SDL_GetAudioStreamQueued is thread-safe.
        let queued = unsafe { sdl3::sys::audio::SDL_GetAudioStreamQueued(self.stream.0) };
        queued >= 0 && queued < self.target_bytes
    }

    fn write(&mut self, block: &[f32], channels: usize) {
        if channels != CHANNELS {
            return;
        }
        let Ok(len) = i32::try_from(std::mem::size_of_val(block)) else {
            return;
        };
        // SAFETY: as in `wants_block`; `block` is valid for `len` bytes and
        // SDL copies it before returning. A failed put only drops this block.
        let _ = unsafe {
            sdl3::sys::audio::SDL_PutAudioStreamData(
                self.stream.0,
                block.as_ptr().cast::<core::ffi::c_void>(),
                len,
            )
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pin_audio::mixer::SoundTrigger;

    #[test]
    fn null_and_offline_sinks() {
        let mut n = NullSink::default();
        n.write(&[0.0; 16], 8);
        assert_eq!(n.frames(), 2);
        assert!(!n.paced() && n.wants_block());
        let (mut s, rec) = OfflineSink::new(3);
        s.write(&[0.5; 16], CHANNELS);
        s.write(&[0.25; 16], CHANNELS);
        s.write(&[1.0; 6], 2); // other layouts are ignored
        let got = rec.samples();
        assert_eq!(got.len(), 24);
        assert_eq!(got[23], 0.25);
        assert_eq!(rec.peak(), 0.5);
    }

    #[test]
    fn engine_plays_a_triggered_sound_on_the_front_channels() {
        let (mut tx, rx) = trigger_ring(4);
        let s = Arc::new(Sample::constant(1000, 0.5, -0.5));
        let mut e = AudioEngine::new(
            SsfConfig::default(),
            MixerConfig::default(),
            Some(rx),
            &[(SoundId(3), s, Bus::Sfx)],
        );
        assert!(e.render_block().iter().all(|&x| x == 0.0), "silent at rest");
        assert!(tx.send(SoundTrigger {
            sound: SoundId(3),
            gain: 1.0
        }));
        let b = e.render_block().to_vec();
        assert_eq!(e.peak_voices(), 1);
        assert_eq!(b[pin_audio::ssf::CH_FL], 0.5);
        assert_eq!(b[pin_audio::ssf::CH_FR], -0.5);
        assert_eq!(e.mixer().frames_rendered(), 2 * AUDIO_BLOCK_FRAMES as u64);
    }
}
