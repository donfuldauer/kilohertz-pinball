//! Independent oracle for the KP-086 spec tests (REQ-WEB-022, D43).
//!
//! Everything here drives native `pin_audio` directly; nothing calls the
//! `pin_web_audio` crate under test. Shared by the `spec_kp086_*` targets
//! through `#[path]`, so some items are unused in some targets.
#![allow(dead_code)]

use std::sync::Arc;

use pin_audio::mixer::{Bus, Mixer, MixerConfig, Sample, SoundSender, SoundTrigger, trigger_ring};
use pin_core::SoundId;

/// The brief's `noise(n, seed)` generator (S6 script).
pub fn noise(n: usize, seed: u64) -> Vec<f32> {
    let mut s = seed;
    (0..n)
        .map(|_| {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((s >> 40) as i32 - (1 << 23)) as f32 / (1u32 << 23) as f32 * 0.5
        })
        .collect()
}

/// FNV-1a 64 over the little-endian bytes of every f32 of `pcm`.
pub fn fnv1a(pcm: &[f32]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for v in pcm {
        for b in v.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

/// One registered sound of a script: `(id, bus code 0..=2, channels, pcm)`.
pub struct Sound {
    pub id: u16,
    pub bus: u32,
    pub channels: u32,
    pub pcm: Vec<f32>,
}

/// Maps a `pwa_register` bus code to the native bus.
pub fn bus_of(code: u32) -> Bus {
    match code {
        0 => Bus::Music,
        1 => Bus::Callout,
        _ => Bus::Sfx,
    }
}

/// The native sample a `(channels, pcm)` registration denotes.
pub fn sample_of(channels: u32, pcm: &[f32]) -> Sample {
    if channels == 1 {
        Sample::from_mono_vec(pcm.to_vec())
    } else {
        Sample::from_interleaved(pcm.to_vec())
    }
}

/// The S6 sounds: id 1 sfx mono noise(9600, 7); id 2 music mono
/// noise(24000, 11); id 3 callout stereo constant(4800, 0.25, -0.125).
pub fn s6_sounds() -> Vec<Sound> {
    let c = Sample::constant(4800, 0.25, -0.125);
    vec![
        Sound {
            id: 1,
            bus: 2,
            channels: 1,
            pcm: noise(9600, 7),
        },
        Sound {
            id: 2,
            bus: 0,
            channels: 1,
            pcm: noise(24000, 11),
        },
        Sound {
            id: 3,
            bus: 1,
            channels: 2,
            pcm: c.data().to_vec(),
        },
    ]
}

/// The S6 triggers `(quantum, id, gain)`.
pub fn s6_triggers() -> Vec<(u32, u16, f32)> {
    vec![
        (0, 2, 0.6),
        (3, 1, 0.9),
        (10, 1, 0.7),
        (11, 3, 1.0),
        (40, 1, 1.2),
        (41, 1, 0.3),
        (42, 1, 0.5),
        (90, 3, 0.8),
        (91, 2, 0.4),
        (200, 1, 1.0),
        (201, 7, 1.0),
        (300, 1, f32::NAN),
    ]
}

/// Native reference: `MixerConfig { sample_rate, buffer_frames: 128, ..Default }`,
/// the sounds registered natively, each trigger sent just before the render
/// of its quantum (the mixer drains the ring first, so it starts at frame 0
/// of that quantum), `quanta` renders of 128 frames. Returns interleaved PCM.
pub fn native_script(
    sample_rate: u32,
    sounds: &[Sound],
    triggers: &[(u32, u16, f32)],
    quanta: u32,
) -> Vec<f32> {
    let (mut tx, rx) = trigger_ring(64);
    let mut m = Mixer::new(
        MixerConfig {
            sample_rate,
            buffer_frames: 128,
            ..MixerConfig::default()
        },
        rx,
    );
    for s in sounds {
        m.register(
            SoundId(s.id),
            Arc::new(sample_of(s.channels, &s.pcm)),
            bus_of(s.bus),
        );
    }
    let mut out = vec![0.0f32; quanta as usize * 256];
    for q in 0..quanta {
        for &(tq, id, gain) in triggers {
            if tq == q {
                assert!(tx.send(SoundTrigger {
                    sound: SoundId(id),
                    gain
                }));
            }
        }
        let s = q as usize * 256;
        m.render(&mut out[s..s + 256]);
    }
    out
}

/// Builds one 16-byte command record by hand from the brief's layout.
pub fn record(op: u32, id: u32, gain_bits: u32, at: u32) -> [u8; 16] {
    let mut r = [0u8; 16];
    r[0..4].copy_from_slice(&op.to_le_bytes());
    r[4..8].copy_from_slice(&id.to_le_bytes());
    r[8..12].copy_from_slice(&gain_bits.to_le_bytes());
    r[12..16].copy_from_slice(&at.to_le_bytes());
    r
}

/// The brief's "next quantum" `at` value.
pub const NEXT: u32 = 0xFFFF_FFFF;

/// Independent model of the `pin_web_audio` engine: the brief's scheduling
/// rule in front of a native `Mixer` + `trigger_ring`.
pub struct Model {
    pub mixer: Mixer,
    tx: SoundSender,
    /// `(effective quantum, arrival, id, gain)`.
    pending: Vec<(u32, u64, u16, f32)>,
    arrival: u64,
    pub q: u32,
    pub dropped: u64,
    pub rejected: u64,
    buffer_frames: u32,
    out: Vec<f32>,
}

/// Pending-list capacity from the brief.
pub const PENDING: usize = 256;

impl Model {
    /// Same configuration as `pwa_new(sample_rate, buffer_frames, cap, _)`.
    pub fn new(sample_rate: u32, buffer_frames: u32, trigger_capacity: u32) -> Self {
        let (tx, rx) = trigger_ring(trigger_capacity as usize);
        let mixer = Mixer::new(
            MixerConfig {
                sample_rate,
                buffer_frames: buffer_frames as usize,
                ..MixerConfig::default()
            },
            rx,
        );
        Self {
            mixer,
            tx,
            pending: Vec::with_capacity(PENDING),
            arrival: 0,
            q: 0,
            dropped: 0,
            rejected: 0,
            buffer_frames,
            out: vec![0.0; buffer_frames as usize * 2],
        }
    }

    /// Registers natively.
    pub fn register(&mut self, id: u16, bus: u32, channels: u32, pcm: &[f32]) {
        self.mixer
            .register(SoundId(id), Arc::new(sample_of(channels, pcm)), bus_of(bus));
    }

    fn accept(&mut self, op: u32, id: u32, gain: f32, at: u32) -> bool {
        if op != 1 || id > 65535 {
            self.rejected += 1;
            return false;
        }
        if self.pending.len() >= PENDING {
            self.dropped += 1;
            return false;
        }
        let eff = if at == NEXT { self.q } else { at.max(self.q) };
        self.pending.push((eff, self.arrival, id as u16, gain));
        self.arrival += 1;
        true
    }

    /// `pwa_submit` over `bytes` (already clamped by the caller to 4096).
    pub fn submit(&mut self, bytes: &[u8]) -> u32 {
        let mut n = 0;
        let mut chunks = bytes.chunks_exact(16);
        for c in &mut chunks {
            let w = |i: usize| u32::from_le_bytes([c[i], c[i + 1], c[i + 2], c[i + 3]]);
            if self.accept(w(0), w(4), f32::from_bits(w(8)), w(12)) {
                n += 1;
            }
        }
        if !chunks.remainder().is_empty() {
            self.rejected += 1;
        }
        n
    }

    /// `pwa_trigger`.
    pub fn trigger(&mut self, id: u32, gain: f32) -> u32 {
        u32::from(self.accept(1, id, gain, NEXT))
    }

    /// `pwa_render`: `None` where the ABI returns null.
    pub fn render(&mut self, frames: u32) -> Option<&[f32]> {
        if frames == 0 || frames > self.buffer_frames {
            return None;
        }
        let q = self.q;
        let mut due: Vec<(u32, u64, u16, f32)> =
            self.pending.iter().copied().filter(|p| p.0 <= q).collect();
        self.pending.retain(|p| p.0 > q);
        due.sort_by_key(|p| (p.0, p.1));
        for (_, _, id, gain) in due {
            if !self.tx.send(SoundTrigger {
                sound: SoundId(id),
                gain,
            }) {
                self.dropped += 1;
            }
        }
        let n = frames as usize * 2;
        self.mixer.render(&mut self.out[..n]);
        self.q += 1;
        Some(&self.out[..n])
    }

    /// The brief's `pwa_stat` selectors 0..=5 and 8 (6 and 7 are budget
    /// figures the caller checks separately).
    pub fn stat(&self, which: u32) -> f64 {
        match which {
            0 => self.dropped as f64,
            1 => self.rejected as f64,
            2 => self.mixer.unknown_sounds() as f64,
            3 => self.mixer.stolen_voices() as f64,
            4 => self.mixer.frames_rendered() as f64,
            5 => f64::from(self.q),
            8 => Bus::ALL
                .into_iter()
                .map(|b| self.mixer.active_voices(b))
                .sum::<usize>() as f64,
            _ => f64::NAN,
        }
    }
}

/// Bitwise slice equality with the first differing index in the message.
pub fn assert_bits_eq(a: &[f32], b: &[f32], what: &str) {
    assert_eq!(a.len(), b.len(), "{what}: length");
    if let Some(i) = (0..a.len()).find(|&i| a[i].to_bits() != b[i].to_bits()) {
        panic!(
            "{what}: first differing value at {i}: {:?} ({:#x}) != {:?} ({:#x})",
            a[i],
            a[i].to_bits(),
            b[i],
            b[i].to_bits()
        );
    }
}
