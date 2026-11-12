//! `pin_audio`: Audio mixer, SSF channel mixer and UDP telemetry output.
//!
//! Module directories are pre-stubbed by Subphase 1.1 (PLAN.md §2.1, D3); each
//! module names the subphase that owns it.
//!
//! * [`mixer`] (Subphase 3.3): three-bus real-time mixer with voice limiting,
//!   callout ducking, WAV/Vorbis decoding and an SDL3 output stream.

pub mod mixer;
pub mod ssf;
pub mod telemetry;

pub use mixer::{
    Bus, DecodeError, Mixer, MixerConfig, Sample, SdlOutput, SoundReceiver, SoundSender,
    SoundTrigger, decode, trigger_ring,
};
