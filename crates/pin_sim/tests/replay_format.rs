//! Subphase 2.5 gate step 5: the replay file parser never panics on
//! arbitrary bytes, bounds its allocation by the input length, and rejects
//! every malformed field.

use pin_core::input::{CabinetAxis, CabinetInput, InputEvent};
use pin_sim::hash::fnv1a64;
use pin_sim::replay::{
    EVENT_BYTES, HEADER_BYTES, MAGIC, MAX_TICKS, Recording, ReplayError, TRAILER_BYTES, VERSION,
};
use proptest::prelude::*;

fn sample() -> Recording {
    Recording {
        seed: 0x4B485A,
        table_hash: 0x0123_4567_89ab_cdef,
        ticks: 50,
        events: vec![
            (1, InputEvent::button(1, CabinetInput::Start, true)),
            (1, InputEvent::axis(2, CabinetAxis::Plunger, 0.75)),
            (7, InputEvent::button(6_500_000, CabinetInput::Tilt, false)),
            (50, InputEvent::axis(49_000_000, CabinetAxis::NudgeY, -1.0)),
        ],
    }
}

/// Recomputes the trailer after editing the body.
fn reseal(bytes: &mut [u8]) {
    let body = bytes.len() - TRAILER_BYTES;
    let sum = fnv1a64(&bytes[..body]);
    bytes[body..].copy_from_slice(&sum.to_le_bytes());
}

#[test]
fn round_trip_and_layout() {
    let r = sample();
    let b = r.to_bytes();
    assert_eq!(b.len(), HEADER_BYTES + 4 * EVENT_BYTES + TRAILER_BYTES);
    assert_eq!(&b[..8], &MAGIC);
    assert_eq!(u32::from_le_bytes(b[8..12].try_into().unwrap()), VERSION);
    assert_eq!(Recording::from_bytes(&b).unwrap(), r);
    let empty = Recording::new(1, 2);
    assert_eq!(Recording::from_bytes(&empty.to_bytes()).unwrap(), empty);
    assert_eq!(r.events_on(1).len(), 2);
    assert_eq!(r.events_on(7).len(), 1);
    assert!(r.events_on(8).is_empty());
}

#[test]
fn tick_count_is_capped() {
    // A valid 52-byte file declaring u64::MAX ticks is refused, not run.
    let mut r = Recording::new(1, 2);
    r.ticks = u64::MAX;
    let b = r.to_bytes();
    assert_eq!(b.len(), 52);
    assert_eq!(
        Recording::from_bytes(&b),
        Err(ReplayError::TooManyTicks(u64::MAX))
    );
    r.ticks = MAX_TICKS + 1;
    let e = Recording::from_bytes(&r.to_bytes()).unwrap_err();
    assert_eq!(e, ReplayError::TooManyTicks(MAX_TICKS + 1));
    assert!(e.to_string().contains("ticks"));
    r.ticks = MAX_TICKS;
    assert_eq!(Recording::from_bytes(&r.to_bytes()).unwrap(), r);
    // replay() refuses a hand-built recording before touching the bundle.
    r.ticks = MAX_TICKS + 1;
    let missing = std::env::temp_dir().join("pin_sim_2_5_no_bundle_for_ticks");
    assert_eq!(
        pin_sim::replay::replay(&missing, &r),
        Err(ReplayError::TooManyTicks(MAX_TICKS + 1))
    );
}

#[test]
fn save_and_load_files() {
    let dir = std::env::temp_dir().join(format!("pin_sim_2_5_fmt_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("r.khzrec");
    sample().save(&path).unwrap();
    assert_eq!(Recording::load(&path).unwrap(), sample());
    assert!(matches!(
        Recording::load(&dir.join("missing")),
        Err(ReplayError::Io(_))
    ));
    assert!(matches!(
        sample().save(&dir.join("no/such/dir/x")),
        Err(ReplayError::Io(_))
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn malformed_files_are_rejected() {
    let good = sample().to_bytes();
    assert_eq!(Recording::from_bytes(b""), Err(ReplayError::BadMagic));
    assert_eq!(
        Recording::from_bytes(b"KHZREPL"),
        Err(ReplayError::BadMagic)
    );
    let mut b = good.clone();
    b[0] = b'X';
    assert_eq!(Recording::from_bytes(&b), Err(ReplayError::BadMagic));
    assert!(matches!(
        Recording::from_bytes(&good[..20]),
        Err(ReplayError::BadLength { .. })
    ));
    let mut b = good.clone();
    b[8] = 2;
    assert_eq!(
        Recording::from_bytes(&b),
        Err(ReplayError::UnsupportedVersion(2))
    );
    assert!(matches!(
        Recording::from_bytes(&good[..good.len() - 1]),
        Err(ReplayError::BadLength { .. })
    ));
    let mut long = good.clone();
    long.push(0);
    assert!(matches!(
        Recording::from_bytes(&long),
        Err(ReplayError::BadLength { .. })
    ));
    // A huge declared count is a length error, not an allocation.
    let mut b = good.clone();
    b[36..44].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(matches!(
        Recording::from_bytes(&b),
        Err(ReplayError::BadLength { .. })
    ));
    let mut b = good.clone();
    let last = b.len() - 1;
    b[last] ^= 1;
    assert_eq!(Recording::from_bytes(&b), Err(ReplayError::Checksum));

    let event = |i: usize| HEADER_BYTES + i * EVENT_BYTES;
    let cases: [(usize, usize, &[u8]); 9] = [
        (0, 0, &0u64.to_le_bytes()),                // tick 0
        (3, 0, &51u64.to_le_bytes()),               // tick > ticks
        (3, 0, &[6]),                               // tick 50 → 6: decreasing
        (0, 16, &[2]),                              // unknown kind
        (0, 17, &[8]),                              // unknown cabinet input
        (1, 17, &[5]),                              // unknown axis
        (0, 20, &2u32.to_le_bytes()),               // button value not 0/1
        (1, 20, &f32::NAN.to_bits().to_le_bytes()), // non-finite axis
        (0, 18, &[1]),                              // reserved byte
    ];
    for (i, (ev, off, patch)) in cases.iter().enumerate() {
        let mut b = good.clone();
        let at = event(*ev) + off;
        b[at..at + patch.len()].copy_from_slice(patch);
        reseal(&mut b);
        assert!(
            matches!(Recording::from_bytes(&b), Err(ReplayError::BadEvent { .. })),
            "case {i}: {:?}",
            Recording::from_bytes(&b)
        );
    }
    for e in [
        ReplayError::BadMagic,
        ReplayError::UnsupportedVersion(9),
        ReplayError::BadLength {
            expected: 1,
            actual: 2,
        },
        ReplayError::Checksum,
        ReplayError::BadEvent { index: 3 },
        ReplayError::TableMismatch {
            recorded: 1,
            bundle: 2,
        },
        ReplayError::Io("x".into()),
    ] {
        assert!(!e.to_string().is_empty());
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2048))]

    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = Recording::from_bytes(&bytes);
    }

    #[test]
    fn arbitrary_bytes_behind_a_valid_header_never_panic(
        tail in proptest::collection::vec(any::<u8>(), 0..400),
        count in any::<u64>(),
        seal in any::<bool>(),
    ) {
        let mut b = Vec::new();
        b.extend_from_slice(&MAGIC);
        b.extend_from_slice(&VERSION.to_le_bytes());
        b.extend_from_slice(&[0u8; 24]);
        b.extend_from_slice(&count.to_le_bytes());
        b.extend_from_slice(&tail);
        if seal && b.len() >= HEADER_BYTES + TRAILER_BYTES {
            reseal(&mut b);
        }
        if let Ok(r) = Recording::from_bytes(&b) {
            prop_assert!(r.events.len() * EVENT_BYTES <= b.len());
            prop_assert_eq!(Recording::from_bytes(&r.to_bytes()).unwrap(), r);
        }
    }

    #[test]
    fn flipped_bits_never_panic(pos in 0usize..1000, bit in 0u8..8, reseal_it in any::<bool>()) {
        let mut b = sample().to_bytes();
        let at = pos % b.len();
        b[at] ^= 1 << bit;
        if reseal_it {
            reseal(&mut b);
        }
        let _ = Recording::from_bytes(&b);
    }

    #[test]
    fn valid_recordings_round_trip(
        seed in any::<u64>(),
        table in any::<u64>(),
        raw in proptest::collection::vec((1u64..100, any::<u64>(), 0usize..8, any::<bool>(), -1.0f32..1.0, any::<bool>()), 0..40),
    ) {
        let mut events: Vec<(u64, InputEvent)> = raw
            .iter()
            .map(|&(t, ts, code, pressed, v, axis)| {
                let e = if axis {
                    InputEvent::axis(ts, CabinetAxis::ALL[code % 5], v)
                } else {
                    InputEvent::button(ts, CabinetInput::ALL[code], pressed)
                };
                (t, e)
            })
            .collect();
        events.sort_by_key(|(t, _)| *t);
        let r = Recording { seed, table_hash: table, ticks: 100, events };
        prop_assert_eq!(Recording::from_bytes(&r.to_bytes()).unwrap(), r);
    }
}
