//! Subphase 2.5 spec tests (Independent Tester): the binary replay file
//! (`Recording::to_bytes` / `from_bytes`): round trip, versioned magic
//! header, and robustness against arbitrary bytes (gate step 5). Needs no
//! simulation.

use pin_core::input::{CabinetAxis, CabinetInput, InputEvent, InputEventKind};
use pin_core::sim::TICK_NS;
use pin_sim::replay::Recording;
use proptest::prelude::*;

/// (tick offset, sub-tick ns, kind selector, index, pressed, value bits)
type RawEvent = (u64, u64, bool, usize, bool, u32);

fn build(seed: u64, table_hash: u64, base_ticks: u64, raw: &[RawEvent]) -> Recording {
    // Non-decreasing ticks in 1..=ticks, timestamps inside their tick.
    let mut tick = 0u64;
    let mut events = Vec::new();
    for &(dt, sub, is_axis, idx, pressed, bits) in raw {
        tick += dt;
        let tick1 = tick.max(1);
        let ts = (tick1 - 1) * TICK_NS + 1 + sub % TICK_NS;
        let ev = if is_axis {
            let v = f32::from_bits(bits);
            let v = if v.is_finite() {
                v.clamp(-1.0, 1.0)
            } else {
                0.25
            };
            InputEvent::axis(ts, CabinetAxis::ALL[idx % CabinetAxis::ALL.len()], v)
        } else {
            InputEvent::button(
                ts,
                CabinetInput::ALL[idx % CabinetInput::ALL.len()],
                pressed,
            )
        };
        events.push((tick1, ev));
    }
    Recording {
        seed,
        table_hash,
        ticks: tick.max(1) + base_ticks,
        events,
    }
}

fn arb_raw() -> impl Strategy<Value = Vec<RawEvent>> {
    prop::collection::vec(
        (
            prop_oneof![3 => 0u64..3, 1 => 0u64..5000],
            any::<u64>(),
            any::<bool>(),
            0usize..8,
            any::<bool>(),
            any::<u32>(),
        ),
        0..64,
    )
}

fn assert_same(a: &Recording, b: &Recording) {
    assert_eq!(a.seed, b.seed);
    assert_eq!(a.table_hash, b.table_hash);
    assert_eq!(a.ticks, b.ticks);
    assert_eq!(a.events.len(), b.events.len());
    for (i, (x, y)) in a.events.iter().zip(&b.events).enumerate() {
        assert_eq!(x.0, y.0, "event {i}: tick");
        assert_eq!(x.1.timestamp_ns, y.1.timestamp_ns, "event {i}: timestamp");
        match (x.1.kind, y.1.kind) {
            (
                InputEventKind::Axis {
                    axis: a1,
                    value: v1,
                },
                InputEventKind::Axis {
                    axis: a2,
                    value: v2,
                },
            ) => {
                assert_eq!(a1, a2, "event {i}: axis");
                assert_eq!(v1.to_bits(), v2.to_bits(), "event {i}: axis value bits");
            }
            (k1, k2) => assert_eq!(k1, k2, "event {i}: kind"),
        }
    }
}

fn sample() -> Recording {
    build(
        0x4B485A,
        0x0123_4567_89AB_CDEF,
        100,
        &[
            (5, 10, false, 5, true, 0),
            (0, 20, false, 0, true, 0),
            (40, 999_999, false, 0, false, 0),
            (1, 0, true, 0, false, 0.75f32.to_bits()),
            (300, 7, true, 1, false, (-0.5f32).to_bits()),
            (2, 7, false, 7, true, 0),
        ],
    )
}

#[test]
fn round_trip_preserves_every_field() {
    let r = sample();
    let bytes = r.to_bytes();
    let back = Recording::from_bytes(&bytes).expect("a written recording parses");
    assert_same(&r, &back);
    assert_eq!(back.to_bytes(), bytes, "to_bytes is canonical");
    assert_eq!(r.to_bytes(), bytes, "to_bytes is deterministic");

    // Extremes.
    let empty = Recording {
        seed: 0,
        table_hash: 0,
        ticks: 0,
        events: Vec::new(),
    };
    assert_same(&empty, &Recording::from_bytes(&empty.to_bytes()).unwrap());
    let max = Recording {
        seed: u64::MAX,
        table_hash: u64::MAX,
        ticks: u64::MAX,
        events: vec![(
            u64::MAX,
            InputEvent::button(u64::MAX, CabinetInput::Tilt, true),
        )],
    };
    assert_same(&max, &Recording::from_bytes(&max.to_bytes()).unwrap());
}

#[test]
fn every_cabinet_input_and_axis_round_trips() {
    let mut events = Vec::new();
    let mut tick = 1;
    for input in CabinetInput::ALL {
        for pressed in [true, false] {
            events.push((tick, InputEvent::button(tick * TICK_NS, input, pressed)));
            tick += 1;
        }
    }
    for axis in CabinetAxis::ALL {
        for value in [
            0.0f32,
            -0.0,
            1.0,
            -1.0,
            0.333_333_34,
            f32::MIN_POSITIVE,
            1.0e-40,
        ] {
            events.push((tick, InputEvent::axis(tick * TICK_NS, axis, value)));
            tick += 1;
        }
    }
    let r = Recording {
        seed: 1,
        table_hash: 2,
        ticks: tick,
        events,
    };
    assert_same(&r, &Recording::from_bytes(&r.to_bytes()).unwrap());
}

#[test]
fn header_carries_seed_table_hash_and_a_checked_magic() {
    let a = sample();
    let bytes = a.to_bytes();
    // Seed, table hash and tick count are in the file: changing one changes it.
    let mut b = sample();
    b.seed ^= 1;
    assert_ne!(b.to_bytes(), bytes);
    let mut b = sample();
    b.table_hash ^= 1 << 63;
    assert_ne!(b.to_bytes(), bytes);
    let mut b = sample();
    b.ticks += 1;
    assert_ne!(b.to_bytes(), bytes);

    // Not a recording at all.
    assert!(Recording::from_bytes(&[]).is_err());
    assert!(Recording::from_bytes(b"\0").is_err());
    assert!(Recording::from_bytes(b"PK\x03\x04 this is a zip, not a replay").is_err());
    assert!(Recording::from_bytes(&[0u8; 4096]).is_err());
    assert!(Recording::from_bytes(&[0xFFu8; 4096]).is_err());
    // The header starts with a magic: any change to the first byte is refused.
    for flip in 1..=255u8 {
        let mut bad = bytes.clone();
        bad[0] ^= flip;
        assert!(
            Recording::from_bytes(&bad).is_err(),
            "first byte {:#04x} -> {:#04x} accepted",
            bytes[0],
            bad[0]
        );
    }
}

#[test]
fn truncated_files_are_errors() {
    for r in [
        sample(),
        Recording {
            seed: 7,
            table_hash: 8,
            ticks: 9,
            events: Vec::new(),
        },
    ] {
        let bytes = r.to_bytes();
        for len in 0..bytes.len() {
            assert!(
                Recording::from_bytes(&bytes[..len]).is_err(),
                "a recording cut to {len} of {} bytes was accepted",
                bytes.len()
            );
        }
    }
}

#[test]
fn every_single_byte_corruption_is_handled_without_panic() {
    let r = sample();
    let bytes = r.to_bytes();
    for i in 0..bytes.len() {
        for v in [
            0x00u8,
            0xFF,
            0x80,
            0x7F,
            bytes[i].wrapping_add(1),
            bytes[i] ^ 0x55,
        ] {
            let mut bad = bytes.clone();
            bad[i] = v;
            if let Ok(parsed) = Recording::from_bytes(&bad) {
                // Whatever is accepted must be a consistent value that can be
                // written again.
                let again = parsed.to_bytes();
                assert_same(&parsed, &Recording::from_bytes(&again).unwrap());
            }
        }
    }
    // 8-byte fields blown up (lengths, counts, ticks) at every offset.
    for i in 0..bytes.len() {
        for fill in [0xFFu8, 0x7F, 0x80] {
            let mut bad = bytes.clone();
            for b in bad.iter_mut().skip(i).take(8) {
                *b = fill;
            }
            let _ = Recording::from_bytes(&bad);
        }
    }
    // Trailing garbage and doubled files.
    let mut long = bytes.clone();
    long.extend_from_slice(&[0xAB; 64]);
    let _ = Recording::from_bytes(&long);
    let mut twice = bytes.clone();
    twice.extend_from_slice(&bytes);
    let _ = Recording::from_bytes(&twice);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    /// Invariant: from_bytes(to_bytes(r)) == r for every valid recording.
    #[test]
    fn round_trip(
        seed in any::<u64>(),
        table_hash in any::<u64>(),
        base_ticks in 0u64..100_000,
        raw in arb_raw(),
    ) {
        let r = build(seed, table_hash, base_ticks, &raw);
        let bytes = r.to_bytes();
        let back = Recording::from_bytes(&bytes);
        prop_assert!(back.is_ok(), "a written recording did not parse");
        let back = back.unwrap();
        assert_same(&r, &back);
        prop_assert_eq!(back.to_bytes(), bytes);
    }

    /// Gate step 5: arbitrary bytes never panic.
    #[test]
    fn random_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
        if let Ok(r) = Recording::from_bytes(&bytes) {
            // Bounded by the input: an event needs at least one byte.
            prop_assert!(r.events.len() <= bytes.len());
            let _ = r.to_bytes();
        }
    }

    /// Arbitrary bytes behind a valid header prefix never panic (random bytes
    /// alone rarely get past the magic).
    #[test]
    fn random_tail_after_a_valid_prefix_never_panics(
        raw in arb_raw(),
        keep in 0usize..256,
        tail in prop::collection::vec(any::<u8>(), 0..512),
    ) {
        let good = build(1, 2, 3, &raw).to_bytes();
        let mut bytes = good[..keep.min(good.len())].to_vec();
        bytes.extend_from_slice(&tail);
        if let Ok(r) = Recording::from_bytes(&bytes) {
            prop_assert!(r.events.len() <= bytes.len());
        }
    }

    /// Random edits of a valid file never panic.
    #[test]
    fn mutated_recordings_never_panic(
        raw in arb_raw(),
        edits in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..12),
        cut in any::<prop::sample::Index>(),
        do_cut in any::<bool>(),
    ) {
        let mut bytes = build(0xDEAD_BEEF, 0xFEED_FACE, 17, &raw).to_bytes();
        for (at, v) in edits {
            let i = at.index(bytes.len());
            bytes[i] = v;
        }
        if do_cut {
            bytes.truncate(cut.index(bytes.len() + 1));
        }
        if let Ok(r) = Recording::from_bytes(&bytes) {
            prop_assert!(r.events.len() <= bytes.len());
            let _ = r.to_bytes();
        }
    }
}
