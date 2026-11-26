//! Audio mixer (Subphase 3.3).
//!
//! A real-time mixer for the audio thread with three buses ([`Bus::Music`],
//! [`Bus::Callout`], [`Bus::Sfx`]), per-bus voice limiting with oldest-voice
//! stealing, callout ducking, WAV and Ogg Vorbis decoding ([`decode`], and
//! [`decode_with_budget`] with a per-bundle [`DecodeBudget`]), and a
//! lock-free trigger ring ([`trigger_ring`]) fed from the sim thread's
//! `HostCommandQueue::sounds`. [`Mixer::render`] is pure Rust and renders into
//! any buffer, so tests run offline; [`SdlOutput`] is a thin SDL3 shell
//! (cargo feature `sdl`, on by default; everything else here is SDL-free).
//!
//! # Render semantics
//! * **Triggers.** `render` first drains every pending [`SoundTrigger`] (at
//!   most the ring capacity per call). Each starts a voice whose first frame
//!   is frame 0 of that call. Unregistered ids count in
//!   [`Mixer::unknown_sounds`]; zero-length samples start nothing.
//! * **Voices.** Bus `b` has `max_voices[b]` slots. A new voice takes the
//!   lowest free slot; on a full bus it steals the *oldest* voice — the one
//!   whose trigger arrived first (voices are numbered in trigger arrival
//!   order, so triggers drained in the same `render` call steal in arrival
//!   order too; slot order would only break a tie, which cannot occur) — and
//!   counts in
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
//!
//! # Memory
//! A [`Sample`] stores mono sounds as one `f32` per frame and stereo sounds
//! interleaved, so a mono sound takes half the memory of the same frames in
//! stereo. The mixer pans mono to both channels at render time, choosing the
//! mono or stereo inner loop once per voice per block; the result is bit for
//! bit what a duplicated stereo copy would mix. A [`DecodeBudget`] charges
//! [`BYTES_PER_MONO_FRAME`] (4) per mono frame and [`BYTES_PER_FRAME`] (8)
//! per stereo frame, i.e. [`Sample::bytes`].
//!
//! # Streaming
//! Music need not be decoded whole. [`open_stream`] turns a music file's
//! compressed bytes (owned by the track) into a [`StreamedTrack`]: a
//! `khz-music-loader` thread decodes and resamples it, through the same
//! code as [`decode_with_budget`], into a bounded lock-free ring of `f32`
//! frames, and the audio thread only pops from that ring.
//! [`Mixer::register_stream`] accepts it on [`Bus::Music`] only (any other
//! bus hands the track back). When the track is long enough and the loader
//! keeps up, the mix is bit-identical to mixing the fully decoded sample.
//! * **Header first.** `open_stream` validates the header synchronously
//!   with the same errors as [`decode_with_budget`], including the
//!   10-minute cap, which also holds while decoding.
//! * **Ring sizing.** The ring holds the requested frames (default
//!   [`STREAM_RING_FRAMES`]) clamped to at least 1, to at most the 10-minute
//!   cap and, when the header gives the output length (WAV data-chunk size,
//!   last Ogg granule), to at most that length. It is allocated once, in
//!   `open_stream`.
//! * **Budget.** The [`DecodeBudget`] is charged the ring, not the decoded
//!   length: `ring_frames × 4 × channels` bytes (the per-frame rule of
//!   [`Sample::bytes`]), all or nothing at open
//!   ([`DecodeError::BudgetExhausted`], nothing charged). Dropping the track
//!   (directly, by replacing its id, or by dropping the mixer) stops and
//!   joins the loader and returns exactly those bytes.
//! * **Playback.** A streamed voice follows the voice rules above (gain
//!   clamp, voice limits and oldest-voice stealing, ducking, master gain,
//!   clipping, mono panning) and counts in [`Mixer::active_voices`].
//!   (a) *Underrun:* frames the ring cannot supply while the pass has not
//!   ended render as silence and count, one per frame, in
//!   [`StreamHandle::underrun_frames`] and [`Mixer::stream_underruns`]; the
//!   voice does not skip ahead but resumes with the next real frame. The
//!   voice ends once the loader has reached the end of the source (or
//!   failed) and the ring is drained. (b) *One voice per track:* a trigger
//!   for a streamed sound that is already playing is ignored (no voice, no
//!   steal); a zero-length track starts nothing. (c) *Rewind:* when a
//!   streamed voice ends or is stolen, the track rewinds so the next trigger
//!   plays from source frame 0; stale buffered frames never play, the
//!   loader refills in the background, and a retrigger before it has
//!   refilled underruns. [`StreamHandle::is_finished`] reads `false` from
//!   the moment the voice ends. (d) *Failure:* corrupt data after a valid
//!   header marks [`StreamHandle::failed`]; the frames buffered before it
//!   still play, then the voice ends as at the end of the source (not an
//!   underrun).
//! * **Threads.** The loader polls with `park_timeout` for free space, a
//!   rewind or a stop; the audio thread never signals, locks or waits on it.
//!   [`StreamHandle::wait_filled`] blocks load and test threads only.

mod decode;
mod engine;
mod ring;
#[cfg(feature = "sdl")]
mod sdl;
mod stream;

pub use decode::{
    BYTES_PER_FRAME, BYTES_PER_MONO_FRAME, DecodeBudget, DecodeError, INITIAL_RESERVE_FRAMES,
    MAX_MIXER_RATE, MAX_SECONDS, MAX_SOURCE_RATE, MIN_SOURCE_RATE, OverRelease, Sample, decode,
    decode_with_budget, resampled_len,
};
pub use engine::{Bus, Mixer, MixerConfig};
pub use ring::{SoundReceiver, SoundSender, SoundTrigger, trigger_ring};
#[cfg(feature = "sdl")]
pub use sdl::SdlOutput;
pub use stream::{STREAM_RING_FRAMES, StreamHandle, StreamedTrack, open_stream};
