//! On-disk TOML schemas (PLAN.md Appendix A, normative; Appendix C).
//!
//! These types mirror the files exactly: lengths in mm, masses in g, angles in
//! degrees, and elements referenced by string name. Runtime SI / interned-ID
//! types (e.g. [`FlipperConfig`](crate::FlipperConfig)) are separate; conversion
//! helpers live next to the on-disk types. Every struct denies unknown keys, and
//! every manifest must carry `schema_version = 1`.
//!
//! Parsing checks syntax, types and the schema version only. Cross-references
//! between names (a flipper's `coil`, a shot's switches, ...) are resolved by
//! `pin_assets` (Subphase 4.1).

pub mod backglass;
pub mod layout;
pub mod physics;
pub mod table;

pub use backglass::BackglassManifest;
pub use layout::LayoutManifest;
pub use physics::{MechDefaults, PhysicsDefaults, PhysicsOverrides, PhysicsParams};
pub use table::MechParams;
pub use table::TableManifest;

use core::fmt;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// The only supported `schema_version`.
pub const SCHEMA_VERSION: u32 = 1;

/// Errors from parsing, serialising or converting a manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchemaError {
    /// TOML syntax error, unknown key, missing key or wrong type.
    Parse(String),
    /// Serialisation failed.
    Serialize(String),
    /// `schema_version` is not [`SCHEMA_VERSION`].
    UnsupportedVersion(u32),
    /// A semantically invalid value.
    Invalid(String),
    /// A name did not resolve to an element.
    UnknownName {
        /// Element kind (`"coil"`, `"switch"`, ...).
        kind: &'static str,
        /// The unresolved name.
        name: String,
    },
}

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SchemaError::Parse(m) => write!(f, "manifest parse error: {m}"),
            SchemaError::Serialize(m) => write!(f, "manifest serialise error: {m}"),
            SchemaError::UnsupportedVersion(v) => {
                write!(
                    f,
                    "unsupported schema_version {v} (expected {SCHEMA_VERSION})"
                )
            }
            SchemaError::Invalid(m) => write!(f, "invalid manifest: {m}"),
            SchemaError::UnknownName { kind, name } => write!(f, "unknown {kind} `{name}`"),
        }
    }
}

impl std::error::Error for SchemaError {}

/// A manifest with a `schema_version` and optional semantic checks.
pub(crate) trait Manifest: Serialize + DeserializeOwned {
    fn schema_version(&self) -> u32;
    fn validate(&self) -> Result<(), SchemaError> {
        Ok(())
    }
}

pub(crate) fn parse<T: Manifest>(s: &str) -> Result<T, SchemaError> {
    let value: T = toml::from_str(s).map_err(|e| SchemaError::Parse(e.to_string()))?;
    if value.schema_version() != SCHEMA_VERSION {
        return Err(SchemaError::UnsupportedVersion(value.schema_version()));
    }
    check_finite(&value)?;
    value.validate()?;
    Ok(value)
}

/// Rejects NaN and ±infinity in every float field of a manifest, naming the
/// offending key path (e.g. `meta.pitch_deg`, `flippers[0].shape.length`).
pub(crate) fn check_finite<T: Serialize>(value: &T) -> Result<(), SchemaError> {
    fn walk(v: &toml::Value, path: &mut String) -> Result<(), SchemaError> {
        match v {
            toml::Value::Float(f) if !f.is_finite() => Err(SchemaError::Invalid(format!(
                "`{path}` must be a finite number, got {f}"
            ))),
            toml::Value::Table(t) => {
                for (k, child) in t {
                    let len = path.len();
                    if !path.is_empty() {
                        path.push('.');
                    }
                    path.push_str(k);
                    walk(child, path)?;
                    path.truncate(len);
                }
                Ok(())
            }
            toml::Value::Array(a) => {
                for (i, child) in a.iter().enumerate() {
                    let len = path.len();
                    path.push_str(&format!("[{i}]"));
                    walk(child, path)?;
                    path.truncate(len);
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
    let v = toml::Value::try_from(value).map_err(|e| SchemaError::Serialize(e.to_string()))?;
    walk(&v, &mut String::new())
}

pub(crate) fn serialize<T: Manifest>(value: &T) -> Result<String, SchemaError> {
    toml::to_string(value).map_err(|e| SchemaError::Serialize(e.to_string()))
}

macro_rules! manifest_io {
    ($t:ty) => {
        impl $t {
            /// Parses the TOML text. Unknown keys, a missing or wrong
            /// `schema_version` and invalid values are errors.
            pub fn from_toml_str(s: &str) -> Result<Self, $crate::schema::SchemaError> {
                $crate::schema::parse(s)
            }

            /// Serialises to TOML text that [`Self::from_toml_str`] parses back
            /// to an equal value.
            pub fn to_toml_string(&self) -> Result<String, $crate::schema::SchemaError> {
                $crate::schema::serialize(self)
            }
        }
    };
}
pub(crate) use manifest_io;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display() {
        for e in [
            SchemaError::Parse("x".into()),
            SchemaError::Serialize("y".into()),
            SchemaError::UnsupportedVersion(2),
            SchemaError::Invalid("z".into()),
            SchemaError::UnknownName {
                kind: "coil",
                name: "n".into(),
            },
        ] {
            assert!(!e.to_string().is_empty());
        }
    }
}
