//! Point lights and the per-frame light list.
//!
//! Light units: a [`PointLight`] of `intensity` I and colour C produces at
//! distance d (inside its range) the irradiance `C · I / d²` on a surface
//! facing it, smoothly windowed to zero at `range` by
//! `(1 − (d / range)⁴)²` (clamped to `0..=1`). A white Lambert surface of
//! albedo 1 therefore reflects at most `I / (π d²)` radiance.

use super::MAX_LIGHTS;
use super::warmup::{LampBank, LampKind};
use arrayvec::ArrayVec;

/// One dynamic point light (world space). The layout is the GPU layout
/// (two `vec4<f32>`).
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PointLight {
    /// Position (world, metres).
    pub position: [f32; 3],
    /// Distance at which the light's contribution reaches zero (metres).
    pub range: f32,
    /// Linear RGB colour.
    pub color: [f32; 3],
    /// Intensity (irradiance at 1 m, see the module docs).
    pub intensity: f32,
}

impl PointLight {
    /// A light at `position` of `color` and `intensity`, reaching `range`.
    pub const fn new(position: [f32; 3], color: [f32; 3], intensity: f32, range: f32) -> Self {
        Self {
            position,
            range,
            color,
            intensity,
        }
    }

    /// `true` when the light can contribute (finite position, positive
    /// finite range and intensity, colour not all zero).
    pub fn is_active(&self) -> bool {
        self.position.iter().all(|p| p.is_finite())
            && self.range.is_finite()
            && self.range > 0.0
            && self.intensity.is_finite()
            && self.intensity > 0.0
            && self.color.iter().all(|c| c.is_finite() && *c >= 0.0)
            && self.color.iter().any(|c| *c > 0.0)
    }
}

/// At most [`MAX_LIGHTS`] lights for one frame. Lights pushed beyond that
/// are dropped and counted ([`rejected`](Self::rejected)); nothing panics.
#[derive(Clone, Debug, Default)]
pub struct LightList {
    lights: ArrayVec<PointLight, MAX_LIGHTS>,
    rejected: u64,
}

impl LightList {
    /// An empty list.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends `light`; `false` (and one more [`rejected`](Self::rejected))
    /// when the list already holds [`MAX_LIGHTS`] lights.
    pub fn push(&mut self, light: PointLight) -> bool {
        if self.lights.try_push(light).is_ok() {
            true
        } else {
            self.rejected = self.rejected.saturating_add(1);
            false
        }
    }

    /// Lights rejected because the list was full, since construction or the
    /// last [`clear`](Self::clear).
    pub fn rejected(&self) -> u64 {
        self.rejected
    }

    /// Empties the list and resets the rejection counter.
    pub fn clear(&mut self) {
        self.lights.clear();
        self.rejected = 0;
    }

    /// The lights.
    pub fn as_slice(&self) -> &[PointLight] {
        &self.lights
    }

    /// Number of lights.
    pub fn len(&self) -> usize {
        self.lights.len()
    }

    /// `true` without lights.
    pub fn is_empty(&self) -> bool {
        self.lights.is_empty()
    }

    /// `true` at [`MAX_LIGHTS`] lights.
    pub fn is_full(&self) -> bool {
        self.lights.is_full()
    }
}

/// A table lamp that casts light: where it is and what it looks like at
/// full level. Positions come from the table (the snapshot carries only
/// levels, indexed by `LampId`).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct LampLight {
    /// Index into `FrameStateSnapshot::lamps` (`LampId`).
    pub lamp: usize,
    /// Position (world, metres).
    pub position: [f32; 3],
    /// Linear RGB colour.
    pub color: [f32; 3],
    /// Intensity at full level.
    pub intensity: f32,
    /// Range (metres).
    pub range: f32,
    /// Bulb type (sets its warm-up; apply with [`LampBank::set_kind`]).
    pub kind: LampKind,
    /// A GI string: the colour is also multiplied by the snapshot's GI tint
    /// (`gi.rgb · gi.a`).
    pub gi: bool,
}

/// Emitted level below which a lamp casts no light (it keeps no slot).
pub const LIGHT_OFF_LEVEL: f32 = 1e-3;

/// Rebuilds `out` (cleared first) from the lamp table: each lamp at its
/// warmed-up level from `bank`, GI strings tinted by `gi`
/// (`[r, g, b, intensity]`). Lamps below [`LIGHT_OFF_LEVEL`] are skipped;
/// lamps past [`MAX_LIGHTS`] are counted in `out.rejected()`, in table order.
pub fn build_light_list(lamps: &[LampLight], bank: &LampBank, gi: [f32; 4], out: &mut LightList) {
    out.clear();
    let g = |x: f32| if x.is_finite() { x.max(0.0) } else { 0.0 };
    let tint = [
        g(gi[0]) * g(gi[3]),
        g(gi[1]) * g(gi[3]),
        g(gi[2]) * g(gi[3]),
    ];
    for l in lamps {
        let level = bank.level(l.lamp);
        if level < LIGHT_OFF_LEVEL {
            continue;
        }
        let mut color = l.color;
        if l.gi {
            for (c, t) in color.iter_mut().zip(tint) {
                *c *= t;
            }
        }
        let light = PointLight::new(l.position, color, l.intensity * level, l.range);
        if light.is_active() {
            out.push(light);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_rejects_past_capacity() {
        let mut l = LightList::new();
        let p = PointLight::new([0.0; 3], [1.0; 3], 1.0, 1.0);
        for _ in 0..MAX_LIGHTS {
            assert!(l.push(p));
        }
        assert!(l.is_full());
        assert!(!l.push(p));
        assert_eq!((l.len(), l.rejected()), (MAX_LIGHTS, 1));
        l.clear();
        assert!(l.is_empty() && l.rejected() == 0 && l.as_slice().is_empty());
    }

    #[test]
    fn lamps_follow_levels_and_gi() {
        let mut bank = LampBank::new();
        let mut levels = [0.0; pin_core::snapshot::MAX_LAMPS];
        levels[3] = 1.0;
        levels[4] = 1.0;
        bank.update_levels(0, &levels);
        bank.update_levels(100, &levels);
        let lamp = |lamp, gi| LampLight {
            lamp,
            position: [0.0; 3],
            color: [1.0, 0.5, 0.25],
            intensity: 2.0,
            range: 0.3,
            kind: LampKind::Incandescent,
            gi,
        };
        let mut out = LightList::new();
        build_light_list(
            &[lamp(3, false), lamp(4, true), lamp(5, false)],
            &bank,
            [0.0, 1.0, 1.0, 0.5],
            &mut out,
        );
        assert_eq!(out.len(), 2, "lamp 5 is off");
        assert_eq!(out.as_slice()[1].color, [0.0, 0.25, 0.125]);
        build_light_list(&[lamp(4, true)], &bank, [1.0, 1.0, 1.0, 0.5], &mut out);
        assert_eq!(out.as_slice()[0].color, [0.5, 0.25, 0.125]);
        assert!((out.as_slice()[0].intensity - 2.0).abs() < 1e-3);
        build_light_list(&[lamp(4, true)], &bank, [f32::NAN; 4], &mut out);
        assert!(out.is_empty());
    }
}
