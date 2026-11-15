//! `glass.toml` (PLAN.md Appendix A.3a, CR-007).

use super::{Manifest, SchemaError, manifest_io};
use serde::{Deserialize, Serialize};

/// File name of the optional glass manifest inside a table bundle.
pub const GLASS_FILE: &str = "glass.toml";

/// `glass.toml`: the playfield glass of a table.
///
/// The file is optional in a table bundle. When it is absent, the simulator
/// derives the glass from the table geometry (geometry max z + 50 mm). When
/// present, `height` must be finite and > 0, and the simulator requires it to
/// be at least the geometry's maximum z.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlassManifest {
    /// Must be 1.
    pub schema_version: u32,
    /// Height of the glass underside above the playfield surface, in mm.
    pub height: f32,
}

impl Manifest for GlassManifest {
    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn validate(&self) -> Result<(), SchemaError> {
        if !(self.height.is_finite() && self.height > 0.0) {
            return Err(SchemaError::Invalid(format!(
                "`height` must be finite and > 0, got {}",
                self.height
            )));
        }
        Ok(())
    }
}

manifest_io!(GlassManifest);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_name_and_validate() {
        assert_eq!(GLASS_FILE, "glass.toml");
        let g = GlassManifest {
            schema_version: 1,
            height: 112.0,
        };
        assert!(g.validate().is_ok());
        assert!(GlassManifest { height: 0.0, ..g }.validate().is_err());
    }
}
