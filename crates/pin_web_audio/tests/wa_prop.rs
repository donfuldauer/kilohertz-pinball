//! REQ-WEB-022: property tests of the trigger message protocol.
//!
//! * encode/parse round trip;
//! * arbitrary bytes through `pwa_submit` never panic and accepted + rejected
//!   + dropped account for every record;
//! * a differential test: the engine against a reference `pin_audio::Mixer`
//!   driven by an independent model of the scheduling rule, bit for bit, for
//!   random record sequences interleaved with renders (past, equal and future
//!   `at`, ring full, pending full, unknown ids, NaN and infinite gains).

use std::sync::Arc;

use pin_audio::{Bus, Mixer, MixerConfig, Sample, SoundSender, SoundTrigger, trigger_ring};
use pin_core::SoundId;
use pin_web_audio::*;
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// REQ-WEB-022: `parse_record(encode_trigger(..))` round-trips every
    /// valid id, every gain bit pattern and every `at`.
    #[test]
    fn req_web_022_round_trip(id in 0u32..=65_535, bits in any::<u32>(), at in any::<u32>()) {
        let gain = f32::from_bits(bits);
        let c = parse_record(&encode_trigger(id, gain, at)).unwrap();
        prop_assert_eq!(c.sound, SoundId(id as u16));
        prop_assert_eq!(c.gain.to_bits(), bits);
        prop_assert_eq!(c.at, At::from_wire(at));
        if at == NEXT_QUANTUM {
            prop_assert_eq!(c.at, At::Next);
        } else {
            prop_assert_eq!(c.at, At::Quantum(at));
        }
    }

    /// REQ-WEB-022: ids above 65535 and any op other than 1 are rejected.
    #[test]
    fn req_web_022_rejects(op in any::<u32>(), id in any::<u32>(), tail in any::<[u8; 8]>()) {
        let mut r = [0u8; 16];
        r[0..4].copy_from_slice(&op.to_le_bytes());
        r[4..8].copy_from_slice(&id.to_le_bytes());
        r[8..16].copy_from_slice(&tail);
        let ok = parse_record(&r).is_ok();
        prop_assert_eq!(ok, op == OP_TRIGGER && id <= 65_535);
    }

    /// REQ-WEB-022 / R11: bounded random bytes through `pwa_submit` never
    /// panic, and every record (a trailing partial one included) is either
    /// accepted, rejected or dropped.
    #[test]
    fn req_web_022_random_bytes_submit(
        chunks in proptest::collection::vec(proptest::collection::vec(any::<u8>(), 0..5000), 0..4),
        renders in proptest::collection::vec(1u32..200, 0..4),
    ) {
        let e = pwa_new(48_000, 128, 4, 1);
        prop_assert!(!e.is_null());
        for (i, bytes) in chunks.iter().enumerate() {
            let buf = unsafe { pwa_cmd_buffer(e) };
            let n = bytes.len().min(CMD_BUFFER_BYTES);
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, n) };
            let before = unsafe { (pwa_stat(e, 0), pwa_stat(e, 1)) };
            let accepted = unsafe { pwa_submit(e, bytes.len() as u32) };
            let after = unsafe { (pwa_stat(e, 0), pwa_stat(e, 1)) };
            let records = n.div_ceil(RECORD_BYTES) as f64;
            prop_assert_eq!(
                f64::from(accepted) + (after.0 - before.0) + (after.1 - before.1),
                records
            );
            if let Some(&f) = renders.get(i) {
                let _ = unsafe { pwa_render(e, f) };
            }
        }
        unsafe { pwa_free(e) };
    }

    /// REQ-WEB-022: the engine equals a reference `Mixer` driven by an
    /// independent model of the scheduling rule, bit for bit.
    #[test]
    fn req_web_022_differential(
        ring in 1u32..6,
        ops in proptest::collection::vec(op_strategy(), 1..60),
    ) {
        differential(ring, &ops)?;
    }
}

#[derive(Clone, Debug)]
enum Op {
    /// Raw records (op, id, gain bits, at offset) plus trailing garbage.
    Submit(Vec<(u32, u32, u32, AtSpec)>, Vec<u8>),
    /// `pwa_trigger(id, gain)`.
    Trigger(u32, u32),
    /// `n` identical records, to fill the pending list.
    Flood(u16, AtSpec),
    /// `pwa_render(frames)` (0 and too-large included).
    Render(u32),
}

/// `at` relative to the engine's quantum index at submission time.
#[derive(Clone, Copy, Debug)]
enum AtSpec {
    Next,
    /// `q - k` (saturating; 0 when k > q).
    Past(u32),
    /// `q + k`.
    Future(u32),
    /// An absolute value.
    Abs(u32),
}

impl AtSpec {
    fn wire(self, q: u64) -> u32 {
        let q = q as u32;
        match self {
            AtSpec::Next => NEXT_QUANTUM,
            AtSpec::Past(k) => q.saturating_sub(k),
            AtSpec::Future(k) => q + k,
            AtSpec::Abs(v) => v,
        }
    }
}

fn at_strategy() -> impl Strategy<Value = AtSpec> {
    prop_oneof![
        3 => Just(AtSpec::Next),
        2 => (0u32..4).prop_map(AtSpec::Past),
        3 => (0u32..4).prop_map(AtSpec::Future),
        1 => prop_oneof![Just(u32::MAX - 1), Just(0u32), 0u32..20].prop_map(AtSpec::Abs),
    ]
}

fn gain_strategy() -> impl Strategy<Value = u32> {
    prop_oneof![
        4 => (0.0f32..1.5).prop_map(f32::to_bits),
        1 => Just(f32::NAN.to_bits()),
        1 => Just(f32::INFINITY.to_bits()),
        1 => Just(f32::NEG_INFINITY.to_bits()),
        1 => (-1.0f32..0.0).prop_map(f32::to_bits),
        1 => any::<u32>(),
    ]
}

fn id_strategy() -> impl Strategy<Value = u32> {
    prop_oneof![
        6 => 1u32..5,          // registered (1..=4)
        1 => 5u32..10,         // unknown
        1 => Just(0u32),       // unknown
        1 => 65_536u32..70_000, // rejected
        1 => Just(u32::MAX),
    ]
}

fn op_strategy() -> impl Strategy<Value = Op> {
    let rec = (
        prop_oneof![8 => Just(1u32), 1 => any::<u32>()],
        id_strategy(),
        gain_strategy(),
        at_strategy(),
    );
    prop_oneof![
        3 => (
            proptest::collection::vec(rec, 0..8),
            prop_oneof![4 => Just(Vec::new()), 1 => proptest::collection::vec(any::<u8>(), 1..16)],
        ).prop_map(|(r, t)| Op::Submit(r, t)),
        2 => (id_strategy(), gain_strategy()).prop_map(|(i, g)| Op::Trigger(i, g)),
        1 => (1u16..300, at_strategy()).prop_map(|(n, a)| Op::Flood(n, a)),
        4 => prop_oneof![10 => 1u32..=32, 1 => Just(0u32), 1 => Just(33u32)].prop_map(Op::Render),
    ]
}

const FRAMES: u32 = 32;

fn samples() -> Vec<(u32, u32, Bus, Sample)> {
    let ramp: Vec<f32> = (0..50).map(|i| i as f32 / 64.0 - 0.3).collect();
    let st: Vec<f32> = (0..80)
        .map(|i| ((i * 7) % 13) as f32 / 13.0 - 0.5)
        .collect();
    vec![
        (1, 2, Bus::Sfx, Sample::from_mono_vec(ramp.clone())),
        (2, 0, Bus::Music, Sample::from_interleaved(st.clone())),
        (3, 1, Bus::Callout, Sample::constant(70, 0.25, -0.125)),
        (4, 2, Bus::Sfx, Sample::from_interleaved(st)),
    ]
}

/// The independent reference: the Mixer plus a model of the pending list.
struct Model {
    mixer: Mixer,
    tx: SoundSender,
    ring: usize,
    /// (effective quantum, arrival number, trigger).
    pending: Vec<(u64, u64, SoundTrigger)>,
    arrivals: u64,
    q: u64,
    dropped: u64,
    rejected: u64,
}

impl Model {
    /// Returns whether the record was accepted into the pending list.
    fn record(&mut self, op: u32, id: u32, gain: u32, at: u32) -> bool {
        if op != 1 || id > 65_535 {
            self.rejected += 1;
            return false;
        }
        if self.pending.len() >= 256 {
            self.dropped += 1;
            return false;
        }
        let eff = if at == u32::MAX {
            self.q
        } else {
            u64::from(at).max(self.q)
        };
        self.arrivals += 1;
        self.pending.push((
            eff,
            self.arrivals,
            SoundTrigger {
                sound: SoundId(id as u16),
                gain: f32::from_bits(gain),
            },
        ));
        true
    }

    fn render(&mut self, frames: u32, out: &mut [f32]) {
        let mut due: Vec<_> = self
            .pending
            .iter()
            .copied()
            .filter(|p| p.0 <= self.q)
            .collect();
        self.pending.retain(|p| p.0 > self.q);
        due.sort_by_key(|p| (p.0, p.1));
        for (k, p) in due.iter().enumerate() {
            if k < self.ring {
                assert!(self.tx.send(p.2));
            } else {
                self.dropped += 1;
            }
        }
        self.mixer.render(&mut out[..frames as usize * 2]);
        self.q += 1;
    }
}

fn differential(ring: u32, ops: &[Op]) -> Result<(), TestCaseError> {
    let e = pwa_new(48_000, FRAMES, ring, 1);
    prop_assert!(!e.is_null());
    let (tx, rx) = trigger_ring(ring as usize);
    let mut model = Model {
        mixer: Mixer::new(
            MixerConfig {
                sample_rate: 48_000,
                buffer_frames: FRAMES as usize,
                ..MixerConfig::default()
            },
            rx,
        ),
        tx,
        ring: ring as usize,
        pending: Vec::new(),
        arrivals: 0,
        q: 0,
        dropped: 0,
        rejected: 0,
    };
    for (id, bus, b, s) in samples() {
        let p = unsafe { pwa_stage(e, s.data().len() as u32) };
        unsafe { std::ptr::copy_nonoverlapping(s.data().as_ptr(), p, s.data().len()) };
        let code = unsafe { pwa_register(e, id, bus, s.channels() as u32, s.frames() as u32) };
        prop_assert_eq!(code, 0);
        model.mixer.register(SoundId(id as u16), Arc::new(s), b);
    }
    let mut out = vec![0.0f32; FRAMES as usize * 2];
    let buf = unsafe { pwa_cmd_buffer(e) };
    for op in ops {
        let q = model.q;
        match op {
            Op::Submit(recs, tail) => {
                let mut bytes = Vec::new();
                let mut want = 0;
                for &(o, id, g, at) in recs {
                    let at = at.wire(q);
                    let mut r = encode_trigger(id, f32::from_bits(g), at);
                    r[0..4].copy_from_slice(&o.to_le_bytes());
                    bytes.extend_from_slice(&r);
                    want += u32::from(model.record(o, id, g, at));
                }
                bytes.extend_from_slice(tail);
                if tail.len() % 16 != 0 {
                    model.rejected += 1;
                }
                unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len()) };
                let got = unsafe { pwa_submit(e, bytes.len() as u32) };
                prop_assert_eq!(got, want);
            }
            Op::Trigger(id, g) => {
                let want = model.record(1, *id, *g, u32::MAX);
                let got = unsafe { pwa_trigger(e, *id, f32::from_bits(*g)) };
                prop_assert_eq!(got, u32::from(want));
            }
            Op::Flood(n, at) => {
                // In chunks of 256 records (the buffer size).
                let mut left = *n as usize;
                while left > 0 {
                    let k = left.min(256);
                    let at = at.wire(model.q);
                    let mut want = 0;
                    for i in 0..k {
                        let r = encode_trigger(1, 0.01, at);
                        unsafe { std::ptr::copy_nonoverlapping(r.as_ptr(), buf.add(i * 16), 16) };
                        want += u32::from(model.record(1, 1, 0.01f32.to_bits(), at));
                    }
                    prop_assert_eq!(unsafe { pwa_submit(e, (k * 16) as u32) }, want);
                    left -= k;
                }
            }
            Op::Render(frames) => {
                let p = unsafe { pwa_render(e, *frames) };
                if *frames == 0 || *frames > FRAMES {
                    prop_assert!(p.is_null());
                    continue;
                }
                prop_assert!(!p.is_null());
                model.render(*frames, &mut out);
                let got = unsafe { std::slice::from_raw_parts(p, *frames as usize * 2) };
                for (a, b) in got.iter().zip(&out) {
                    prop_assert_eq!(a.to_bits(), b.to_bits());
                }
            }
        }
        let stats = unsafe { [pwa_stat(e, 0), pwa_stat(e, 1), pwa_stat(e, 5)] };
        prop_assert_eq!(
            stats,
            [model.dropped as f64, model.rejected as f64, model.q as f64]
        );
        prop_assert_eq!(
            unsafe { pwa_stat(e, 2) },
            model.mixer.unknown_sounds() as f64
        );
        prop_assert_eq!(
            unsafe { pwa_stat(e, 3) },
            model.mixer.stolen_voices() as f64
        );
        prop_assert_eq!(
            unsafe { pwa_stat(e, 4) },
            model.mixer.frames_rendered() as f64
        );
        let active: usize = Bus::ALL.iter().map(|&b| model.mixer.active_voices(b)).sum();
        prop_assert_eq!(unsafe { pwa_stat(e, 8) }, active as f64);
    }
    unsafe { pwa_free(e) };
    Ok(())
}

/// REQ-WEB-022: a fixed scenario through the differential harness that hits
/// ring-full, pending-full, a past `at`, equal `at` and NaN/inf gains.
#[test]
fn req_web_022_differential_fixed_edges() {
    let nan = f32::NAN.to_bits();
    let inf = f32::INFINITY.to_bits();
    let ops = vec![
        Op::Submit(
            vec![
                (1, 1, 0.5f32.to_bits(), AtSpec::Future(1)),
                (1, 4, 0.5f32.to_bits(), AtSpec::Future(1)),
                (1, 2, nan, AtSpec::Next),
                (1, 3, inf, AtSpec::Next),
                (1, 9, 1.0f32.to_bits(), AtSpec::Next),
                (7, 1, 1.0f32.to_bits(), AtSpec::Next),
            ],
            vec![1, 2, 3],
        ),
        Op::Render(32),
        Op::Trigger(1, 1.0f32.to_bits()),
        Op::Trigger(1, 0.2f32.to_bits()),
        Op::Trigger(1, 0.3f32.to_bits()),
        Op::Render(16),
        Op::Submit(vec![(1, 1, 0.7f32.to_bits(), AtSpec::Past(5))], Vec::new()),
        Op::Render(32),
        Op::Flood(300, AtSpec::Future(2)),
        Op::Render(32),
        Op::Render(32),
        Op::Render(32),
    ];
    differential(2, &ops).unwrap();
}
