//! Curved "Banana" flipper shape.
//!
//! Owner: Subphase 2.6. Stub created by Subphase 1.1; hook point prepared by
//! Subphase 2.3.
//!
//! Subphase 2.6 adds the `CurvedBanana` bat here by implementing
//! [`BatShape`] (closest point, reach, height range; the surface velocity
//! and the swept test have defaults that work for any rigid shape) and
//! adding its variant to the flipper solver's shape selection. Until then
//! [`FlipperSet::new`](crate::flipper::FlipperSet::new) rejects a
//! `FlipperShape::CurvedBanana` configuration with
//! [`FlipperError::ShapeNotSupported`](crate::flipper::FlipperError::ShapeNotSupported).

pub use crate::flipper::BatShape;
