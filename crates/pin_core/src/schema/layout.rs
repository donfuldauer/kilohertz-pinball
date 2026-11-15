//! `layout.toml`, the input to `pin_tablegen` (PLAN.md Appendix A.3). Units mm;
//! 2-D points are playfield-plane `[x, y]`.

use super::{Manifest, SchemaError, manifest_io};
use crate::materials::Material;
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

    fn validate(&self) -> Result<(), SchemaError> {
        if let Some(g) = self.playfield.glass_height
            && !(g.is_finite() && g > 0.0)
        {
            return Err(SchemaError::Invalid(format!(
                "playfield.glass_height must be finite and > 0 mm, got {g}"
            )));
        }
        Ok(())
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
    /// Height of the playfield glass underside above the playfield surface, mm.
    /// Absent: the simulator derives it from the table geometry. When present it
    /// must be finite and > 0, and the simulator requires it to be at least the
    /// geometry's maximum z.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glass_height: Option<f32>,
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
    /// Surface material (e.g. `wood`).
    pub material: Material,
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
    /// Surface material (e.g. `acrylic`).
    pub material: Material,
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

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str =
        "schema_version = 1\n[playfield]\nwidth = 500.0\nlength = 1000.0\nwall_height = 28.0\n";

    #[test]
    fn absent_is_none_and_not_serialised() {
        let l = LayoutManifest::from_toml_str(BASE).unwrap();
        assert_eq!(l.playfield.glass_height, None);
        assert!(!l.to_toml_string().unwrap().contains("glass_height"));
    }

    #[test]
    fn present_round_trips() {
        let l = LayoutManifest::from_toml_str(&format!("{BASE}glass_height = 120.5\n")).unwrap();
        assert_eq!(l.playfield.glass_height, Some(120.5));
        let back = LayoutManifest::from_toml_str(&l.to_toml_string().unwrap()).unwrap();
        assert_eq!(back, l);
    }

    #[test]
    fn bad_values_rejected() {
        for v in ["0.0", "-5.0", "nan", "inf", "-inf"] {
            let t = format!("{BASE}glass_height = {v}\n");
            assert!(LayoutManifest::from_toml_str(&t).is_err(), "{v}");
        }
        let mut l = LayoutManifest::from_toml_str(BASE).unwrap();
        l.playfield.glass_height = Some(0.0);
        assert!(l.to_toml_string().is_err());
    }
}
