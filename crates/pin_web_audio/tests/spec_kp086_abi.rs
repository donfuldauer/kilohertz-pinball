//! Independent Tester, KP-086 (#180): C ABI contract tests for
//! `pin_web_audio` (REQ-WEB-022, D43, D25).
//!
//! Black box: only the brief's `pwa_*` contract is used; expected values come
//! from native `pin_audio` (`spec_kp086/oracle.rs`).

#[path = "spec_kp086/eng.rs"]
mod eng;
#[path = "spec_kp086/oracle.rs"]
mod oracle;

use eng::Eng;
use oracle::*;
use pin_audio::mixer::Sample;
use pin_web_audio::ffi::{pwa_free, pwa_new, pwa_stat};

const MIB: f64 = 1024.0 * 1024.0;

/// REQ-WEB-022: every `pwa_new` null case, and accepting cases at the edges.
#[test]
fn req_web_022_pwa_new_null_cases_and_accepting_edges() {
    for (rate, frames, cap) in [
        (7999, 128, 64),
        (384_001, 128, 64),
        (0, 128, 64),
        (u32::MAX, 128, 64),
        (48_000, 0, 64),
        (48_000, 4097, 64),
        (48_000, u32::MAX, 64),
        (48_000, 128, 0),
    ] {
        let e = pwa_new(rate, frames, cap, 64);
        assert!(
            e.is_null(),
            "pwa_new({rate}, {frames}, {cap}, 64) must be null"
        );
    }
    for (rate, frames, cap) in [
        (8000, 1, 1),
        (384_000, 4096, 1),
        (48_000, 128, 64),
        (44_100, 128, 64),
    ] {
        let e = Eng::new(rate, frames, cap, 64);
        assert!(e.is_some(), "pwa_new({rate}, {frames}, {cap}, 64) accepted");
    }
}

/// REQ-WEB-022: `pwa_free(null)` is a no-op; `pwa_stat` is NaN for an
/// unknown selector.
#[test]
fn req_web_022_free_null_and_unknown_stat_selector() {
    // SAFETY: the brief makes pwa_free null-safe.
    unsafe { pwa_free(std::ptr::null_mut()) };
    let e = Eng::new(48_000, 128, 64, 64).unwrap();
    for which in [9, 10, 100, u32::MAX] {
        assert!(e.stat(which).is_nan(), "stat({which}) must be NaN");
    }
    for which in 0..=8 {
        assert!(!e.stat(which).is_nan(), "stat({which}) is defined");
    }
    // A fresh engine: nothing dropped, rejected, unknown, stolen, rendered.
    for which in [0, 1, 2, 3, 4, 5, 6, 8] {
        assert_eq!(e.stat(which), 0.0, "fresh stat({which})");
    }
    // Null engine: documented by the implementation as NaN (not in the
    // brief, so only checked for not crashing).
    // SAFETY: null engine; the rustdoc says NaN for a null `e`.
    let _ = unsafe { pwa_stat(std::ptr::null_mut(), 0) };
}

/// REQ-WEB-022, D25: `pwa_stat(7) == budget_mib << 20`, and `pwa_stage` is
/// null exactly when `n_values * 4` exceeds the budget limit.
#[test]
fn req_web_022_d25_budget_limit_and_stage_bound() {
    for mib in [0u32, 1, 2, 64, 4096, 5000] {
        let e = Eng::new(48_000, 128, 64, mib).unwrap();
        assert_eq!(e.stat(7), f64::from(mib) * MIB, "stat 7 for {mib} MiB");
    }
    let mut e = Eng::new(48_000, 128, 64, 1).unwrap();
    assert!(
        e.stage(262_144, &[]),
        "exactly 1 MiB of values may be staged"
    );
    assert!(!e.stage(262_145, &[]), "1 MiB + 4 bytes must be null");
    assert!(!e.stage(u32::MAX, &[]), "u32::MAX values must be null");
    let mut e = Eng::new(48_000, 128, 64, 64).unwrap();
    assert!(e.stage(16 << 20, &[]), "64 MiB of values may be staged");
    assert!(!e.stage((16 << 20) + 1, &[]));
    assert!(!e.stage(u32::MAX, &[]));
    let mut e = Eng::new(48_000, 128, 64, 0).unwrap();
    assert!(!e.stage(1, &[]), "a 0 MiB budget stages nothing");
}

/// REQ-WEB-022: `pwa_register` codes -1, -2, -3, -4 and 0, with nothing
/// charged on a rejection.
#[test]
fn req_web_022_register_return_codes() {
    let mut e = Eng::new(48_000, 128, 64, 64).unwrap();
    let pcm = noise(64, 3);
    assert!(e.stage(64, &pcm));
    assert_eq!(e.register_raw(65_536, 2, 1, 8), -1, "id 65536");
    assert_eq!(e.register_raw(u32::MAX, 2, 1, 8), -1, "id u32::MAX");
    assert_eq!(e.register_raw(1, 3, 1, 8), -2, "bus 3");
    assert_eq!(e.register_raw(1, u32::MAX, 1, 8), -2, "bus u32::MAX");
    assert_eq!(e.register_raw(1, 2, 0, 8), -3, "channels 0");
    assert_eq!(e.register_raw(1, 2, 3, 8), -3, "channels 3");
    assert_eq!(e.register_raw(1, 2, 1, 0), -4, "frames 0");
    assert_eq!(e.register_raw(1, 2, 1, 65), -4, "mono frames > staged");
    assert_eq!(e.register_raw(1, 2, 2, 33), -4, "stereo frames*2 > staged");
    assert_eq!(e.register_raw(1, 2, 2, u32::MAX), -4, "overflowing frames");
    assert_eq!(e.register_raw(1, 2, 1, u32::MAX), -4, "huge frames");
    assert_eq!(e.stat(6), 0.0, "rejections charge nothing");
    assert_eq!(e.register_raw(65_535, 2, 1, 64), 0, "id 65535 mono");
    assert_eq!(e.stat(6), 256.0);
    assert_eq!(e.register_raw(0, 1, 2, 32), 0, "id 0 stereo");
    assert_eq!(e.stat(6), 512.0);
}

/// REQ-WEB-022, D25: budget accounting equals `Sample::bytes()` (4 bytes
/// per mono frame, 8 per stereo frame); replacing an id returns the old
/// bytes first.
#[test]
fn req_web_022_d25_budget_accounting_and_replace() {
    let mut e = Eng::new(48_000, 128, 64, 64).unwrap();
    let mono = noise(1000, 1);
    let stereo = noise(2000, 2);
    assert_eq!(e.register(1, 2, 1, &mono), 0);
    let mb = Sample::from_mono_vec(mono.clone()).bytes();
    assert_eq!(mb, 4000);
    assert_eq!(e.stat(6), mb as f64);
    assert_eq!(e.register(2, 0, 2, &stereo), 0);
    let sb = Sample::from_interleaved(stereo.clone()).bytes();
    assert_eq!(sb, 8000);
    assert_eq!(e.stat(6), (mb + sb) as f64);
    // Replace id 2 with a smaller sample: used goes back down.
    assert_eq!(e.register(2, 0, 1, &mono[..10]), 0);
    assert_eq!(e.stat(6), (mb + 40) as f64);
    // Replace id 1 with a bigger one: up by the difference only.
    assert_eq!(e.register(1, 2, 2, &stereo), 0);
    assert_eq!(e.stat(6), (sb + 40) as f64);
}

/// REQ-WEB-022, D25: -5 when the budget is exhausted; nothing is charged or
/// registered, and an existing sound with that id keeps playing (output
/// equals a native mixer holding the old sample).
#[test]
fn req_web_022_d25_register_budget_exhausted_keeps_existing() {
    // 2 MiB budget.
    let mut e = Eng::new(48_000, 128, 64, 2).unwrap();
    let a: Vec<f32> = noise(131_072, 5); // 0.5 MiB mono
    let b: Vec<f32> = noise(327_680, 6); // 1.25 MiB mono
    assert_eq!(e.register(1, 2, 1, &a), 0);
    assert_eq!(e.register(2, 0, 1, &b), 0);
    let used = e.stat(6);
    assert_eq!(used, 1.75 * MIB);
    // New id that does not fit: -5, nothing charged.
    let c = noise(65_537, 8); // 0.25 MiB + 4 bytes
    assert_eq!(e.register(3, 2, 1, &c), -5);
    assert_eq!(e.stat(6), used);
    // Exactly the remaining 0.25 MiB fits (succeeds exactly at the limit).
    assert_eq!(e.register(3, 2, 1, &c[..65_536]), 0);
    assert_eq!(e.stat(6), 2.0 * MIB);
    // Replacing id 1 with 0.875 MiB: after releasing id 1 (0.5) the total
    // would be 2.375 MiB > 2: -5, id 1 keeps the old sample and the bytes.
    let big = noise(229_376, 9);
    assert_eq!(e.register(1, 2, 1, &big), -5);
    assert_eq!(e.stat(6), 2.0 * MIB, "a failed replace must not leak bytes");
    // Id 1 still plays the old sample.
    let mut m = Model::new(48_000, 128, 64);
    m.register(1, 2, 1, &a);
    m.register(2, 0, 1, &b);
    m.register(3, 2, 1, &c[..65_536]);
    assert_eq!(e.trigger(1, 0.75), 1);
    m.trigger(1, 0.75);
    for q in 0..20 {
        let got = e.render(128).unwrap().to_vec();
        let want = m.render(128).unwrap();
        assert_bits_eq(&got, want, &format!("quantum {q}"));
    }
    assert!(e.render(128).unwrap().iter().any(|v| *v != 0.0));
}

/// REQ-WEB-022: `pwa_render` is null for 0 frames and for more than
/// `buffer_frames`, and a null render does not advance the quantum index.
#[test]
fn req_web_022_render_null_cases() {
    let mut e = Eng::new(48_000, 64, 64, 64).unwrap();
    assert!(e.render(0).is_none());
    assert!(e.render(65).is_none());
    assert!(e.render(u32::MAX).is_none());
    assert_eq!(e.stat(5), 0.0);
    assert_eq!(e.stat(4), 0.0);
    assert_eq!(e.render(64).unwrap().len(), 128);
    assert_eq!(e.render(1).unwrap().len(), 2);
    assert_eq!(e.stat(5), 2.0);
    assert_eq!(e.stat(4), 65.0);
}

/// REQ-WEB-022 (stereo): PCM registered mono and stereo through the ABI
/// renders bit for bit like `Sample::from_mono_vec` / `from_interleaved`
/// registered natively, on every bus, at 48 000 and 44 100 Hz.
#[test]
fn req_web_022_mono_and_stereo_pcm_render_like_native() {
    for rate in [48_000, 44_100] {
        let mut e = Eng::new(rate, 128, 64, 64).unwrap();
        let mut m = Model::new(rate, 128, 64);
        let sounds = [
            (5u32, 2u32, 1u32, noise(3000, 21)),
            (6, 1, 2, noise(5000, 22)),
            (7, 0, 2, noise(9000, 23)),
            (8, 2, 1, noise(700, 24)),
        ];
        for (id, bus, ch, pcm) in &sounds {
            assert_eq!(e.register(*id, *bus, *ch, pcm), 0);
            m.register(*id as u16, *bus, *ch, pcm);
        }
        let plan = [
            (0, 5, 0.8),
            (0, 7, 0.5),
            (2, 6, 1.0),
            (5, 8, 0.3),
            (9, 5, 1.1),
        ];
        for q in 0..60u32 {
            for &(tq, id, g) in &plan {
                if tq == q {
                    assert_eq!(e.trigger(id, g), 1);
                    m.trigger(id, g);
                }
            }
            let got = e.render(128).unwrap().to_vec();
            assert_bits_eq(&got, m.render(128).unwrap(), &format!("{rate} Hz q {q}"));
        }
        for which in [0, 1, 2, 3, 4, 5, 8] {
            assert_eq!(e.stat(which), m.stat(which), "{rate} Hz stat {which}");
        }
    }
}

/// REQ-WEB-022: the S6 script through the ABI (records built by hand)
/// equals native `Mixer::render` bit for bit, and its FNV-1a digest matches.
#[test]
fn req_web_022_s6_script_through_abi_equals_native_digest() {
    for rate in [48_000, 44_100] {
        let want = native_script(rate, &s6_sounds(), &s6_triggers(), 600);
        let mut e = Eng::new(rate, 128, 64, 64).unwrap();
        for s in s6_sounds() {
            assert_eq!(e.register(u32::from(s.id), s.bus, s.channels, &s.pcm), 0);
        }
        let mut bytes = Vec::new();
        for (q, id, g) in s6_triggers() {
            bytes.extend_from_slice(&record(1, u32::from(id), g.to_bits(), q));
        }
        assert_eq!(e.submit(&bytes), 12);
        let mut got = Vec::with_capacity(want.len());
        for _ in 0..600 {
            got.extend_from_slice(e.render(128).unwrap());
        }
        assert_bits_eq(&got, &want, &format!("S6 at {rate} Hz"));
        assert_eq!(fnv1a(&got), fnv1a(&want));
        assert_eq!(e.stat(5), 600.0);
        assert_eq!(e.stat(4), 76_800.0);
        assert_eq!(e.stat(2), 1.0, "id 7 is unknown");
        println!("S6 native digest at {rate} Hz: {:016x}", fnv1a(&want));
    }
}
