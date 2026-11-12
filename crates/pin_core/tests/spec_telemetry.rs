//! Black-box spec tests for `pin_core::telemetry` (PLAN.md Appendix B, normative).

use pin_core::telemetry::{
    ENTRY_LEN, FLAG_FULL_STATE, HEADER_LEN, MAGIC, MAX_ENTRIES_PER_DATAGRAM, TelemetryEntry,
    TelemetryKind, VERSION, decode_datagram, encode_datagram,
};
use proptest::prelude::*;

const BUF: usize = 16 + 8 * 181;

fn kind_from(n: u8) -> TelemetryKind {
    match n % 6 {
        0 => TelemetryKind::Switch,
        1 => TelemetryKind::Coil,
        2 => TelemetryKind::Lamp,
        3 => TelemetryKind::Gi,
        4 => TelemetryKind::Magnet,
        _ => TelemetryKind::HapticImpulse,
    }
}

fn kind_code(k: &TelemetryKind) -> u8 {
    match k {
        TelemetryKind::Switch => 0,
        TelemetryKind::Coil => 1,
        TelemetryKind::Lamp => 2,
        TelemetryKind::Gi => 3,
        TelemetryKind::Magnet => 4,
        TelemetryKind::HapticImpulse => 5,
    }
}

fn entries_from(raw: &[(u8, u16, f32)]) -> Vec<TelemetryEntry> {
    raw.iter()
        .map(|&(k, id, value)| TelemetryEntry {
            kind: kind_from(k),
            id,
            value,
        })
        .collect()
}

fn sample(n: usize) -> Vec<(u8, u16, f32)> {
    (0..n)
        .map(|i| (i as u8, (i * 7 + 3) as u16, i as f32 * 0.25))
        .collect()
}

/// Hand-built reference encoder following Appendix B literally.
fn reference_bytes(tick: u64, flags: u8, raw: &[(u8, u16, f32)]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"PNTL");
    v.push(1);
    v.push(flags);
    v.extend_from_slice(&(raw.len() as u16).to_le_bytes());
    v.extend_from_slice(&tick.to_le_bytes());
    for &(k, id, value) in raw {
        v.push(k % 6);
        v.push(0);
        v.extend_from_slice(&id.to_le_bytes());
        v.extend_from_slice(&value.to_le_bytes());
    }
    v
}

#[test]
fn constants_match_appendix_b() {
    assert_eq!(MAGIC, *b"PNTL");
    assert_eq!(VERSION, 1u8);
    assert_eq!(HEADER_LEN, 16);
    assert_eq!(ENTRY_LEN, 8);
    assert_eq!(MAX_ENTRIES_PER_DATAGRAM, 180);
    assert_eq!(FLAG_FULL_STATE, 1u8);
}

#[test]
fn kind_discriminants_match_appendix_b() {
    assert_eq!(TelemetryKind::Switch as u8, 0);
    assert_eq!(TelemetryKind::Coil as u8, 1);
    assert_eq!(TelemetryKind::Lamp as u8, 2);
    assert_eq!(TelemetryKind::Gi as u8, 3);
    assert_eq!(TelemetryKind::Magnet as u8, 4);
    assert_eq!(TelemetryKind::HapticImpulse as u8, 5);
}

#[test]
fn exact_byte_layout() {
    let entries = [
        TelemetryEntry {
            kind: TelemetryKind::Lamp,
            id: 0x1234,
            value: 0.5,
        },
        TelemetryEntry {
            kind: TelemetryKind::HapticImpulse,
            id: 0xBEEF,
            value: 1.0,
        },
    ];
    let mut out = [0xAAu8; 64];
    let n = encode_datagram(0x0102_0304_0506_0708, FLAG_FULL_STATE, &entries, &mut out)
        .expect("encode");
    assert_eq!(n, 32);
    #[rustfmt::skip]
    let expect: [u8; 32] = [
        // header
        b'P', b'N', b'T', b'L',                         // magic
        1,                                              // version
        1,                                              // flags (full state)
        2, 0,                                           // entry count, u16 LE
        0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01, // tick, u64 LE
        // entry 0
        2,                                              // kind = lamp
        0,                                              // reserved
        0x34, 0x12,                                     // id, u16 LE
        0x00, 0x00, 0x00, 0x3F,                         // 0.5f32 LE
        // entry 1
        5,                                              // kind = haptic impulse
        0,                                              // reserved
        0xEF, 0xBE,                                     // id, u16 LE
        0x00, 0x00, 0x80, 0x3F,                         // 1.0f32 LE
    ];
    assert_eq!(&out[..n], &expect[..]);
}

#[test]
fn header_only_datagram_layout() {
    let mut out = [0xFFu8; 16];
    let n = encode_datagram(u64::MAX - 1, 0, &[], &mut out).expect("encode empty");
    assert_eq!(n, HEADER_LEN);
    assert_eq!(&out[0..4], b"PNTL");
    assert_eq!(out[4], 1);
    assert_eq!(out[5], 0);
    assert_eq!(&out[6..8], &[0, 0]);
    assert_eq!(&out[8..16], &(u64::MAX - 1).to_le_bytes());

    let (header, entries) = decode_datagram(&out).expect("decode empty");
    assert_eq!(header.flags, 0);
    assert_eq!(header.entry_count, 0);
    assert_eq!(header.tick, u64::MAX - 1);
    assert_eq!(entries.len(), 0);
}

#[test]
fn encoded_length_is_header_plus_entries() {
    let mut out = [0u8; BUF];
    for count in [0usize, 1, 2, 17, 179, 180] {
        let raw = sample(count);
        let n = encode_datagram(9, 0, &entries_from(&raw), &mut out).expect("encode");
        assert_eq!(n, HEADER_LEN + count * ENTRY_LEN);
        assert_eq!(&out[..n], &reference_bytes(9, 0, &raw)[..]);
    }
}

#[test]
fn round_trip_every_kind() {
    let raw: Vec<(u8, u16, f32)> = vec![
        (0, 0, 0.0),
        (0, 255, 1.0),
        (1, 31, 1.0),
        (2, 40, 0.75),
        (3, 0, 0.3),
        (4, 1, 0.125),
        (5, u16::MAX, 0.9),
    ];
    let mut out = [0u8; BUF];
    let n = encode_datagram(123_456_789, FLAG_FULL_STATE, &entries_from(&raw), &mut out)
        .expect("encode");
    let (header, entries) = decode_datagram(&out[..n]).expect("decode");
    assert_eq!(header.flags, FLAG_FULL_STATE);
    assert_eq!(header.entry_count as usize, raw.len());
    assert_eq!(header.tick, 123_456_789);
    assert_eq!(entries.len(), raw.len());
    for (got, &(k, id, value)) in entries.iter().zip(&raw) {
        assert_eq!(kind_code(&got.kind), k);
        assert_eq!(got.id, id);
        assert_eq!(got.value.to_bits(), value.to_bits());
    }
}

#[test]
fn decodes_a_hand_built_datagram() {
    let raw = [(4u8, 0x0102u16, 0.25f32), (0, 7, 1.0)];
    let bytes = reference_bytes(0xDEAD_BEEF_0000_0001, 0, &raw);
    let (header, entries) = decode_datagram(&bytes).expect("decode");
    assert_eq!(header.flags, 0);
    assert_eq!(header.entry_count, 2);
    assert_eq!(header.tick, 0xDEAD_BEEF_0000_0001);
    assert_eq!(entries.len(), 2);
    assert_eq!(kind_code(&entries[0].kind), 4);
    assert_eq!(entries[0].id, 0x0102);
    assert_eq!(entries[0].value, 0.25);
    assert_eq!(kind_code(&entries[1].kind), 0);
    assert_eq!(entries[1].id, 7);
    assert_eq!(entries[1].value, 1.0);
}

#[test]
fn max_entries_ok_one_more_is_an_error() {
    let mut out = [0u8; BUF];

    let raw = sample(180);
    let n = encode_datagram(1, 0, &entries_from(&raw), &mut out).expect("180 entries fit");
    assert_eq!(n, 16 + 180 * 8);
    let (header, entries) = decode_datagram(&out[..n]).expect("decode 180");
    assert_eq!(header.entry_count, 180);
    assert_eq!(entries.len(), 180);
    assert_eq!(entries[179].id, raw[179].1);

    let raw = sample(181);
    assert!(encode_datagram(1, 0, &entries_from(&raw), &mut out).is_err());
    let raw = sample(1000);
    assert!(encode_datagram(1, 0, &entries_from(&raw), &mut out).is_err());
}

#[test]
fn encode_rejects_short_output_buffers() {
    let raw = sample(3);
    let entries = entries_from(&raw);
    let need = 16 + 3 * 8;
    for len in [0usize, 1, 15, 16, 17, need - 8, need - 1] {
        let mut out = vec![0u8; len];
        assert!(
            encode_datagram(5, 0, &entries, &mut out).is_err(),
            "buffer of {len} bytes is too small for {need}"
        );
    }
    let mut out = vec![0u8; need];
    assert_eq!(
        encode_datagram(5, 0, &entries, &mut out).expect("exact fit"),
        need
    );

    let mut out = [0u8; 15];
    assert!(encode_datagram(5, 0, &[], &mut out).is_err());
}

#[test]
fn decode_rejects_bad_magic() {
    let good = reference_bytes(1, 0, &sample(2));
    assert!(decode_datagram(&good).is_ok());
    for i in 0..4 {
        let mut bad = good.clone();
        bad[i] ^= 0x20;
        assert!(decode_datagram(&bad).is_err(), "magic byte {i} corrupted");
    }
    let mut bad = good.clone();
    bad[0..4].copy_from_slice(b"LTNP");
    assert!(decode_datagram(&bad).is_err());
}

#[test]
fn decode_rejects_bad_version() {
    let good = reference_bytes(1, 0, &sample(2));
    for v in [0u8, 2, 3, 0x80, 0xFF] {
        let mut bad = good.clone();
        bad[4] = v;
        assert!(decode_datagram(&bad).is_err(), "version {v}");
    }
}

#[test]
fn decode_rejects_truncated_datagrams() {
    let good = reference_bytes(77, FLAG_FULL_STATE, &sample(4));
    assert!(decode_datagram(&good).is_ok());
    for len in 0..good.len() {
        assert!(
            decode_datagram(&good[..len]).is_err(),
            "prefix of {len}/{} bytes must be rejected",
            good.len()
        );
    }
}

#[test]
fn decode_rejects_unknown_kind() {
    let good = reference_bytes(1, 0, &sample(3));
    for kind in [6u8, 7, 100, 255] {
        for entry in 0..3 {
            let mut bad = good.clone();
            bad[16 + entry * 8] = kind;
            assert!(
                decode_datagram(&bad).is_err(),
                "kind {kind} in entry {entry}"
            );
        }
    }
}

#[test]
fn decode_rejects_entry_count_above_maximum() {
    // A well-formed-looking datagram that claims 181 entries and carries them.
    let mut bytes = reference_bytes(1, 0, &sample(181));
    assert_eq!(bytes.len(), 16 + 181 * 8);
    assert!(decode_datagram(&bytes).is_err());
    // Claims u16::MAX entries.
    bytes[6] = 0xFF;
    bytes[7] = 0xFF;
    assert!(decode_datagram(&bytes).is_err());
}

#[test]
fn decode_rejects_count_larger_than_payload() {
    let mut bytes = reference_bytes(1, 0, &sample(2));
    bytes[6] = 3; // claims three entries, carries two
    assert!(decode_datagram(&bytes).is_err());
    assert!(decode_datagram(&[]).is_err());
    assert!(decode_datagram(b"PNTL").is_err());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn decode_never_panics_on_arbitrary_bytes(
        bytes in proptest::collection::vec(any::<u8>(), 0..1600)
    ) {
        let _ = decode_datagram(&bytes);
    }

    /// Random bodies behind a valid magic + version reach the deeper code paths.
    #[test]
    fn decode_never_panics_behind_a_valid_prefix(
        flags in any::<u8>(),
        count in any::<u16>(),
        tick in any::<u64>(),
        body in proptest::collection::vec(any::<u8>(), 0..1600),
    ) {
        let mut bytes = Vec::with_capacity(16 + body.len());
        bytes.extend_from_slice(b"PNTL");
        bytes.push(1);
        bytes.push(flags);
        bytes.extend_from_slice(&count.to_le_bytes());
        bytes.extend_from_slice(&tick.to_le_bytes());
        bytes.extend_from_slice(&body);
        if let Ok((header, entries)) = decode_datagram(&bytes) {
            prop_assert_eq!(header.entry_count as usize, entries.len());
            prop_assert_eq!(header.entry_count, count);
            prop_assert_eq!(header.tick, tick);
            prop_assert!(entries.len() <= 180);
            prop_assert!(bytes.len() >= 16 + entries.len() * 8);
        }
    }

    /// Entry counts consistent with the body length, kinds mostly valid.
    #[test]
    fn decode_never_panics_on_nearly_valid_datagrams(
        tick in any::<u64>(),
        raw in proptest::collection::vec((0u8..8, any::<u16>(), any::<u32>()), 0..200),
        cut in any::<usize>(),
    ) {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"PNTL");
        bytes.push(1);
        bytes.push(0);
        bytes.extend_from_slice(&(raw.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&tick.to_le_bytes());
        for &(k, id, bits) in &raw {
            bytes.push(k);
            bytes.push(0);
            bytes.extend_from_slice(&id.to_le_bytes());
            bytes.extend_from_slice(&bits.to_le_bytes());
        }
        let valid = raw.len() <= 180 && raw.iter().all(|&(k, _, _)| k < 6);
        let result = decode_datagram(&bytes);
        if !valid {
            prop_assert!(result.is_err());
        }
        let cut = cut % (bytes.len() + 1);
        let _ = decode_datagram(&bytes[..cut]);
    }

    #[test]
    fn arbitrary_valid_entries_round_trip(
        tick in any::<u64>(),
        full in any::<bool>(),
        raw in proptest::collection::vec((0u8..6, any::<u16>(), -1.0e6f32..1.0e6f32), 0..=180),
    ) {
        let flags = if full { FLAG_FULL_STATE } else { 0 };
        let mut out = [0u8; BUF];
        let n = encode_datagram(tick, flags, &entries_from(&raw), &mut out).expect("encode");
        prop_assert_eq!(n, 16 + raw.len() * 8);
        prop_assert_eq!(&out[..n], &reference_bytes(tick, flags, &raw)[..]);

        let (header, entries) = decode_datagram(&out[..n]).expect("decode");
        prop_assert_eq!(header.flags, flags);
        prop_assert_eq!(header.entry_count as usize, raw.len());
        prop_assert_eq!(header.tick, tick);
        prop_assert_eq!(entries.len(), raw.len());
        for (got, &(k, id, value)) in entries.iter().zip(&raw) {
            prop_assert_eq!(kind_code(&got.kind), k);
            prop_assert_eq!(got.id, id);
            prop_assert_eq!(got.value.to_bits(), value.to_bits());
        }
    }

    #[test]
    fn encode_never_panics_on_any_sizes(
        count in 0usize..400,
        out_len in 0usize..3400,
        tick in any::<u64>(),
        flags in any::<u8>(),
    ) {
        let raw = sample(count);
        let mut out = vec![0u8; out_len];
        let need = 16 + count * 8;
        match encode_datagram(tick, flags, &entries_from(&raw), &mut out) {
            Ok(n) => {
                prop_assert!(count <= 180);
                prop_assert!(out_len >= need);
                prop_assert_eq!(n, need);
            }
            Err(_) => prop_assert!(count > 180 || out_len < need),
        }
    }
}
