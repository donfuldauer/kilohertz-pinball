//! Implementer property tests: `decode` never panics on random or truncated
//! input; mixer output is bounded and a pure sum below full scale.

use std::io::Cursor;
use std::sync::Arc;

use pin_audio::mixer::{Bus, Mixer, MixerConfig, Sample, SoundTrigger, decode, trigger_ring};
use pin_core::SoundId;
use proptest::prelude::*;

fn wav_bytes(channels: u16, rate: u32, bits: u16, float: bool, n: usize) -> Vec<u8> {
    let spec = hound::WavSpec {
        channels,
        sample_rate: rate,
        bits_per_sample: bits,
        sample_format: if float {
            hound::SampleFormat::Float
        } else {
            hound::SampleFormat::Int
        },
    };
    let mut buf = Vec::new();
    {
        let mut w = hound::WavWriter::new(Cursor::new(&mut buf), spec).unwrap();
        for k in 0..n * usize::from(channels) {
            if float {
                w.write_sample((k as f32 * 0.01).sin()).unwrap();
            } else {
                let max = (1i64 << (bits - 1)) - 1;
                w.write_sample(((k as i64 * 7919) % max) as i32).unwrap();
            }
        }
        w.finalize().unwrap();
    }
    buf
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn decode_random_bytes(bytes in prop::collection::vec(any::<u8>(), 0..512), prefix in 0u8..3) {
        let mut b = match prefix {
            0 => b"RIFF\0\0\0\0WAVE".to_vec(),
            1 => b"OggS".to_vec(),
            _ => Vec::new(),
        };
        b.extend(bytes);
        let _ = decode(&b, 48_000);
    }

    #[test]
    fn decode_truncated_and_corrupted_wav(
        ch in 1u16..3, rate in prop::sample::select(vec![22_050u32, 44_100, 48_000]),
        fmt in 0usize..5, n in 0usize..64, cut in any::<prop::sample::Index>(),
        flip in any::<prop::sample::Index>(), x in any::<u8>(),
    ) {
        let (bits, float) = [(8, false), (16, false), (24, false), (32, false), (32, true)][fmt];
        let full = wav_bytes(ch, rate, bits, float, n);
        let s = decode(&full, 48_000).unwrap();
        prop_assert_eq!(s.data().len(), 2 * s.frames());
        let _ = decode(&full[..cut.index(full.len())], 48_000);
        let mut bad = full.clone();
        let i = flip.index(bad.len());
        bad[i] ^= x;
        let _ = decode(&bad, 48_000);
    }

    #[test]
    fn mix_is_bounded_sum(levels in prop::collection::vec((-0.05f32..0.05, 0u8..3, 0.0f32..2.0), 0..40),
                          frames in 1usize..700) {
        let (mut tx, rx) = trigger_ring(64);
        let mut m = Mixer::new(MixerConfig { duck_gain: 1.0, ..MixerConfig::default() }, rx);
        let cfg = *m.config();
        // Model of the voice slots: lowest free slot, else steal the oldest
        // (all start in this call, so equally old: the lowest slot).
        let mut slots: [Vec<Option<f32>>; 3] = cfg.max_voices.map(|n| vec![None; n]);
        for (i, &(lv, b, g)) in levels.iter().enumerate() {
            let bus = Bus::ALL[usize::from(b)];
            m.register(SoundId(i as u16), Arc::new(Sample::constant(10_000, lv, -lv)), bus);
            tx.send(SoundTrigger { sound: SoundId(i as u16), gain: g });
            let s = &mut slots[bus.index()];
            if let Some(free) = s.iter().position(Option::is_none) {
                s[free] = Some(lv * g);
            } else if !s.is_empty() {
                s[0] = Some(lv * g);
            }
        }
        let mut out = vec![0.0; 2 * frames];
        m.render(&mut out);
        let expect: f64 = slots.iter().flatten().flatten().map(|&x| f64::from(x)).sum();
        for f in out.chunks(2) {
            prop_assert!(f[0].abs() <= 1.0 && f[1].abs() <= 1.0);
            prop_assert!((f64::from(f[0]) - expect).abs() < 1e-4);
            prop_assert!((f64::from(f[1]) + expect).abs() < 1e-4);
        }
    }
}
