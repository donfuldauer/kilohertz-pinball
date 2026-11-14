//! 3.3-H follow-up R9: randomised `DecodeBudget` accounting.
//!
//! A random sequence of decodes (random WAV lengths, file rates and mixer
//! rate) and releases runs against one budget and a model: after every step
//! `used_bytes` is exactly the sum of `8 × frames()` of the samples still
//! held, a decode succeeds if and only if its output fits into what is left
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
    s.frames() as u64 * BYTES_PER_FRAME
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
                        budget.release(&s);
                    }
                }
            }
            let model: u64 = held.iter().map(bytes_of).sum();
            prop_assert_eq!(budget.used_bytes(), model);
            prop_assert_eq!(budget.remaining_bytes(), budget.limit_bytes() - model);
        }
    }
}
