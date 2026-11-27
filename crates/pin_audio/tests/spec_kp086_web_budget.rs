//! Independent Tester, KP-086 (#180): the web decoded-audio budget
//! (D25, D43, REQ-WEB-022 "decode budget 64 MiB").
//!
//! `DecodeBudget::WEB_BYTES` and `DecodeBudget::try_charge_sample`: exact
//! `Sample::bytes()` charges, all or nothing, never over the limit, never
//! overflowing, paired with `release`, and safe from several threads.

use std::sync::Arc;

use pin_audio::mixer::{DecodeBudget, Sample};
use proptest::prelude::*;

/// D25, REQ-WEB-022: the web budget is 64 MiB; native default stays 512 MiB.
#[test]
fn d25_req_web_022_web_bytes_is_64_mib() {
    assert_eq!(DecodeBudget::WEB_BYTES, 64 * 1024 * 1024);
    assert_eq!(DecodeBudget::DEFAULT_BYTES, 512 * 1024 * 1024);
    let b = DecodeBudget::new(DecodeBudget::WEB_BYTES);
    assert_eq!(b.limit_bytes(), 67_108_864);
    assert_eq!(b.used_bytes(), 0);
}

/// D25: `try_charge_sample` charges exactly `Sample::bytes()` (4 per mono
/// frame, 8 per stereo frame).
#[test]
fn d25_try_charge_sample_charges_exact_bytes() {
    let b = DecodeBudget::new(1 << 20);
    let mono = Sample::from_mono_vec(vec![0.25; 1000]);
    let stereo = Sample::constant(700, 0.1, -0.1);
    assert_eq!(mono.bytes(), 4000);
    assert_eq!(stereo.bytes(), 5600);
    assert!(b.try_charge_sample(&mono));
    assert_eq!(b.used_bytes(), 4000);
    assert!(b.try_charge_sample(&stereo));
    assert_eq!(b.used_bytes(), 9600);
    assert_eq!(b.remaining_bytes(), (1 << 20) - 9600);
    let empty = Sample::from_mono_vec(Vec::new());
    assert!(b.try_charge_sample(&empty));
    assert_eq!(b.used_bytes(), 9600);
}

/// D25: all or nothing at the limit; succeeds exactly at the limit.
#[test]
fn d25_try_charge_sample_all_or_nothing_and_exact_limit() {
    let b = DecodeBudget::new(8000);
    let s = Sample::from_mono_vec(vec![0.0; 1500]); // 6000 bytes
    assert!(b.try_charge_sample(&s));
    let over = Sample::from_mono_vec(vec![0.0; 501]); // 2004 > 2000 left
    assert!(!b.try_charge_sample(&over));
    assert_eq!(b.used_bytes(), 6000, "a failed charge changes nothing");
    let exact = Sample::constant(250, 0.0, 0.0); // 2000 bytes
    assert!(b.try_charge_sample(&exact));
    assert_eq!(b.used_bytes(), 8000);
    assert_eq!(b.remaining_bytes(), 0);
    assert!(!b.try_charge_sample(&Sample::from_mono_vec(vec![0.0; 1])));
    assert_eq!(b.used_bytes(), 8000);
    let unl = DecodeBudget::unlimited();
    assert!(unl.try_charge_sample(&s));
    assert_eq!(unl.used_bytes(), 6000);
    let zero = DecodeBudget::new(0);
    assert!(!zero.try_charge_sample(&Sample::from_mono_vec(vec![0.0; 1])));
    assert_eq!(zero.used_bytes(), 0);
}

/// D25: charge then release returns to the previous `used_bytes`; a second
/// release of the same sample reports `OverRelease`.
#[test]
fn d25_try_charge_sample_pairs_with_release() {
    let b = DecodeBudget::new(DecodeBudget::WEB_BYTES);
    let a = Sample::from_mono_vec(vec![0.5; 100]);
    let c = Sample::constant(64, 0.5, 0.5);
    assert!(b.try_charge_sample(&a));
    let before = b.used_bytes();
    assert!(b.try_charge_sample(&c));
    assert_eq!(b.used_bytes(), before + 512);
    assert_eq!(b.release(&c), Ok(()));
    assert_eq!(b.used_bytes(), before);
    assert_eq!(b.release(&a), Ok(()));
    assert_eq!(b.used_bytes(), 0);
    assert!(b.release(&a).is_err(), "second release is OverRelease");
    assert_eq!(b.used_bytes(), 0);
}

/// D25: the budget is `Sync`; concurrent charges never exceed the limit and
/// the used bytes equal the successful charges.
#[test]
fn d25_concurrent_try_charge_sample_never_exceeds_limit() {
    fn assert_sync<T: Sync + Send>() {}
    assert_sync::<DecodeBudget>();
    let limit = 1_000_000u64;
    let b = Arc::new(DecodeBudget::new(limit));
    let handles: Vec<_> = (0..6usize)
        .map(|t| {
            let b = Arc::clone(&b);
            std::thread::spawn(move || {
                let s = Sample::from_mono_vec(vec![0.0; 97 + t * 13]);
                let mut ok = 0u64;
                for _ in 0..2_000 {
                    if b.try_charge_sample(&s) {
                        ok += s.bytes();
                    }
                    assert!(b.used_bytes() <= limit);
                }
                ok
            })
        })
        .collect();
    let charged: u64 = handles.into_iter().map(|h| h.join().unwrap()).sum();
    assert_eq!(b.used_bytes(), charged);
    assert!(charged <= limit);
    // Every thread's sample is at most 680 bytes, so the budget is nearly full.
    assert!(limit - charged < 680);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// D25: over random sample sizes and limits (including limits near
    /// `u64::MAX`), `used_bytes() <= limit_bytes()` always holds, a charge
    /// succeeds exactly when it fits, and no charge panics or overflows.
    #[test]
    fn d25_try_charge_sample_never_exceeds_limit(
        limit in prop_oneof![
            0u64..200_000,
            Just(u64::MAX),
            (u64::MAX - 100_000)..=u64::MAX,
            Just(DecodeBudget::WEB_BYTES),
        ],
        sizes in prop::collection::vec((1usize..=2, 0usize..5_000), 1..40),
    ) {
        let b = DecodeBudget::new(limit);
        // Model: a charge fits when used + bytes <= limit (checked add).
        let mut model = 0u64;
        for (ch, frames) in sizes {
            let s = if ch == 1 {
                Sample::from_mono_vec(vec![0.0; frames])
            } else {
                Sample::constant(frames, 0.0, 0.0)
            };
            let fits = model.checked_add(s.bytes()).is_some_and(|t| t <= limit);
            prop_assert_eq!(b.try_charge_sample(&s), fits);
            if fits {
                model += s.bytes();
            }
            prop_assert_eq!(b.used_bytes(), model);
            prop_assert!(b.used_bytes() <= b.limit_bytes());
            prop_assert_eq!(b.remaining_bytes(), limit - model);
        }
    }
}
