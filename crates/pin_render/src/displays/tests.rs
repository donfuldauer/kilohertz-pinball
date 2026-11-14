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
