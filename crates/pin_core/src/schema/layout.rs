//! `layout.toml`, the input to `pin_tablegen` (PLAN.md Appendix A.3). Units mm;
//! 2-D points are playfield-plane `[x, y]`.

use super::{Manifest, manifest_io};
use serde::{Deserialize, Serialize};

/// `layout.toml`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutManifest {
    /// Must be 1.
    pub schema_version: u32,
    /// `[playfield]`.
    pub playfield: Playfield,
    /// `[[walls]]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub walls: Vec<Wall>,
    /// `[[posts]]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub posts: Vec<Post>,
    /// `[[lanes]]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lanes: Vec<Lane>,
    /// `[[ramps]]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ramps: Vec<Ramp>,
    /// `[[decks]]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decks: Vec<Deck>,
    /// `[[inserts]]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inserts: Vec<Insert>,
}

impl Manifest for LayoutManifest {
    fn schema_version(&self) -> u32 {
        self.schema_version
    }
}

manifest_io!(LayoutManifest);

/// `[playfield]`, mm.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Playfield {
    /// Width, mm.
    pub width: f32,
    /// Length, mm.
    pub length: f32,
    /// Outer wall height, mm.
    pub wall_height: f32,
}

/// `[[walls]]`: a polyline.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wall {
    /// Points, mm.
    pub points: Vec<[f32; 2]>,
    /// Closed loop.
    #[serde(default)]
    pub closed: bool,
    /// Surface material name (e.g. `wood`).
    pub material: String,
}

/// `[[posts]]`.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Post {
    /// Centre, mm.
    pub position: [f32; 2],
    /// Radius, mm.
    pub radius: f32,
    /// Has a rubber ring.
    #[serde(default)]
    pub rubber: bool,
}

/// Lane kinds.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaneKind {
    /// Inlane.
    Inlane,
    /// Outlane.
    Outlane,
    /// Shooter lane.
    Shooter,
    /// Orbit.
    Orbit,
}

/// `[[lanes]]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lane {
    /// Kind.
    pub kind: LaneKind,
    /// Centre-line points, mm.
    pub points: Vec<[f32; 2]>,
    /// Width, mm.
    pub width: f32,
}

/// `[[ramps]]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ramp {
    /// `table.toml` `[[splines]]` id.
    pub spline: String,
    /// Entry width, mm.
    pub entry_width: f32,
}

/// `[[decks]]`: a raised playfield level.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Deck {
    /// Collision layer.
    pub layer: u8,
    /// Height above the playfield, mm.
    pub z: f32,
    /// Outline polygon, mm.
    pub outline: Vec<[f32; 2]>,
    /// Surface material name (e.g. `acrylic`).
    pub material: String,
}

/// Insert shapes.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InsertShape {
    /// Circle.
    Circle,
    /// Arrow.
    Arrow,
    /// Triangle.
    Triangle,
    /// Rectangle.
    Rect,
}

/// `[[inserts]]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Insert {
    /// Lamp name (from `table.toml`).
    pub lamp: String,
    /// Centre, mm.
    pub position: [f32; 2],
    /// Shape.
    pub shape: InsertShape,
    /// Size, mm.
    pub size: f32,
}
