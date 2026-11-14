//! `backglass.toml` (PLAN.md Appendix A.2).

use super::{Manifest, manifest_io};
use serde::{Deserialize, Serialize};

/// `backglass.toml`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackglassManifest {
    /// Must be 1.
    pub schema_version: u32,
    /// `[translite]`.
    pub translite: Translite,
    /// `[[lamp_masks]]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lamp_masks: Vec<LampMask>,
    /// `[score_display]`.
    pub score_display: ScoreDisplay,
    /// Optional `[backbox]` (Subphase 5.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backbox: Option<Backbox>,
}

impl Manifest for BackglassManifest {
    fn schema_version(&self) -> u32 {
        self.schema_version
    }
}

manifest_io!(BackglassManifest);

/// `[translite]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Translite {
    /// Image layers, back to front by `depth`.
    pub layers: Vec<TransliteLayer>,
}

/// One translite image layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransliteLayer {
    /// Bundle path.
    pub file: String,
    /// Layer depth (0 = back).
    pub depth: u8,
    /// Lamp mask id that lights this layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lamp_mask: Option<String>,
}

/// `[[lamp_masks]]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LampMask {
    /// Mask id.
    pub id: String,
    /// Lamp name (from `table.toml`).
    pub lamp: String,
    /// Mask image.
    pub file: String,
}

/// Score display mode.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScoreDisplayMode {
    /// Dot-matrix display.
    Dmd,
    /// Full-colour LCD.
    Lcd,
}

/// DMD dot colour.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DmdColor {
    /// Classic orange plasma.
    Orange,
    /// Red.
    Red,
    /// Full colour.
    Rgb,
}

/// DMD resolution and colour.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DmdConfig {
    /// Dots.
    pub width: u32,
    /// Dots.
    pub height: u32,
    /// Dot colour.
    pub color: DmdColor,
}

/// LCD resolution.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LcdConfig {
    /// Pixels.
    pub width: u32,
    /// Pixels.
    pub height: u32,
}

/// `[score_display]`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScoreDisplay {
    /// Active mode.
    pub mode: ScoreDisplayMode,
    /// DMD settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dmd: Option<DmdConfig>,
    /// LCD settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lcd: Option<LcdConfig>,
}

/// Backbox camera, mm and degrees.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackboxCamera {
    /// Eye position, mm.
    pub position: [f32; 3],
    /// Target, mm.
    pub look_at: [f32; 3],
    /// Vertical field of view, degrees.
    pub fov_deg: f32,
}

/// `[backbox]` mini-playfield (Subphase 5.5).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Backbox {
    /// Scene glTF.
    pub scene: String,
    /// Camera.
    pub camera: BackboxCamera,
    /// Collision layer (bit 4 by convention).
    pub collision_layer: u8,
}
