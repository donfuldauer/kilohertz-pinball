//! Implementer regression tests for hostile Ogg Vorbis / WAV input (3.3 fix
//! round 1; tables are untrusted downloads, D25). Each case used to abort the
//! process (allocation failure) or amplify a few KB into hundreds of MB.
//! Files are derived from the committed fixture with CRC-correct page edits,
//! or assembled from packets with a minimal Ogg page writer.

use pin_audio::mixer::{DecodeError, decode};

const OGG: &[u8] = include_bytes!("../src/mixer/fixtures/tone440_mono_22k.ogg");

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

/// `(start, header_len, total_len)` of every page.
fn pages(b: &[u8]) -> Vec<(usize, usize, usize)> {
    let mut v = vec![];
    let mut p = 0;
    while p + 27 <= b.len() && &b[p..p + 4] == b"OggS" {
        let n = b[p + 26] as usize;
        let body: usize = b[p + 27..p + 27 + n].iter().map(|&x| x as usize).sum();
        v.push((p, 27 + n, 27 + n + body));
        p += 27 + n + body;
    }
    v
}

fn recrc(b: &mut [u8]) {
    for (s, _, l) in pages(b) {
        b[s + 22..s + 26].fill(0);
        let c = crc(&b[s..s + l]);
        b[s + 22..s + 26].copy_from_slice(&c.to_le_bytes());
    }
}

/// Reassembles the packets of a single-stream Ogg buffer.
fn packets(b: &[u8]) -> Vec<Vec<u8>> {
    let mut out = vec![];
    let mut cur = vec![];
    for (s, h, _) in pages(b) {
        let n = b[s + 26] as usize;
        let mut at = s + h;
        for &lace in &b[s + 27..s + 27 + n] {
            cur.extend_from_slice(&b[at..at + lace as usize]);
            at += lace as usize;
            if lace < 255 {
                out.push(std::mem::take(&mut cur));
            }
        }
    }
    out
}

/// One packet per page; `flags` per page (0x02 first, 0x04 last).
fn write_ogg(packets: &[(Vec<u8>, u64)], serial: u32) -> Vec<u8> {
    let mut out = vec![];
    for (seq, (data, granule)) in packets.iter().enumerate() {
        let mut lacing = vec![255u8; data.len() / 255];
        lacing.push((data.len() % 255) as u8);
        assert!(lacing.len() <= 255);
        let flags = if seq == 0 {
            0x02
        } else if seq == packets.len() - 1 {
            0x04
        } else {
            0
        };
        let start = out.len();
        out.extend_from_slice(b"OggS\0");
        out.push(flags);
        out.extend_from_slice(&granule.to_le_bytes());
        out.extend_from_slice(&serial.to_le_bytes());
        out.extend_from_slice(&(seq as u32).to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.push(lacing.len() as u8);
        out.extend_from_slice(&lacing);
        out.extend_from_slice(data);
        let c = crc(&out[start..]);
        out[start + 22..start + 26].copy_from_slice(&c.to_le_bytes());
    }
    out
}

/// Vorbis bit packer (LSB first).
#[derive(Default)]
struct Bits {
    bytes: Vec<u8>,
    bit: u32,
}

impl Bits {
    fn put(&mut self, value: u64, n: u32) {
        for k in 0..n {
            if self.bit == 0 {
                self.bytes.push(0);
            }
            let b = ((value >> k) & 1) as u8;
            *self.bytes.last_mut().unwrap() |= b << self.bit;
            self.bit = (self.bit + 1) % 8;
        }
    }
    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.put(u64::from(x), 8);
        }
    }
}

fn ilog(v: u64) -> u32 {
    64 - v.leading_zeros()
}

/// A setup packet whose single codebook is `ordered` with `entries` entries
/// of codeword length `len`, followed by a VQ lookup of `lookup_type` and
/// `dims` dimensions, then padding bits.
fn setup_packet(dims: u16, entries: u32, len: u8, lookup_type: u8) -> Vec<u8> {
    let mut w = Bits::default();
    w.bytes(b"\x05vorbis");
    w.put(0, 8); // one codebook
    w.put(0x564342, 24);
    w.put(u64::from(dims), 16);
    w.put(u64::from(entries), 24);
    w.put(1, 1); // ordered
    w.put(u64::from(len - 1), 5);
    w.put(u64::from(entries), ilog(u64::from(entries)));
    w.put(u64::from(lookup_type), 4);
    if lookup_type != 0 {
        w.put(0, 32); // minimum value
        w.put(0, 32); // delta value
        w.put(7, 4); // value bits − 1
        w.put(0, 1); // sequence_p
    }
    w.bytes(&[0xAA; 64]);
    w.bytes
}

fn with_setup(setup: Vec<u8>) -> Vec<u8> {
    let p = packets(OGG);
    write_ogg(&[(p[0].clone(), 0), (p[1].clone(), 0), (setup, 0)], 0x1234)
}

#[test]
fn page_tools_round_trip_the_fixture() {
    let p = packets(OGG);
    assert!(p.len() > 3);
    assert_eq!(&p[0][..7], b"\x01vorbis");
    assert_eq!(&p[1][..7], b"\x03vorbis");
    assert_eq!(&p[2][..7], b"\x05vorbis");
    let mut copy = OGG.to_vec();
    recrc(&mut copy);
    assert_eq!(copy, OGG);
    // A file rebuilt from the same packets decodes (granules: last = length).
    let last = i64::from_le_bytes(
        OGG[pages(OGG).last().unwrap().0 + 6..][..8]
            .try_into()
            .unwrap(),
    );
    let mut rebuilt: Vec<(Vec<u8>, u64)> = p.iter().map(|d| (d.clone(), 0)).collect();
    rebuilt.last_mut().unwrap().1 = last as u64;
    let s = decode(&write_ogg(&rebuilt, 7), 22_050).unwrap();
    assert_eq!(s.frames() as i64, last);
}

#[test]
fn comment_counts_are_never_trusted() {
    let base = decode(OGG, 48_000).unwrap();
    let (s, h, _) = pages(OGG)[1];
    let p = s + h;
    assert_eq!(&OGG[p..p + 7], b"\x03vorbis");
    let vendor_len = u32::from_le_bytes(OGG[p + 7..p + 11].try_into().unwrap()) as usize;
    let count_at = p + 11 + vendor_len;
    for at in [count_at, p + 7] {
        let mut b = OGG.to_vec();
        b[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        recrc(&mut b);
        // The comment header is skipped, so the file still decodes.
        assert_eq!(decode(&b, 48_000).unwrap(), base);
    }
}

#[test]
fn vendored_comment_parser_checks_lengths() {
    let mut pkt = b"\x03vorbis".to_vec();
    pkt.extend_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
    assert!(lewton::header::read_header_comment(&pkt).is_err());
    let mut pkt = b"\x03vorbis".to_vec();
    pkt.extend_from_slice(&0u32.to_le_bytes());
    pkt.extend_from_slice(&u32::MAX.to_le_bytes()); // comment count
    pkt.extend_from_slice(&u32::MAX.to_le_bytes()); // first comment length
    assert!(lewton::header::read_header_comment(&pkt).is_err());
}

#[test]
fn identification_rate_is_bounded() {
    for (rate, expect_ok) in [
        (u32::MAX, false),
        (384_001, false),
        (100, false),
        (44_100, true),
    ] {
        let mut b = OGG.to_vec();
        b[40..44].copy_from_slice(&rate.to_le_bytes());
        recrc(&mut b);
        let r = decode(&b, 48_000);
        if expect_ok {
            assert!(r.is_ok(), "rate {rate}");
        } else {
            assert_eq!(r, Err(DecodeError::Unsupported), "rate {rate}");
        }
    }
}

#[test]
fn granule_length_is_checked_up_front() {
    let (last, _, _) = *pages(OGG).last().unwrap();
    for (frames, too_long) in [
        (600 * 22_050u64, false),
        (600 * 22_050 + 1, true),
        (25 * 60 * 22_050, true),
    ] {
        let mut b = OGG.to_vec();
        b[last + 6..last + 14].copy_from_slice(&frames.to_le_bytes());
        recrc(&mut b);
        let r = decode(&b, 48_000);
        if too_long {
            assert_eq!(r, Err(DecodeError::TooLong), "{frames}");
        } else {
            // Within the cap: decodes the real (short) audio.
            assert!(r.unwrap().frames() < 20_000);
        }
    }
}

#[test]
fn chained_or_multiplexed_streams_are_unsupported() {
    let mut other = OGG.to_vec();
    for (s, _, _) in pages(&other) {
        other[s + 14] ^= 0x55;
    }
    recrc(&mut other);
    assert!(decode(&other, 22_050).is_ok());
    let mut chained = OGG.to_vec();
    chained.extend_from_slice(&other);
    assert_eq!(decode(&chained, 22_050), Err(DecodeError::Unsupported));
    // Same with the second link's pages placed first.
    let mut swapped = other.clone();
    swapped.extend_from_slice(OGG);
    assert_eq!(decode(&swapped, 22_050), Err(DecodeError::Unsupported));
}

#[test]
fn setup_codebook_bombs_are_rejected_without_aborting() {
    // Sanity: the packer produces a setup lewton parses up to the bombs.
    let small = with_setup(setup_packet(1, 2, 1, 0));
    assert!(matches!(
        decode(&small, 48_000),
        Err(DecodeError::Malformed(_))
    ));
    for (dims, entries, len, lookup) in [
        // 2^24 − 1 ordered entries from 24 bits of input.
        (1u16, (1u32 << 24) - 1, 1u8, 0u8),
        // VQ type 2: entries × dims multiplicands (upstream reserved 68 GB).
        (65_535, 1 << 18, 18, 2),
        // VQ type 1: entries × dims computed lookup vectors.
        (4_096, 1 << 18, 18, 1),
        // Over the cumulative VQ budget with a modest codebook.
        (8, 1 << 18, 18, 2),
    ] {
        let file = with_setup(setup_packet(dims, entries, len, lookup));
        assert!(
            matches!(decode(&file, 48_000), Err(DecodeError::Malformed(_))),
            "dims {dims} entries {entries} lookup {lookup}"
        );
    }
}

#[test]
fn low_rate_wav_amplification_is_refused() {
    // 600 frames at 1 Hz would resample to 28.8 M frames (230 MB) at 48 kHz.
    let n = 600u32;
    let mut w = Vec::new();
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + n).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&1u32.to_le_bytes()); // 1 Hz
    w.extend_from_slice(&1u32.to_le_bytes()); // byte rate
    w.extend_from_slice(&1u16.to_le_bytes()); // block align
    w.extend_from_slice(&8u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&n.to_le_bytes());
    w.extend(std::iter::repeat_n(128u8, n as usize));
    assert_eq!(decode(&w, 48_000), Err(DecodeError::Unsupported));
}
