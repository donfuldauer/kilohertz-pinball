//! Curved "Banana" flipper shape.
//!
//! Owner: Subphase 2.6. Stub created by Subphase 1.1; hook point prepared by
//! Subphase 2.3.
//!
//! [`CurvedBanana`] is a bat whose outline is a circular-arc tube: every
//! point of the mounting plane within `tube_radius` of a circular
//! centreline arc, extruded along the rotation axis. It implements
//! [`BatShape`] and is carried by the flipper solver of
//! [`crate::flipper`] exactly like the straight bat:
//! [`FlipperSet::new`](crate::flipper::FlipperSet::new) builds one for a
//! `FlipperShape::CurvedBanana` configuration. Coil, stops, rubber law, the
//! two-body impulse and the continuous collision test are the solver's and
//! are shared with the straight bat; this module only supplies the surface.
//!
//! # Geometry convention
//!
//! In the bat frame (origin at the pivot, `x` along bat angle 0 turned by
//! the bat angle, `z` the rotation axis, `y = z × x`):
//!
//! * The **pivot is the start of the centreline arc** and the arc leaves it
//!   **tangent to +x**, so the flipper's angle is the direction of the bat
//!   at its pivot. From there the arc curls toward its concave side
//!   ([`ConcaveSide`]): the centre of curvature is at `(0, ±arc_radius, 0)`.
//! * `length` is the **chord** from the pivot to the centre of the tip cap.
//!   The centreline subtends the **arc angle** `φ = 2·asin(length / (2·arc_radius))`
//!   at its centre, and the tip-cap centre is at
//!   `length·(cos(φ/2), ±sin(φ/2), 0)`.
//! * Both ends are round caps of `tube_radius`; the top and bottom faces
//!   are flat. The bat reaches `length + tube_radius` from the pivot, like a
//!   [`StraightCapsule`](crate::flipper::StraightCapsule) of that length,
//!   and becomes that capsule (base radius = tip radius = `tube_radius`) as
//!   `arc_radius → ∞`.
//! * From a configuration ([`CurvedBanana::from_shape`]) the concave face
//!   is the leading face of the swing for `arc_sweep_deg ≥ 0` and the
//!   trailing one for `arc_sweep_deg < 0` ([`ConcaveSide::leading`]). Only
//!   the sign of `arc_sweep_deg` is read: chord and radius already fix the
//!   arc.
//!
//! # Collision
//!
//! [`BatShape::closest_point`] is analytic (point to arc in the bat plane,
//! minus the tube radius, combined with the flat faces) and exact outside
//! the bat, which is what the solver's swept test needs: conservative
//! advancement in the bat frame ([`BatShape::cast_sphere`]) steps by
//! gap / (ball speed + pivot speed + `|ω|·reach`) and so cannot step over
//! the concave face, the convex face or either cap at any speed. The
//! surface velocity is the rigid rotation `ω ẑ × r`
//! ([`BatShape::surface_velocity`]). Nothing here allocates.

mod arc;

pub use crate::flipper::BatShape;
pub use arc::{BananaError, BananaFace, ConcaveSide, CurvedBanana};
