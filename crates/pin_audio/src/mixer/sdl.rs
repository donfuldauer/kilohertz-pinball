//! SDL3 output shell: an `F32` stereo audio stream whose callback calls
//! [`Mixer::render`]. All mixing logic lives in [`Mixer`]; this file only
//! moves samples into SDL.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use sdl3::audio::{AudioCallback, AudioFormat, AudioSpec, AudioStream, AudioStreamWithCallback};

use super::engine::Mixer;

struct Callback {
    /// `None` only after [`SdlOutput::close`] took the mixer back.
    mixer: Option<Mixer>,
    /// Interleaved scratch of one audio buffer, allocated at open.
    scratch: Vec<f32>,
    calls: Arc<AtomicU64>,
}

impl AudioCallback<f32> for Callback {
    fn callback(&mut self, stream: &mut AudioStream, requested: i32) {
        // `requested` counts f32 values; render whole stereo frames.
        let Some(mixer) = self.mixer.as_mut() else {
            return;
        };
        let mut left = usize::try_from(requested).unwrap_or(0);
        left += left % 2;
        while left > 0 {
            let n = left.min(self.scratch.len());
            let buf = &mut self.scratch[..n];
            mixer.render(buf);
            // A failed put only drops this buffer; nothing to recover here.
            // Note: SDL_PutAudioStreamData copies into SDL's own queue and may
            // allocate (and builds an error string on failure) on this audio
            // thread. That is inherent to SDL3 audio streams; `Mixer::render`
            // itself never allocates.
            let _ = stream.put_data_f32(buf);
            left -= n;
        }
        self.calls.fetch_add(1, Ordering::Release);
    }
}

/// An open SDL3 playback stream driven by a [`Mixer`] on SDL's audio thread.
pub struct SdlOutput {
    stream: Option<AudioStreamWithCallback<Callback>>,
    calls: Arc<AtomicU64>,
}

impl SdlOutput {
    /// Opens the default playback device as `F32`, stereo,
    /// `mixer.config().sample_rate`, and starts it.
    pub fn open(audio: &sdl3::AudioSubsystem, mixer: Mixer) -> Result<SdlOutput, String> {
        let rate = i32::try_from(mixer.config().sample_rate).map_err(|e| e.to_string())?;
        let spec = AudioSpec {
            freq: Some(rate),
            channels: Some(2),
            format: Some(AudioFormat::f32_sys()),
        };
        let calls = Arc::new(AtomicU64::new(0));
        let cb = Callback {
            scratch: vec![0.0; 2 * mixer.config().buffer_frames.max(1)],
            mixer: Some(mixer),
            calls: Arc::clone(&calls),
        };
        let stream = audio
            .open_playback_stream(&spec, cb)
            .map_err(|e| e.to_string())?;
        stream.resume().map_err(|e| e.to_string())?;
        Ok(SdlOutput {
            stream: Some(stream),
            calls,
        })
    }

    /// Completed audio callbacks so far (each rendered at least one buffer
    /// when SDL asked for data).
    pub fn callbacks(&self) -> u64 {
        self.calls.load(Ordering::Acquire)
    }

    /// Stops playback, closes the stream and returns the mixer.
    pub fn close(mut self) -> Mixer {
        let mut stream = self.stream.take().expect("stream present until close");
        // Pausing stops further callbacks; the lock waits for one in flight.
        let _ = stream.pause();
        let mixer = stream
            .lock()
            .and_then(|mut guard| guard.mixer.take())
            .expect("SDL audio stream lock failed");
        drop(stream);
        mixer
    }
}

impl Drop for SdlOutput {
    fn drop(&mut self) {
        // sdl3 frees the callback before destroying the stream; pause first so
        // no callback can run in between.
        if let Some(stream) = self.stream.take() {
            let _ = stream.pause();
            drop(stream);
        }
    }
}
