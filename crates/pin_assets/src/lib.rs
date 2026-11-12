//! `pin_assets`: Bundle VFS, zip hardening, glTF loader and manifest validation.
//!
//! Tables are untrusted downloads (D25): every byte of a bundle is treated as
//! hostile. [`Vfs`] reads a bundle from a folder or a `.pinbundle` / `.zip`
//! archive with the §3.5 hardening, [`load_table`] parses and validates
//! `table.toml` / `backglass.toml` (interning names to `u16` ids and converting
//! to SI), and [`load_gltf`] loads meshes, PBR materials and Blender `extras`
//! through the VFS only.
//!
//! Module directories are pre-stubbed by Subphase 1.1 (PLAN.md §2.1, D3); all
//! four belong to Subphase 4.1.

pub mod gltf;
pub mod validate;
pub mod vfs;
pub mod zip;

use core::fmt;

pub use gltf::{
    GltfLimits, GltfScene, ImageData, MeshData, NodeExtras, PbrMaterial, load_gltf,
    load_gltf_with_limits, validate_gltf_bindings,
    validate_gltf_bindings_in,
};
pub use validate::{
    BackboxSi, DispenserSi, FlipperStageHint, IdKind, LoadedTable, MagnetSi, MechSi, ModeSi,
    ShotSi, SplineSi, SwitchSi, TableIds, TransitionSi, ValidationError, load_table,
};
pub use vfs::Vfs;
pub use zip::ZipLimits;

/// Errors from opening a bundle, reading from it, or loading its contents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetError {
    /// An archive entry or requested path is absolute (`/x`, `C:\x`, `\\server\x`).
    AbsolutePath(String),
    /// An archive entry or requested path has a `..` component or escapes the
    /// bundle root (also through a symlink).
    PathTraversal(String),
    /// An archive entry is a symbolic link.
    Symlink(String),
    /// A single file exceeds [`ZipLimits::max_file`] (`size` in bytes).
    TooLarge {
        /// Entry path.
        path: String,
        /// Uncompressed size, bytes.
        size: u64,
    },
    /// The archive's total uncompressed size (or the archive itself) exceeds
    /// [`ZipLimits::max_total_uncompressed`].
    TotalTooLarge(u64),
    /// An entry's uncompressed / compressed ratio exceeds [`ZipLimits::max_ratio`].
    CompressionRatio {
        /// Entry path.
        path: String,
        /// `uncompressed / compressed`, rounded down.
        ratio: u64,
    },
    /// The archive has more entries than [`ZipLimits::max_entries`].
    TooManyEntries(usize),
    /// Two entries have the same path after normalisation.
    DuplicateEntry(String),
    /// An entry is encrypted.
    Encrypted(String),
    /// An entry name is not valid UTF-8, contains NUL or is empty.
    BadName,
    /// The requested file does not exist.
    NotFound(String),
    /// An I/O error.
    Io(String),
    /// A malformed or unsupported archive.
    Zip(String),
    /// A malformed or unsupported glTF file.
    Gltf(String),
    /// Manifest validation failed; every error, sorted by (file, path).
    Invalid(Vec<ValidationError>),
}

impl fmt::Display for AssetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AssetError::AbsolutePath(p) => write!(f, "absolute path not allowed: {p:?}"),
            AssetError::PathTraversal(p) => write!(f, "path escapes the bundle: {p:?}"),
            AssetError::Symlink(p) => write!(f, "symbolic links are not allowed: {p:?}"),
            AssetError::TooLarge { path, size } => {
                write!(f, "file {path:?} is too large ({size} bytes)")
            }
            AssetError::TotalTooLarge(size) => {
                write!(f, "bundle is too large ({size} bytes uncompressed)")
            }
            AssetError::CompressionRatio { path, ratio } => {
                write!(
                    f,
                    "file {path:?} has a suspicious compression ratio ({ratio}:1)"
                )
            }
            AssetError::TooManyEntries(n) => write!(f, "archive has too many entries ({n})"),
            AssetError::DuplicateEntry(p) => write!(f, "duplicate archive entry {p:?}"),
            AssetError::Encrypted(p) => write!(f, "encrypted archive entry {p:?}"),
            AssetError::BadName => f.write_str("archive entry name is not valid UTF-8"),
            AssetError::NotFound(p) => write!(f, "file not found in bundle: {p:?}"),
            AssetError::Io(m) => write!(f, "I/O error: {m}"),
            AssetError::Zip(m) => write!(f, "invalid archive: {m}"),
            AssetError::Gltf(m) => write!(f, "invalid glTF: {m}"),
            AssetError::Invalid(errors) => {
                write!(f, "{} validation error(s)", errors.len())?;
                for e in errors {
                    write!(f, "\n  {e}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for AssetError {}

impl From<std::io::Error> for AssetError {
    fn from(e: std::io::Error) -> Self {
        AssetError::Io(e.to_string())
    }
}
