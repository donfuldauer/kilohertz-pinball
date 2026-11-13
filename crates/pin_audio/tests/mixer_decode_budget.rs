//! Implementer regression tests for decode-memory hardening (fix task 3.3-H):
//! * N10: the output reservation never trusts a header or granule claim
//!   (largest single allocation request measured with a counting allocator);
//! * N3: the per-bundle [`DecodeBudget`] is charged by produced frames,
//!   exhausts with `DecodeError::BudgetExhausted` across many files and
//!   stays exactly consistent.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::io::Cursor;
use std::sync::Arc;

use pin_audio::mixer::{
    BYTES_PER_FRAME, DecodeBudget, DecodeError, INITIAL_RESERVE_FRAMES, MAX_SECONDS, Sample,
    decode, decode_with_budget,
};

// ------------------------------------------------------------ counting allocator

struct Peak;

thread_local! {
    /// Largest single allocation request (alloc or realloc target) seen.
    static PEAK: Cell<usize> = const { Cell::new(0) };
}

fn note(size: usize) {
    let _ = PEAK.try_with(|p| p.set(p.get().max(size)));
}

unsafe impl GlobalAlloc for Peak {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        note(l.size());
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        note(n);
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static A: Peak = Peak;

/// Runs `f` and returns its result with the largest allocation request made
/// on this thread meanwhile.
fn peak<R>(f: impl FnOnce() -> R) -> (R, usize) {
    PEAK.with(|p| p.set(0));
    let r = f();
    (r, PEAK.with(Cell::get))
}

/// Decoder-internal allocations (hound, lewton packet buffers, setup
/// tables) are small; anything above this is an output reservation.
const SLACK: usize = 256 * 1024;

fn reservation_bound(out_frames: usize) -> usize {
    (INITIAL_RESERVE_FRAMES as usize).max(2 * out_frames) * BYTES_PER_FRAME as usize + SLACK
}

// ------------------------------------------------------------------- fixtures

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

/// `(start, total_len)` of every page.
fn pages(b: &[u8]) -> Vec<(usize, usize)> {
    let mut v = vec![];
    let mut p = 0;
    while p + 27 <= b.len() && &b[p..p + 4] == b"OggS" {
        let n = b[p + 26] as usize;
        let body: usize = b[p + 27..p + 27 + n].iter().map(|&x| x as usize).sum();
        v.push((p, 27 + n + body));
        p += 27 + n + body;
    }
    v
}

/// The fixture with its last page claiming `granule` frames (CRC fixed).
fn ogg_claiming(granule: u64) -> Vec<u8> {
    let mut b = OGG.to_vec();
    let (last, len) = *pages(&b).last().unwrap();
    b[last + 6..last + 14].copy_from_slice(&granule.to_le_bytes());
    b[last + 22..last + 26].fill(0);
    let c = crc(&b[last..last + len]);
    b[last + 22..last + 26].copy_from_slice(&c.to_le_bytes());
    b
}

fn wav_u8(rate: u32, n: usize) -> Vec<u8> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 8,
        sample_format: hound::SampleFormat::Int,
    };
    let mut buf = Vec::new();
    {
        let mut w = hound::WavWriter::new(Cursor::new(&mut buf), spec).unwrap();
        for k in 0..n {
            w.write_sample(((k % 200) as i32 - 100) as i8).unwrap();
        }
        w.finalize().unwrap();
    }
    buf
}

fn bytes_of(s: &Sample) -> u64 {
    s.frames() as u64 * BYTES_PER_FRAME
}

// ------------------------------------------------------------------------ N10

#[test]
fn granule_claim_does_not_size_the_reservation() {
    // A 3.8 KB stream whose last page claims the full 10 minutes: before the
    // fix, 2 × the resampled claim was reserved (230 MB at 48 kHz, 1.8 GB at
    // 384 kHz).
    let claim = MAX_SECONDS * 22_050;
    let hostile = ogg_claiming(claim);
    for rate in [22_050u32, 48_000, 384_000] {
        let (r, p) = peak(|| decode(&hostile, rate));
        let s = r.unwrap();
        assert!(s.frames() < 20_000 * (rate as usize / 22_050 + 1));
        assert!(
            p <= reservation_bound(s.frames()),
            "rate {rate}: peak request {p} for {} frames",
            s.frames()
        );
        assert!(p < 4 << 20, "rate {rate}: peak request {p}");
    }
}

#[test]
fn reservation_tracks_actual_output_for_large_amplified_wav() {
    // 8 kHz → 384 kHz is a 48× amplification of honest bytes: the buffer
    // grows with the output and stays within 2× of it.
    let w = wav_u8(8_000, 40_000);
    let (r, p) = peak(|| decode(&w, 384_000));
    let s = r.unwrap();
    assert_eq!(s.frames(), 40_000 * 48);
    assert!(p <= reservation_bound(s.frames()), "peak {p}");
    // A tiny honest file reserves only what it needs.
    let w = wav_u8(48_000, 10);
    let (r, p) = peak(|| decode(&w, 48_000));
    assert_eq!(r.unwrap().frames(), 10);
    assert!(p < SLACK, "peak {p}");
}

#[test]
fn budget_bounds_the_reservation_of_a_failing_decode() {
    let w = wav_u8(48_000, 200_000);
    let budget = DecodeBudget::new(50_000 * BYTES_PER_FRAME);
    let (r, p) = peak(|| decode_with_budget(&w, 48_000, &budget));
    assert_eq!(r, Err(DecodeError::BudgetExhausted));
    assert_eq!(budget.used_bytes(), 0);
    assert!(p <= 50_000 * BYTES_PER_FRAME as usize + SLACK, "peak {p}");
}

// ------------------------------------------------------------------------- N3

#[test]
fn budget_exhausts_across_many_files_with_exact_accounting() {
    let w = wav_u8(22_050, 1_000);
    let one = decode(&w, 48_000).unwrap();
    let per = bytes_of(&one);
    assert_eq!(one.frames(), 2_177); // ceil(1000 · 48000 / 22050)
    // Room for 7 files and about half of another (whole frames).
    let half = (one.frames() / 2) as u64 * BYTES_PER_FRAME;
    let budget = DecodeBudget::new(7 * per + half);
    for n in 1..=7u64 {
        let s = decode_with_budget(&w, 48_000, &budget).unwrap();
        assert_eq!(s, one, "bit-identical to decode");
        assert_eq!(budget.used_bytes(), n * per);
        assert_eq!(budget.remaining_bytes(), (7 - n) * per + half);
    }
    for _ in 0..5 {
        assert_eq!(
            decode_with_budget(&w, 48_000, &budget),
            Err(DecodeError::BudgetExhausted)
        );
        assert_eq!(
            budget.used_bytes(),
            7 * per,
            "failed decode leaves no charge"
        );
    }
    // A smaller file still fits in what is left.
    let small = wav_u8(48_000, one.frames() / 2);
    let s = decode_with_budget(&small, 48_000, &budget).unwrap();
    assert_eq!(budget.used_bytes(), 7 * per + bytes_of(&s));
    assert_eq!(budget.remaining_bytes(), 0);
    // Releasing a sample makes room again.
    budget.release(&one);
    assert_eq!(budget.used_bytes(), 6 * per + bytes_of(&s));
    decode_with_budget(&w, 48_000, &budget).unwrap();
    assert_eq!(budget.used_bytes(), 7 * per + bytes_of(&s));
}

#[test]
fn budget_boundary_is_exact() {
    for bytes in [wav_u8(44_100, 3_333), OGG.to_vec()] {
        let full = decode(&bytes, 48_000).unwrap();
        let need = bytes_of(&full);
        let exact = DecodeBudget::new(need);
        assert_eq!(decode_with_budget(&bytes, 48_000, &exact).unwrap(), full);
        assert_eq!(exact.used_bytes(), need);
        assert_eq!(exact.remaining_bytes(), 0);
        let short = DecodeBudget::new(need - 1);
        assert_eq!(
            decode_with_budget(&bytes, 48_000, &short),
            Err(DecodeError::BudgetExhausted)
        );
        assert_eq!(short.used_bytes(), 0);
        let none = DecodeBudget::new(0);
        assert_eq!(
            decode_with_budget(&bytes, 48_000, &none),
            Err(DecodeError::BudgetExhausted)
        );
    }
}

#[test]
fn hostile_claims_charge_only_produced_frames() {
    // The 10-minute granule claim is not charged: only the real output is.
    let hostile = ogg_claiming(MAX_SECONDS * 22_050);
    let budget = DecodeBudget::default();
    assert_eq!(budget.limit_bytes(), DecodeBudget::DEFAULT_BYTES);
    assert_eq!(DecodeBudget::DEFAULT_BYTES, 512 << 20);
    let mut total = 0;
    for _ in 0..50 {
        let s = decode_with_budget(&hostile, 48_000, &budget).unwrap();
        total += bytes_of(&s);
    }
    assert_eq!(budget.used_bytes(), total);
    assert!(total < 50 * 20_000 * BYTES_PER_FRAME);
}

#[test]
fn errors_other_than_budget_leave_no_charge() {
    let budget = DecodeBudget::new(1 << 30);
    let w = wav_u8(48_000, 5_000);
    // Truncated mid-data: Malformed after frames were produced.
    let cut = &w[..w.len() - 1_000];
    assert!(matches!(
        decode_with_budget(cut, 48_000, &budget),
        Err(DecodeError::Malformed(_))
    ));
    assert_eq!(budget.used_bytes(), 0);
    for k in (0..OGG.len()).step_by(97) {
        let _ = decode_with_budget(&OGG[..k], 48_000, &budget);
        assert_eq!(budget.used_bytes(), 0, "cut at {k}");
    }
    assert_eq!(
        decode_with_budget(b"nope", 48_000, &budget),
        Err(DecodeError::Unsupported)
    );
    assert_eq!(budget.used_bytes(), 0);
    assert!(DecodeError::BudgetExhausted.to_string().contains("budget"));
}

#[test]
fn shared_budget_across_threads_never_overcommits() {
    let w = Arc::new(wav_u8(48_000, 2_000));
    let per = 2_000 * BYTES_PER_FRAME;
    let budget = Arc::new(DecodeBudget::new(25 * per + 7));
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let (w, budget) = (Arc::clone(&w), Arc::clone(&budget));
            std::thread::spawn(move || {
                (0..10)
                    .filter(|_| decode_with_budget(&w, 48_000, &budget).is_ok())
                    .count() as u64
            })
        })
        .collect();
    let ok: u64 = handles.into_iter().map(|h| h.join().unwrap()).sum();
    assert!(ok <= 25 && ok > 0, "ok {ok}");
    assert_eq!(budget.used_bytes(), ok * per);
}
