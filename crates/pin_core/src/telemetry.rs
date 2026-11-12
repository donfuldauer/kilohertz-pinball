//! UDP telemetry protocol v1 (PLAN.md Appendix B, normative).
//!
//! Little-endian. Header (16 bytes): magic `"PNTL"` (4) · version `u8` = 1 ·
//! flags `u8` · entry count `u16` · tick `u64`. Entry (8 bytes): kind `u8` ·
//! reserved `u8` · id `u16` · value `f32`. At most 180 entries per datagram;
//! overflow splits across datagrams with the same tick
//! (see [`for_each_datagram`]).

use arrayvec::ArrayVec;
use core::fmt;

/// Datagram magic.
pub const MAGIC: [u8; 4] = *b"PNTL";
/// Protocol version.
pub const VERSION: u8 = 1;
/// Header length, bytes.
pub const HEADER_LEN: usize = 16;
/// Entry length, bytes.
pub const ENTRY_LEN: usize = 8;
/// Maximum entries in one datagram.
pub const MAX_ENTRIES_PER_DATAGRAM: usize = 180;
/// Flags bit 0: this datagram (with any same-tick continuations) is a full-state resync.
pub const FLAG_FULL_STATE: u8 = 1;
/// Largest possible datagram, bytes (16 + 180 × 8 = 1456).
pub const MAX_DATAGRAM_LEN: usize = HEADER_LEN + MAX_ENTRIES_PER_DATAGRAM * ENTRY_LEN;
/// Default destination.
pub const DEFAULT_DESTINATION: &str = "127.0.0.1:35120";

/// Entry kind.
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum TelemetryKind {
    /// Switch state (0/1).
    Switch = 0,
    /// Coil state (0/1).
    Coil = 1,
    /// Lamp intensity (0..1).
    Lamp = 2,
    /// General illumination (0..1).
    Gi = 3,
    /// Magnet power (0..1).
    Magnet = 4,
    /// Haptic impulse strength (0..1); id = haptic source.
    HapticImpulse = 5,
}

impl TryFrom<u8> for TelemetryKind {
    type Error = TelemetryError;

    fn try_from(v: u8) -> Result<Self, TelemetryError> {
        Ok(match v {
            0 => TelemetryKind::Switch,
            1 => TelemetryKind::Coil,
            2 => TelemetryKind::Lamp,
            3 => TelemetryKind::Gi,
            4 => TelemetryKind::Magnet,
            5 => TelemetryKind::HapticImpulse,
            other => return Err(TelemetryError::BadKind(other)),
        })
    }
}

/// One state change.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct TelemetryEntry {
    /// What changed.
    pub kind: TelemetryKind,
    /// Element number (switch/coil/lamp number, haptic source, ...).
    pub id: u16,
    /// New value.
    pub value: f32,
}

/// Decoded datagram header.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TelemetryHeader {
    /// Flags (bit 0 = full state).
    pub flags: u8,
    /// Number of entries that follow.
    pub entry_count: u16,
    /// Sim tick.
    pub tick: u64,
}

impl TelemetryHeader {
    /// True when [`FLAG_FULL_STATE`] is set.
    pub fn is_full_state(&self) -> bool {
        self.flags & FLAG_FULL_STATE != 0
    }
}

/// Encode/decode failures.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TelemetryError {
    /// More than [`MAX_ENTRIES_PER_DATAGRAM`] entries.
    TooManyEntries(usize),
    /// The output buffer is shorter than the datagram.
    BufferTooSmall {
        /// Bytes needed.
        needed: usize,
        /// Bytes available.
        available: usize,
    },
    /// Shorter than a header.
    Truncated(usize),
    /// Magic is not `"PNTL"`.
    BadMagic,
    /// Unsupported version.
    BadVersion(u8),
    /// Datagram length does not equal `16 + 8 × entry_count`.
    BadLength {
        /// Length implied by the header.
        expected: usize,
        /// Actual length.
        actual: usize,
    },
    /// Unknown entry kind.
    BadKind(u8),
}

impl fmt::Display for TelemetryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TelemetryError::TooManyEntries(n) => {
                write!(
                    f,
                    "{n} entries exceed {MAX_ENTRIES_PER_DATAGRAM} per datagram"
                )
            }
            TelemetryError::BufferTooSmall { needed, available } => {
                write!(f, "buffer too small: need {needed} bytes, have {available}")
            }
            TelemetryError::Truncated(n) => write!(f, "datagram truncated: {n} bytes"),
            TelemetryError::BadMagic => f.write_str("bad magic"),
            TelemetryError::BadVersion(v) => write!(f, "unsupported version {v}"),
            TelemetryError::BadLength { expected, actual } => {
                write!(f, "bad length: expected {expected} bytes, got {actual}")
            }
            TelemetryError::BadKind(k) => write!(f, "unknown entry kind {k}"),
        }
    }
}

impl std::error::Error for TelemetryError {}

/// Datagram length for `entries` entries.
#[inline]
pub const fn datagram_len(entries: usize) -> usize {
    HEADER_LEN + entries * ENTRY_LEN
}

/// Encodes one datagram into `out`, returning the number of bytes written.
pub fn encode_datagram(
    tick: u64,
    flags: u8,
    entries: &[TelemetryEntry],
    out: &mut [u8],
) -> Result<usize, TelemetryError> {
    if entries.len() > MAX_ENTRIES_PER_DATAGRAM {
        return Err(TelemetryError::TooManyEntries(entries.len()));
    }
    let needed = datagram_len(entries.len());
    if out.len() < needed {
        return Err(TelemetryError::BufferTooSmall {
            needed,
            available: out.len(),
        });
    }
    out[0..4].copy_from_slice(&MAGIC);
    out[4] = VERSION;
    out[5] = flags;
    // Lossless: entries.len() <= 180.
    out[6..8].copy_from_slice(&(entries.len() as u16).to_le_bytes());
    out[8..16].copy_from_slice(&tick.to_le_bytes());
    for (e, chunk) in entries
        .iter()
        .zip(out[HEADER_LEN..needed].chunks_exact_mut(ENTRY_LEN))
    {
        chunk[0] = e.kind as u8;
        chunk[1] = 0;
        chunk[2..4].copy_from_slice(&e.id.to_le_bytes());
        chunk[4..8].copy_from_slice(&e.value.to_le_bytes());
    }
    Ok(needed)
}

/// Decodes one datagram. Never panics; rejects bad magic, version, length and
/// kind. The reserved byte is ignored.
pub fn decode_datagram(
    bytes: &[u8],
) -> Result<(TelemetryHeader, ArrayVec<TelemetryEntry, 180>), TelemetryError> {
    if bytes.len() < HEADER_LEN {
        return Err(TelemetryError::Truncated(bytes.len()));
    }
    if bytes[0..4] != MAGIC {
        return Err(TelemetryError::BadMagic);
    }
    if bytes[4] != VERSION {
        return Err(TelemetryError::BadVersion(bytes[4]));
    }
    let flags = bytes[5];
    let entry_count = u16::from_le_bytes([bytes[6], bytes[7]]);
    let mut tick = [0u8; 8];
    tick.copy_from_slice(&bytes[8..16]);
    let tick = u64::from_le_bytes(tick);
    let count = usize::from(entry_count);
    if count > MAX_ENTRIES_PER_DATAGRAM {
        return Err(TelemetryError::TooManyEntries(count));
    }
    let expected = datagram_len(count);
    if bytes.len() != expected {
        return Err(TelemetryError::BadLength {
            expected,
            actual: bytes.len(),
        });
    }
    let mut entries = ArrayVec::new();
    for chunk in bytes[HEADER_LEN..].chunks_exact(ENTRY_LEN) {
        let kind = TelemetryKind::try_from(chunk[0])?;
        let id = u16::from_le_bytes([chunk[2], chunk[3]]);
        let value = f32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
        entries.push(TelemetryEntry { kind, id, value });
    }
    Ok((
        TelemetryHeader {
            flags,
            entry_count,
            tick,
        },
        entries,
    ))
}

/// Encodes `entries` for one tick as as many datagrams as needed (≤ 180 entries
/// each, same tick and flags), calling `send` with each. An empty `entries`
/// produces no datagram unless `flags` has [`FLAG_FULL_STATE`] (a full-state
/// resync is always sent). Uses a stack buffer; never allocates.
pub fn for_each_datagram(
    tick: u64,
    flags: u8,
    entries: &[TelemetryEntry],
    mut send: impl FnMut(&[u8]),
) -> usize {
    let mut buf = [0u8; MAX_DATAGRAM_LEN];
    if entries.is_empty() {
        if flags & FLAG_FULL_STATE == 0 {
            return 0;
        }
        // Cannot fail: zero entries always fit.
        let n = encode_datagram(tick, flags, &[], &mut buf).unwrap_or(HEADER_LEN);
        send(&buf[..n]);
        return 1;
    }
    let mut sent = 0;
    for chunk in entries.chunks(MAX_ENTRIES_PER_DATAGRAM) {
        // Cannot fail: chunk.len() <= 180 and buf holds 180 entries.
        if let Ok(n) = encode_datagram(tick, flags, chunk, &mut buf) {
            send(&buf[..n]);
            sent += 1;
        }
    }
    sent
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn kind_strategy() -> impl Strategy<Value = TelemetryKind> {
        (0u8..6).prop_map(|k| TelemetryKind::try_from(k).unwrap())
    }

    fn entry_strategy() -> impl Strategy<Value = TelemetryEntry> {
        (kind_strategy(), any::<u16>(), -1.0e6f32..1.0e6)
            .prop_map(|(kind, id, value)| TelemetryEntry { kind, id, value })
    }

    #[test]
    fn exact_layout() {
        let entries = [TelemetryEntry {
            kind: TelemetryKind::Lamp,
            id: 0x0102,
            value: 1.0,
        }];
        let mut buf = [0u8; 64];
        let n =
            encode_datagram(0x1122_3344_5566_7788, FLAG_FULL_STATE, &entries, &mut buf).unwrap();
        assert_eq!(n, 24);
        assert_eq!(
            &buf[..24],
            &[
                b'P', b'N', b'T', b'L', 1, 1, 1, 0, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11,
                2, 0, 0x02, 0x01, 0x00, 0x00, 0x80, 0x3f
            ]
        );
        let (h, e) = decode_datagram(&buf[..n]).unwrap();
        assert!(h.is_full_state());
        assert_eq!(h.tick, 0x1122_3344_5566_7788);
        assert_eq!(e.as_slice(), &entries);
    }

    #[test]
    fn encode_errors() {
        let e = TelemetryEntry {
            kind: TelemetryKind::Switch,
            id: 1,
            value: 1.0,
        };
        let many = [e; 181];
        let mut buf = [0u8; 4096];
        assert_eq!(
            encode_datagram(0, 0, &many, &mut buf),
            Err(TelemetryError::TooManyEntries(181))
        );
        assert_eq!(encode_datagram(0, 0, &many[..180], &mut buf), Ok(1456));
        assert_eq!(
            encode_datagram(0, 0, &many[..2], &mut buf[..31]),
            Err(TelemetryError::BufferTooSmall {
                needed: 32,
                available: 31
            })
        );
    }

    #[test]
    fn decode_errors() {
        let mut buf = [0u8; 32];
        let e = TelemetryEntry {
            kind: TelemetryKind::Coil,
            id: 9,
            value: 0.0,
        };
        let n = encode_datagram(5, 0, &[e, e], &mut buf).unwrap();
        assert_eq!(
            decode_datagram(&buf[..15]),
            Err(TelemetryError::Truncated(15))
        );
        assert_eq!(
            decode_datagram(&buf[..n - 1]),
            Err(TelemetryError::BadLength {
                expected: 32,
                actual: 31
            })
        );
        let mut bad = buf;
        bad[0] = b'X';
        assert_eq!(decode_datagram(&bad), Err(TelemetryError::BadMagic));
        let mut bad = buf;
        bad[4] = 2;
        assert_eq!(decode_datagram(&bad), Err(TelemetryError::BadVersion(2)));
        let mut bad = buf;
        bad[16] = 6;
        assert_eq!(decode_datagram(&bad), Err(TelemetryError::BadKind(6)));
        let mut bad = [0u8; 16];
        bad[..4].copy_from_slice(&MAGIC);
        bad[4] = VERSION;
        bad[6..8].copy_from_slice(&181u16.to_le_bytes());
        assert_eq!(
            decode_datagram(&bad),
            Err(TelemetryError::TooManyEntries(181))
        );
        // Reserved byte is ignored.
        let mut ok = buf;
        ok[17] = 0xff;
        assert!(decode_datagram(&ok).is_ok());
        for err in [
            TelemetryError::TooManyEntries(1),
            TelemetryError::BufferTooSmall {
                needed: 1,
                available: 0,
            },
            TelemetryError::Truncated(0),
            TelemetryError::BadMagic,
            TelemetryError::BadVersion(0),
            TelemetryError::BadLength {
                expected: 0,
                actual: 1,
            },
            TelemetryError::BadKind(9),
        ] {
            assert!(!err.to_string().is_empty());
        }
    }

    #[test]
    fn splitting() {
        let e = TelemetryEntry {
            kind: TelemetryKind::Lamp,
            id: 0,
            value: 0.5,
        };
        let entries = [e; 400];
        let mut counts = Vec::new();
        let sent = for_each_datagram(42, 0, &entries, |d| {
            let (h, es) = decode_datagram(d).unwrap();
            assert_eq!(h.tick, 42);
            counts.push(es.len());
        });
        assert_eq!(sent, 3);
        assert_eq!(counts, vec![180, 180, 40]);
        assert_eq!(for_each_datagram(1, 0, &[], |_| panic!("no datagram")), 0);
        let mut full = 0;
        assert_eq!(
            for_each_datagram(1, FLAG_FULL_STATE, &[], |d| {
                full = decode_datagram(d).unwrap().0.flags;
            }),
            1
        );
        assert_eq!(full, FLAG_FULL_STATE);
    }

    proptest! {
        #[test]
        fn round_trip(tick in any::<u64>(), flags in any::<u8>(),
                      entries in proptest::collection::vec(entry_strategy(), 0..=180)) {
            let mut buf = [0u8; MAX_DATAGRAM_LEN];
            let n = encode_datagram(tick, flags, &entries, &mut buf).unwrap();
            prop_assert_eq!(n, datagram_len(entries.len()));
            let (h, e) = decode_datagram(&buf[..n]).unwrap();
            prop_assert_eq!(h, TelemetryHeader { flags, entry_count: entries.len() as u16, tick });
            prop_assert_eq!(e.as_slice(), entries.as_slice());
        }

        #[test]
        fn random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..2000)) {
            let _ = decode_datagram(&bytes);
        }

        #[test]
        fn random_bodies_with_valid_header_never_panic(
            count in 0u16..400, tick in any::<u64>(),
            body in proptest::collection::vec(any::<u8>(), 0..1600)) {
            let mut bytes = Vec::with_capacity(16 + body.len());
            bytes.extend_from_slice(&MAGIC);
            bytes.push(VERSION);
            bytes.push(0);
            bytes.extend_from_slice(&count.to_le_bytes());
            bytes.extend_from_slice(&tick.to_le_bytes());
            bytes.extend_from_slice(&body);
            if let Ok((h, e)) = decode_datagram(&bytes) {
                prop_assert_eq!(usize::from(h.entry_count), e.len());
                prop_assert_eq!(bytes.len(), datagram_len(e.len()));
            }
        }

        #[test]
        fn split_preserves_order(n in 0usize..800) {
            let entries: Vec<_> = (0..n).map(|i| TelemetryEntry { kind: TelemetryKind::Switch, id: i as u16, value: 1.0 }).collect();
            let mut got = Vec::new();
            for_each_datagram(7, 0, &entries, |d| got.extend(decode_datagram(d).unwrap().1));
            prop_assert_eq!(got, entries);
        }
    }
}
