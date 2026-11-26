//! 3.3-H follow-up R9: randomised `DecodeBudget` accounting.
//!
//! A random sequence of decodes (random WAV lengths, file rates and mixer
//! rate) and releases runs against one budget and a model: after every step
//! `used_bytes` is exactly the sum of `Sample::bytes()` (4 bytes per mono
//! frame, 8 per stereo frame, #227) of the samples still held, a decode succeeds if and only if its output fits into what is left
//! (decodes are sequential), a successful decode is bit-identical to the
//! unbudgeted `decode`, and a failed one leaves nothing charged.

use std::io::Cursor;

use pin_audio::mixer::decode_with_budget;
use pin_audio::mixer::{BYTES_PER_FRAME, DecodeBudget, DecodeError, Sample, decode};
use proptest::prelude::*;

fn wav(rate: u32, channels: u16, n: usize) -> Vec<u8> {
    let spec = hound::WavSpec {
        channels,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut buf = Vec::new();
    {
        let mut w = hound::WavWriter::new(Cursor::new(&mut buf), spec).unwrap();
        for k in 0..n * usize::from(channels) {
            w.write_sample(((k * 7919) % 30_000) as i16 - 15_000)
                .unwrap();
        }
        w.finalize().unwrap();
    }
    buf
}

#[derive(Clone, Debug)]
enum Op {
    Decode {
        rate: u32,
        channels: u16,
        frames: usize,
    },
    Release(usize),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (
            prop::sample::select(vec![8_000u32, 11_025, 22_050, 44_100, 48_000, 96_000]),
            1u16..=2,
            0usize..6_000,
        )
            .prop_map(|(rate, channels, frames)| Op::Decode {
                rate,
                channels,
                frames
            }),
        1 => any::<usize>().prop_map(Op::Release),
    ]
}

fn bytes_of(s: &Sample) -> u64 {
    s.bytes()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    #[test]
    fn accounting_matches_model(
        limit_frames in 0u64..40_000,
        mixer_rate in prop::sample::select(vec![22_050u32, 44_100, 48_000]),
        ops in prop::collection::vec(op(), 1..24),
    ) {
        let budget = DecodeBudget::new(limit_frames * BYTES_PER_FRAME);
        let mut held: Vec<Sample> = Vec::new();
        for op in ops {
            match op {
                Op::Decode { rate, channels, frames } => {
                    let bytes = wav(rate, channels, frames);
                    let reference = decode(&bytes, mixer_rate).unwrap();
                    let need = bytes_of(&reference);
                    let before = budget.used_bytes();
                    let fits = need <= budget.remaining_bytes();
                    match decode_with_budget(&bytes, mixer_rate, &budget) {
                        Ok(s) => {
                            prop_assert!(fits, "decoded {need} B with {} B left", budget.limit_bytes() - before);
                            prop_assert_eq!(&s, &reference);
                            prop_assert_eq!(budget.used_bytes(), before + need);
                            held.push(s);
                        }
                        Err(e) => {
                            prop_assert_eq!(e, DecodeError::BudgetExhausted);
                            prop_assert!(!fits, "refused {need} B with {} B left", budget.limit_bytes() - before);
                            prop_assert_eq!(budget.used_bytes(), before);
                        }
                    }
                }
                Op::Release(i) => {
                    if !held.is_empty() {
                        let s = held.swap_remove(i % held.len());
                        prop_assert!(budget.release(&s).is_ok(), "balanced release");
                    }
                }
            }
            let model: u64 = held.iter().map(bytes_of).sum();
            prop_assert_eq!(budget.used_bytes(), model);
            prop_assert_eq!(budget.remaining_bytes(), budget.limit_bytes() - model);
        }
    }
}

// ------------------------------------------------------------------ release order

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    /// D25, DECISIONS "Human ruling - decoded-audio budget": for a random
    /// sequence of decodes against a limit of 0..=4 MiB, releasing every
    /// returned sample exactly once in a random order brings `used_bytes`
    /// back to 0, and after every step it equals the bytes still held
    /// (`Sample::bytes`: 4 per mono frame, 8 per stereo frame, #227).
    #[test]
    fn used_bytes_return_to_zero_after_random_order_release(
        limit in 0u64..=(4 << 20),
        mixer_rate in prop::sample::select(vec![22_050u32, 44_100, 48_000]),
        decodes in prop::collection::vec(
            (
                prop::sample::select(vec![8_000u32, 22_050, 44_100, 48_000, 96_000]),
                1u16..=2,
                0usize..20_000,
            ),
            1..=12,
        ),
        order in prop::collection::vec(any::<usize>(), 12),
    ) {
        let budget = DecodeBudget::new(limit);
        let mut held: Vec<Sample> = Vec::new();
        for (rate, channels, frames) in decodes {
            let bytes = wav(rate, channels, frames);
            if let Ok(s) = decode_with_budget(&bytes, mixer_rate, &budget) {
                held.push(s);
            }
            let model: u64 = held.iter().map(bytes_of).sum();
            prop_assert_eq!(budget.used_bytes(), model);
        }
        let mut k = 0;
        while !held.is_empty() {
            let s = held.swap_remove(order[k % order.len()] % held.len());
            k += 1;
            prop_assert!(budget.release(&s).is_ok());
            let model: u64 = held.iter().map(bytes_of).sum();
            prop_assert_eq!(budget.used_bytes(), model);
        }
        prop_assert_eq!(budget.used_bytes(), 0);
    }
}

// ---------------------------------------------------------- mutation fuzz (R11)

mod fuzz {
    use super::*;
    use std::panic;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static PANICS: AtomicUsize = AtomicUsize::new(0);
    /// Serialises the global panic hook against any other test of this file.
    static HOOK: Mutex<()> = Mutex::new(());

    const TONE: &[u8] = include_bytes!("../src/mixer/fixtures/tone440_mono_22k.ogg");
    const SINE_48K: &[u8] = include_bytes!("spec_fixtures/sine440_mono_48k.ogg");
    const SINE_44K: &[u8] = include_bytes!("spec_fixtures/sine880_left_44k1.ogg");

    fn crc(data: &[u8]) -> u32 {
        let mut c: u32 = 0;
        for &b in data {
            c ^= u32::from(b) << 24;
            for _ in 0..8 {
                c = if c & 0x8000_0000 != 0 {
                    (c << 1) ^ 0x04c1_1db7
                } else {
                    c << 1
                };
            }
        }
        c
    }

    /// Recomputes the checksum of every page that is still well-framed.
    fn recrc(b: &mut [u8]) {
        let mut p = 0;
        while p + 27 <= b.len() && &b[p..p + 4] == b"OggS" {
            let n = b[p + 26] as usize;
            if p + 27 + n > b.len() {
                break;
            }
            let body: usize = b[p + 27..p + 27 + n].iter().map(|&x| x as usize).sum();
            let end = (p + 27 + n + body).min(b.len());
            b[p + 22..p + 26].fill(0);
            let c = crc(&b[p..end]);
            b[p + 22..p + 26].copy_from_slice(&c.to_le_bytes());
            p = end;
        }
    }

    /// xorshift64* (R3: own seeded PRNG).
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    /// D25, R11, SPEC §4 Subphase 3.3: 2000 seeded mutations (1..=8 byte
    /// flips, mostly in the setup header, page CRCs repaired) of the three
    /// committed Ogg fixtures decode to `Ok` or `Err` without any panic
    /// inside the decoder (a panic the decoder catches and reports as
    /// `Malformed` still counts), and the budget stays exactly consistent.
    #[test]
    fn mutated_ogg_fixtures_decode_without_any_panic() {
        let _g = HOOK.lock().unwrap_or_else(|e| e.into_inner());
        let fixtures: [&[u8]; 3] = [TONE, SINE_48K, SINE_44K];
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let mut failures: Vec<String> = Vec::new();
        PANICS.store(0, Ordering::SeqCst);
        let old = panic::take_hook();
        panic::set_hook(Box::new(|_| {
            PANICS.fetch_add(1, Ordering::SeqCst);
        }));
        let (mut oks, mut errs) = (0usize, 0usize);
        for i in 0..2000usize {
            let base = fixtures[i % fixtures.len()];
            let mut b = base.to_vec();
            let header = b.len().min(4096);
            for _ in 0..1 + rng.below(8) {
                let at = if rng.below(10) < 8 {
                    rng.below(header)
                } else {
                    rng.below(b.len())
                };
                b[at] ^= 1 + rng.below(255) as u8;
            }
            recrc(&mut b);
            let before = PANICS.load(Ordering::SeqCst);
            let budget = DecodeBudget::new(1 << 20);
            let r = decode_with_budget(&b, 48_000, &budget);
            let expect = match &r {
                Ok(s) => {
                    oks += 1;
                    bytes_of(s)
                }
                Err(_) => {
                    errs += 1;
                    0
                }
            };
            if budget.used_bytes() != expect {
                failures.push(format!(
                    "mutation {i}: used {} != {expect}",
                    budget.used_bytes()
                ));
            }
            if PANICS.load(Ordering::SeqCst) != before {
                failures.push(format!(
                    "mutation {i} (fixture {}) panicked in the decoder: {:?}",
                    i % fixtures.len(),
                    b.iter()
                        .zip(base)
                        .enumerate()
                        .filter(|(_, (x, y))| x != y)
                        .map(|(k, (x, _))| (k, *x))
                        .collect::<Vec<_>>()
                ));
            }
        }
        panic::set_hook(old);
        assert!(
            failures.is_empty(),
            "{} failures, first: {:?}",
            failures.len(),
            &failures[..failures.len().min(5)]
        );
        assert_eq!(PANICS.load(Ordering::SeqCst), 0);
        assert!(
            oks + errs == 2000 && oks > 0 && errs > 0,
            "oks {oks} errs {errs}"
        );
    }
}
