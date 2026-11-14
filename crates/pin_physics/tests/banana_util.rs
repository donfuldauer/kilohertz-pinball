//! Shared rig of the Subphase 2.6 implementer tests (`banana_*.rs`), on top
//! of the 2.3 rig; included with `#[path]`, and an empty test crate on its
//! own.
#![allow(dead_code)]

#[path = "flipper_util.rs"]
pub mod util;

use glam::Vec3;
use pin_core::ids::FlipperId;
use pin_core::{FlipperConfig, FlipperShape};
use pin_physics::ball::BallHandle;
use pin_physics::flipper::BatShape;
use pin_physics::flipper_banana::{BananaFace, CurvedBanana};
pub use util::*;

/// Chord of every test bat, mm (the 2.3 bat's length).
pub const LENGTH_MM: f32 = 76.0;
/// Tube radius of every test bat, mm.
pub const TUBE_MM: f32 = 8.0;
/// Tube radius, metres.
pub const TUBE: f32 = 0.008;
/// The reference bend: 100 mm arc radius (arc angle 44.7°).
pub const RADIUS_MM: f32 = 100.0;

/// The 2.3 left flipper (rest −30°, sweep +50°, Appendix C defaults) with a
/// banana bat of `arc_radius_mm`, concave face leading.
pub fn banana_config(id: u16, base_mm: [f32; 3], arc_radius_mm: f32) -> FlipperConfig {
    FlipperConfig {
        shape: FlipperShape::CurvedBanana {
            length_mm: LENGTH_MM,
            arc_radius_mm,
            arc_sweep_deg: 30.0,
            tube_radius_mm: TUBE_MM,
        },
        ..left_config(id, base_mm)
    }
}

/// Its mirror image (rest 210°, sweep −50°).
pub fn banana_right_config(id: u16, base_mm: [f32; 3], arc_radius_mm: f32) -> FlipperConfig {
    FlipperConfig {
        shape: banana_config(id, base_mm, arc_radius_mm).shape,
        ..right_config(id, base_mm)
    }
}

/// The straight bat of equal length and thickness: what the banana becomes
/// without its bend.
pub fn straight_config(id: u16, base_mm: [f32; 3]) -> FlipperConfig {
    FlipperConfig {
        shape: FlipperShape::StraightCapsule {
            length_mm: LENGTH_MM,
            base_radius_mm: TUBE_MM,
            tip_radius_mm: TUBE_MM,
        },
        ..left_config(id, base_mm)
    }
}

/// A point of the leading face of the bat of `config` at `fraction` of its
/// length (of the arc for a banana) and the outward normal there, in the
/// bat frame.
pub fn leading_face(config: &FlipperConfig, fraction: f32) -> (Vec3, Vec3) {
    match config.shape {
        FlipperShape::CurvedBanana { arc_sweep_deg, .. } => {
            let bat = CurvedBanana::from_config(config, 0.03).expect("valid banana");
            let face = if arc_sweep_deg >= 0.0 {
                BananaFace::Concave
            } else {
                BananaFace::Convex
            };
            bat.face_point(fraction, face)
        }
        FlipperShape::StraightCapsule {
            length_mm,
            base_radius_mm,
            tip_radius_mm,
        } => {
            // Both radii are equal in these tests: the face is parallel to
            // the axis.
            assert_eq!(base_radius_mm, tip_radius_mm);
            let side = if config.sweep_angle_deg < 0.0 { -1.0 } else { 1.0 };
            let n = v(0.0, side, 0.0);
            (
                v(length_mm * 1.0e-3 * fraction, 0.0, 0.0) + n * (base_radius_mm * 1.0e-3),
                n,
            )
        }
    }
}

/// Bat-frame vector → world for a flipper on a z-normal plane at
/// `angle_deg`.
pub fn bat_to_world(angle_deg: f32, local: Vec3) -> Vec3 {
    let x = bat_dir(angle_deg);
    let y = Vec3::Z.cross(x);
    x * local.x + y * local.y + Vec3::Z * local.z
}

/// World position of a ball centre that touches the leading face of the
/// bat of `config` (pivot at the config's base, z-normal plane) at
/// `fraction` with `gap` to spare, the bat at `angle_deg`; and the face's
/// outward normal in the world.
pub fn ball_on_face(config: &FlipperConfig, angle_deg: f32, fraction: f32, gap: f32) -> (Vec3, Vec3) {
    let (p, n) = leading_face(config, fraction);
    let base = Vec3::from_array(config.base_position_mm) * 1.0e-3;
    let normal = bat_to_world(angle_deg, n);
    (
        base + bat_to_world(angle_deg, p) + normal * (BALL_R + gap) + v(0.0, 0.0, BALL_R),
        normal,
    )
}

/// Bat-frame position of ball `h` relative to flipper `id` (z-normal
/// plane).
pub fn in_bat_frame(rig: &Rig, id: FlipperId, h: BallHandle) -> Vec3 {
    let x = rig.flippers.bat_direction(id).unwrap();
    let y = Vec3::Z.cross(x);
    let d = rig.ball(h).position - rig.flippers.pivot_m(id).unwrap();
    v(d.dot(x), d.dot(y), d.z)
}

/// Kinetic energy (translation and spin) of ball `h`, joules.
pub fn ball_energy(rig: &Rig, h: BallHandle) -> f64 {
    let b = rig.ball(h);
    let p = &profiles()[0];
    let (m, r) = (f64::from(p.mass_kg), f64::from(p.radius_m()));
    0.5 * m * f64::from(b.velocity.length_squared())
        + 0.5 * 0.4 * m * r * r * f64::from(b.angular_velocity.length_squared())
}

/// The reference banana as a shape.
pub fn reference_bat() -> CurvedBanana {
    CurvedBanana::from_config(&banana_config(0, [0.0; 3], RADIUS_MM), 0.03).unwrap()
}

/// Reach of a shape (so the tests need not import the trait).
pub fn reach_of(bat: &CurvedBanana) -> f32 {
    bat.reach()
}
