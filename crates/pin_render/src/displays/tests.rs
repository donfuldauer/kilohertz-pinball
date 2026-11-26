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

    /// REQ-VIEW-002 / R11: any size is either accepted (1..=max per side) or
    /// refused with `InvalidArgument`, never a panic.
    #[test]
    fn req_view_002_texture_size_check(w in any::<u32>(), h in any::<u32>(), max in 1u32..=16384) {
        let ok = w >= 1 && h >= 1 && w <= max && h <= max;
        match check_texture_size(w, h, max) {
            Ok(()) => prop_assert!(ok),
            Err(GpuError::InvalidArgument(_)) => prop_assert!(!ok),
            Err(e) => prop_assert!(false, "unexpected error {e}"),
        }
    }
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
