//! Implementer offline render tests for `pin_audio::mixer` (Subphase 3.3).

use std::sync::Arc;

use pin_audio::mixer::{Bus, Mixer, MixerConfig, Sample, SoundSender, SoundTrigger, trigger_ring};
use pin_core::SoundId;
use pin_core::script::HostCommandQueue;

const MUSIC: SoundId = SoundId(1);
const CALLOUT: SoundId = SoundId(2);
const SFX: SoundId = SoundId(3);

fn setup(cfg: MixerConfig) -> (SoundSender, Mixer) {
    let (tx, rx) = trigger_ring(64);
    (tx, Mixer::new(cfg, rx))
}

fn trig(tx: &mut SoundSender, sound: SoundId, gain: f32) {
    assert!(tx.send(SoundTrigger { sound, gain }));
}

fn render(m: &mut Mixer, frames: usize) -> Vec<f32> {
    let mut out = vec![9.0; frames * 2];
    m.render(&mut out);
    out
}

#[test]
fn silence_without_voices() {
    let (_tx, mut m) = setup(MixerConfig::default());
    assert!(render(&mut m, 300).iter().all(|&v| v == 0.0));
    let mut odd = [5.0f32; 7];
    m.render(&mut odd);
    assert_eq!(odd, [0.0; 7]);
    assert_eq!(m.frames_rendered(), 303);
}

#[test]
fn trigger_lands_at_frame_zero_of_next_call() {
    let cfg = MixerConfig::default();
    let (mut tx, mut m) = setup(cfg);
    // An impulse: first frame 1.0 then zeros.
    let mut data = vec![0.0; 64];
    data[0] = 0.5;
    data[1] = -0.25;
    m.register(SFX, Arc::new(Sample::from_interleaved(data)), Bus::Sfx);
    for call in 0..5 {
        let _ = render(&mut m, cfg.buffer_frames);
        trig(&mut tx, SFX, 1.0);
        let out = render(&mut m, cfg.buffer_frames);
        assert_eq!(&out[..2], &[0.5, -0.25], "call {call}");
        assert!(out[2..].iter().all(|&v| v == 0.0));
    }
}

#[test]
fn plain_sum_and_independent_channels() {
    let (mut tx, mut m) = setup(MixerConfig::default());
    m.register(
        SFX,
        Arc::new(Sample::constant(1000, 0.25, -0.125)),
        Bus::Sfx,
    );
    m.register(
        MUSIC,
        Arc::new(Sample::constant(1000, 0.0, 0.5)),
        Bus::Music,
    );
    trig(&mut tx, SFX, 0.5);
    trig(&mut tx, SFX, 2.0);
    trig(&mut tx, MUSIC, 0.25);
    let out = render(&mut m, 100);
    for f in out.chunks(2) {
        assert_eq!(f[0], 0.25 * 0.5 + 0.25 * 2.0);
        assert_eq!(f[1], -0.125 * 0.5 + -0.125 * 2.0 + 0.5 * 0.25);
    }
    assert_eq!(m.active_voices(Bus::Sfx), 2);
    assert_eq!(m.active_voices(Bus::Music), 1);
}

#[test]
fn negative_and_nan_gain_are_silent_and_clipping() {
    let (mut tx, mut m) = setup(MixerConfig {
        master_gain: 2.0,
        ..MixerConfig::default()
    });
    m.register(SFX, Arc::new(Sample::constant(10, 0.75, -0.25)), Bus::Sfx);
    trig(&mut tx, SFX, -1.0);
    trig(&mut tx, SFX, f32::NAN);
    assert!(render(&mut m, 10).iter().all(|&v| v == 0.0));
    trig(&mut tx, SFX, 1.0);
    let out = render(&mut m, 10);
    assert_eq!(&out[..2], &[1.0, -0.5]);
}

#[test]
fn voice_ends_with_its_sample() {
    let (mut tx, mut m) = setup(MixerConfig::default());
    m.register(SFX, Arc::new(Sample::constant(300, 0.5, 0.5)), Bus::Sfx);
    trig(&mut tx, SFX, 1.0);
    let out = render(&mut m, 512);
    assert!(out[..600].iter().all(|&v| v == 0.5));
    assert!(out[600..].iter().all(|&v| v == 0.0));
    assert_eq!(m.active_voices(Bus::Sfx), 0);
}

#[test]
fn unknown_and_replaced_sounds() {
    let (mut tx, mut m) = setup(MixerConfig::default());
    trig(&mut tx, SoundId(77), 1.0);
    let _ = render(&mut m, 4);
    assert_eq!(m.unknown_sounds(), 1);
    m.register(SFX, Arc::new(Sample::constant(100, 0.5, 0.5)), Bus::Sfx);
    m.register(SoundId(4), Arc::new(Sample::default()), Bus::Sfx);
    trig(&mut tx, SFX, 1.0);
    trig(&mut tx, SoundId(4), 1.0); // zero-length: starts nothing
    let _ = render(&mut m, 4);
    assert_eq!(m.active_voices(Bus::Sfx), 1);
    assert!(m.is_registered(SFX) && !m.is_registered(SoundId(5)));
    m.register(SFX, Arc::new(Sample::constant(100, 0.1, 0.1)), Bus::Music);
    assert_eq!(m.active_voices(Bus::Sfx), 0);
}

#[test]
fn oldest_voice_is_stolen() {
    let cfg = MixerConfig {
        max_voices: [2, 4, 3],
        ..MixerConfig::default()
    };
    let (mut tx, mut m) = setup(cfg);
    // Distinct DC levels identify which voices are playing.
    for (i, level) in [
        (10u16, 1.0 / 8.0),
        (11, 1.0 / 16.0),
        (12, 1.0 / 32.0),
        (13, 1.0 / 64.0),
        (14, 1.0 / 128.0),
    ] {
        m.register(
            SoundId(i),
            Arc::new(Sample::constant(10_000, level, level)),
            Bus::Sfx,
        );
    }
    trig(&mut tx, SoundId(10), 1.0); // slot 0, age t0
    let _ = render(&mut m, 8);
    trig(&mut tx, SoundId(11), 1.0); // slot 1, age t1
    let _ = render(&mut m, 8);
    trig(&mut tx, SoundId(12), 1.0); // slot 2, age t2
    trig(&mut tx, SoundId(13), 1.0); // steals 10 (oldest), slot 0
    let out = render(&mut m, 8);
    assert_eq!(out[0], 1.0 / 16.0 + 1.0 / 32.0 + 1.0 / 64.0);
    assert_eq!(m.stolen_voices(), 1);
    // Next steal: 11 (t1) is oldest.
    trig(&mut tx, SoundId(14), 1.0);
    let out = render(&mut m, 8);
    assert_eq!(out[0], 1.0 / 32.0 + 1.0 / 64.0 + 1.0 / 128.0);
    // Now 12 and 13 started together (tie) at slots 2 and 0: lowest slot (0,
    // sound 13) is stolen... but 14 (slot 1) is younger, so 13 goes.
    trig(&mut tx, SoundId(10), 1.0);
    let out = render(&mut m, 8);
    assert_eq!(out[0], 1.0 / 8.0 + 1.0 / 32.0 + 1.0 / 128.0);
    assert_eq!(m.stolen_voices(), 3);
    assert_eq!(m.active_voices(Bus::Sfx), 3);
}

#[test]
fn muted_bus_ignores_triggers() {
    let (mut tx, mut m) = setup(MixerConfig {
        max_voices: [0, 4, 24],
        ..MixerConfig::default()
    });
    m.register(MUSIC, Arc::new(Sample::constant(10, 0.5, 0.5)), Bus::Music);
    trig(&mut tx, MUSIC, 1.0);
    assert!(render(&mut m, 10).iter().all(|&v| v == 0.0));
    assert_eq!(m.stolen_voices(), 0);
}

fn duck_formula(cfg: &MixerConfig, k: f64, ms: f32) -> f64 {
    let frames = f64::from(ms) * f64::from(cfg.sample_rate) / 1000.0;
    (1.0 - f64::from(cfg.duck_gain)) * (k / frames).min(1.0)
}

#[test]
fn ducking_curve_checkpoints() {
    let cfg = MixerConfig::default();
    let (mut tx, mut m) = setup(cfg);
    let attack = 1440; // 30 ms at 48 kHz
    let release = 12_000; // 250 ms
    let callout_len = 4000;
    m.register(
        MUSIC,
        Arc::new(Sample::constant(100_000, 1.0, 0.5)),
        Bus::Music,
    );
    m.register(
        CALLOUT,
        Arc::new(Sample::constant(callout_len, 0.0, 0.0)),
        Bus::Callout,
    );
    trig(&mut tx, MUSIC, 0.5);
    trig(&mut tx, CALLOUT, 1.0);
    // Render in odd-sized pieces to exercise block boundaries.
    let mut out = Vec::new();
    while out.len() < 2 * (callout_len + release + 100) {
        out.extend(render(&mut m, 333));
    }
    let left = |k: usize| f64::from(out[2 * k]);
    let right = |k: usize| f64::from(out[2 * k + 1]);
    for k in [0, 1, attack / 2, attack, attack + 1, 3000] {
        let d = 1.0 - duck_formula(&cfg, k as f64, cfg.duck_attack_ms);
        assert!((left(k) - 0.5 * d).abs() < 1e-6, "attack k={k}");
        assert!((right(k) - 0.25 * d).abs() < 1e-6, "attack k={k}");
    }
    let g = f64::from(cfg.duck_gain);
    for r in [0, release / 2, release, release + 50] {
        let k = callout_len + r;
        let d = g + duck_formula(&cfg, r as f64, cfg.duck_release_ms);
        assert!((left(k) - 0.5 * d).abs() < 1e-6, "release r={r}");
    }
    assert_eq!(m.duck_level(), 1.0);
}

#[test]
fn duck_level_after_whole_buffers_and_callouts_not_ducked() {
    let cfg = MixerConfig::default();
    let (mut tx, mut m) = setup(cfg);
    m.register(
        CALLOUT,
        Arc::new(Sample::constant(48_000, 0.5, 0.5)),
        Bus::Callout,
    );
    trig(&mut tx, CALLOUT, 1.0);
    let out = render(&mut m, 720);
    assert!(out.iter().all(|&v| v == 0.5));
    assert!((m.duck_level() - 0.675).abs() < 1e-6);
    let _ = render(&mut m, 720);
    assert!((m.duck_level() - 0.35).abs() < 1e-6);
    let _ = render(&mut m, 1000);
    assert!((m.duck_level() - 0.35).abs() < 1e-6);
}

#[test]
fn instant_ramps() {
    let (mut tx, mut m) = setup(MixerConfig {
        duck_attack_ms: 0.0,
        duck_release_ms: 0.0,
        ..MixerConfig::default()
    });
    m.register(
        CALLOUT,
        Arc::new(Sample::constant(4, 0.0, 0.0)),
        Bus::Callout,
    );
    trig(&mut tx, CALLOUT, 1.0);
    let _ = render(&mut m, 2);
    assert_eq!(m.duck_level(), 0.35);
    let _ = render(&mut m, 4);
    assert_eq!(m.duck_level(), 1.0);
}

#[test]
fn send_queue_feeds_mixer() {
    let (mut tx, mut m) = setup(MixerConfig::default());
    m.register(SFX, Arc::new(Sample::constant(10, 0.5, 0.5)), Bus::Sfx);
    let mut q = HostCommandQueue::default();
    q.play_sound(SFX, 1.0).unwrap();
    q.play_sound(SoundId(99), 1.0).unwrap();
    assert_eq!(tx.send_queue(&q), 2);
    let out = render(&mut m, 2);
    assert_eq!(out[0], 0.5);
    assert_eq!(m.unknown_sounds(), 1);
}

#[test]
fn bus_names() {
    for b in Bus::ALL {
        assert_eq!(Bus::from_name(b.name()), Some(b));
    }
    assert_eq!(Bus::from_name("Music"), None);
    assert_eq!(
        Bus::from(pin_core::schema::table::SoundBus::Callout),
        Bus::Callout
    );
    assert_eq!(Bus::Sfx.index(), 2);
    assert_eq!(MixerConfig::default().max_voices, [2, 4, 24]);
}

/// Non-gating timing report: `cargo test --release -p pin_audio --test
/// mixer_render -- --ignored --nocapture`.
#[test]
#[ignore]
fn report_render_time_30_voices() {
    let (mut tx, mut m) = setup(MixerConfig::default());
    m.register(
        SFX,
        Arc::new(Sample::constant(10_000_000, 0.01, 0.01)),
        Bus::Sfx,
    );
    m.register(
        MUSIC,
        Arc::new(Sample::constant(10_000_000, 0.01, 0.01)),
        Bus::Music,
    );
    m.register(
        CALLOUT,
        Arc::new(Sample::constant(10_000_000, 0.01, 0.01)),
        Bus::Callout,
    );
    for _ in 0..24 {
        trig(&mut tx, SFX, 1.0);
    }
    for _ in 0..2 {
        trig(&mut tx, MUSIC, 1.0);
    }
    for _ in 0..4 {
        trig(&mut tx, CALLOUT, 1.0);
    }
    let mut out = vec![0.0; 512];
    m.render(&mut out);
    let n = 10_000;
    let t = std::time::Instant::now();
    for _ in 0..n {
        m.render(&mut out);
    }
    let per = t.elapsed() / n;
    println!("render 256 frames, 30 voices: {per:?} per call");
}
