//! Spec tests (Independent Tester) for #227: mono samples are stored as mono.
//!
//! SPEC §4 Subphase 3.3 (mixer), D5 (render never allocates), D25 (decode
//! budget). Written from the public API in the brief only.
//!
//! 1. Memory: half the bytes, `bytes()` is `frames * channels * 4`.
//! 2. Mixed output of a mono sample is bit-identical to duplicated stereo.
//! 3. Decode keeps mono mono (WAV, resampling, Ogg fixtures).
//! 4. Budget accounting charges 4 bytes per mono frame.
//! 5. Hot-path render allocates zero times.
//! 6. Hostile bytes never panic and never leave anything charged.

use pin_audio::mixer::{
    BYTES_PER_FRAME, BYTES_PER_MONO_FRAME, Bus, DecodeBudget, DecodeError, Mixer, MixerConfig,
    Sample, SoundSender, SoundTrigger, decode, decode_with_budget, trigger_ring,
};
use pin_core::SoundId;
use proptest::prelude::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// ---------------------------------------------------------------- allocator

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
}
static EVENTS: AtomicUsize = AtomicUsize::new(0);

struct Counting;

impl Counting {
    fn note() {
        if ARMED.try_with(Cell::get).unwrap_or(false) {
            EVENTS.fetch_add(1, Ordering::Relaxed);
        }
    }
}

// SAFETY: defers to `System`; only counts.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        Self::note();
        // SAFETY: forwarded unchanged.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        Self::note();
        // SAFETY: forwarded unchanged.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        Self::note();
        // SAFETY: forwarded unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        Self::note();
        // SAFETY: forwarded unchanged.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Allocator calls made by this thread inside `f`.
fn allocator_calls(f: impl FnOnce()) -> usize {
    static GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = GATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    EVENTS.store(0, Ordering::Relaxed);
    ARMED.with(|a| a.set(true));
    f();
    ARMED.with(|a| a.set(false));
    EVENTS.load(Ordering::Relaxed)
}

// ------------------------------------------------------------------ helpers

const RATE: u32 = 48_000;

/// Deterministic non-trivial signal in roughly [-1.5, 1.5] (beyond +-1 on purpose).
fn signal(seed: u64, n: usize) -> Vec<f32> {
    let mut s = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (0..n)
        .map(|i| {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let noise = ((s >> 40) as f32 / (1u64 << 24) as f32) - 0.5; // [-0.5, 0.5)
            let tone = (i as f32 * 0.0731 + seed as f32).sin();
            tone * 1.1 + noise * 0.8
        })
        .collect()
}

fn dup(x: &[f32]) -> Vec<f32> {
    x.iter().flat_map(|&v| [v, v]).collect()
}

fn mono_wav_i16(rate: u32, v: &[i16]) -> Vec<u8> {
    wav_i16(1, rate, v)
}

fn wav_i16(channels: u16, rate: u32, v: &[i16]) -> Vec<u8> {
    let spec = hound::WavSpec {
        channels,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cur = std::io::Cursor::new(Vec::new());
    {
        let mut w = hound::WavWriter::new(&mut cur, spec).unwrap();
        for &s in v {
            w.write_sample(s).unwrap();
        }
        w.finalize().unwrap();
    }
    cur.into_inner()
}

fn wav_f32(channels: u16, rate: u32, v: &[f32]) -> Vec<u8> {
    let spec = hound::WavSpec {
        channels,
        sample_rate: rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut cur = std::io::Cursor::new(Vec::new());
    {
        let mut w = hound::WavWriter::new(&mut cur, spec).unwrap();
        for &s in v {
            w.write_sample(s).unwrap();
        }
        w.finalize().unwrap();
    }
    cur.into_inner()
}

fn i16_signal(seed: u64, n: usize) -> Vec<i16> {
    signal(seed, n)
        .iter()
        .map(|v| (v * 20_000.0).clamp(-32768.0, 32767.0) as i16)
        .collect()
}

// ------------------------------------------------------------- 1. memory

/// [3.3] Mono storage is one value per frame, half the bytes of stereo.
#[test]
fn spec_3_3_mono_sample_stores_one_value_per_frame() {
    assert_eq!(BYTES_PER_FRAME, 8);
    assert_eq!(BYTES_PER_MONO_FRAME, 4);
    for len in [0usize, 1, 7, 1000] {
        let x = signal(len as u64 + 1, len);
        let m = Sample::from_mono(&x);
        let st = Sample::from_interleaved(dup(&x));
        assert_eq!(m.channels(), 1, "len {len}");
        assert_eq!(m.frames(), len);
        assert_eq!(m.data().len(), len);
        assert_eq!(m.bytes(), 4 * len as u64);
        assert_eq!(m.bytes() * 2, st.bytes(), "half the memory of stereo");
        assert_eq!(st.channels(), 2);
        assert_eq!(st.frames(), len);
        assert_eq!(st.data().len(), 2 * len);
        assert_eq!(st.bytes(), 8 * len as u64);

        let mv = Sample::from_mono_vec(x.clone());
        assert_eq!(mv.channels(), 1);
        assert_eq!(mv.frames(), len);
        assert_eq!(mv.data(), m.data());
        assert_eq!(mv.bytes(), m.bytes());

        for (i, &v) in x.iter().enumerate() {
            assert_eq!(m.frame(i), (v, v));
            assert_eq!(st.frame(i), (v, v));
        }
        let to_bits = |v: Vec<f32>| v.into_iter().map(f32::to_bits).collect::<Vec<_>>();
        assert_eq!(to_bits(m.to_stereo_vec()), to_bits(dup(&x)));
        assert_eq!(to_bits(mv.to_stereo_vec()), to_bits(dup(&x)));
        assert_eq!(to_bits(st.to_stereo_vec()), to_bits(dup(&x)));
    }
}

/// [3.3] `frame(i)` panics when `i >= frames()`; genuine stereo keeps L and R.
#[test]
fn spec_3_3_frame_view_bounds_and_stereo_values() {
    let m = Sample::from_mono(&[0.25, -0.5]);
    assert!(std::panic::catch_unwind(|| m.frame(2)).is_err());
    let st = Sample::from_interleaved(vec![0.1, 0.2, 0.3, 0.4, 0.5]); // odd tail dropped
    assert_eq!(st.frames(), 2);
    assert_eq!(st.frame(1), (0.3, 0.4));
    assert_eq!(st.to_stereo_vec(), vec![0.1, 0.2, 0.3, 0.4]);
    let c = Sample::constant(5, 0.5, -0.5);
    assert_eq!(c.channels(), 2);
    assert_eq!(c.bytes(), 40);
    let e = Sample::default();
    assert_eq!(e.frames(), 0);
    assert_eq!(e.bytes(), 0);
    assert!(e.to_stereo_vec().is_empty());
}

// ------------------------------------------------------ 2. bit-identical mix

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Mono,
    Stereo,
}

/// (id, bus, kind, frames)
const SOUNDS: [(u16, Bus, Kind, usize); 8] = [
    (1, Bus::Music, Kind::Mono, 20_000),
    (2, Bus::Music, Kind::Stereo, 9_000),
    (3, Bus::Callout, Kind::Mono, 3_000),
    (4, Bus::Callout, Kind::Stereo, 1_500),
    (5, Bus::Sfx, Kind::Mono, 2_000),
    (6, Bus::Sfx, Kind::Stereo, 777),
    (7, Bus::Sfx, Kind::Mono, 100),
    (8, Bus::Music, Kind::Mono, 301),
];

const BLOCKS: [usize; 6] = [1, 3, 255, 256, 257, 1000];
const CALLS: usize = 90;

/// Runs one scripted scenario; mono sounds are stored as mono when `use_mono`,
/// else as duplicated stereo. Returns every output sample's bits and the
/// lowest duck level seen after any render call.
fn run_scenario(
    cfg: MixerConfig,
    use_mono: bool,
    all_mono: bool,
    ids: &[u16],
    gain: f32,
) -> (Vec<u32>, f32) {
    let (mut tx, rx): (SoundSender, _) = trigger_ring(2048);
    let mut mixer = Mixer::new(cfg, rx);
    for &(id, bus, kind, frames) in &SOUNDS {
        let x = signal(u64::from(id) * 31, frames);
        let kind = if all_mono { Kind::Mono } else { kind };
        let sample = match (kind, use_mono) {
            (Kind::Mono, true) => Sample::from_mono(&x),
            (Kind::Mono, false) => Sample::from_interleaved(dup(&x)),
            (Kind::Stereo, _) => {
                let r = signal(u64::from(id) * 77 + 5, frames);
                Sample::from_interleaved(x.iter().zip(&r).flat_map(|(&a, &b)| [a, b]).collect())
            }
        };
        mixer.register(SoundId(id), Arc::new(sample), bus);
    }
    let bus_of = |id: u16| SOUNDS.iter().find(|s| s.0 == id).unwrap().1;
    let mut bits = Vec::new();
    let mut min_duck = 1.0f32;
    for call in 0..CALLS {
        for &id in ids {
            let is_callout = bus_of(id) == Bus::Callout;
            let fire = if is_callout {
                // Callouts begin mid-run so the duck ramps are exercised.
                call == 14 || (call >= 12 && call % 13 == 12)
            } else {
                call % 3 == 0
            };
            let burst = if is_callout { call == 14 } else { call == 4 };
            let n = if burst { 30 } else { usize::from(fire) };
            for _ in 0..n {
                let _ = tx.send(SoundTrigger {
                    sound: SoundId(id),
                    gain,
                });
            }
        }
        let frames = BLOCKS[call % BLOCKS.len()];
        let mut out = vec![0.0f32; 2 * frames];
        mixer.render(&mut out);
        bits.extend(out.iter().map(|v| v.to_bits()));
        min_duck = min_duck.min(mixer.duck_level());
    }
    (bits, min_duck)
}

fn assert_mono_matches_stereo(cfg: MixerConfig, ids: &[u16], gain: f32, all_mono: bool) -> f32 {
    let (mono, duck) = run_scenario(cfg, true, all_mono, ids, gain);
    let (stereo, _) = run_scenario(cfg, false, all_mono, ids, gain);
    assert_eq!(mono.len(), stereo.len());
    for (i, (a, b)) in mono.iter().zip(&stereo).enumerate() {
        assert_eq!(
            a,
            b,
            "bit mismatch at output sample {i}: {} vs {} (ids {ids:?}, gain {gain}, all_mono {all_mono})",
            f32::from_bits(*a),
            f32::from_bits(*b)
        );
    }
    duck
}

/// [3.3] Sweep buses x gains x master gain x mono/stereo mixes: output of a
/// mono sample equals the duplicated-stereo output bit for bit.
#[test]
fn spec_3_3_mono_mix_is_bit_identical_to_duplicated_stereo() {
    let bus_sets: [&[u16]; 5] = [
        &[1, 2, 8],                // music (steals: cap 2)
        &[3, 4],                   // callout (steals: cap 4)
        &[5, 6, 7],                // sfx (steals: cap 24, short sample ends mid-block)
        &[1, 3, 5, 7],             // mono voices on every bus
        &[1, 2, 3, 4, 5, 6, 7, 8], // mono and stereo together
    ];
    let gains = [0.0f32, 0.5, 1.0, 1.7, f32::NAN, -0.5];
    let masters = [1.0f32, 0.7, 4.0];
    for &master in &masters {
        for &gain in &gains {
            for ids in bus_sets {
                for all_mono in [false, true] {
                    let cfg = MixerConfig {
                        master_gain: master,
                        ..MixerConfig::default()
                    };
                    assert_mono_matches_stereo(cfg, ids, gain, all_mono);
                }
            }
        }
    }
}

/// [3.3] The scenario really exercises ducking, steals, clipping and silence.
#[test]
fn spec_3_3_mono_mix_scenario_is_not_vacuous() {
    let cfg = MixerConfig::default();
    let all = [1u16, 2, 3, 4, 5, 6, 7, 8];
    let duck = assert_mono_matches_stereo(cfg, &all, 1.0, false);
    assert!(duck < 1.0, "callouts must duck the other buses, min {duck}");
    let (bits, _) = run_scenario(cfg, true, false, &all, 1.0);
    assert!(bits.iter().any(|&b| f32::from_bits(b) != 0.0));
    let loud = MixerConfig {
        master_gain: 4.0,
        ..MixerConfig::default()
    };
    let (bits, _) = run_scenario(loud, true, false, &all, 1.7);
    assert!(
        bits.iter().any(|&b| f32::from_bits(b).abs() == 1.0),
        "clips"
    );
    assert!(bits.iter().all(|&b| f32::from_bits(b).abs() <= 1.0));
    // NaN trigger gain is silence.
    let (bits, _) = run_scenario(MixerConfig::default(), true, true, &[1, 5], f32::NAN);
    assert!(bits.iter().all(|&b| f32::from_bits(b) == 0.0));
}

/// [3.3] Mixer config variations (rate, voice limits, ducking) stay identical.
#[test]
fn spec_3_3_mono_mix_bit_identical_across_configs() {
    let all = [1u16, 2, 3, 4, 5, 6, 7, 8];
    let cfgs = [
        MixerConfig {
            max_voices: [1, 1, 3],
            duck_gain: 0.1,
            ..MixerConfig::default()
        },
        MixerConfig {
            duck_gain: 0.8,
            ..MixerConfig::default()
        },
    ];
    for cfg in cfgs {
        for gain in [0.5f32, 1.7] {
            assert_mono_matches_stereo(cfg, &all, gain, true);
            assert_mono_matches_stereo(cfg, &all, gain, false);
        }
    }
}

// ---------------------------------------------------------------- 3. decode

/// [3.3, D25] 16-bit and float mono WAV decode to mono, values as before.
#[test]
fn spec_3_3_decode_wav_mono_stays_mono() {
    for n in [1usize, 5, 1000] {
        let v = i16_signal(n as u64, n);
        let s = decode(&mono_wav_i16(RATE, &v), RATE).unwrap();
        assert_eq!(s.channels(), 1);
        assert_eq!(s.frames(), n);
        assert_eq!(s.data().len(), n);
        let expect: Vec<f32> = v.iter().map(|&q| f32::from(q) / 32768.0).collect();
        assert_eq!(s.to_stereo_vec(), dup(&expect));

        let f = signal(n as u64, n);
        let s = decode(&wav_f32(1, RATE, &f), RATE).unwrap();
        assert_eq!(s.channels(), 1);
        assert_eq!(s.frames(), n);
        assert_eq!(s.to_stereo_vec(), dup(&f));
    }
}

/// [3.3] Stereo WAV stays stereo with exact values.
#[test]
fn spec_3_3_decode_wav_stereo_stays_stereo() {
    let n = 500;
    let v = i16_signal(9, 2 * n);
    let s = decode(&wav_i16(2, RATE, &v), RATE).unwrap();
    assert_eq!(s.channels(), 2);
    assert_eq!(s.frames(), n);
    assert_eq!(s.data().len(), 2 * n);
    let expect: Vec<f32> = v.iter().map(|&q| f32::from(q) / 32768.0).collect();
    assert_eq!(s.to_stereo_vec(), expect);
    let f = signal(4, 2 * n);
    let s = decode(&wav_f32(2, RATE, &f), RATE).unwrap();
    assert_eq!(s.channels(), 2);
    assert_eq!(s.to_stereo_vec(), f);
}

fn reference_resample(x: &[f32], src: u32, dst: u32) -> Vec<f32> {
    let n_in = x.len() as u64;
    let n_out = (n_in * u64::from(dst)).div_ceil(u64::from(src));
    let last = x.len() - 1;
    (0..n_out)
        .map(|j| {
            let num = j * u64::from(src);
            let i = (num / u64::from(dst)) as usize;
            let f = (num % u64::from(dst)) as f64 / f64::from(dst);
            let a = f64::from(x[i.min(last)]);
            let b = f64::from(x[(i + 1).min(last)]);
            (a * (1.0 - f) + b * f) as f32
        })
        .collect()
}

/// [3.3, D25] Resampled mono stays mono and matches the linear resampler.
#[test]
fn spec_3_3_decode_resampled_mono_stays_mono() {
    for (src, dst) in [(22_050u32, 48_000u32), (48_000, 44_100), (44_100, 44_100)] {
        let x: Vec<f32> = signal(u64::from(src), 1000)
            .iter()
            .map(|v| v * 0.5)
            .collect();
        let s = decode(&wav_f32(1, src, &x), dst).unwrap();
        assert_eq!(s.channels(), 1, "{src}->{dst}");
        let want = reference_resample(&x, src, dst);
        assert_eq!(s.frames(), want.len(), "{src}->{dst}");
        assert_eq!(s.data().len(), want.len());
        let got = s.to_stereo_vec();
        for (j, w) in want.iter().enumerate() {
            let (l, r) = (got[2 * j], got[2 * j + 1]);
            assert_eq!(l.to_bits(), r.to_bits(), "L==R at {j}");
            let tol = if src == dst { 0.0 } else { 1e-6 };
            assert!((l - w).abs() <= tol, "{src}->{dst} frame {j}: {l} vs {w}");
        }
    }
}

/// [3.3, D25] Both mono Ogg fixtures decode to mono (L == R for every frame).
#[test]
fn spec_3_3_decode_ogg_fixtures_stay_mono() {
    let fixtures: [&[u8]; 2] = [
        include_bytes!("spec_fixtures/sine440_mono_48k.ogg"),
        include_bytes!("../src/mixer/fixtures/tone440_mono_22k.ogg"),
    ];
    for (k, bytes) in fixtures.iter().enumerate() {
        let s = decode(bytes, RATE).unwrap();
        assert_eq!(s.channels(), 1, "fixture {k}");
        assert!(s.frames() > 0);
        assert_eq!(s.data().len(), s.frames());
        assert_eq!(s.bytes(), 4 * s.frames() as u64);
        let st = s.to_stereo_vec();
        assert_eq!(st.len(), 2 * s.frames());
        for (i, p) in st.chunks_exact(2).enumerate() {
            assert_eq!(p[0].to_bits(), p[1].to_bits(), "fixture {k} frame {i}");
        }
    }
    let s = decode(fixtures[0], RATE).unwrap();
    assert_eq!(s.frames(), 12_000);
}

// ---------------------------------------------------------------- 4. budget

const BIG_N: usize = 70_000;

fn big_mono_wav() -> Vec<u8> {
    wav_i16(1, RATE, &vec![1000i16; BIG_N])
}

fn big_stereo_wav() -> Vec<u8> {
    wav_i16(2, RATE, &vec![1000i16; 2 * BIG_N])
}

/// [D25] Mono is charged 4 bytes per frame, stereo 8; release returns it once.
#[test]
fn spec_d25_budget_charges_mono_at_four_bytes_per_frame() {
    for n in [1usize, 100, 5000] {
        let budget = DecodeBudget::new(1 << 30);
        let s = decode_with_budget(&wav_i16(1, RATE, &vec![7i16; n]), RATE, &budget).unwrap();
        assert_eq!(s.bytes(), 4 * n as u64);
        assert_eq!(budget.used_bytes(), 4 * n as u64);
        assert_eq!(budget.remaining_bytes(), (1 << 30) - 4 * n as u64);
        assert!(budget.release(&s).is_ok());
        assert_eq!(budget.used_bytes(), 0);
        assert!(budget.release(&s).is_err(), "second release is OverRelease");
        assert_eq!(budget.used_bytes(), 0);

        let s = decode_with_budget(&wav_i16(2, RATE, &vec![7i16; 2 * n]), RATE, &budget).unwrap();
        assert_eq!(s.bytes(), 8 * n as u64);
        assert_eq!(budget.used_bytes(), 8 * n as u64);
        assert!(budget.release(&s).is_ok());
        assert_eq!(budget.used_bytes(), 0);
        assert!(budget.release(&s).is_err());
    }
}

/// [D25] A budget of exactly 4*N bytes fits an N-frame mono sound; 4*N-4 does not.
#[test]
fn spec_d25_budget_exact_fit_for_mono_and_stereo() {
    let n = BIG_N as u64;
    let mono = big_mono_wav();
    let budget = DecodeBudget::new(4 * n);
    let s = decode_with_budget(&mono, RATE, &budget).expect("exact fit");
    assert_eq!(budget.used_bytes(), 4 * n);
    assert_eq!(budget.remaining_bytes(), 0);
    budget.release(&s).unwrap();
    assert_eq!(budget.used_bytes(), 0);

    let budget = DecodeBudget::new(4 * n - 4);
    let e = decode_with_budget(&mono, RATE, &budget).unwrap_err();
    assert!(matches!(e, DecodeError::BudgetExhausted), "{e:?}");
    assert_eq!(budget.used_bytes(), 0);

    let stereo = big_stereo_wav();
    let budget = DecodeBudget::new(8 * n);
    let s = decode_with_budget(&stereo, RATE, &budget).expect("stereo exact fit");
    assert_eq!(budget.used_bytes(), 8 * n);
    budget.release(&s).unwrap();
    let budget = DecodeBudget::new(8 * n - 4);
    let e = decode_with_budget(&stereo, RATE, &budget).unwrap_err();
    assert!(matches!(e, DecodeError::BudgetExhausted), "{e:?}");
    assert_eq!(budget.used_bytes(), 0);
}

/// [D25] Two mono sounds share one budget.
#[test]
fn spec_d25_two_mono_sounds_share_a_budget() {
    let n = BIG_N as u64;
    let mono = big_mono_wav();
    let budget = DecodeBudget::new(8 * n);
    let a = decode_with_budget(&mono, RATE, &budget).unwrap();
    let b = decode_with_budget(&mono, RATE, &budget).unwrap();
    assert_eq!(budget.used_bytes(), 8 * n);
    assert_eq!(budget.remaining_bytes(), 0);
    let e = decode_with_budget(&mono, RATE, &budget).unwrap_err();
    assert!(matches!(e, DecodeError::BudgetExhausted), "{e:?}");
    assert_eq!(
        budget.used_bytes(),
        8 * n,
        "failed decode leaves nothing charged"
    );
    budget.release(&a).unwrap();
    assert_eq!(budget.used_bytes(), 4 * n);
    let c = decode_with_budget(&mono, RATE, &budget).unwrap();
    assert_eq!(budget.used_bytes(), 8 * n);
    budget.release(&b).unwrap();
    budget.release(&c).unwrap();
    assert_eq!(budget.used_bytes(), 0);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// [D25] Whatever the lengths and channel counts, releasing every sample
    /// in any order returns the budget to zero.
    #[test]
    fn spec_d25_budget_returns_to_zero_after_release(
        sounds in proptest::collection::vec((1usize..2000, 1u16..=2), 1..8),
        keys in proptest::collection::vec(any::<u32>(), 8),
    ) {
        let budget = DecodeBudget::new(1 << 30);
        let mut held = Vec::new();
        let mut expect = 0u64;
        for &(n, ch) in &sounds {
            let wav = wav_i16(ch, RATE, &vec![123i16; n * usize::from(ch)]);
            let s = decode_with_budget(&wav, RATE, &budget).unwrap();
            prop_assert_eq!(s.channels(), usize::from(ch));
            prop_assert_eq!(s.frames(), n);
            expect += 4 * u64::from(ch) * n as u64;
            prop_assert_eq!(s.bytes(), 4 * u64::from(ch) * n as u64);
            prop_assert_eq!(budget.used_bytes(), expect);
            held.push(s);
        }
        let mut order: Vec<usize> = (0..held.len()).collect();
        order.sort_by_key(|&i| (keys[i % keys.len()], i));
        for i in order {
            prop_assert!(budget.release(&held[i]).is_ok());
        }
        prop_assert_eq!(budget.used_bytes(), 0);
    }
}

// ----------------------------------------------------------- 5. allocation

/// [3.3, D5] Rendering mono and stereo voices on all buses allocates nothing.
#[test]
fn spec_3_3_d5_render_with_mono_voices_does_not_allocate() {
    let (mut tx, rx) = trigger_ring(256);
    let mut mixer = Mixer::new(MixerConfig::default(), rx);
    for &(id, bus, kind, frames) in &SOUNDS {
        let x = signal(u64::from(id), frames);
        let s = match kind {
            Kind::Mono => Sample::from_mono(&x),
            Kind::Stereo => Sample::from_interleaved(dup(&x)),
        };
        mixer.register(SoundId(id), Arc::new(s), bus);
    }
    let mut out = vec![0.0f32; 2 * 256];
    let mut odd = vec![0.0f32; 2 * 1000];
    let n = allocator_calls(|| {
        mixer.render(&mut out);
        for round in 0..120u16 {
            for id in 1..=8u16 {
                if (round + id) % 3 == 0 {
                    assert!(tx.send(SoundTrigger {
                        sound: SoundId(id),
                        gain: 0.8,
                    }));
                }
            }
            if round % 5 == 0 {
                mixer.render(&mut odd);
            } else {
                mixer.render(&mut out);
            }
        }
    });
    assert_eq!(n, 0, "render/send made {n} allocator calls");
    assert!(mixer.active_voices(Bus::Music) + mixer.active_voices(Bus::Sfx) > 0);
}

// ------------------------------------------------------------ 6. robustness

fn check_decodes(bytes: &[u8]) {
    let _ = decode(bytes, RATE);
    let budget = DecodeBudget::new(1 << 16);
    match decode_with_budget(bytes, RATE, &budget) {
        Ok(s) => {
            assert!(s.bytes() <= 1 << 16);
            assert_eq!(budget.used_bytes(), s.bytes());
            budget.release(&s).unwrap();
            assert_eq!(budget.used_bytes(), 0);
        }
        Err(_) => assert_eq!(
            budget.used_bytes(),
            0,
            "failed decode leaves nothing charged"
        ),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// [D25, R11] Random bytes never panic and never leave anything charged.
    #[test]
    fn spec_d25_random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        check_decodes(&bytes);
    }

    /// [D25, R11] Mutated mono WAV headers (and truncations) never panic.
    #[test]
    fn spec_d25_mutated_mono_wav_never_panics(
        n in 0usize..300,
        edits in proptest::collection::vec((0usize..64, any::<u8>()), 0..6),
        cut in proptest::option::of(0usize..400),
        rate in prop_oneof![Just(RATE), Just(22_050u32), any::<u32>()],
    ) {
        let mut wav = wav_i16(1, RATE, &i16_signal(3, n));
        // Patch the rate in the header directly (hound overflows on huge rates).
        wav[24..28].copy_from_slice(&rate.to_le_bytes());
        for (pos, val) in edits {
            if pos < wav.len() {
                wav[pos] = val;
            }
        }
        if let Some(c) = cut {
            wav.truncate(c.min(wav.len()));
        }
        check_decodes(&wav);
    }

    /// [D25, R11] Mutated Ogg fixtures never panic.
    #[test]
    fn spec_d25_mutated_ogg_never_panics(
        edits in proptest::collection::vec((0usize..4096, any::<u8>()), 1..6),
        cut in proptest::option::of(0usize..6000),
    ) {
        let mut ogg = include_bytes!("../src/mixer/fixtures/tone440_mono_22k.ogg").to_vec();
        for (pos, val) in edits {
            if pos < ogg.len() {
                ogg[pos] = val;
            }
        }
        if let Some(c) = cut {
            ogg.truncate(c.min(ogg.len()));
        }
        check_decodes(&ogg);
    }
}
