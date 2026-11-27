//! `pin_web_audio`: `pin_audio::Mixer` inside a browser AudioWorklet
//! (REQ-WEB-022, D43, SPEC §2.2 web build).
//!
//! The crate builds as a `wasm32-unknown-unknown` cdylib with no imports that
//! exports `memory` and the `pwa_*` C ABI ([`ffi`]), and as an rlib so the
//! engine module can link it (the main-thread fallback renders through the
//! same ABI) and native tests can call it as plain Rust.
//!
//! * [`protocol`]: 16-byte trigger records, their parser and the fixed
//!   pending list that applies them at the next (or a scheduled) render
//!   quantum.
//! * [`WebMixer`]: the safe engine: PCM staging, registration under a
//!   [`DecodeBudget`](pin_audio::mixer::DecodeBudget) (64 MiB on the web,
//!   [`DecodeBudget::WEB_BYTES`](pin_audio::mixer::DecodeBudget::WEB_BYTES)),
//!   rendering into an engine-owned stereo buffer.
//! * `www/`: `worklet.js` (the `pin-web-audio` processor), the JS-only
//!   `fallback-worklet.js` and the main-thread helper `audio.js`.
//!
//! For any trigger sequence the output equals native
//! [`Mixer::render`](pin_audio::Mixer::render) bit for bit. Stereo only (SSF
//! 7.1 stays native-only); audio is presentation-only and cannot affect
//! replays. Rendering, triggering and submitting never allocate.

pub mod ffi;
pub mod protocol;
mod web_mixer;

pub use ffi::{
    Engine, REGISTER_NULL_ENGINE, pwa_cmd_buffer, pwa_free, pwa_new, pwa_register, pwa_render,
    pwa_stage, pwa_stat, pwa_submit, pwa_trigger,
};
pub use protocol::{
    At, CMD_BUFFER_BYTES, Command, NEXT_QUANTUM, OP_TRIGGER, PENDING_CAPACITY, RECORD_BYTES,
    Reject, Scheduler, encode_trigger, parse_record,
};
pub use web_mixer::{
    MAX_BUFFER_FRAMES, MAX_SAMPLE_RATE, MIN_SAMPLE_RATE, RegisterError, WebMixer, bus_from_code,
};
