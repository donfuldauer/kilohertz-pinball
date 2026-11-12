//! Layer-transition trigger zones (ramp entrances, VUKs, drop holes).

use glam::Vec3;

/// An axis-aligned zone that moves a ball from one layer to another.
///
/// `from_layer` and `to_layer` are whatever layer identifier the caller keeps
/// per ball (a layer index for [`super::layers::layer_bit`], or a mask bit);
/// they are only compared for equality.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayerTransition {
    /// Layer the ball must be on for the zone to fire.
    pub from_layer: u8,
    /// Layer the ball is on after the zone fired.
    pub to_layer: u8,
    /// Minimum corner (metres).
    pub min: Vec3,
    /// Maximum corner (metres).
    pub max: Vec3,
}

impl LayerTransition {
    /// Whether `p` is inside the zone (bounds inclusive). False for NaN.
    #[must_use]
    pub fn contains(&self, p: Vec3) -> bool {
        p.x >= self.min.x
            && p.x <= self.max.x
            && p.y >= self.min.y
            && p.y <= self.max.y
            && p.z >= self.min.z
            && p.z <= self.max.z
    }
}

/// The table's layer-transition zones.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TransitionSet {
    zones: Vec<LayerTransition>,
}

impl TransitionSet {
    /// Creates the set. Zones are tested in the given order.
    #[must_use]
    pub fn new(zones: Vec<LayerTransition>) -> Self {
        Self { zones }
    }

    /// The zones, in test order.
    #[must_use]
    pub fn zones(&self) -> &[LayerTransition] {
        &self.zones
    }

    /// Returns the ball's layer after it moved from `prev_pos` to `pos`.
    ///
    /// The first zone whose `from_layer` equals `current_layer` and which the
    /// ball centre entered during the move (`prev_pos` outside, `pos` inside)
    /// switches the ball to its `to_layer`; otherwise `current_layer` is
    /// returned. At most one zone fires per call, and a ball that stays inside
    /// a zone does not fire it again.
    ///
    /// Only the end points are tested, so a zone must be thicker than the
    /// distance a ball travels between two calls (call once per CCD sub-step).
    #[must_use]
    pub fn update(&self, current_layer: u8, prev_pos: Vec3, pos: Vec3) -> u8 {
        for zone in &self.zones {
            if zone.from_layer == current_layer && zone.contains(pos) && !zone.contains(prev_pos) {
                return zone.to_layer;
            }
        }
        current_layer
    }
}
