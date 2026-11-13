//! Spec tests, Subphase 4.6 — synthesised sound effects (brief: deliverable 1,
//! FIXED PUBLIC API `sounds::{SFX_IDS, render_sfx}`, verification 2 and 3).

mod spec_4_6_support;

use pin_assetgen::sounds::{SFX_IDS, render_sfx};
use proptest::prelude::*;
use spec_4_6_support::*;

const EXPECTED: [&str; 9] = [
    "sfx_flipper",
    "sfx_bumper",
    "sfx_sling",
    "sfx_drop_target",
    "sfx_spinner",
    "sfx_ball_roll",
    "sfx_knocker",
    "sfx_drain",
    "sfx_launch",
];

#[test]
fn sfx_ids_are_the_fixed_stable_list() {
    assert_eq!(SFX_IDS, &EXPECTED[..]);
}

#[test]
fn every_sfx_renders_non_silent_and_below_clipping_at_48k() {
    for id in EXPECTED {
        let s = render_sfx(id, 48_000).unwrap_or_else(|| panic!("{id}: render_sfx returned None"));
        assert!(!s.is_empty(), "{id}: empty");
        assert!(s.iter().all(|v| v.is_finite()), "{id}: non-finite sample");
        let (r, p) = (rms(&s), peak(&s));
        assert!(r > RMS_FLOOR, "{id}: RMS {r} is not above -40 dBFS");
        assert!(p <= PEAK_CEIL, "{id}: peak {p} is above -0.1 dBFS");
    }
}

#[test]
fn sfx_levels_hold_at_other_sample_rates_and_duration_is_rate_independent() {
    for id in EXPECTED {
        let base = render_sfx(id, 48_000).unwrap();
        let secs = base.len() as f64 / 48_000.0;
        for rate in [44_100u32, 96_000] {
            let s = render_sfx(id, rate).unwrap_or_else(|| panic!("{id}@{rate}: None"));
            assert!(s.iter().all(|v| v.is_finite()), "{id}@{rate}: non-finite");
            let (r, p) = (rms(&s), peak(&s));
            assert!(r > RMS_FLOOR, "{id}@{rate}: RMS {r}");
            assert!(p <= PEAK_CEIL, "{id}@{rate}: peak {p}");
            let d = s.len() as f64 / f64::from(rate);
            assert!(
                (d - secs).abs() <= 0.05 * secs + 0.001,
                "{id}: {secs} s at 48 kHz but {d} s at {rate} Hz"
            );
        }
    }
}

#[test]
fn render_sfx_is_deterministic() {
    for id in EXPECTED {
        let a = render_sfx(id, 48_000).unwrap();
        let b = render_sfx(id, 48_000).unwrap();
        assert_eq!(a.len(), b.len(), "{id}");
        assert!(
            a.iter().zip(&b).all(|(x, y)| x.to_bits() == y.to_bits()),
            "{id}: two renders differ"
        );
    }
}

#[test]
fn sfx_are_pairwise_distinct() {
    let all: Vec<Vec<u32>> = EXPECTED
        .iter()
        .map(|id| {
            render_sfx(id, 48_000)
                .unwrap()
                .iter()
                .map(|v| v.to_bits())
                .collect()
        })
        .collect();
    for i in 0..all.len() {
        for j in i + 1..all.len() {
            assert_ne!(all[i], all[j], "{} == {}", EXPECTED[i], EXPECTED[j]);
        }
    }
}

/// Seamless loop: the jump from the last sample back to the first is no
/// larger than the largest sample-to-sample step inside the loop (so the wrap
/// point is indistinguishable from the interior) and is below 0.1 full scale.
#[test]
fn ball_roll_loop_is_seamless() {
    for rate in [44_100u32, 48_000] {
        let s = render_sfx("sfx_ball_roll", rate).unwrap();
        assert!(s.len() >= rate as usize / 10, "loop shorter than 100 ms");
        let (j, m) = (seam(&s), max_step(&s));
        assert!(
            j <= m + 1e-6,
            "@{rate}: wrap jump {j} exceeds largest interior step {m}"
        );
        assert!(j < 0.1, "@{rate}: wrap jump {j}");
    }
}

#[test]
fn unknown_ids_render_none() {
    for id in [
        "",
        "sfx",
        "sfx_",
        "SFX_FLIPPER",
        "sfx_flipper ",
        "flipper",
        "music",
        "\0",
        "sfx_bumper\n",
    ] {
        assert!(render_sfx(id, 48_000).is_none(), "{id:?} rendered");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn arbitrary_ids_never_panic_and_only_listed_ids_render(id in ".{0,24}") {
        let out = render_sfx(&id, 48_000);
        prop_assert_eq!(out.is_some(), EXPECTED.contains(&id.as_str()));
    }
}
