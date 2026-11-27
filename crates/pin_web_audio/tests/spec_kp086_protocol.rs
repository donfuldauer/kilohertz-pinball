//! Independent Tester, KP-086 (#180): the binary trigger protocol of
//! `pin_web_audio` (REQ-WEB-022; R11 hostile input).
//!
//! Property tests call the `pwa_*` ABI as plain Rust and compare it with an
//! independent model (`spec_kp086/oracle.rs`: the brief's scheduling rule in
//! front of native `pin_audio::Mixer` + `trigger_ring`). Records are built by
//! hand from the brief's 16-byte layout, never with the crate's encoder.

#[path = "spec_kp086/eng.rs"]
mod eng;
#[path = "spec_kp086/oracle.rs"]
mod oracle;

use eng::Eng;
use oracle::*;
use proptest::prelude::*;

/// Registered sounds for the differential runs: short so voices end, on
/// every bus, mono and stereo.
fn rig(rate: u32, frames: u32, cap: u32) -> (Eng, Model) {
    let mut e = Eng::new(rate, frames, cap, 64).unwrap();
    let mut m = Model::new(rate, frames, cap);
    let sounds = [
        (0u32, 2u32, 1u32, noise(300, 1)),
        (1, 2, 2, noise(500, 2)),
        (2, 1, 1, noise(900, 3)),
        (3, 0, 2, noise(2000, 4)),
        (65_535, 2, 1, noise(130, 5)),
    ];
    for (id, bus, ch, pcm) in &sounds {
        assert_eq!(e.register(*id, *bus, *ch, pcm), 0);
        m.register(*id as u16, *bus, *ch, pcm);
    }
    (e, m)
}

fn check_stats(e: &Eng, m: &Model, ctx: &str) {
    for which in [0, 1, 2, 3, 4, 5, 8] {
        let (g, w) = (e.stat(which), m.stat(which));
        assert_eq!(g, w, "{ctx}: pwa_stat({which}) = {g}, model {w}");
    }
}

/// One step of a random session.
#[derive(Debug, Clone)]
enum Step {
    /// Records `(op, id, gain bits, at)`, submitted with `extra` trailing
    /// garbage bytes (0..16).
    Submit(Vec<(u32, u32, u32, u32)>, Vec<u8>),
    /// `pwa_trigger(id, gain bits)`.
    Trigger(u32, u32),
    /// `pwa_render(frames)` (including invalid frame counts).
    Render(u32),
}

fn op() -> impl Strategy<Value = u32> {
    prop_oneof![8 => Just(1u32), 1 => Just(0u32), 1 => Just(2u32), 1 => any::<u32>()]
}

fn id() -> impl Strategy<Value = u32> {
    prop_oneof![
        6 => 0u32..4,
        1 => Just(65_535u32),
        2 => 4u32..20, // unregistered
        1 => Just(65_536u32),
        1 => any::<u32>(),
    ]
}

fn gain_bits() -> impl Strategy<Value = u32> {
    prop_oneof![
        6 => (0.0f32..1.5).prop_map(f32::to_bits),
        1 => Just(f32::NAN.to_bits()),
        1 => Just(0xFFC0_0001u32), // another NaN pattern
        1 => Just(f32::INFINITY.to_bits()),
        1 => Just(f32::NEG_INFINITY.to_bits()),
        1 => (-2.0f32..0.0).prop_map(f32::to_bits),
        1 => Just(3.0e38f32.to_bits()),
        1 => any::<u32>(),
    ]
}

fn at() -> impl Strategy<Value = u32> {
    prop_oneof![
        3 => Just(NEXT),
        4 => 0u32..12, // past, present and near future (equal values likely)
        1 => 12u32..40,
        1 => any::<u32>(),
    ]
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        3 => (
            prop::collection::vec((op(), id(), gain_bits(), at()), 0..40),
            prop_oneof![4 => Just(vec![]), 1 => prop::collection::vec(any::<u8>(), 1..16)],
        )
            .prop_map(|(r, x)| Step::Submit(r, x)),
        2 => (id(), gain_bits()).prop_map(|(i, g)| Step::Trigger(i, g)),
        4 => prop_oneof![10 => 1u32..=64, 1 => Just(0u32), 1 => 65u32..200]
            .prop_map(Step::Render),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(160))]

    /// REQ-WEB-022, R11 (a): arbitrary bytes copied into `pwa_cmd_buffer`
    /// and submitted with any length (multiples of 16 or not, above 4096)
    /// never panic, and accept at most one record per 16 bytes.
    #[test]
    fn req_web_022_random_command_bytes_never_panic(
        bytes in prop::collection::vec(any::<u8>(), 0..4097),
        len_kind in 0u8..4,
        extra in any::<u32>(),
    ) {
        let mut e = Eng::new(48_000, 128, 4, 64).unwrap();
        assert_eq!(e.register(1, 2, 1, &noise(64, 1)), 0);
        let n_bytes = match len_kind {
            0 => bytes.len() as u32,
            1 => extra,
            2 => extra % 5000,
            _ => u32::MAX,
        };
        let accepted = e.submit_with_len(&bytes, n_bytes);
        let clamped = n_bytes.min(4096);
        prop_assert!(accepted <= clamped / 16);
        let records = clamped.div_ceil(16);
        // Every record is accepted, rejected or dropped exactly once.
        prop_assert_eq!(
            f64::from(accepted) + e.stat(0) + e.stat(1),
            f64::from(records)
        );
        for _ in 0..3 {
            prop_assert!(e.render(128).is_some());
        }
    }

    /// REQ-WEB-022 (b): random sessions of hand-built records (random op,
    /// id, gain bits, `at`), `pwa_trigger` and `pwa_render` calls equal the
    /// independent model bit for bit, and the `pwa_stat` counters match
    /// (dropped, rejected, unknown, stolen, frames, quantum, active voices).
    /// A small trigger ring (1..=4) and up to 40 records per submit make
    /// both the ring and the 256-entry pending list overflow.
    #[test]
    fn req_web_022_protocol_differential_against_native_model(
        cap in 1u32..=4,
        rate in prop_oneof![Just(48_000u32), Just(44_100u32), Just(8_000u32)],
        steps in prop::collection::vec(step(), 1..60),
    ) {
        let (mut e, mut m) = rig(rate, 64, cap);
        for (i, s) in steps.iter().enumerate() {
            match s {
                Step::Submit(recs, extra) => {
                    let mut bytes: Vec<u8> = recs
                        .iter()
                        .flat_map(|&(o, d, g, a)| record(o, d, g, a))
                        .collect();
                    bytes.extend_from_slice(extra);
                    bytes.truncate(4096);
                    let got = e.submit(&bytes);
                    let want = m.submit(&bytes);
                    prop_assert_eq!(got, want, "step {} submit accepted", i);
                }
                Step::Trigger(d, g) => {
                    let got = e.trigger(*d, f32::from_bits(*g));
                    let want = m.trigger(*d, f32::from_bits(*g));
                    prop_assert_eq!(got, want, "step {} trigger", i);
                }
                Step::Render(f) => {
                    let got = e.render(*f).map(<[f32]>::to_vec);
                    let want = m.render(*f).map(<[f32]>::to_vec);
                    prop_assert_eq!(got.is_some(), want.is_some(), "step {} null", i);
                    if let (Some(g), Some(w)) = (got, want) {
                        assert_bits_eq(&g, &w, &format!("step {i} render {f}"));
                    }
                }
            }
            check_stats(&e, &m, &format!("after step {i}"));
        }
        // Drain everything scheduled in the near future.
        for k in 0..45 {
            let g = e.render(64).unwrap().to_vec();
            assert_bits_eq(&g, m.render(64).unwrap(), &format!("drain {k}"));
        }
        check_stats(&e, &m, "after drain");
    }
}

/// REQ-WEB-022 (c): the hand-built record encoding; the accepted count and
/// the rejected counter follow the brief (op 1 only, id <= 65535, a
/// trailing partial record counts as one rejected record, n_bytes clamped
/// to 4096 = 256 records).
#[test]
fn req_web_022_hand_built_record_encoding_and_accept_count() {
    let mut e = Eng::new(48_000, 128, 64, 64).unwrap();
    let mut b = Vec::new();
    b.extend_from_slice(&record(1, 1, 1.0f32.to_bits(), NEXT)); // ok
    b.extend_from_slice(&record(0, 1, 1.0f32.to_bits(), NEXT)); // op 0
    b.extend_from_slice(&record(2, 1, 1.0f32.to_bits(), NEXT)); // op 2
    b.extend_from_slice(&record(1, 65_535, f32::NAN.to_bits(), 0)); // ok
    b.extend_from_slice(&record(1, 65_536, 0, 0)); // id too big
    b.extend_from_slice(&record(1, 7, (-1.0f32).to_bits(), 5)); // ok
    b.extend_from_slice(&record(u32::MAX, 1, 0, 0)); // op
    assert_eq!(e.submit(&b), 3);
    assert_eq!(e.stat(1), 4.0);
    // Trailing partial record: 1 accepted + 1 rejected.
    let mut p = record(1, 2, 0.5f32.to_bits(), NEXT).to_vec();
    p.extend_from_slice(&[1, 0, 0, 0, 3]);
    assert_eq!(e.submit(&p), 1);
    assert_eq!(e.stat(1), 5.0);
    // n_bytes = 0 parses nothing.
    assert_eq!(e.submit(&[]), 0);
    assert_eq!(e.stat(1), 5.0);
    assert_eq!(e.stat(0), 0.0);
    // 256 good records with n_bytes clamped from u32::MAX to 4096: the
    // pending list already holds 4, so 252 are accepted and 4 dropped.
    let mut f = Vec::new();
    for _ in 0..256 {
        f.extend_from_slice(&record(1, 1, 0.1f32.to_bits(), 1_000));
    }
    assert_eq!(e.submit_with_len(&f, u32::MAX), 252);
    assert_eq!(e.stat(0), 4.0, "pending list full: dropped");
    assert_eq!(e.stat(1), 5.0);
    // A full pending list also drops pwa_trigger.
    assert_eq!(e.trigger(1, 1.0), 0);
    assert_eq!(e.stat(0), 5.0);
    // Little-endian: the same fields byte-swapped are a different op.
    let mut be = Vec::new();
    for w in [1u32, 1, 1.0f32.to_bits(), NEXT] {
        be.extend_from_slice(&w.to_be_bytes());
    }
    let mut e2 = Eng::new(48_000, 128, 64, 64).unwrap();
    assert_eq!(e2.submit(&be), 0);
    assert_eq!(e2.stat(1), 1.0);
}

/// REQ-WEB-022 (scheduling rule): `at` in the past and "next quantum" apply
/// at the current quantum; a future `at` applies exactly at that quantum;
/// `at = 0xFFFF_FFFF` is "next quantum", not "never".
#[test]
fn req_web_022_at_scheduling_past_next_future() {
    let mut e = Eng::new(48_000, 128, 64, 64).unwrap();
    assert_eq!(e.register(1, 2, 1, &vec![0.5; 128]), 0);
    for _ in 0..5 {
        e.render(128).unwrap();
    }
    // q = 5: at 2 (past), NEXT, at 5 (now) all start this quantum: three
    // voices of 0.5 at gain 0.1/0.2/0.3.
    let mut b = Vec::new();
    b.extend_from_slice(&record(1, 1, 0.1f32.to_bits(), 2));
    b.extend_from_slice(&record(1, 1, 0.2f32.to_bits(), NEXT));
    b.extend_from_slice(&record(1, 1, 0.3f32.to_bits(), 5));
    b.extend_from_slice(&record(1, 1, 1.0f32.to_bits(), 8)); // future
    assert_eq!(e.submit(&b), 4);
    assert_eq!(e.stat(8), 0.0, "nothing starts before a render");
    let out = e.render(128).unwrap().to_vec();
    assert!(out.iter().all(|v| *v != 0.0), "q5 voices start at frame 0");
    for q in 6..8 {
        assert!(
            e.render(128).unwrap().iter().all(|v| *v == 0.0),
            "q{q} silent"
        );
    }
    assert!(e.render(128).unwrap().iter().all(|v| *v != 0.0), "q8 plays");
    assert!(e.render(128).unwrap().iter().all(|v| *v == 0.0));
}

/// REQ-WEB-022 (d): two triggers for the same sound in one quantum start two
/// voices in arrival order (observable through voice stealing: with the
/// callout bus limited to 4 voices, the earliest arrivals are stolen).
#[test]
fn req_web_022_same_quantum_triggers_start_in_arrival_order() {
    let mut e = Eng::new(48_000, 128, 64, 64).unwrap();
    let mut m = Model::new(48_000, 128, 64);
    let pcm = noise(4000, 77);
    assert_eq!(e.register(3, 1, 1, &pcm), 0);
    m.register(3, 1, 1, &pcm);
    // Two triggers for the same sound in one quantum, mixing records with
    // equal `at` and pwa_trigger, with distinct gains so order matters.
    let gains = [0.1f32, 0.9, 0.3, 0.7, 0.5, 1.3];
    let mut b = Vec::new();
    for g in &gains[..3] {
        b.extend_from_slice(&record(1, 3, g.to_bits(), 0));
    }
    assert_eq!(e.submit(&b), 3);
    assert_eq!(m.submit(&b), 3);
    for g in &gains[3..] {
        assert_eq!(e.trigger(3, *g), 1);
        m.trigger(3, *g);
    }
    for q in 0..40 {
        let got = e.render(128).unwrap().to_vec();
        assert_bits_eq(&got, m.render(128).unwrap(), &format!("q {q}"));
        if q == 0 {
            assert_eq!(e.stat(8), 4.0, "callout bus keeps 4 voices");
            assert_eq!(e.stat(3), 2.0, "two stolen");
        }
    }
    // Reversed arrival would differ from the model: prove order matters.
    let mut r = Model::new(48_000, 128, 64);
    r.register(3, 1, 1, &pcm);
    for g in gains.iter().rev() {
        r.trigger(3, *g);
    }
    let mut e2 = Eng::new(48_000, 128, 64, 64).unwrap();
    assert_eq!(e2.register(3, 1, 1, &pcm), 0);
    for g in &gains {
        e2.trigger(3, *g);
    }
    let a = e2.render(128).unwrap().to_vec();
    let rv = r.render(128).unwrap().to_vec();
    assert!(
        a.iter().zip(&rv).any(|(x, y)| x.to_bits() != y.to_bits()),
        "arrival order must be observable"
    );
}

/// REQ-WEB-022: a ring that overflows in one quantum (trigger_capacity 2)
/// counts the extra triggers in stat 0 and plays the first two in arrival
/// order.
#[test]
fn req_web_022_ring_overflow_counts_dropped() {
    let mut e = Eng::new(48_000, 128, 2, 64).unwrap();
    let mut m = Model::new(48_000, 128, 2);
    let pcm = noise(1000, 4);
    assert_eq!(e.register(1, 2, 1, &pcm), 0);
    m.register(1, 2, 1, &pcm);
    let mut b = Vec::new();
    for k in 0..5u32 {
        b.extend_from_slice(&record(1, 1, (0.2 * (k + 1) as f32).to_bits(), 3));
    }
    assert_eq!(e.submit(&b), 5, "accepted into the pending list");
    m.submit(&b);
    for q in 0..10 {
        let g = e.render(128).unwrap().to_vec();
        assert_bits_eq(&g, m.render(128).unwrap(), &format!("q {q}"));
    }
    assert_eq!(e.stat(0), 3.0, "three ring-full pushes dropped");
    check_stats(&e, &m, "overflow");
}
