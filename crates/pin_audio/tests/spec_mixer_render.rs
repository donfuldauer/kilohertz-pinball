//! Spec tests (Independent Tester, Subphase 3.3): offline rendering.
//!
//! Verification criteria 1–3: trigger latency, ducking curve, voice limiting,
//! plain-sum mixing with DC samples, silence, channel independence, clipping,
//! and the trigger ring. Everything runs on simulated time (frame counts).

#[path = "spec_fixtures/util.rs"]
mod util;

use pin_audio::mixer::{Bus, MixerConfig, SoundTrigger, trigger_ring};
use pin_core::{HostCommandQueue, SoundId};
use util::*;

const EPS: f32 = 1e-6;
/// The brief's tolerance for ducking checkpoints.
const DUCK_TOL: f32 = 1e-4;

/// The brief does not promise `Bus: Copy`, so loops go through an index.
fn bus(i: usize) -> Bus {
    match i {
        0 => Bus::Music,
        1 => Bus::Callout,
        _ => Bus::Sfx,
    }
}

fn cfg() -> MixerConfig {
    MixerConfig::default()
}

/// A long silent callout: holds the duck without adding to the output.
fn hold_callout(rig: &mut Rig, id: u16, frames: usize) {
    let s = dc(0.0, 0.0, frames, RATE);
    rig.reg(id, &s, Bus::Callout);
}

// ---------------------------------------------------------------- config / API

#[test]
fn default_config_matches_the_brief() {
    let c = MixerConfig::default();
    assert_eq!(c.sample_rate, 48_000);
    assert_eq!(c.buffer_frames, 256);
    assert_eq!(c.max_voices, [2, 4, 24]);
    assert_eq!(c.duck_gain, 0.35);
    assert_eq!(c.duck_attack_ms, 30.0);
    assert_eq!(c.duck_release_ms, 250.0);
    assert_eq!(c.master_gain, 1.0);
}

#[test]
fn bus_from_name() {
    assert!(matches!(Bus::from_name("music"), Some(Bus::Music)));
    assert!(matches!(Bus::from_name("callout"), Some(Bus::Callout)));
    assert!(matches!(Bus::from_name("sfx"), Some(Bus::Sfx)));
    for bad in ["", "Music", "SFX", "effects", "music ", "voice"] {
        assert!(Bus::from_name(bad).is_none(), "{bad:?} is not a bus name");
    }
}

#[test]
fn sound_trigger_is_copy_eq_debug() {
    let t = SoundTrigger {
        sound: SoundId(7),
        gain: 0.5,
    };
    let u = t;
    assert_eq!(t, u);
    assert_ne!(
        t,
        SoundTrigger {
            sound: SoundId(8),
            gain: 0.5
        }
    );
    assert!(format!("{t:?}").contains("SoundTrigger"));
}

// ---------------------------------------------------------------- silence

#[test]
fn silence_when_no_voices() {
    let mut rig = Rig::new(cfg());
    let s = dc(0.5, 0.5, 100, RATE);
    rig.reg(1, &s, Bus::Sfx); // registered but never triggered
    for frames in [1, 2, 255, 256, 257, 1000] {
        let out = rig.render(frames);
        assert!(out.iter().all(|&x| x == 0.0), "{frames} frames of silence");
    }
    for b in 0..3 {
        assert_eq!(rig.mixer.active_voices(bus(b)), 0);
    }
    assert_eq!(rig.mixer.duck_level(), 1.0);
    assert_eq!(rig.mixer.stolen_voices(), 0);
    assert_eq!(rig.mixer.unknown_sounds(), 0);
}

#[test]
fn empty_output_buffer_is_fine() {
    let mut rig = Rig::new(cfg());
    let mut out: [f32; 0] = [];
    rig.mixer.render(&mut out);
    assert_eq!(rig.mixer.duck_level(), 1.0);
}

#[test]
fn unknown_sounds_are_counted_and_silent() {
    let mut rig = Rig::new(cfg());
    let s = dc(0.25, 0.25, 100, RATE);
    rig.reg(1, &s, Bus::Sfx);
    rig.play(99, 1.0);
    rig.play(1, 1.0);
    rig.play(u16::MAX, 1.0);
    rig.play(0, 1.0);
    let out = rig.render(10);
    assert_all_frames(&out, 0.25, 0.25, EPS, "only the known sound plays");
    assert_eq!(rig.mixer.unknown_sounds(), 3);
    assert_eq!(rig.mixer.active_voices(Bus::Sfx), 1);
    assert_eq!(rig.mixer.stolen_voices(), 0);
}

// ---------------------------------------------------------------- latency

/// A 600-frame sample whose every frame is distinct: L = (i+1)/1024, R = −L/2.
fn ramp600() -> Vec<f32> {
    (0..600)
        .flat_map(|i| {
            let l = (i + 1) as f32 / 1024.0;
            [l, -l / 2.0]
        })
        .collect()
}

#[test]
fn trigger_lands_on_frame_zero_of_the_next_render() {
    let data = ramp600();
    let s = sample_from(&data, RATE);
    let mut rig = Rig::new(cfg());
    rig.reg(1, &s, Bus::Sfx);

    // A few silent buffers first, then the trigger.
    for _ in 0..3 {
        assert!(rig.render(256).iter().all(|&x| x == 0.0));
    }
    rig.play(1, 1.0);
    let out = rig.render_chunked(4 * 256, 256);
    assert_eq!(
        out[0], data[0],
        "first sample frame is frame 0 of the call (L)"
    );
    assert_eq!(
        out[1], data[1],
        "first sample frame is frame 0 of the call (R)"
    );
    for (i, (a, b)) in out[..1200].iter().zip(&data).enumerate() {
        assert_close(
            *a,
            *b,
            EPS,
            &format!("sample value {i} across buffer boundaries"),
        );
    }
    assert!(
        out[1200..].iter().all(|&x| x == 0.0),
        "silence after the sample ends"
    );
    assert_eq!(
        rig.mixer.active_voices(Bus::Sfx),
        0,
        "voice freed at the end"
    );
}

#[test]
fn trigger_never_waits_more_than_one_buffer() {
    // Whatever the buffer length, a trigger sent between two render calls
    // starts at frame 0 of the second: latency < one buffer.
    let data = ramp600();
    let s = sample_from(&data, RATE);
    for buffer in [1usize, 2, 64, 255, 256, 257, 512, 4096] {
        for b in 0..3 {
            let mut rig = Rig::new(cfg());
            rig.reg(1, &s, bus(b));
            let _ = rig.render(buffer);
            rig.play(1, 1.0);
            let out = rig.render(buffer);
            // First four frames: before any duck ramp could matter (the
            // callout bus is never ducked; the others have no callout).
            for (i, (a, e)) in out.iter().zip(&data).take(8).enumerate() {
                assert_close(*a, *e, EPS, &format!("buffer {buffer}, value {i}"));
            }
        }
    }
}

#[test]
fn all_pending_triggers_are_drained_at_the_start_of_a_render() {
    let mut rig = Rig::with_ring(cfg(), 64);
    for i in 0..64u16 {
        rig.play(1000 + i, 1.0); // all unknown
    }
    let _ = rig.render(1);
    assert_eq!(
        rig.mixer.unknown_sounds(),
        64,
        "one render drains the whole ring"
    );
    // And the ring has room again.
    for i in 0..64u16 {
        rig.play(2000 + i, 1.0);
    }
    assert_eq!(rig.tx.dropped(), 0);
}

#[test]
fn render_split_does_not_change_the_output() {
    // Same trigger schedule rendered as one call and as odd-sized calls.
    let build = || {
        let mut rig = Rig::new(cfg());
        rig.reg(1, &dc(0.25, -0.125, 5_000, RATE), Bus::Music);
        rig.reg(2, &dc(0.0625, 0.03125, 700, RATE), Bus::Callout);
        rig.reg(3, &sample_from(&ramp600(), RATE), Bus::Sfx);
        rig.play(1, 1.0);
        rig.play(2, 0.5);
        rig.play(3, 0.75);
        rig
    };
    let mut whole = build();
    let reference = whole.render(6_000);
    for chunk in [1usize, 7, 256, 333, 1024] {
        let mut rig = build();
        let out = rig.render_chunked(6_000, chunk);
        for (i, (a, b)) in out.iter().zip(&reference).enumerate() {
            assert_close(*a, *b, 2e-5, &format!("chunk {chunk}, value {i}"));
        }
        assert_close(
            rig.mixer.duck_level(),
            whole.mixer.duck_level(),
            2e-5,
            "duck level",
        );
    }
}

// ---------------------------------------------------------------- mixing

#[test]
fn mixing_is_a_plain_sum_of_voices_times_gains() {
    let mut rig = Rig::new(cfg());
    rig.reg(1, &dc(0.25, -0.125, 1_000, RATE), Bus::Sfx);
    rig.reg(2, &dc(0.0625, 0.5, 1_000, RATE), Bus::Sfx);
    rig.reg(3, &dc(-0.03125, 0.015625, 1_000, RATE), Bus::Music);
    rig.play(1, 1.0);
    rig.play(2, 0.5);
    rig.play(3, 2.0);
    let out = rig.render(256);
    // L: 0.25 + 0.03125 − 0.0625 ; R: −0.125 + 0.25 + 0.03125 (all exact in f32)
    assert_all_frames(&out, 0.21875, 0.15625, EPS, "sum of three voices");
    assert_eq!(rig.mixer.active_voices(Bus::Sfx), 2);
    assert_eq!(rig.mixer.active_voices(Bus::Music), 1);
    assert_eq!(rig.mixer.active_voices(Bus::Callout), 0);
}

#[test]
fn stereo_channels_are_independent() {
    let mut rig = Rig::new(cfg());
    rig.reg(1, &dc(0.5, 0.0, 400, RATE), Bus::Sfx);
    rig.reg(2, &dc(0.0, -0.25, 400, RATE), Bus::Music);
    rig.play(1, 1.0);
    let out = rig.render(100);
    assert_all_frames(&out, 0.5, 0.0, 0.0, "left-only sample leaves right silent");
    rig.play(2, 1.0);
    let out = rig.render(100);
    assert_all_frames(&out, 0.5, -0.25, EPS, "left and right never bleed");
}

#[test]
fn the_same_sound_can_overlap_itself() {
    let mut rig = Rig::new(cfg());
    rig.reg(1, &dc(0.125, 0.25, 300, RATE), Bus::Sfx);
    rig.play(1, 1.0);
    let out = rig.render(100);
    assert_all_frames(&out, 0.125, 0.25, EPS, "one voice");
    rig.play(1, 1.0);
    let out = rig.render(200);
    assert_all_frames(&out, 0.25, 0.5, EPS, "two voices of the same sound");
}

#[test]
fn voices_end_independently() {
    let mut rig = Rig::new(cfg());
    rig.reg(1, &dc(0.125, 0.125, 100, RATE), Bus::Sfx);
    rig.reg(2, &dc(0.25, 0.25, 300, RATE), Bus::Sfx);
    rig.play(1, 1.0);
    rig.play(2, 1.0);
    let out = rig.render(400);
    assert_all_frames(&out[..200], 0.375, 0.375, EPS, "both");
    assert_all_frames(&out[200..600], 0.25, 0.25, EPS, "long one only");
    assert_all_frames(&out[600..], 0.0, 0.0, 0.0, "none");
    assert_eq!(rig.mixer.active_voices(Bus::Sfx), 0);
}

#[test]
fn trigger_gain_scales_and_negative_gain_is_clamped_to_zero() {
    for (gain, expect) in [
        (1.0f32, 0.25f32),
        (0.5, 0.125),
        (0.0, 0.0),
        (2.0, 0.5),
        (-1.0, 0.0),
        (-0.001, 0.0),
        (f32::NEG_INFINITY, 0.0),
    ] {
        let mut rig = Rig::new(cfg());
        rig.reg(1, &dc(0.25, -0.25, 100, RATE), Bus::Sfx);
        rig.play(1, gain);
        let out = rig.render(50);
        assert_all_frames(&out, expect, -expect, EPS, &format!("gain {gain}"));
    }
}

#[test]
fn master_gain_scales_everything_including_callouts() {
    let mut rig = Rig::new(MixerConfig {
        master_gain: 0.5,
        ..cfg()
    });
    rig.reg(1, &dc(0.5, -0.25, 100, RATE), Bus::Callout);
    rig.play(1, 1.0);
    let out = rig.render(50);
    assert_all_frames(&out, 0.25, -0.125, EPS, "master 0.5");
}

#[test]
fn output_is_hard_clipped_after_the_master_gain() {
    // Three voices of ±0.5 sum to ±1.5 → clipped to ±1.
    let mut rig = Rig::new(cfg());
    rig.reg(1, &dc(0.5, -0.5, 100, RATE), Bus::Sfx);
    for _ in 0..3 {
        rig.play(1, 1.0);
    }
    let out = rig.render(50);
    assert_all_frames(&out, 1.0, -1.0, 0.0, "clip at ±1");

    // Sum 1.5 with master 0.5 → 0.75: the clip is after the master gain.
    let mut rig = Rig::new(MixerConfig {
        master_gain: 0.5,
        ..cfg()
    });
    rig.reg(1, &dc(0.5, -0.5, 100, RATE), Bus::Sfx);
    for _ in 0..3 {
        rig.play(1, 1.0);
    }
    let out = rig.render(50);
    assert_all_frames(&out, 0.75, -0.75, EPS, "no clip before master gain");

    // Sum 0.75 with master 2 → 1.5 → clipped.
    let mut rig = Rig::new(MixerConfig {
        master_gain: 2.0,
        ..cfg()
    });
    rig.reg(1, &dc(0.75, -0.75, 100, RATE), Bus::Sfx);
    rig.play(1, 1.0);
    let out = rig.render(50);
    assert_all_frames(&out, 1.0, -1.0, 0.0, "clip after master gain");

    // Just inside the range is untouched (hard clip, not a soft knee).
    let mut rig = Rig::new(cfg());
    rig.reg(1, &dc(0.96875, -0.96875, 100, RATE), Bus::Sfx);
    rig.play(1, 1.0);
    let out = rig.render(50);
    assert_all_frames(&out, 0.96875, -0.96875, EPS, "linear right up to ±1");
}

#[test]
fn render_overwrites_the_buffer() {
    // The caller's buffer content must not leak into the output.
    let mut rig = Rig::new(cfg());
    rig.reg(1, &dc(0.25, 0.25, 10, RATE), Bus::Sfx);
    rig.play(1, 1.0);
    let mut out = vec![0.9f32; 40];
    rig.mixer.render(&mut out);
    assert_all_frames(&out[..20], 0.25, 0.25, EPS, "sample");
    assert_all_frames(&out[20..], 0.0, 0.0, 0.0, "then silence");
}

#[test]
fn re_registering_an_id_replaces_the_sound_for_new_triggers() {
    let mut rig = Rig::new(cfg());
    rig.reg(1, &dc(0.25, 0.25, 100, RATE), Bus::Sfx);
    rig.reg(1, &dc(0.5, 0.5, 100, RATE), Bus::Sfx);
    rig.play(1, 1.0);
    let out = rig.render(10);
    assert_all_frames(&out, 0.5, 0.5, EPS, "latest registration wins");
}

// ---------------------------------------------------------------- voice limiting

fn limit_cfg(max_voices: [usize; 3]) -> MixerConfig {
    MixerConfig {
        max_voices,
        ..cfg()
    }
}

/// Registers sounds 1..=6 on `bus` with DC values 1/64, 1/32, … 1/2 (so any
/// sum identifies exactly which voices are playing).
fn reg_bits(rig: &mut Rig, bus_index: usize) {
    for k in 0..6u16 {
        let v = 1.0 / f32::from(1u16 << (6 - k));
        rig.reg(k + 1, &dc(v, v, 100_000, RATE), bus(bus_index));
    }
}

fn bit(id: u16) -> f32 {
    1.0 / f32::from(1u16 << (7 - id))
}

#[test]
fn oldest_voice_is_stolen_when_the_bus_is_full() {
    for b in 0..3 {
        let mut rig = Rig::new(limit_cfg([2, 2, 2]));
        reg_bits(&mut rig, b);
        rig.play(1, 1.0);
        let _ = rig.render(10);
        rig.play(2, 1.0);
        let _ = rig.render(10);
        assert_eq!(rig.mixer.active_voices(bus(b)), 2);
        assert_eq!(rig.mixer.stolen_voices(), 0);

        // Third trigger steals sound 1 (the oldest).
        rig.play(3, 1.0);
        let out = rig.render(10);
        // No ducking in play: music/sfx have no callout, callouts never duck.
        assert_all_frames(&out, bit(2) + bit(3), bit(2) + bit(3), EPS, "1 stolen");
        assert_eq!(rig.mixer.active_voices(bus(b)), 2);
        assert_eq!(rig.mixer.stolen_voices(), 1);

        // Fourth steals sound 2, which is now the oldest although sound 3
        // sits in the slot that was freed first.
        rig.play(4, 1.0);
        let out = rig.render(10);
        assert_all_frames(&out, bit(3) + bit(4), bit(3) + bit(4), EPS, "2 stolen");
        assert_eq!(rig.mixer.stolen_voices(), 2);

        rig.play(5, 1.0);
        let out = rig.render(10);
        assert_all_frames(&out, bit(4) + bit(5), bit(4) + bit(5), EPS, "3 stolen");
        assert_eq!(rig.mixer.stolen_voices(), 3);
        assert_eq!(rig.mixer.active_voices(bus(b)), 2);
    }
}

#[test]
fn triggers_in_one_render_call_steal_consistently() {
    // 1, 2 and 3 arrive in the same call on a two-voice bus: 3 steals 1
    // (older by arrival, and in the lowest slot if the two count as a tie).
    let mut rig = Rig::new(limit_cfg([2, 2, 2]));
    reg_bits(&mut rig, 2);
    rig.play(1, 1.0);
    rig.play(2, 1.0);
    rig.play(3, 1.0);
    let out = rig.render(10);
    assert_all_frames(&out, bit(2) + bit(3), bit(2) + bit(3), EPS, "1 stolen");
    assert_eq!(rig.mixer.stolen_voices(), 1);
    // Next call: 4 steals exactly one of them. The brief allows either: 2 is
    // older by arrival, but 2 and 3 started on the same frame, which the
    // brief's tie rule resolves by slot.
    rig.play(4, 1.0);
    let out = rig.render(10);
    let by_arrival = bit(3) + bit(4);
    let by_slot_tie = bit(2) + bit(4);
    assert!(
        out.iter().all(|&x| (x - by_arrival).abs() <= EPS)
            || out.iter().all(|&x| (x - by_slot_tie).abs() <= EPS),
        "exactly one of the two tied voices is stolen, got {}",
        out[0]
    );
    assert_eq!(rig.mixer.stolen_voices(), 2);
    assert_eq!(rig.mixer.active_voices(Bus::Sfx), 2);
}

#[test]
fn a_stealing_voice_starts_from_its_first_frame() {
    let data = ramp600();
    let mut rig = Rig::new(limit_cfg([1, 1, 1]));
    rig.reg(1, &dc(0.5, 0.5, 10_000, RATE), Bus::Sfx);
    rig.reg(2, &sample_from(&data, RATE), Bus::Sfx);
    rig.play(1, 1.0);
    let _ = rig.render(256);
    rig.play(2, 1.0);
    let out = rig.render(256);
    for (i, (a, b)) in out.iter().zip(&data).enumerate() {
        assert_close(
            *a,
            *b,
            EPS,
            &format!("value {i}: stolen voice fully replaced"),
        );
    }
    assert_eq!(rig.mixer.stolen_voices(), 1);
    assert_eq!(rig.mixer.active_voices(Bus::Sfx), 1);
}

#[test]
fn voice_limits_are_per_bus() {
    let mut rig = Rig::new(limit_cfg([1, 1, 2]));
    rig.reg(1, &dc(0.25, 0.25, 100_000, RATE), Bus::Music);
    rig.reg(2, &dc(0.0, 0.0, 100_000, RATE), Bus::Callout);
    rig.reg(3, &dc(0.03125, 0.03125, 100_000, RATE), Bus::Sfx);
    rig.play(1, 1.0);
    rig.play(2, 1.0);
    for _ in 0..5 {
        rig.play(3, 1.0);
    }
    let _ = rig.render(64);
    assert_eq!(rig.mixer.active_voices(Bus::Music), 1, "music untouched");
    assert_eq!(
        rig.mixer.active_voices(Bus::Callout),
        1,
        "callout untouched"
    );
    assert_eq!(rig.mixer.active_voices(Bus::Sfx), 2);
    assert_eq!(rig.mixer.stolen_voices(), 3, "only sfx voices were stolen");
}

#[test]
fn default_limits_are_2_4_24() {
    let mut rig = Rig::new(cfg());
    rig.reg(1, &dc(0.001, 0.001, 100_000, RATE), Bus::Music);
    rig.reg(2, &dc(0.001, 0.001, 100_000, RATE), Bus::Callout);
    rig.reg(3, &dc(0.001, 0.001, 100_000, RATE), Bus::Sfx);
    for _ in 0..5 {
        rig.play(1, 1.0);
    }
    for _ in 0..7 {
        rig.play(2, 1.0);
    }
    for _ in 0..30 {
        rig.play(3, 1.0);
    }
    let _ = rig.render(256);
    assert_eq!(rig.mixer.active_voices(Bus::Music), 2);
    assert_eq!(rig.mixer.active_voices(Bus::Callout), 4);
    assert_eq!(rig.mixer.active_voices(Bus::Sfx), 24);
    assert_eq!(rig.mixer.stolen_voices(), 3 + 3 + 6);
}

#[test]
fn a_finished_voice_frees_its_slot_without_a_steal() {
    let mut rig = Rig::new(limit_cfg([1, 1, 1]));
    rig.reg(1, &dc(0.25, 0.25, 100, RATE), Bus::Sfx);
    rig.play(1, 1.0);
    let _ = rig.render(101);
    assert_eq!(rig.mixer.active_voices(Bus::Sfx), 0);
    rig.play(1, 1.0);
    let out = rig.render(10);
    assert_all_frames(&out, 0.25, 0.25, EPS, "plays again");
    assert_eq!(rig.mixer.stolen_voices(), 0, "nothing was stolen");
}

// ---------------------------------------------------------------- ducking

/// Default curve at 48 kHz: attack 30 ms = 1440 frames, release 250 ms =
/// 12 000 frames, between 1.0 and 0.35.
const ATTACK: usize = 1_440;
const RELEASE: usize = 12_000;
const DUCK: f32 = 0.35;
const ATTACK_STEP: f32 = (1.0 - DUCK) / ATTACK as f32;

fn attack_level(frames: usize) -> f32 {
    (1.0 - (1.0 - DUCK) * frames as f32 / ATTACK as f32).max(DUCK)
}

#[test]
fn duck_attack_checkpoints() {
    for chunk in [usize::MAX, 256, 100] {
        let mut rig = Rig::new(cfg());
        hold_callout(&mut rig, 1, 50_000);
        assert_eq!(rig.mixer.duck_level(), 1.0);
        rig.play(1, 1.0);
        let _ = rig.render_chunked(ATTACK / 2, chunk.min(ATTACK));
        assert_close(rig.mixer.duck_level(), 0.675, DUCK_TOL, "attack midpoint");
        let _ = rig.render_chunked(ATTACK / 4, chunk.min(ATTACK));
        assert_close(rig.mixer.duck_level(), 0.5125, DUCK_TOL, "attack 3/4");
        let _ = rig.render_chunked(ATTACK / 4, chunk.min(ATTACK));
        assert_close(rig.mixer.duck_level(), DUCK, DUCK_TOL, "attack end");
        let _ = rig.render_chunked(3_000, chunk.min(ATTACK));
        assert_close(rig.mixer.duck_level(), DUCK, 1e-6, "holds at duck_gain");
        assert!(
            rig.mixer.duck_level() >= DUCK - 1e-6,
            "never below duck_gain"
        );
    }
}

#[test]
fn duck_release_checkpoints() {
    const CALLOUT: usize = 2_000; // longer than the attack, not a multiple of 256
    for chunk in [usize::MAX, 256, 100] {
        let mut rig = Rig::new(cfg());
        hold_callout(&mut rig, 1, CALLOUT);
        rig.play(1, 1.0);
        let _ = rig.render_chunked(CALLOUT, chunk.min(CALLOUT));
        assert_close(
            rig.mixer.duck_level(),
            DUCK,
            DUCK_TOL,
            "fully ducked at callout end",
        );
        let _ = rig.render_chunked(RELEASE / 2, chunk.min(CALLOUT));
        assert_close(rig.mixer.duck_level(), 0.675, DUCK_TOL, "release midpoint");
        assert_eq!(rig.mixer.active_voices(Bus::Callout), 0);
        let _ = rig.render_chunked(RELEASE / 2 + 1, chunk.min(CALLOUT));
        assert_close(rig.mixer.duck_level(), 1.0, DUCK_TOL, "release end");
        let _ = rig.render_chunked(1_000, chunk.min(CALLOUT));
        assert_eq!(rig.mixer.duck_level(), 1.0, "rests at exactly 1.0");
    }
}

#[test]
fn duck_applies_to_music_and_sfx_but_not_callouts() {
    let mut rig = Rig::new(cfg());
    rig.reg(1, &dc(0.25, -0.25, 100_000, RATE), Bus::Music);
    rig.reg(2, &dc(0.125, 0.0625, 100_000, RATE), Bus::Sfx);
    rig.reg(3, &dc(0.0625, -0.03125, 4_000, RATE), Bus::Callout);
    rig.play(1, 1.0);
    rig.play(2, 1.0);
    let out = rig.render(256);
    assert_all_frames(&out, 0.375, -0.1875, EPS, "no callout: no duck");
    assert_eq!(rig.mixer.duck_level(), 1.0);

    rig.play(3, 1.0);
    let out = rig.render(ATTACK);
    // Frame k is ducked by the level after k or k+1 updates (the brief fixes
    // the per-frame ramp, not which side of the update the frame is mixed on).
    let mut prev = f32::INFINITY;
    for (k, f) in out.chunks_exact(2).enumerate() {
        let hi = 0.0625 + 0.375 * attack_level(k) + DUCK_TOL;
        let lo = 0.0625 + 0.375 * attack_level(k + 1) - DUCK_TOL;
        assert!(
            f[0] >= lo && f[0] <= hi,
            "attack frame {k}: left {} outside [{lo}, {hi}]",
            f[0]
        );
        let hi_r = -0.03125 - 0.1875 * attack_level(k + 1) + DUCK_TOL;
        let lo_r = -0.03125 - 0.1875 * attack_level(k) - DUCK_TOL;
        assert!(
            f[1] >= lo_r && f[1] <= hi_r,
            "attack frame {k}: right {} outside [{lo_r}, {hi_r}]",
            f[1]
        );
        assert!(f[0] <= prev + 1e-6, "attack is monotonic at frame {k}");
        prev = f[0];
    }

    // Plateau: callout at full level, music + sfx at 0.35.
    let out = rig.render(1_000);
    assert_all_frames(
        &out,
        0.0625 + 0.375 * DUCK,
        -0.03125 - 0.1875 * DUCK,
        2e-6,
        "plateau",
    );

    // Run past the callout end and the whole release: back to the plain sum.
    let _ = rig.render(4_000 - ATTACK - 1_000 + RELEASE + 1);
    let out = rig.render(256);
    assert_all_frames(&out, 0.375, -0.1875, EPS, "released");
}

#[test]
fn duck_release_is_a_linear_per_frame_ramp_in_the_output() {
    const CALLOUT: usize = 1_500;
    let mut rig = Rig::new(cfg());
    rig.reg(1, &dc(0.5, 0.5, 100_000, RATE), Bus::Music);
    hold_callout(&mut rig, 2, CALLOUT);
    rig.play(1, 1.0);
    rig.play(2, 1.0);
    let _ = rig.render(CALLOUT);
    let out = rig.render_chunked(RELEASE + 512, 256);
    let level = |k: usize| (DUCK + (1.0 - DUCK) * k as f32 / RELEASE as f32).min(1.0);
    for (k, f) in out.chunks_exact(2).enumerate() {
        // One frame of slack either side (see the attack test), within 1e-4.
        let lo = 0.5 * level(k.saturating_sub(1)) - DUCK_TOL;
        let hi = 0.5 * level(k + 1) + DUCK_TOL;
        assert!(
            f[0] >= lo && f[0] <= hi,
            "release frame {k}: {} outside [{lo}, {hi}]",
            f[0]
        );
        assert_eq!(f[0], f[1]);
    }
    // Midpoint and end, sample-exact within the brief's tolerance.
    assert_close(
        out[2 * (RELEASE / 2)],
        0.5 * 0.675,
        DUCK_TOL,
        "release midpoint",
    );
    assert_close(out[2 * (RELEASE + 2)], 0.5, 1e-6, "release end");
}

#[test]
fn a_short_callout_ducks_partially_then_releases_at_the_release_slope() {
    // 720-frame callout: the level reaches the attack midpoint (0.675), then
    // climbs back at (1 − duck_gain)/release per frame.
    let mut rig = Rig::new(cfg());
    hold_callout(&mut rig, 1, ATTACK / 2);
    rig.play(1, 1.0);
    let _ = rig.render(ATTACK / 2);
    assert_close(rig.mixer.duck_level(), 0.675, DUCK_TOL, "half attack");
    let _ = rig.render(3_000);
    let expect = 0.675 + (1.0 - DUCK) * 3_000.0 / RELEASE as f32;
    // One attack step of slack: whether the callout's last frame counts.
    assert_close(
        rig.mixer.duck_level(),
        expect,
        ATTACK_STEP + DUCK_TOL,
        "partial release",
    );
    let _ = rig.render(3_100);
    assert_close(rig.mixer.duck_level(), 1.0, 1e-6, "back at 1.0 early");
}

#[test]
fn a_callout_during_release_resumes_the_attack_from_the_current_level() {
    let mut rig = Rig::new(cfg());
    hold_callout(&mut rig, 1, 2_000);
    rig.play(1, 1.0);
    let _ = rig.render(2_000);
    let _ = rig.render(RELEASE / 2);
    assert_close(rig.mixer.duck_level(), 0.675, DUCK_TOL, "release midpoint");
    // New callout: from 0.675 down at the attack slope; 360 frames → 0.5125.
    rig.play(1, 1.0);
    let _ = rig.render(ATTACK / 4);
    assert_close(
        rig.mixer.duck_level(),
        0.675 - (1.0 - DUCK) * 0.25,
        2.0 * DUCK_TOL,
        "attack resumes from the current level",
    );
    let _ = rig.render(ATTACK / 4);
    assert_close(
        rig.mixer.duck_level(),
        DUCK,
        2.0 * DUCK_TOL,
        "and reaches duck_gain",
    );
    let _ = rig.render(100);
    assert_close(rig.mixer.duck_level(), DUCK, 1e-6, "and stays there");
}

#[test]
fn overlapping_callouts_hold_the_duck_until_the_last_one_ends() {
    let mut rig = Rig::new(cfg());
    hold_callout(&mut rig, 1, 2_000);
    rig.play(1, 1.0);
    let _ = rig.render(1_500);
    rig.play(1, 1.0); // second callout ends at frame 3 500
    let _ = rig.render(1_900);
    assert_eq!(rig.mixer.active_voices(Bus::Callout), 1);
    assert_close(
        rig.mixer.duck_level(),
        DUCK,
        1e-6,
        "still ducked at frame 3 400",
    );
    let _ = rig.render(100 + RELEASE / 2);
    assert_close(
        rig.mixer.duck_level(),
        0.675,
        DUCK_TOL,
        "release counted from 3 500",
    );
}

#[test]
fn music_and_sfx_alone_never_duck() {
    let mut rig = Rig::new(cfg());
    rig.reg(1, &dc(0.25, 0.25, 5_000, RATE), Bus::Music);
    rig.reg(2, &dc(0.125, 0.125, 5_000, RATE), Bus::Sfx);
    rig.play(1, 1.0);
    rig.play(2, 1.0);
    let out = rig.render(4_000);
    assert_all_frames(&out, 0.375, 0.375, EPS, "no callout, no duck");
    assert_eq!(rig.mixer.duck_level(), 1.0);
}

#[test]
fn duck_curve_follows_a_custom_config_and_sample_rate() {
    // 44.1 kHz, duck to 0.5 in 10 ms (441 frames), release in 20 ms (882).
    let rate = 44_100;
    let mut rig = Rig::new(MixerConfig {
        sample_rate: rate,
        duck_gain: 0.5,
        duck_attack_ms: 10.0,
        duck_release_ms: 20.0,
        ..cfg()
    });
    rig.reg(1, &dc(0.0, 0.0, 1_000, rate), Bus::Callout);
    rig.reg(2, &dc(0.5, 0.5, 50_000, rate), Bus::Sfx);
    rig.play(1, 1.0);
    rig.play(2, 1.0);
    let _ = rig.render(147);
    assert_close(
        rig.mixer.duck_level(),
        1.0 - 0.5 / 3.0,
        DUCK_TOL,
        "attack third",
    );
    let _ = rig.render(294);
    assert_close(rig.mixer.duck_level(), 0.5, DUCK_TOL, "attack end");
    let out = rig.render(559); // callout ends at frame 1 000
    assert_all_frames(&out, 0.25, 0.25, 2e-6, "plateau at duck_gain 0.5");
    let _ = rig.render(441);
    // Release step is 5.7e-4 here: allow one frame of slack at the callout end.
    assert_close(
        rig.mixer.duck_level(),
        0.75,
        0.5 / 882.0 + DUCK_TOL,
        "release midpoint",
    );
    let _ = rig.render(443);
    assert_close(rig.mixer.duck_level(), 1.0, 1e-6, "release end");
    let out = rig.render(64);
    assert_all_frames(&out, 0.5, 0.5, EPS, "sfx back at full level");
}

// ---------------------------------------------------------------- trigger ring

#[test]
fn full_ring_drops_and_counts() {
    let (mut tx, rx) = trigger_ring(4);
    let t = |i: u16| SoundTrigger {
        sound: SoundId(i),
        gain: 1.0,
    };
    for i in 0..4 {
        assert!(tx.send(t(i)), "send {i} fits");
    }
    assert_eq!(tx.dropped(), 0);
    assert!(!tx.send(t(4)), "fifth does not fit");
    assert!(!tx.send(t(5)));
    assert_eq!(tx.dropped(), 2);

    // The four that fit are delivered; the dropped ones are not.
    let mut mixer = pin_audio::mixer::Mixer::new(cfg(), rx);
    let mut out = [0.0f32; 8];
    mixer.render(&mut out);
    assert_eq!(mixer.unknown_sounds(), 4);
    assert!(tx.send(t(6)), "room again after a render");
    assert_eq!(tx.dropped(), 2);
    mixer.render(&mut out);
    assert_eq!(mixer.unknown_sounds(), 5);
}

#[test]
fn send_queue_forwards_every_sound_in_order() {
    let mut rig = Rig::new(limit_cfg([1, 1, 1]));
    reg_bits(&mut rig, 2);
    let mut q = HostCommandQueue::default();
    q.play_sound(SoundId(1), 1.0).unwrap();
    q.play_sound(SoundId(2), 0.5).unwrap();
    q.play_sound(SoundId(3), 0.25).unwrap();
    assert_eq!(rig.tx.send_queue(&q), 3);
    assert_eq!(q.sounds.len(), 3, "the queue itself is not modified");
    let out = rig.render(16);
    // One sfx voice: the last trigger in queue order survives, with its gain.
    assert_all_frames(
        &out,
        bit(3) * 0.25,
        bit(3) * 0.25,
        EPS,
        "last in order wins",
    );
    assert_eq!(rig.mixer.stolen_voices(), 2);
    assert_eq!(
        rig.tx.send_queue(&HostCommandQueue::default()),
        0,
        "empty queue"
    );
}

#[test]
fn send_queue_carries_gains_and_a_full_queue() {
    let mut rig = Rig::new(cfg());
    rig.reg(1, &dc(1.0 / 64.0, -1.0 / 128.0, 1_000, RATE), Bus::Sfx);
    let mut q = HostCommandQueue::default();
    for _ in 0..16 {
        q.play_sound(SoundId(1), 0.5).unwrap();
    }
    assert_eq!(rig.tx.send_queue(&q), 16);
    let out = rig.render(32);
    assert_all_frames(&out, 0.125, -0.0625, EPS, "16 voices × 1/64 × 0.5");
    assert_eq!(rig.mixer.active_voices(Bus::Sfx), 16);
}

#[test]
fn send_queue_reports_how_many_fit() {
    let (mut tx, rx) = trigger_ring(2);
    let mut q = HostCommandQueue::default();
    for i in 0..5 {
        q.play_sound(SoundId(i), 1.0).unwrap();
    }
    assert_eq!(tx.send_queue(&q), 2, "only two fit");
    assert_eq!(tx.dropped(), 3, "the rest are counted as dropped");
    let mut mixer = pin_audio::mixer::Mixer::new(cfg(), rx);
    let mut out = [0.0f32; 4];
    mixer.render(&mut out);
    assert_eq!(mixer.unknown_sounds(), 2);
}

#[test]
fn the_sender_works_from_another_thread() {
    // The ring is the sim-thread → audio-thread hand-over: the two ends must
    // be usable on different threads (no sleeping, no timing assertion).
    let (mut tx, rx) = trigger_ring(128);
    let handle = std::thread::spawn(move || {
        for i in 0..100u16 {
            assert!(tx.send(SoundTrigger {
                sound: SoundId(i),
                gain: 1.0
            }));
        }
        tx.dropped()
    });
    assert_eq!(handle.join().unwrap(), 0);
    let mut mixer = std::thread::spawn(move || {
        let mut mixer = pin_audio::mixer::Mixer::new(MixerConfig::default(), rx);
        let mut out = [0.0f32; 2];
        mixer.render(&mut out);
        mixer
    })
    .join()
    .unwrap();
    assert_eq!(mixer.unknown_sounds(), 100);
    let mut out = [0.5f32; 2];
    mixer.render(&mut out);
    assert_eq!(out, [0.0, 0.0]);
}
