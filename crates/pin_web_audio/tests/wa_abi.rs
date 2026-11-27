//! REQ-WEB-022: the `pwa_*` C ABI, called as plain Rust: every return code,
//! the statistics, the scheduling rules and native parity of the S6 script.

use std::sync::Arc;

use pin_audio::mixer::DecodeBudget;
use pin_audio::{Bus, Mixer, MixerConfig, Sample, SoundTrigger, trigger_ring};
use pin_core::SoundId;
use pin_web_audio::*;

fn noise(n: usize, seed: u64) -> Vec<f32> {
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

fn fnv(h: &mut u64, xs: &[f32]) {
    for x in xs {
        for b in x.to_bits().to_le_bytes() {
            *h ^= u64::from(b);
            *h = h.wrapping_mul(0x100000001b3);
        }
    }
}

const SCRIPT: &[(u32, u32, f32)] = &[
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
];

struct Guard(*mut Engine);
impl Drop for Guard {
    fn drop(&mut self) {
        unsafe { pwa_free(self.0) };
    }
}

fn new(rate: u32, frames: u32, cap: u32, mib: u32) -> Guard {
    let e = pwa_new(rate, frames, cap, mib);
    assert!(!e.is_null());
    Guard(e)
}

fn stage(e: *mut Engine, values: &[f32]) {
    let p = unsafe { pwa_stage(e, values.len() as u32) };
    assert!(!p.is_null());
    unsafe { std::ptr::copy_nonoverlapping(values.as_ptr(), p, values.len()) };
}

fn render(e: *mut Engine, frames: u32) -> Vec<f32> {
    let p = unsafe { pwa_render(e, frames) };
    assert!(!p.is_null());
    unsafe { std::slice::from_raw_parts(p, frames as usize * 2) }.to_vec()
}

fn submit(e: *mut Engine, bytes: &[u8]) -> u32 {
    let buf = unsafe { pwa_cmd_buffer(e) };
    assert!(!buf.is_null());
    let n = bytes.len().min(CMD_BUFFER_BYTES);
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, n) };
    unsafe { pwa_submit(e, n as u32) }
}

/// REQ-WEB-022: `pwa_new` range checks.
#[test]
fn req_web_022_new_ranges() {
    assert!(pwa_new(7_999, 128, 64, 64).is_null());
    assert!(pwa_new(384_001, 128, 64, 64).is_null());
    assert!(pwa_new(48_000, 0, 64, 64).is_null());
    assert!(pwa_new(48_000, 4097, 64, 64).is_null());
    assert!(pwa_new(48_000, 128, 0, 64).is_null());
    for (r, f) in [(8_000, 1), (384_000, 4096), (48_000, 128)] {
        let g = new(r, f, 1, 0);
        assert_eq!(unsafe { pwa_stat(g.0, 7) }, 0.0);
        assert_eq!(unsafe { pwa_stat(g.0, 5) }, 0.0);
    }
    unsafe { pwa_free(std::ptr::null_mut()) };
}

/// REQ-WEB-022: a null engine never crashes.
#[test]
fn req_web_022_null_engine() {
    let n = std::ptr::null_mut();
    unsafe {
        assert!(pwa_stage(n, 4).is_null());
        assert_eq!(pwa_register(n, 1, 0, 1, 1), REGISTER_NULL_ENGINE);
        assert_eq!(pwa_trigger(n, 1, 1.0), 0);
        assert!(pwa_cmd_buffer(n).is_null());
        assert_eq!(pwa_submit(n, 16), 0);
        assert!(pwa_render(n, 128).is_null());
        assert!(pwa_stat(n, 0).is_nan());
    }
}

/// REQ-WEB-022 / D25: every `pwa_register` code and exact budget accounting.
#[test]
fn req_web_022_register_codes_and_budget() {
    let g = new(48_000, 128, 64, 1); // 1 MiB budget
    let e = g.0;
    assert_eq!(unsafe { pwa_stat(e, 7) }, (1u64 << 20) as f64);
    // Staging limit: n * 4 > limit -> null.
    assert!(unsafe { pwa_stage(e, (1 << 18) + 1) }.is_null());
    assert!(!unsafe { pwa_stage(e, 1 << 18) }.is_null());
    stage(e, &[0.25; 100]);
    unsafe {
        assert_eq!(pwa_register(e, 65_536, 0, 1, 10), -1);
        assert_eq!(pwa_register(e, 1, 3, 1, 10), -2);
        assert_eq!(pwa_register(e, 1, 0, 0, 10), -3);
        assert_eq!(pwa_register(e, 1, 0, 3, 10), -3);
        assert_eq!(pwa_register(e, 1, 0, 1, 0), -4);
        assert_eq!(pwa_register(e, 1, 0, 1, 101), -4);
        assert_eq!(pwa_register(e, 1, 0, 2, 51), -4);
        assert_eq!(pwa_register(e, 1, 0, 2, u32::MAX), -4);
        assert_eq!(pwa_stat(e, 6), 0.0);
        assert_eq!(pwa_register(e, 1, 2, 1, 100), 0);
        assert_eq!(pwa_stat(e, 6), 400.0);
        assert_eq!(pwa_register(e, 65_535, 1, 2, 50), 0);
        assert_eq!(pwa_stat(e, 6), 800.0);
        // Replacing releases the old bytes first.
        assert_eq!(pwa_register(e, 1, 0, 1, 10), 0);
        assert_eq!(pwa_stat(e, 6), 440.0);
    }
    // Fill the budget: one big sound of (1 MiB - 440 - 40) bytes keeps 40 free.
    let big = ((1usize << 20) - 440 - 40) / 4;
    stage(e, &vec![0.0; big]);
    unsafe {
        assert_eq!(pwa_register(e, 2, 0, 1, big as u32), 0);
        assert_eq!(pwa_stat(e, 6), ((1u64 << 20) - 40) as f64);
    }
    stage(e, &[0.5; 64]);
    unsafe {
        // 11 mono frames (44 bytes) for a new id do not fit: -5, nothing charged.
        assert_eq!(pwa_register(e, 3, 2, 1, 11), -5);
        assert_eq!(pwa_stat(e, 6), ((1u64 << 20) - 40) as f64);
        // Replacing id 1 (40 bytes) by 20 frames (80 bytes): 40 + 40 free fits.
        assert_eq!(pwa_register(e, 1, 2, 1, 20), 0);
        assert_eq!(pwa_stat(e, 6), (1u64 << 20) as f64);
        // Replacing id 1 by 21 frames (84 bytes) does not fit: old kept.
        assert_eq!(pwa_register(e, 1, 2, 1, 21), -5);
        assert_eq!(pwa_stat(e, 6), (1u64 << 20) as f64);
    }
    // The kept old sound (20 frames of 0.5 on sfx) still plays.
    assert_eq!(unsafe { pwa_trigger(e, 1, 1.0) }, 1);
    let out = render(e, 32);
    assert_eq!(&out[..40], &[0.5; 40]);
    assert_eq!(&out[40..], &[0.0; 24]);
}

/// REQ-WEB-022: `pwa_render` null cases and the stats it advances.
#[test]
fn req_web_022_render_and_stats() {
    let g = new(48_000, 64, 4, 1);
    let e = g.0;
    unsafe {
        assert!(pwa_render(e, 0).is_null());
        assert!(pwa_render(e, 65).is_null());
        assert_eq!(pwa_stat(e, 5), 0.0);
        assert!(!pwa_render(e, 64).is_null());
        assert!(!pwa_render(e, 1).is_null());
        assert_eq!(pwa_stat(e, 4), 65.0);
        assert_eq!(pwa_stat(e, 5), 2.0);
        for w in [9, 100, u32::MAX] {
            assert!(pwa_stat(e, w).is_nan());
        }
    }
    stage(e, &[0.1; 1000]);
    assert_eq!(unsafe { pwa_register(e, 4, 2, 1, 1000) }, 0);
    // Unknown id, then five triggers into a ring of 4: one ring-full drop.
    unsafe {
        assert_eq!(pwa_trigger(e, 9, 1.0), 1);
        for _ in 0..4 {
            assert_eq!(pwa_trigger(e, 4, 0.1), 1);
        }
        assert_eq!(pwa_trigger(e, 70_000, 1.0), 0);
        assert_eq!(pwa_stat(e, 1), 1.0);
    }
    render(e, 64);
    unsafe {
        assert_eq!(pwa_stat(e, 0), 1.0, "ring full");
        assert_eq!(pwa_stat(e, 2), 1.0, "unknown id 9");
        // Default sfx voices: 24; 3 voices of id 4 started (the 4th dropped).
        assert_eq!(pwa_stat(e, 8), 3.0);
        assert_eq!(pwa_stat(e, 3), 0.0);
    }
}

/// REQ-WEB-022: command records: rejects, trailing partial, the 4096 clamp,
/// a full pending list, and scheduling at past/future/next quanta.
#[test]
fn req_web_022_submit_rules() {
    let g = new(48_000, 128, 256, 1);
    let e = g.0;
    stage(e, &[1.0; 4]);
    assert_eq!(unsafe { pwa_register(e, 1, 2, 1, 4) }, 0);
    let mut b = Vec::new();
    b.extend_from_slice(&encode_trigger(1, 0.5, 2)); // quantum 2
    b.extend_from_slice(&encode_trigger(1, 0.25, NEXT_QUANTUM)); // quantum 0
    let mut bad = encode_trigger(1, 1.0, 0);
    bad[0] = 9;
    b.extend_from_slice(&bad);
    b.extend_from_slice(&encode_trigger(65_536, 1.0, 0));
    b.extend_from_slice(&[1, 2, 3]);
    assert_eq!(submit(e, &b), 2);
    assert_eq!(unsafe { pwa_stat(e, 1) }, 3.0);
    let q0 = render(e, 8);
    assert_eq!(&q0[..8], &[0.25; 8]);
    assert_eq!(&q0[8..], &[0.0; 8]);
    let q1 = render(e, 8);
    assert_eq!(q1, vec![0.0; 16]);
    // A record in the past (at 0, now q = 2) applies at quantum 2 after the
    // earlier-submitted at-2 record.
    assert_eq!(submit(e, &encode_trigger(1, 0.125, 0)), 1);
    let q2 = render(e, 4);
    assert_eq!(q2, vec![0.625; 8]);
    // The 4096-byte clamp: 300 records offered via n_bytes, only 256 parsed.
    let buf = unsafe { pwa_cmd_buffer(e) };
    for i in 0..256 {
        let r = encode_trigger(1, 0.0, 1_000);
        unsafe { std::ptr::copy_nonoverlapping(r.as_ptr(), buf.add(i * 16), 16) };
    }
    assert_eq!(unsafe { pwa_submit(e, 300 * 16) }, 256);
    // The pending list is full now: one more is dropped (stat 0).
    assert_eq!(unsafe { pwa_trigger(e, 1, 1.0) }, 0);
    assert_eq!(unsafe { pwa_stat(e, 0) }, 1.0);
    assert_eq!(submit(e, &encode_trigger(1, 1.0, 5)), 0);
    assert_eq!(unsafe { pwa_stat(e, 0) }, 2.0);
}

/// Native reference: the S6 script through `Mixer::render` directly.
fn native_digest() -> u64 {
    let (mut tx, rx) = trigger_ring(64);
    let cfg = MixerConfig {
        sample_rate: 48_000,
        buffer_frames: 128,
        ..MixerConfig::default()
    };
    let mut m = Mixer::new(cfg, rx);
    m.register(
        SoundId(1),
        Arc::new(Sample::from_mono(&noise(9_600, 7))),
        Bus::Sfx,
    );
    m.register(
        SoundId(2),
        Arc::new(Sample::from_mono(&noise(24_000, 11))),
        Bus::Music,
    );
    m.register(
        SoundId(3),
        Arc::new(Sample::constant(4_800, 0.25, -0.125)),
        Bus::Callout,
    );
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut out = vec![0.0f32; 256];
    for q in 0..600u32 {
        for &(at, id, g) in SCRIPT {
            if at == q {
                tx.send(SoundTrigger {
                    sound: SoundId(id as u16),
                    gain: g,
                });
            }
        }
        m.render(&mut out);
        fnv(&mut h, &out);
    }
    h
}

/// REQ-WEB-022: the ABI driven like the worklet (PCM staged, every trigger
/// scheduled up front with `at`) equals native `Mixer::render` bit for bit,
/// and so does the immediate `pwa_trigger` path.
#[test]
fn req_web_022_abi_parity_with_native() {
    let native = native_digest();
    for scheduled in [true, false] {
        let g = new(48_000, 128, 64, (DecodeBudget::WEB_BYTES >> 20) as u32);
        let e = g.0;
        stage(e, &noise(9_600, 7));
        assert_eq!(unsafe { pwa_register(e, 1, 2, 1, 9_600) }, 0);
        stage(e, &noise(24_000, 11));
        assert_eq!(unsafe { pwa_register(e, 2, 0, 1, 24_000) }, 0);
        stage(e, Sample::constant(4_800, 0.25, -0.125).data());
        assert_eq!(unsafe { pwa_register(e, 3, 1, 2, 4_800) }, 0);
        if scheduled {
            let mut b = Vec::new();
            for &(at, id, gain) in SCRIPT {
                b.extend_from_slice(&encode_trigger(id, gain, at));
            }
            assert_eq!(submit(e, &b), SCRIPT.len() as u32);
        }
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for q in 0..600u32 {
            if !scheduled {
                for &(at, id, gain) in SCRIPT {
                    if at == q {
                        assert_eq!(unsafe { pwa_trigger(e, id, gain) }, 1);
                    }
                }
            }
            fnv(&mut h, &render(e, 128));
        }
        assert_eq!(h, native, "scheduled = {scheduled}");
        unsafe {
            assert_eq!(pwa_stat(e, 5), 600.0);
            assert_eq!(pwa_stat(e, 4), 600.0 * 128.0);
            assert_eq!(pwa_stat(e, 2), 1.0, "id 7 is unknown");
            assert_eq!(pwa_stat(e, 0), 0.0);
        }
    }
    println!("native S6 digest: {native:016x}");
}

/// REQ-WEB-022: the safe wrapper exposes the same engine.
#[test]
fn req_web_022_web_mixer_wrapper() {
    assert!(WebMixer::new(48_000, 128, 0, 1).is_none());
    let mut m = WebMixer::new(48_000, 128, 8, 1).unwrap();
    assert!(format!("{m:?}").contains("WebMixer"));
    m.stage(2).unwrap().copy_from_slice(&[0.5, -0.5]);
    assert_eq!(m.register(1, 1, 2, 1), Ok(()));
    assert_eq!(m.register(1, 1, 2, 2), Err(RegisterError::Frames));
    assert_eq!(RegisterError::Budget.code(), -5);
    assert_eq!(bus_from_code(0), Some(Bus::Music));
    assert_eq!(bus_from_code(3), None);
    assert!(m.mixer().is_registered(SoundId(1)));
    assert_eq!(m.submit_bytes(&encode_trigger(1, 1.0, NEXT_QUANTUM)), 1);
    assert_eq!(m.pending(), 1);
    assert_eq!(m.render(1).unwrap(), &[0.5, -0.5]);
    assert_eq!(m.pending(), 0);
    assert_eq!(m.quantum(), 1);
    assert_eq!(m.budget().used_bytes(), 8);
    assert_eq!((m.dropped(), m.rejected(), m.active_voices()), (0, 0, 0));
    m.cmd_buffer()[..16].copy_from_slice(&encode_trigger(1, 1.0, NEXT_QUANTUM));
    assert_eq!(m.submit(16), 1);
    // Clamped to 4096 bytes: record 0 is valid, the 255 zeroed ones (op 0)
    // are rejected.
    assert_eq!(m.submit(u32::MAX), 1);
    assert_eq!(m.rejected(), 255);
}
