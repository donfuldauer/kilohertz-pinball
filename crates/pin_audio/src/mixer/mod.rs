//! Audio mixer (Subphase 3.3).
//!
//! A real-time mixer for the audio thread with three buses ([`Bus::Music`],
//! [`Bus::Callout`], [`Bus::Sfx`]), per-bus voice limiting with oldest-voice
//! stealing, callout ducking, WAV and Ogg Vorbis decoding ([`decode`]), and a
//! lock-free trigger ring ([`trigger_ring`]) fed from the sim thread's
//! `HostCommandQueue::sounds`. [`Mixer::render`] is pure Rust and renders into
//! any buffer, so tests run offline; [`SdlOutput`] is a thin SDL3 shell.
//!
//! # Render semantics
//! * **Triggers.** `render` first drains every pending [`SoundTrigger`] (at
//!   most the ring capacity per call). Each starts a voice whose first frame
//!   is frame 0 of that call. Unregistered ids count in
//!   [`Mixer::unknown_sounds`]; zero-length samples start nothing.
//! * **Voices.** Bus `b` has `max_voices[b]` slots. A new voice takes the
//!   lowest free slot; on a full bus it steals the *oldest* voice (earliest
//!   start frame; voices started in the same `render` call are equally old,
//!   and the tie goes to the lowest slot) and counts in
//!   [`Mixer::stolen_voices`]. A bus with 0 slots ignores its triggers.
//!   A voice's gain is the trigger gain clamped to `≥ 0` (NaN → 0); it stops
//!   after its sample's last frame (no looping).
//! * **Ducking.** With `g = duck_gain`, `A`/`R` the attack/release ramp
//!   lengths in frames (`ms · sample_rate / 1000`), the duck multiplier `d`
//!   (starting at 1.0) is applied to music and sfx, never to callouts. Frame
//!   `k` is mixed with the current `d`, after which `d` takes one step: toward
//!   `g` by `|1 − g| / A` if a callout voice played frame `k`, else toward 1.0
//!   by `|1 − g| / R`, never overshooting. Hence a callout starting at frame
//!   0 from `d = 1` gives `d(k) = 1 − (1 − g)·k/A` for `k ≤ A` — frame `k` of
//!   the output uses `d(k)` and [`Mixer::duck_level`] after `k` frames reads
//!   `d(k)` — and release mirrors it. `d` is kept in `f64`. A ramp of 0 ms
//!   (or a non-positive one) jumps in one step.
//! * **Mix.** `out[ch] = clip(master · (Σ_music s·gain·d + Σ_callout s·gain +
//!   Σ_sfx s·gain·d))` per channel; channels are independent; no voices ⇒
//!   exact silence.
//! * **Clipping.** The master stage hard-clips to `[−1, 1]` (NaN → 0). Hard
//!   clipping was chosen over a soft limiter because it is exact and
//!   stateless (mixing tests see the plain sum below full scale) and costs
//!   nothing; tables are expected to leave headroom via gains.
//!
//! `render` never allocates, locks or blocks; all storage is sized in
//! [`Mixer::new`] / [`Mixer::register`].

mod decode;
mod engine;
mod ring;
mod sdl;

pub use decode::{
    DecodeError, MAX_MIXER_RATE, MAX_SECONDS, MAX_SOURCE_RATE, MIN_SOURCE_RATE, Sample, decode,
    resampled_len,
};
pub use engine::{Bus, Mixer, MixerConfig};
pub use ring::{SoundReceiver, SoundSender, SoundTrigger, trigger_ring};
pub use sdl::SdlOutput;
