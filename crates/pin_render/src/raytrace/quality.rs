//! The reflection-quality setting and the roughness cutoff.

use core::fmt;
use core::str::FromStr;

/// Which surfaces get ray-traced reflections (D33: [`Off`](Self::Off) by
/// default).
///
/// Text form (settings files, command line). [`FromStr`] is exact-match,
/// like the serde form: no trimming and no case folding, so only the
/// canonical lower-case names below parse; [`Display`](fmt::Display) writes
/// them:
///
/// | variant | text |
/// |---|---|
/// | [`Off`](Self::Off) | `off` |
/// | [`BallsOnly`](Self::BallsOnly) | `balls` |
/// | [`BallsAndClearCoat`](Self::BallsAndClearCoat) | `balls_and_clearcoat` |
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum ReflectionQuality {
    /// No ray tracing: the probe path (5.2) only.
    #[default]
    #[serde(rename = "off")]
    Off,
    /// Ray-traced reflections on ball surfaces only (every texel of a
    /// [`RayQueryConfig::map_resolution`](super::RayQueryConfig::map_resolution)²
    /// map traced).
    #[serde(rename = "balls")]
    BallsOnly,
    /// Ball surfaces plus the clear coat of the playfield (one ray cell per
    /// [`RayQueryConfig::clearcoat_block`](super::RayQueryConfig::clearcoat_block)²
    /// texels).
    #[serde(rename = "balls_and_clearcoat")]
    BallsAndClearCoat,
}

impl ReflectionQuality {
    /// Every setting, cheapest first.
    pub const ALL: [ReflectionQuality; 3] = [
        ReflectionQuality::Off,
        ReflectionQuality::BallsOnly,
        ReflectionQuality::BallsAndClearCoat,
    ];

    /// The canonical text form.
    pub const fn as_str(self) -> &'static str {
        match self {
            ReflectionQuality::Off => "off",
            ReflectionQuality::BallsOnly => "balls",
            ReflectionQuality::BallsAndClearCoat => "balls_and_clearcoat",
        }
    }

    /// `true` when ball surfaces are traced.
    pub const fn traces_balls(self) -> bool {
        !matches!(self, ReflectionQuality::Off)
    }

    /// `true` when the playfield clear coat is traced.
    pub const fn traces_clear_coat(self) -> bool {
        matches!(self, ReflectionQuality::BallsAndClearCoat)
    }
}

impl fmt::Display for ReflectionQuality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A string that names no [`ReflectionQuality`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseReflectionQualityError(pub String);

impl fmt::Display for ParseReflectionQualityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown reflection quality {:?} (expected off, balls or balls_and_clearcoat)",
            self.0
        )
    }
}

impl std::error::Error for ParseReflectionQualityError {}

impl FromStr for ReflectionQuality {
    type Err = ParseReflectionQualityError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ReflectionQuality::ALL
            .into_iter()
            .find(|q| q.as_str() == s)
            .ok_or_else(|| ParseReflectionQualityError(s.to_owned()))
    }
}

/// Default [`RayQueryConfig::ray_roughness_cutoff`](super::RayQueryConfig::ray_roughness_cutoff).
pub const DEFAULT_RAY_ROUGHNESS_CUTOFF: f32 = 0.3;

/// Where a surface's mirror term comes from.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ReflectionChoice {
    /// A traced ray.
    Ray,
    /// The probe / environment result.
    Probe,
}

/// The roughness cutoff: [`Ray`](ReflectionChoice::Ray) at or below
/// `cutoff`, [`Probe`](ReflectionChoice::Probe) above it (rough surfaces
/// blur the mirror term so much that a probe is indistinguishable).
///
/// Monotone in `roughness` (once `Probe`, every larger roughness is
/// `Probe`). A NaN `roughness` or `cutoff` gives `Probe` (never trace on
/// invalid input).
pub fn reflection_choice(roughness: f32, cutoff: f32) -> ReflectionChoice {
    if roughness <= cutoff {
        ReflectionChoice::Ray
    } else {
        ReflectionChoice::Probe
    }
}
