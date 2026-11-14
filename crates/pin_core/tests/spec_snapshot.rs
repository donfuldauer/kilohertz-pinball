//! Black-box spec tests for `pin_core::snapshot` (Subphase 1.1 brief, deliverable 5).

use pin_core::snapshot::{
    FrameStateSnapshot, MAX_BALLS, MAX_DYNAMIC_MESHES, MAX_FLIPPERS, MAX_LAMPS,
};

fn assert_copy_default<T: Copy + Default>() {}
fn assert_send_static<T: Send + 'static>() {}

#[test]
fn capacity_constants() {
    assert_eq!(MAX_BALLS, 16);
    assert_eq!(MAX_FLIPPERS, 16);
    assert_eq!(MAX_DYNAMIC_MESHES, 64);
    assert_eq!(MAX_LAMPS, 256);
}

#[test]
fn snapshot_is_copy_default_and_send() {
    // `Copy` rules out any heap-owning field (Vec, String, Box), so the
    // snapshot is fixed-capacity by construction.
    assert_copy_default::<FrameStateSnapshot>();
    assert_send_static::<FrameStateSnapshot>();
}

#[test]
fn default_snapshot_starts_at_tick_zero() {
    let snap = FrameStateSnapshot::default();
    assert_eq!(snap.tick, 0u64);
}

#[test]
#[allow(clippy::field_reassign_with_default)]
fn tick_is_a_public_u64_and_survives_copies() {
    let mut a = FrameStateSnapshot::default();
    a.tick = u64::MAX;
    let b = a; // Copy
    let c = b;
    a.tick = 5;
    assert_eq!(b.tick, u64::MAX);
    assert_eq!(c.tick, u64::MAX);
    assert_eq!(a.tick, 5);
}

#[test]
fn size_is_fixed_and_bounded() {
    let size = size_of::<FrameStateSnapshot>();
    // Must at least hold the tick plus one float per lamp.
    assert!(
        size >= 8 + 256 * 4,
        "snapshot too small to carry 256 lamps: {size}"
    );
    // And stay small enough to copy by value every millisecond.
    assert!(
        size <= 1 << 20,
        "snapshot is unreasonably large: {size} bytes"
    );
}
