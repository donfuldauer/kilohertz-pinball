//! KP-017 Independent Tester, GPU-free tests of the display-texture helpers
//! (REQ-VIEW-002, SPEC §7.1): the mip-count rule and the texture format.

use pin_render::displays::{DISPLAY_TEXTURE_FORMAT, mip_level_count};
use proptest::prelude::*;

/// Reference: 1 + the number of halvings of `n` down to 1.
fn halvings_plus_one(mut n: u32) -> u32 {
    let mut count = 1;
    while n > 1 {
        n /= 2;
        count += 1;
    }
    count
}

#[test]
fn req_view_002_display_texture_format_is_rgba8_unorm() {
    assert_eq!(DISPLAY_TEXTURE_FORMAT, wgpu::TextureFormat::Rgba8Unorm);
}

#[test]
fn req_view_002_mip_level_count_fixed_values() {
    for ((w, h), want) in [
        ((1, 1), 1),
        ((2, 1), 2),
        ((1, 2), 2),
        ((3, 3), 2),
        ((4, 4), 3),
        ((128, 32), 8),
        ((200, 50), 8),
        ((512, 128), 10),
        ((0, 5), 0),
        ((5, 0), 0),
        ((0, 0), 0),
        ((u32::MAX, 1), 32),
    ] {
        assert_eq!(mip_level_count(w, h), want, "REQ-VIEW-002 ({w},{h})");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// REQ-VIEW-002: the count is symmetric in the two sides.
    #[test]
    fn req_view_002_mip_count_symmetric(w in 0u32..=u32::MAX, h in 0u32..=u32::MAX) {
        prop_assert_eq!(mip_level_count(w, h), mip_level_count(h, w));
    }

    /// REQ-VIEW-002: monotonic in the larger side (non-zero sides).
    #[test]
    fn req_view_002_mip_count_monotonic(
        w1 in 1u32..=u32::MAX, h1 in 1u32..=u32::MAX,
        w2 in 1u32..=u32::MAX, h2 in 1u32..=u32::MAX,
    ) {
        let (m1, m2) = (w1.max(h1), w2.max(h2));
        if m1 <= m2 {
            prop_assert!(mip_level_count(w1, h1) <= mip_level_count(w2, h2));
        }
    }

    /// REQ-VIEW-002: equals one plus the halvings of the larger side to 1.
    #[test]
    fn req_view_002_mip_count_is_halvings(w in 1u32..=u32::MAX, h in 1u32..=u32::MAX) {
        prop_assert_eq!(mip_level_count(w, h), halvings_plus_one(w.max(h)));
    }

    /// REQ-VIEW-002: a texture whose larger side is 1 has a single level.
    #[test]
    fn req_view_002_mip_count_one_for_unit_side(other in 1u32..=1) {
        prop_assert_eq!(mip_level_count(1, other), 1);
    }

    /// REQ-VIEW-002: a zero side gives zero levels.
    #[test]
    fn req_view_002_mip_count_zero_side(n in 0u32..=u32::MAX) {
        prop_assert_eq!(mip_level_count(0, n), 0);
        prop_assert_eq!(mip_level_count(n, 0), 0);
    }
}

/// #97 / #163 gate comment 4703749506 (API shape, GPU-free): the score
/// display renderer and the score texture both register LCD images with
/// `set_image(ctx, file, image) -> Result<(), GpuError>`. Behaviour (an
/// LCD draws the image, a DMD refuses with `InvalidArgument`) is covered by
/// the `req_view_002_lcd_*image*` tests in `displays_textures_gpu.rs`.
#[test]
fn req_view_002_score_set_image_api_shape() {
    use pin_render::displays::{RgbaImage, ScoreRenderer, ScoreTexture};
    use pin_render::gpu::{GpuContext, GpuError};
    type SetImage<T> = fn(&mut T, &GpuContext, &str, &RgbaImage) -> Result<(), GpuError>;
    let texture: SetImage<ScoreTexture> = ScoreTexture::set_image;
    let renderer: SetImage<ScoreRenderer> = ScoreRenderer::set_image;
    assert_ne!(texture as usize, 0);
    assert_ne!(renderer as usize, 0);
}
