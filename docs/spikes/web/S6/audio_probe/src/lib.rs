//! S6 check: pin_audio's Mixer behind a C ABI, for a native run and an AudioWorklet run.
use pin_audio::{Bus, Mixer, MixerConfig, Sample, SoundSender, SoundTrigger, trigger_ring};
use pin_core::SoundId;
use std::sync::Arc;

pub struct State {
    mixer: Mixer,
    tx: SoundSender,
    out: Vec<f32>,
}

/// Deterministic PCG-style noise, scalar integer math only.
fn noise(n: usize, seed: u64) -> Vec<f32> {
    let mut s = seed;
    (0..n)
        .map(|_| {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((s >> 40) as i32 - (1 << 23)) as f32 / (1u32 << 23) as f32 * 0.5
        })
        .collect()
}

pub fn new_state(rate: u32, max_frames: usize) -> Box<State> {
    let (tx, rx) = trigger_ring(64);
    let cfg = MixerConfig { sample_rate: rate, buffer_frames: 128, ..MixerConfig::default() };
    let mut mixer = Mixer::new(cfg, rx);
    mixer.register(SoundId(1), Arc::new(Sample::from_mono(&noise(9_600, 7))), Bus::Sfx);
    mixer.register(SoundId(2), Arc::new(Sample::from_mono(&noise(24_000, 11))), Bus::Music);
    mixer.register(SoundId(3), Arc::new(Sample::constant(4_800, 0.25, -0.125)), Bus::Callout);
    Box::new(State { mixer, tx, out: vec![0.0; max_frames * 2] })
}

pub fn trigger(st: &mut State, id: u16, gain: f32) -> bool {
    st.tx.send(SoundTrigger { sound: SoundId(id), gain })
}

pub fn render(st: &mut State, frames: usize) -> &[f32] {
    let buf = &mut st.out[..frames * 2];
    st.mixer.render(buf);
    buf
}

/// The trigger script both runs use: (quantum index, sound, gain).
pub const SCRIPT: &[(u32, u16, f32)] = &[
    (0, 2, 0.6), (3, 1, 0.9), (10, 1, 0.7), (11, 3, 1.0), (40, 1, 1.2), (41, 1, 0.3), (42, 1, 0.5),
    (90, 3, 0.8), (91, 2, 0.4), (200, 1, 1.0), (201, 7, 1.0), (300, 1, f32::NAN),
];

pub fn fnv(h: &mut u64, xs: &[f32]) {
    for x in xs {
        for b in x.to_bits().to_le_bytes() {
            *h ^= u64::from(b);
            *h = h.wrapping_mul(0x100000001b3);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn s6_new(rate: u32, max_frames: u32) -> *mut State {
    Box::into_raw(new_state(rate, max_frames as usize))
}

/// # Safety
/// `st` comes from `s6_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn s6_trigger(st: *mut State, id: u32, gain: f32) -> u32 {
    trigger(unsafe { &mut *st }, id as u16, gain) as u32
}

/// # Safety
/// `st` comes from `s6_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn s6_render(st: *mut State, frames: u32) -> *const f32 {
    render(unsafe { &mut *st }, frames as usize).as_ptr()
}
