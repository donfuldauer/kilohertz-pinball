//! Independent Tester spec tests, Subphase 3.4, deliverable 3: the SSF voice
//! mixer renders haptic events (ball roll loop gain/pitch from speed, impulses
//! for coil/bumper) into 8-channel buffers offline.
//!
//! Black box: the brief leaves this API open, so these tests use the public
//! signatures and documentation of `pin_audio::ssf::{SsfMixer, SsfConfig}`.

// Style lints only; test code favours explicit index loops and step-by-step setup.
#![allow(
    clippy::field_reassign_with_default,
    clippy::manual_is_multiple_of,
    clippy::needless_range_loop,
    clippy::type_complexity
)]

use pin_audio::ssf::{
    CH_BL, CH_BR, CH_FC, CH_FL, CH_FR, CH_LFE, CH_SL, CH_SR, SsfConfig, SsfMixer, SsfPanner,
};
use pin_core::haptics::{HapticEvent, HapticKind};
use pin_core::ids::{CoilId, FlipperId};
use proptest::prelude::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Counting;

thread_local! {
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
}

fn bump() {
    let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
}

// SAFETY: forwards to the system allocator, only adding a thread-local counter.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        bump();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        bump();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        bump();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn allocs() -> u64 {
    ALLOCS.with(Cell::get)
}

// ------------------------------------------------------------------ helpers

const RATE: u32 = 48_000;
/// Frames per 1 kHz sim tick at 48 kHz.
const TICK_FRAMES: usize = 48;
const EXCITERS: [usize; 4] = [CH_SL, CH_SR, CH_BL, CH_BR];
const FRONT: [usize; 4] = [CH_FL, CH_FR, CH_FC, CH_LFE];

fn config() -> SsfConfig {
    let cfg = SsfConfig::default();
    assert_eq!(cfg.sample_rate, RATE, "documented default rate");
    assert_eq!(cfg.playfield_mm, [-257.0, 257.0, 0.0, 1070.0]);
    cfg
}

fn roll(tick: u64, ball: u16, x: f32, y: f32, speed: f32) -> HapticEvent {
    HapticEvent::new(
        tick,
        HapticKind::BallRoll {
            ball,
            position_mm: [x, y, 0.0],
            mass_kg: 0.0806,
            speed_m_s: speed,
        },
        1.0,
    )
}

fn coil(tick: u64, n: u16, strength: f32) -> HapticEvent {
    HapticEvent::new(tick, HapticKind::CoilFire { coil: CoilId(n) }, strength)
}

fn bumper(tick: u64, n: u16, strength: f32) -> HapticEvent {
    HapticEvent::new(tick, HapticKind::Bumper { mech: n }, strength)
}

fn drop_at(tick: u64, x: f32, y: f32, strength: f32) -> HapticEvent {
    HapticEvent::new(
        tick,
        HapticKind::BallDrop {
            ball: 0,
            position_mm: [x, y, 0.0],
        },
        strength,
    )
}

/// Renders `frames` frames with no further events.
fn render(m: &mut SsfMixer, frames: usize) -> Vec<f32> {
    let mut out = vec![f32::NAN; frames * 8];
    m.render(&mut out);
    out
}

/// Rolls one ball for `ticks` sim ticks (one event and 48 frames per tick).
fn roll_for(m: &mut SsfMixer, ticks: u64, x: f32, y: f32, speed: f32) -> Vec<f32> {
    let mut out = Vec::with_capacity(ticks as usize * TICK_FRAMES * 8);
    let mut buf = [0.0f32; TICK_FRAMES * 8];
    for t in 0..ticks {
        m.feed(roll(t, 0, x, y, speed));
        m.render(&mut buf);
        out.extend_from_slice(&buf);
    }
    out
}

fn channel(buf: &[f32], ch: usize) -> Vec<f32> {
    buf.chunks_exact(8).map(|f| f[ch]).collect()
}

fn rms(buf: &[f32], ch: usize) -> f64 {
    let n = buf.len() / 8;
    let sum: f64 = buf
        .chunks_exact(8)
        .map(|f| f64::from(f[ch]) * f64::from(f[ch]))
        .sum();
    (sum / n.max(1) as f64).sqrt()
}

fn total_rms(buf: &[f32]) -> f64 {
    EXCITERS
        .iter()
        .map(|&c| rms(buf, c).powi(2))
        .sum::<f64>()
        .sqrt()
}

fn peak(buf: &[f32]) -> f32 {
    buf.iter().fold(0.0f32, |a, &v| a.max(v.abs()))
}

fn zero_crossings(samples: &[f32]) -> usize {
    samples
        .windows(2)
        .filter(|w| (w[0] < 0.0 && w[1] >= 0.0) || (w[0] >= 0.0 && w[1] < 0.0))
        .count()
}

fn assert_sane(buf: &[f32]) {
    for (i, f) in buf.chunks_exact(8).enumerate() {
        for &ch in &FRONT {
            assert_eq!(f[ch], 0.0, "frame {i}: channel {ch} must stay silent");
        }
        for &ch in &EXCITERS {
            assert!(
                f[ch].is_finite() && f[ch].abs() <= 1.0,
                "frame {i}: {}",
                f[ch]
            );
        }
    }
}

fn bits(buf: &[f32]) -> Vec<u32> {
    buf.iter().map(|v| v.to_bits()).collect()
}

// -------------------------------------------------------------------- tests

#[test]
fn silent_without_events() {
    let mut m = SsfMixer::new(config());
    let out = render(&mut m, 4800);
    assert!(out.iter().all(|&v| v == 0.0), "no events, no output");
}

#[test]
fn rolling_ball_is_panned_like_the_panner() {
    let panner = SsfPanner::new(-257.0, 257.0, 0.0, 1070.0);
    for (x, y) in [
        (-150.0, 100.0),
        (150.0, 800.0),
        (0.0, 535.0),
        (-257.0, 0.0),
        (257.0, 1070.0),
        (200.0, 50.0),
        (-200.0, 1000.0),
    ] {
        let mut m = SsfMixer::new(config());
        let out = roll_for(&mut m, 1000, x, y, 2.0);
        assert_sane(&out);
        // Skip the attack; measure 0.8 s of steady roll.
        let steady = &out[200 * TICK_FRAMES * 8..];
        let total = total_rms(steady);
        assert!(
            total > 1e-3,
            "({x}, {y}): the roll loop is inaudible ({total})"
        );
        let g = panner.gains(x, y);
        for &ch in &EXCITERS {
            let share = rms(steady, ch) / total;
            assert!(
                (share - f64::from(g[ch])).abs() < 2e-3,
                "({x}, {y}) channel {ch}: share {share}, panner gain {}",
                g[ch]
            );
        }
    }
}

#[test]
fn roll_gain_follows_speed() {
    let level = |speed: f32| {
        let mut m = SsfMixer::new(config());
        let out = roll_for(&mut m, 1000, 30.0, 400.0, speed);
        assert_sane(&out);
        total_rms(&out[200 * TICK_FRAMES * 8..])
    };
    let full = config().roll_full_speed_m_s;
    assert!(full > 0.0);
    let levels: Vec<f64> = [0.1, 0.25, 0.5, 0.75, 1.0]
        .iter()
        .map(|k| level(k * full))
        .collect();
    assert!(level(0.0) < 1e-6, "a ball at rest makes no roll noise");
    for w in levels.windows(2) {
        assert!(w[1] > w[0] * 1.15, "gain must grow with speed: {levels:?}");
    }
    // Roughly proportional to speed up to full speed, flat above it.
    assert!((levels[2] / levels[4] - 0.5).abs() < 0.05, "{levels:?}");
    let above = level(3.0 * full);
    assert!(
        (above / levels[4] - 1.0).abs() < 0.02,
        "gain saturates at full speed"
    );
}

#[test]
fn roll_pitch_follows_speed() {
    let crossings = |speed: f32| {
        let mut m = SsfMixer::new(config());
        // Front-left corner: the whole signal is on SL.
        let out = roll_for(&mut m, 2000, -257.0, 0.0, speed);
        zero_crossings(&channel(&out[500 * TICK_FRAMES * 8..], CH_SL))
    };
    let cfg = config();
    let full = cfg.roll_full_speed_m_s;
    let counts: Vec<usize> = [0.2, 0.4, 0.6, 0.8, 1.0]
        .iter()
        .map(|k| crossings(k * full))
        .collect();
    for w in counts.windows(2) {
        assert!(w[1] > w[0], "pitch must rise with speed: {counts:?}");
    }
    // 1.5 s of signal: a sine at f Hz crosses zero 2·f·1.5 times.
    for (k, &n) in [0.2f32, 0.4, 0.6, 0.8, 1.0].iter().zip(&counts) {
        let ratio = cfg.roll_pitch_min + (cfg.roll_pitch_max - cfg.roll_pitch_min) * k;
        let want = 2.0 * cfg.roll_base_hz * ratio * 1.5;
        assert!(
            (n as f32 - want).abs() <= 3.0,
            "speed {k}·full: {n} zero crossings, expected about {want}"
        );
    }
}

#[test]
fn roll_gain_follows_mass() {
    let level = |mass: f32| {
        let mut m = SsfMixer::new(config());
        let mut out = Vec::new();
        let mut buf = [0.0f32; TICK_FRAMES * 8];
        for t in 0..600 {
            m.feed(HapticEvent::new(
                t,
                HapticKind::BallRoll {
                    ball: 0,
                    position_mm: [0.0, 500.0, 0.0],
                    mass_kg: mass,
                    speed_m_s: 1.5,
                },
                1.0,
            ));
            m.render(&mut buf);
            out.extend_from_slice(&buf);
        }
        total_rms(&out[200 * TICK_FRAMES * 8..])
    };
    let (light, steel) = (level(0.0403), level(0.0806));
    assert!(
        light > 0.0 && steel > light * 1.5,
        "light {light}, steel {steel}"
    );
}

#[test]
fn roll_stops_when_the_ball_stops_sending() {
    let mut m = SsfMixer::new(config());
    let out = roll_for(&mut m, 300, 50.0, 300.0, 2.0);
    assert!(peak(&out) > 0.01);
    assert!(m.active_roll_voices() >= 1);
    let tail = render(&mut m, RATE as usize / 2);
    assert_sane(&tail);
    // Hold (40 ms) + ramp (10 ms) by default: silent well within 250 ms.
    let late = &tail[RATE as usize / 4 * 8..];
    assert!(late.iter().all(|&v| v == 0.0), "roll voice never stopped");
    assert_eq!(m.active_roll_voices(), 0);
}

#[test]
fn roll_moves_smoothly_along_the_criterion_path() {
    // The ball of verification criterion 1, travelling (−150,100) → (150,800)
    // in one second: power moves from SL to BR and no sample jumps.
    let mut m = SsfMixer::new(config());
    let mut out = Vec::new();
    let mut buf = [0.0f32; TICK_FRAMES * 8];
    for t in 0..1000u64 {
        let k = t as f32 / 999.0;
        m.feed(roll(t, 0, -150.0 + 300.0 * k, 100.0 + 700.0 * k, 0.76));
        m.render(&mut buf);
        out.extend_from_slice(&buf);
    }
    assert_sane(&out);
    let first = &out[50 * TICK_FRAMES * 8..150 * TICK_FRAMES * 8];
    let last = &out[900 * TICK_FRAMES * 8..];
    for &ch in &EXCITERS {
        if ch != CH_SL {
            assert!(
                rms(first, CH_SL) > rms(first, ch),
                "SL dominates at the start"
            );
        }
        if ch != CH_BR {
            assert!(rms(last, CH_BR) > rms(last, ch), "BR dominates at the end");
        }
    }
    // A ≤ 63 Hz sine below 0.6 amplitude moves < 0.006 per frame; a pan or
    // gain step would show up as a much larger jump.
    for &ch in &EXCITERS {
        let c = channel(&out, ch);
        for (i, w) in c.windows(2).enumerate() {
            assert!(
                (w[1] - w[0]).abs() < 0.02,
                "channel {ch} frame {i}: click {w:?}"
            );
        }
    }
}

#[test]
fn two_balls_get_two_voices() {
    let mut m = SsfMixer::new(config());
    let mut buf = [0.0f32; TICK_FRAMES * 8];
    let mut out = Vec::new();
    for t in 0..600 {
        m.feed(roll(t, 0, -257.0, 0.0, 1.0));
        m.feed(roll(t, 1, 257.0, 1070.0, 1.0));
        m.render(&mut buf);
        out.extend_from_slice(&buf);
    }
    assert_sane(&out);
    assert_eq!(m.active_roll_voices(), 2);
    let steady = &out[200 * TICK_FRAMES * 8..];
    assert!(rms(steady, CH_SL) > 0.01 && rms(steady, CH_BR) > 0.01);
    assert!(rms(steady, CH_SR) < 1e-6 && rms(steady, CH_BL) < 1e-6);
    assert!((rms(steady, CH_SL) / rms(steady, CH_BR) - 1.0).abs() < 0.02);
}

#[test]
fn coil_and_bumper_impulses_thump_and_die_away() {
    for (e, name) in [(coil(0, 3, 1.0), "coil"), (bumper(0, 2, 1.0), "bumper")] {
        let mut m = SsfMixer::new(config());
        m.feed(e);
        let out = render(&mut m, RATE as usize * 2);
        assert_sane(&out);
        let ms = |a: usize, b: usize| &out[a * 48 * 8..b * 48 * 8];
        let early = total_rms(ms(0, 50));
        let mid = total_rms(ms(100, 150));
        let late = total_rms(ms(250, 300));
        assert!(early > 0.02, "{name}: no thump ({early})");
        assert!(
            mid < early * 0.6 && late < mid * 0.6,
            "{name}: {early} {mid} {late}"
        );
        assert!(
            ms(1500, 2000).iter().all(|&v| v == 0.0),
            "{name}: an impulse must end"
        );
        assert_eq!(m.active_impulse_voices(), 0, "{name}");
        // No source position registered: centre of the playfield, all four equal.
        let g: Vec<f64> = EXCITERS.iter().map(|&c| rms(ms(0, 50), c)).collect();
        for v in &g {
            assert!((v / g[0] - 1.0).abs() < 1e-3, "{name}: {g:?}");
        }
    }
}

#[test]
fn impulse_amplitude_follows_strength() {
    let peak_of = |strength: f32| {
        let mut m = SsfMixer::new(config());
        m.feed(coil(0, 3, strength));
        peak(&render(&mut m, RATE as usize / 2))
    };
    let (quarter, half, full) = (peak_of(0.25), peak_of(0.5), peak_of(1.0));
    assert!(full > 0.05);
    assert!((half / full - 0.5).abs() < 0.01 && (quarter / full - 0.25).abs() < 0.01);
    assert_eq!(peak_of(0.0), 0.0);
    assert_eq!(peak_of(-1.0), 0.0);
    assert_eq!(peak_of(f32::NAN), 0.0);
    assert_eq!(
        peak_of(9.0).to_bits(),
        full.to_bits(),
        "strength above 1 saturates"
    );
}

#[test]
fn impulses_are_panned_at_their_position() {
    // A ball drop carries its own position.
    for (x, y, ch) in [
        (-257.0, 0.0, CH_SL),
        (257.0, 0.0, CH_SR),
        (-257.0, 1070.0, CH_BL),
        (257.0, 1070.0, CH_BR),
    ] {
        let mut m = SsfMixer::new(config());
        m.feed(drop_at(0, x, y, 1.0));
        let out = render(&mut m, 4800);
        assert_sane(&out);
        assert!(
            rms(&out, ch) > 0.01,
            "drop at ({x}, {y}) must be on channel {ch}"
        );
        for &other in &EXCITERS {
            if other != ch {
                assert!(
                    rms(&out, other) < 1e-6,
                    "drop at ({x}, {y}) leaks to {other}"
                );
            }
        }
    }
    // A coil or bumper uses the position registered for its source id.
    let left_flipper = coil(0, 1, 1.0);
    let right_flipper = coil(0, 2, 1.0);
    let top_bumper = bumper(0, 0, 1.0);
    let eos = HapticEvent::new(
        0,
        HapticKind::EosHit {
            flipper: FlipperId(0),
        },
        1.0,
    );
    for (e, x, y) in [
        (left_flipper, -100.0f32, 120.0f32),
        (right_flipper, 100.0, 120.0),
        (top_bumper, 60.0, 900.0),
        (eos, -90.0, 130.0),
    ] {
        let mut m = SsfMixer::new(config());
        m.set_source_position(left_flipper.source(), -100.0, 120.0);
        m.set_source_position(right_flipper.source(), 100.0, 120.0);
        m.set_source_position(top_bumper.source(), 60.0, 900.0);
        m.set_source_position(eos.source(), -90.0, 130.0);
        m.feed(e);
        let out = render(&mut m, 4800);
        let total = total_rms(&out);
        assert!(total > 0.01);
        let g = m.panner().gains(x, y);
        for &ch in &EXCITERS {
            assert!(
                (rms(&out, ch) / total - f64::from(g[ch])).abs() < 1e-3,
                "{e:?}: channel {ch}"
            );
        }
    }
}

#[test]
fn rendering_is_independent_of_buffer_size() {
    let scenario = |chunk_frames: usize| {
        let mut m = SsfMixer::new(config());
        m.feed(coil(0, 1, 0.9));
        m.feed(drop_at(0, 100.0, 700.0, 0.6));
        m.feed(roll(0, 0, -50.0, 300.0, 1.2));
        let mut out = vec![0.0f32; 9600 * 8];
        for c in out.chunks_mut(chunk_frames * 8) {
            m.render(c);
        }
        assert_eq!(m.frames_rendered(), 9600);
        out
    };
    let whole = scenario(9600);
    assert!(peak(&whole) > 0.05);
    for chunk in [1usize, 7, 48, 64, 511, 4096] {
        assert_eq!(
            bits(&scenario(chunk)),
            bits(&whole),
            "chunk of {chunk} frames"
        );
    }
}

fn busy_scenario(m: &mut SsfMixer, ticks: u64, sink: &mut dyn FnMut(&[f32])) {
    let mut buf = [0.0f32; TICK_FRAMES * 8];
    let mut x = 0x1234_5678u32;
    for t in 0..ticks {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        let k = t as f32 / ticks as f32;
        m.feed(roll(t, 0, -200.0 + 400.0 * k, 1000.0 * k, 0.5 + 2.0 * k));
        if t % 3 == 0 {
            m.feed(roll(t, 5, 200.0 - 400.0 * k, 500.0, 3.5));
        }
        match x % 23 {
            0 => m.feed(coil(t, (x >> 8) as u16 % 8, 1.0)),
            1 => m.feed(bumper(t, (x >> 8) as u16 % 4, 0.8)),
            2 => m.feed(drop_at(
                t,
                (x >> 8) as f32 % 257.0,
                (x >> 4) as f32 % 1070.0,
                1.0,
            )),
            _ => {}
        }
        if t % 400 == 0 {
            // More simultaneous impulses than there are voices.
            for i in 0..40 {
                m.feed(coil(t, i, 1.0));
            }
        }
        m.render(&mut buf);
        sink(&buf);
    }
}

#[test]
fn busy_table_stays_bounded_and_is_bit_identical_over_10_runs() {
    let run = || {
        let mut m = SsfMixer::new(config());
        let mut out = Vec::new();
        busy_scenario(&mut m, 2000, &mut |b| out.extend_from_slice(b));
        out
    };
    let first = run();
    assert_sane(&first);
    assert!(peak(&first) > 0.2);
    let first = bits(&first);
    for _ in 0..9 {
        assert_eq!(bits(&run()), first);
    }
}

#[test]
fn counter_detects_allocation() {
    let before = allocs();
    let v = std::hint::black_box(vec![1u8; 64]);
    assert!(allocs() > before);
    drop(v);
}

#[test]
fn feed_and_render_do_not_allocate() {
    let mut m = SsfMixer::new(config());
    m.set_source_position(coil(0, 1, 1.0).source(), -100.0, 120.0);
    let mut energy = 0.0f64;
    let before = allocs();
    busy_scenario(&mut m, 2000, &mut |b| {
        energy += b.iter().map(|&v| f64::from(v * v)).sum::<f64>();
    });
    let stereo = [0.25f32; 2 * 256];
    let mut out = [0.0f32; 8 * 256];
    m.feed(bumper(2000, 1, 1.0));
    m.render_with_stereo(&stereo, &mut out);
    std::hint::black_box((&stereo, &out));
    let after = allocs();
    assert_eq!(after - before, 0, "feed/render allocated");
    assert!(energy > 1.0);
}

#[test]
fn stereo_mix_lands_on_the_front_pair_only() {
    let frames = 480;
    let stereo: Vec<f32> = (0..frames * 2)
        .map(|i| if i % 2 == 0 { 0.5 } else { -0.25 } * ((i / 2) as f32 / frames as f32))
        .collect();
    let mut a = SsfMixer::new(config());
    let mut b = SsfMixer::new(config());
    for m in [&mut a, &mut b] {
        m.feed(drop_at(0, -40.0, 600.0, 1.0));
        m.feed(roll(0, 0, 100.0, 200.0, 2.0));
    }
    let plain = render(&mut a, frames);
    let mut mixed = vec![f32::NAN; frames * 8];
    b.render_with_stereo(&stereo, &mut mixed);
    for i in 0..frames {
        let (p, m) = (&plain[i * 8..i * 8 + 8], &mixed[i * 8..i * 8 + 8]);
        assert_eq!(m[CH_FL], stereo[2 * i], "frame {i} FL");
        assert_eq!(m[CH_FR], stereo[2 * i + 1], "frame {i} FR");
        assert_eq!((m[CH_FC], m[CH_LFE]), (0.0, 0.0));
        for &ch in &EXCITERS {
            assert_eq!(m[ch].to_bits(), p[ch].to_bits(), "frame {i} channel {ch}");
        }
    }
    // Hot stereo input is clipped, not wrapped or passed through.
    let loud = vec![3.0f32; frames * 2];
    let mut out = vec![0.0f32; frames * 8];
    SsfMixer::new(config()).render_with_stereo(&loud, &mut out);
    assert!(
        out.chunks_exact(8)
            .all(|f| f[CH_FL] == 1.0 && f[CH_FR] == 1.0)
    );
}

#[test]
fn partial_trailing_frame_and_empty_buffers_are_handled() {
    let mut m = SsfMixer::new(config());
    m.feed(coil(0, 1, 1.0));
    m.render(&mut []);
    let mut out = [7.0f32; 8 * 10 + 5];
    m.render(&mut out);
    assert!(out[..80].iter().any(|&v| v != 0.0));
    assert!(out[..80].iter().all(|&v| v.abs() <= 1.0));
    assert!(
        out[80..].iter().all(|&v| v == 0.0),
        "the partial frame is zeroed"
    );
}

fn wild_f32() -> impl Strategy<Value = f32> {
    prop_oneof![
        any::<f32>(),
        -2000.0f32..2000.0,
        Just(f32::NAN),
        Just(f32::INFINITY),
        Just(f32::NEG_INFINITY),
    ]
}

fn wild_event() -> impl Strategy<Value = HapticEvent> {
    (
        0u8..6,
        any::<u16>(),
        [wild_f32(), wild_f32(), wild_f32()],
        wild_f32(),
        wild_f32(),
        wild_f32(),
        any::<u64>(),
    )
        .prop_map(|(k, n, p, mass, speed, strength, tick)| {
            let kind = match k {
                0 => HapticKind::CoilFire { coil: CoilId(n) },
                1 => HapticKind::EosHit {
                    flipper: FlipperId(n),
                },
                2 => HapticKind::Bumper { mech: n },
                3 => HapticKind::BallDrop {
                    ball: n,
                    position_mm: p,
                },
                4 => HapticKind::BallRoll {
                    ball: n % 20,
                    position_mm: p,
                    mass_kg: mass,
                    speed_m_s: speed,
                },
                _ => HapticKind::Impulse { source: n },
            };
            HapticEvent::new(tick, kind, strength)
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    /// Any event stream at all (NaN and infinite positions, masses, speeds
    /// and strengths, unknown balls, voice floods): no panic, finite output
    /// in [−1, 1], FL/FR/FC/LFE silent.
    #[test]
    fn hostile_events_never_break_the_output(
        steps in proptest::collection::vec(
            (proptest::collection::vec(wild_event(), 0..24), 1usize..200), 1..24),
        master in prop_oneof![Just(1.0f32), 0.0f32..50.0],
    ) {
        let mut cfg = config();
        cfg.master_gain = master;
        let mut m = SsfMixer::new(cfg);
        for (events, frames) in steps {
            for e in events {
                m.feed(e);
            }
            let mut out = vec![f32::NAN; frames * 8];
            m.render(&mut out);
            for f in out.chunks_exact(8) {
                for &ch in &FRONT {
                    prop_assert_eq!(f[ch], 0.0);
                }
                for &ch in &EXCITERS {
                    prop_assert!(f[ch].is_finite() && f[ch].abs() <= 1.0, "sample {}", f[ch]);
                }
            }
        }
    }

    /// A steady roll anywhere on the playfield puts its power on the four
    /// exciters in the panner's proportions.
    #[test]
    fn roll_power_split_matches_the_panner(
        x in -257.0f32..=257.0, y in 0.0f32..=1070.0, speed in 0.5f32..3.0,
    ) {
        let mut m = SsfMixer::new(config());
        let out = roll_for(&mut m, 400, x, y, speed);
        let steady = &out[150 * TICK_FRAMES * 8..];
        let total = total_rms(steady);
        prop_assert!(total > 1e-3);
        let g = m.panner().gains(x, y);
        for &ch in &EXCITERS {
            prop_assert!((rms(steady, ch) / total - f64::from(g[ch])).abs() < 2e-3);
        }
    }
}
