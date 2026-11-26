//! GPU-free unit tests of crate-private display helpers.

use super::quad::{GLYPH_SLOTS, glyph_slot, scissor};
use super::*;
use crate::topology::Rect;

#[test]
fn premultiply_rounds() {
    let img = RgbaImage::from_rgba(2, 1, vec![255, 128, 0, 128, 10, 20, 30, 0]).unwrap();
    assert_eq!(img.premultiplied(), vec![128, 64, 0, 128, 0, 0, 0, 0]);
}

#[test]
fn glyph_slots_cover_ascii_and_fallback() {
    assert_eq!(glyph_slot(' '), 0);
    assert_eq!(glyph_slot('~'), 94);
    assert_eq!(glyph_slot('é'), GLYPH_SLOTS - 1);
    assert_eq!(glyph_slot('\n'), GLYPH_SLOTS - 1);
}

#[test]
fn scissor_clips_to_target() {
    assert_eq!(
        scissor(Rect::new(-10, 5, 30, 10), (16, 12)),
        Some((0, 5, 16, 7))
    );
    assert_eq!(scissor(Rect::new(20, 0, 5, 5), (16, 12)), None);
    assert_eq!(scissor(Rect::new(0, 0, 0, 5), (16, 12)), None);
}

// ---- KP-017 render-to-texture helpers (REQ-VIEW-002) ----

use super::texture::{check_texture_size, mip_shader_source};
use crate::gpu::GpuError;
use proptest::prelude::*;

/// REQ-VIEW-002: full mip chain length, `1 + floor(log2(max side))`.
#[test]
fn req_view_002_mip_level_count_known_sizes() {
    assert_eq!(mip_level_count(1, 1), 1);
    assert_eq!(mip_level_count(2, 1), 2);
    assert_eq!(mip_level_count(1, 2), 2);
    assert_eq!(mip_level_count(3, 3), 2);
    assert_eq!(mip_level_count(4, 4), 3);
    assert_eq!(mip_level_count(128, 32), 8);
    assert_eq!(mip_level_count(192, 64), 8);
    assert_eq!(mip_level_count(255, 1), 8);
    assert_eq!(mip_level_count(256, 1), 9);
    assert_eq!(mip_level_count(1920, 1080), 11);
    assert_eq!(mip_level_count(u32::MAX, 1), 32);
}

/// REQ-VIEW-002: no mip chain for a zero side.
#[test]
fn req_view_002_mip_level_count_zero_side() {
    assert_eq!(mip_level_count(0, 0), 0);
    assert_eq!(mip_level_count(0, 512), 0);
    assert_eq!(mip_level_count(512, 0), 0);
}

/// Halvings (`n → max(1, n / 2)`) until 1, plus one.
fn levels_by_halving(width: u32, height: u32) -> u32 {
    let (mut w, mut h, mut n) = (width, height, 1);
    while w > 1 || h > 1 {
        w = (w / 2).max(1);
        h = (h / 2).max(1);
        n += 1;
    }
    n
}

proptest! {
    /// REQ-VIEW-002: equals the number of halvings to 1×1 plus one.
    #[test]
    fn req_view_002_mip_level_count_is_halvings_plus_one(w in 1u32.., h in 1u32..) {
        prop_assert_eq!(mip_level_count(w, h), levels_by_halving(w, h));
    }

    /// REQ-VIEW-002: symmetric in width and height.
    #[test]
    fn req_view_002_mip_level_count_symmetric(w in any::<u32>(), h in any::<u32>()) {
        prop_assert_eq!(mip_level_count(w, h), mip_level_count(h, w));
    }

    /// REQ-VIEW-002: monotonic in the larger side and a function of it only.
    #[test]
    fn req_view_002_mip_level_count_monotonic(a in 1u32.., b in 1u32.., s in 1u32..) {
        let (lo, hi) = (a.min(b), a.max(b));
        prop_assert!(mip_level_count(lo, 1) <= mip_level_count(hi, 1));
        let m = a.max(b);
        let small = s.min(m);
        prop_assert_eq!(mip_level_count(m, small), mip_level_count(m, 1));
    }

    /// REQ-VIEW-002: 0 whenever a side is 0.
    #[test]
    fn req_view_002_mip_level_count_zero(n in any::<u32>()) {
        prop_assert_eq!(mip_level_count(0, n), 0);
        prop_assert_eq!(mip_level_count(n, 0), 0);
    }

    /// REQ-VIEW-002 / R11 (#163 round 12, finding 24): `max` first, then one
    /// of three size classes, so both branches are exercised on both axes:
    /// both sides in `1..=max` (accepted), exactly one side `0` or `max + 1`
    /// with the other in `1..=max` (refused with `InvalidArgument`), and
    /// arbitrary `u32` pairs (accepted iff both sides are in `1..=max`, never
    /// a panic).
    #[test]
    fn req_view_002_texture_size_check(
        (w, h, max, class) in (1u32..=16384).prop_flat_map(texture_size_case)
    ) {
        let ok = w >= 1 && h >= 1 && w <= max && h <= max;
        match class {
            SizeClass::Valid => prop_assert!(ok, "valid class {w}x{h} max {max}"),
            SizeClass::OneSideBad => prop_assert!(!ok, "bad class {w}x{h} max {max}"),
            SizeClass::Any => {}
        }
        match check_texture_size(w, h, max) {
            Ok(()) => prop_assert!(ok, "accepted {w}x{h} with max {max}"),
            Err(GpuError::InvalidArgument(_)) => {
                prop_assert!(!ok, "refused {w}x{h} with max {max}")
            }
            Err(e) => prop_assert!(false, "unexpected error {e}"),
        }
    }
}

/// Which size class [`texture_size_case`] drew.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum SizeClass {
    /// Both sides in `1..=max`.
    Valid,
    /// Exactly one side `0` or `max + 1`, the other in `1..=max`.
    OneSideBad,
    /// Any `u32` pair.
    Any,
}

/// `(width, height, max, class)` for a given `max` (`1..=16384`, so
/// `max + 1` cannot overflow).
fn texture_size_case(max: u32) -> impl Strategy<Value = (u32, u32, u32, SizeClass)> {
    let bad = prop_oneof![Just(0u32), Just(max + 1)];
    prop_oneof![
        (1..=max, 1..=max).prop_map(move |(w, h)| (w, h, max, SizeClass::Valid)),
        (bad.clone(), 1..=max).prop_map(move |(w, h)| (w, h, max, SizeClass::OneSideBad)),
        (1..=max, bad).prop_map(move |(w, h)| (w, h, max, SizeClass::OneSideBad)),
        (any::<u32>(), any::<u32>()).prop_map(move |(w, h)| (w, h, max, SizeClass::Any)),
    ]
}

/// REQ-VIEW-002 (#163 round 12, finding 24): the size-check strategy really
/// draws every class, and the valid class is accepted (a fixed-seed run of
/// the strategy, not a sample of the property above).
#[test]
fn req_view_002_texture_size_strategy_covers_every_class() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
    let mut runner = TestRunner::new_with_rng(
        Config::default(),
        TestRng::from_seed(RngAlgorithm::ChaCha, &[7; 32]),
    );
    let strategy = (1u32..=16384).prop_flat_map(texture_size_case);
    let (mut valid, mut bad_w, mut bad_h, mut any) = (0, 0, 0, 0);
    for _ in 0..400 {
        let (w, h, max, class) = strategy.new_tree(&mut runner).unwrap().current();
        match class {
            SizeClass::Valid => {
                assert!(check_texture_size(w, h, max).is_ok(), "{w}x{h} max {max}");
                valid += 1;
            }
            SizeClass::OneSideBad => {
                assert!(matches!(
                    check_texture_size(w, h, max),
                    Err(GpuError::InvalidArgument(_))
                ));
                if w == 0 || w == max + 1 {
                    bad_w += 1;
                } else {
                    bad_h += 1;
                }
            }
            SizeClass::Any => any += 1,
        }
    }
    assert!(
        valid > 0 && bad_w > 0 && bad_h > 0 && any > 0,
        "classes drawn: valid {valid}, bad width {bad_w}, bad height {bad_h}, any {any}"
    );
}

/// REQ-VIEW-002: size validation edges.
#[test]
fn req_view_002_texture_size_check_edges() {
    assert!(check_texture_size(1, 1, 8192).is_ok());
    assert!(check_texture_size(8192, 8192, 8192).is_ok());
    for (w, h) in [(0, 1), (1, 0), (0, 0), (8193, 1), (1, 8193)] {
        assert!(matches!(
            check_texture_size(w, h, 8192),
            Err(GpuError::InvalidArgument(_))
        ));
    }
}

/// REQ-VIEW-002: the format stores display-encoded bytes unchanged.
#[test]
fn req_view_002_display_texture_format_is_non_srgb_rgba8() {
    assert_eq!(DISPLAY_TEXTURE_FORMAT, wgpu::TextureFormat::Rgba8Unorm);
    assert!(!DISPLAY_TEXTURE_FORMAT.is_srgb());
}

/// REQ-VIEW-002: the mip generator WGSL validates and translates.
#[test]
fn req_view_002_mip_shader_validates() {
    let module = naga::front::wgsl::parse_str(mip_shader_source()).expect("mip WGSL parses");
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .expect("mip WGSL validates");
}

// ---- #163 gate round 11 (review 6070807835): area-weighted odd mips ----

use super::texture::{mip_axis_weights, mip_reference, mip_split_divide};

/// Runs [`mip_reference`] down to 1 × 1; returns every level with its size.
fn mip_chain(src: Vec<[u8; 4]>, w: u32, h: u32) -> Vec<(Vec<[u8; 4]>, u32, u32)> {
    let mut chain = vec![(src, w, h)];
    loop {
        let (l, lw, lh) = chain.last().expect("level 0");
        if *lw == 1 && *lh == 1 {
            return chain;
        }
        let next = mip_reference(l, *lw, *lh);
        chain.push(next);
    }
}

/// Mean of channel `c` over `texels`.
fn channel_mean(texels: &[[u8; 4]], c: usize) -> f64 {
    texels.iter().map(|t| f64::from(t[c])).sum::<f64>() / texels.len() as f64
}

/// The old 2 × 2 tap average: `(sum + 2) / 4` of texels `2x, 2x + 1` ×
/// `2y, 2y + 1`.
fn four_texel_average(src: &[[u8; 4]], w: u32, x: u32, y: u32, c: usize) -> u8 {
    let at = |x: u32, y: u32| u32::from(src[(y * w + x) as usize][c]);
    let sum =
        at(2 * x, 2 * y) + at(2 * x + 1, 2 * y) + at(2 * x, 2 * y + 1) + at(2 * x + 1, 2 * y + 1);
    ((sum + 2) / 4) as u8
}

/// REQ-VIEW-002 (review 6070807835): a 3 × 1 red `[0, 0, 255]` image has
/// the whole-image average 85 as its 1 × 1 mip (the old taps gave 0).
#[test]
fn req_view_002_mip_reference_3x1_is_whole_average() {
    let src = [[0, 0, 0, 255], [0, 0, 0, 255], [255, 0, 0, 255]];
    let (out, w, h) = mip_reference(&src, 3, 1);
    assert_eq!((w, h), (1, 1));
    assert_eq!(out, vec![[85, 0, 0, 255]]);
}

/// REQ-VIEW-002 (review 6070807835): an edge-only 5 × 3 image (255 only in
/// the last column and the last row) has a non-zero 1 × 1 mip equal to the
/// whole-image average 7 × 255 / 15 = 119.
#[test]
fn req_view_002_mip_reference_edge_only_npot() {
    let src: Vec<[u8; 4]> = (0..15u32)
        .map(|i| {
            let v = if i % 5 == 4 || i / 5 == 2 { 255 } else { 0 };
            [v, v, v, 255]
        })
        .collect();
    let chain = mip_chain(src, 5, 3);
    assert_eq!(chain.len(), 3);
    assert_eq!((chain[1].1, chain[1].2), (2, 1));
    assert_eq!(chain[1].0, vec![[85, 85, 85, 255], [153, 153, 153, 255]]);
    assert_eq!(chain[2].0, vec![[119, 119, 119, 255]]);
}

/// REQ-VIEW-002: sides of 1, 2 and 3 each reduce to one texel covering the
/// whole axis.
#[test]
fn req_view_002_mip_axis_weights_cover_small_sides() {
    assert_eq!(mip_axis_weights(1, 0), [1, 0, 0, 1]);
    assert_eq!(mip_axis_weights(2, 0), [1, 1, 0, 2]);
    assert_eq!(mip_axis_weights(3, 0), [1, 1, 1, 3]);
}

/// REQ-VIEW-002: the 32-bit split division of the shader is exact at the
/// largest odd sides it supports.
#[test]
fn req_view_002_mip_split_divide_extremes() {
    for s in [32_767u32, 16_383, 3] {
        let t = s / 2;
        let w = mip_axis_weights(s, t - 1);
        let rows: Vec<(u32, u32)> = (0..3).map(|j| (w[j], 255 * s)).collect();
        assert_eq!(mip_split_divide(&rows, s, s), Some(255), "side {s}");
        let rows: Vec<(u32, u32)> = (0..3).map(|j| (w[j], 0)).collect();
        assert_eq!(mip_split_divide(&rows, s, s), Some(0), "side {s}");
    }
}

proptest! {
    /// REQ-VIEW-002: per axis the weights sum to the divisor, and every
    /// source texel carries the same total weight over the destination
    /// texels (no texel of an odd side is dropped).
    #[test]
    fn req_view_002_mip_axis_weights_cover_every_texel(s in 1u32..=4097) {
        let t = (s / 2).max(1);
        let mut total = vec![0u64; s as usize];
        let mut den = 0;
        for d in 0..t {
            let w = mip_axis_weights(s, d);
            prop_assert_eq!(w[0] + w[1] + w[2], w[3]);
            den = w[3];
            for i in 0..3 {
                let idx = (2 * d + i) as usize;
                if w[i as usize] > 0 {
                    prop_assert!(idx < s as usize, "weight beyond the side");
                    total[idx] += u64::from(w[i as usize]);
                }
            }
        }
        // Each source texel's share of the axis: total / den == t / s.
        for (i, &v) in total.iter().enumerate() {
            prop_assert_eq!(v * u64::from(s), u64::from(t) * u64::from(den), "texel {}", i);
        }
    }

    /// REQ-VIEW-002: exact 2:1 reductions keep the four-texel average
    /// `(sum + 2) / 4` byte for byte.
    #[test]
    fn req_view_002_mip_reference_exact_2to1_is_four_texel_average(
        hw in 1u32..=8,
        hh in 1u32..=8,
        bytes in proptest::collection::vec(any::<u8>(), 4 * 16 * 16),
    ) {
        let (w, h) = (2 * hw, 2 * hh);
        let src: Vec<[u8; 4]> = bytes
            .chunks_exact(4)
            .take((w * h) as usize)
            .map(|c| [c[0], c[1], c[2], c[3]])
            .collect();
        let (out, w2, h2) = mip_reference(&src, w, h);
        prop_assert_eq!((w2, h2), (hw, hh));
        for y in 0..h2 {
            for x in 0..w2 {
                for (c, &byte) in out[(y * w2 + x) as usize].iter().enumerate() {
                    prop_assert_eq!(byte, four_texel_average(&src, w, x, y, c));
                }
            }
        }
    }

    /// REQ-VIEW-002: for a random image of any size up to 65 × 65, every
    /// step keeps the mean of the level above within the 0.5 of one
    /// round-to-nearest (the weights preserve the mean exactly before
    /// rounding, so no texel is dropped), and the 1 × 1 mip is within
    /// 0.5 per reduction of the whole-image average (the bound chained
    /// rounding allows; exact 2:1 chains such as 16 × 1 already reach 1.44,
    /// so "within 1" holds only up to two reductions).
    #[test]
    fn req_view_002_mip_reference_last_level_is_whole_average(
        w in 1u32..=65,
        h in 1u32..=65,
        bytes in proptest::collection::vec(any::<u8>(), 4 * 65 * 65),
    ) {
        let src: Vec<[u8; 4]> = bytes
            .chunks_exact(4)
            .take((w * h) as usize)
            .map(|c| [c[0], c[1], c[2], c[3]])
            .collect();
        let chain = mip_chain(src.clone(), w, h);
        for k in 1..chain.len() {
            let (above, here) = (&chain[k - 1].0, &chain[k].0);
            let (na, nh) = (above.len() as i64, here.len() as i64);
            for c in 0..4 {
                // |sum_here / nh - sum_above / na| <= 1/2, in integers.
                let sum = |t: &[[u8; 4]]| t.iter().map(|p| i64::from(p[c])).sum::<i64>();
                let gap = (sum(here) * na - sum(above) * nh).abs();
                prop_assert!(
                    2 * gap <= na * nh,
                    "{}x{} level {} ch {}: mean {} vs {} above",
                    w, h, k, c, channel_mean(here, c), channel_mean(above, c)
                );
            }
        }
        let last = chain.last().unwrap();
        prop_assert_eq!((last.1, last.2), (1, 1));
        let bound = 0.5 * (chain.len() - 1) as f64;
        for c in 0..4 {
            let avg = channel_mean(&src, c);
            let got = f64::from(last.0[0][c]);
            prop_assert!((got - avg).abs() <= bound, "{}x{} ch {}: {} vs {}", w, h, c, got, avg);
        }
    }

    /// REQ-VIEW-002: the shader's 32-bit split division equals the exact
    /// 64-bit `(S + D / 2) / D` for odd sides up to 32 767 and any bytes.
    #[test]
    fn req_view_002_mip_split_divide_is_exact(
        sx in 1u32..=32_767,
        sy in 1u32..=32_767,
        fx in 0.0f64..1.0,
        fy in 0.0f64..1.0,
        bytes in proptest::collection::vec(any::<u8>(), 9),
    ) {
        let (tx, ty) = ((sx / 2).max(1), (sy / 2).max(1));
        let (dx, dy) = (((f64::from(tx) * fx) as u32).min(tx - 1), ((f64::from(ty) * fy) as u32).min(ty - 1));
        let (wx, wy) = (mip_axis_weights(sx, dx), mip_axis_weights(sy, dy));
        let mut rows = Vec::new();
        let mut exact = 0u64;
        for j in 0..3 {
            let row: u32 = (0..3).map(|i| wx[i] * u32::from(bytes[3 * j + i])).sum();
            exact += u64::from(wy[j]) * u64::from(row);
            rows.push((wy[j], row));
        }
        let den = u64::from(wx[3]) * u64::from(wy[3]);
        let want = (exact + den / 2) / den;
        prop_assert_eq!(mip_split_divide(&rows, wx[3], wy[3]).map(u64::from), Some(want));
    }
}
