//! Bundle virtual filesystem (`.pinbundle` zip or folder).
//!
//! Owner: Subphase 4.1.
//!
//! Paths are `/`-separated and relative to the bundle root. Every requested
//! path is normalised with [`normalize_path`](crate::zip::normalize_path), so
//! absolute paths and `..` are refused before anything is looked up. A folder
//! bundle additionally refuses any path whose canonical form (after resolving
//! symlinks) lies outside the canonical root.

use crate::AssetError;
use crate::zip::{ZipIndex, ZipLimits, normalize_path};
use std::path::{Path, PathBuf};

/// Directory recursion limit for folder listings.
const MAX_DEPTH: usize = 64;

/// The manifest whose position decides whether a wrapping folder is stripped.
const ROOT_MARKER: &str = "table.toml";

#[derive(Debug)]
enum Backend {
    Dir { root: PathBuf, max_file: u64 },
    Zip(ZipIndex),
}

/// A read-only view of a table bundle: a folder, or a zip archive held in memory.
#[derive(Debug)]
pub struct Vfs {
    backend: Backend,
}

fn has_archive_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("zip") || e.eq_ignore_ascii_case("pinbundle"))
}

/// The wrapping folder to strip: `table.toml` is not at the root, every file
/// lives under one top-level folder `d`, and `d/table.toml` exists.
fn wrapper_folder<'a>(mut names: impl Iterator<Item = &'a str>) -> Option<String> {
    let first = names.next()?;
    let (dir, _) = first.split_once('/')?;
    let mut has_marker = first == format!("{dir}/{ROOT_MARKER}");
    for name in names {
        match name.split_once('/') {
            Some((d, rest)) if d == dir => has_marker |= rest == ROOT_MARKER,
            _ => return None,
        }
    }
    has_marker.then(|| dir.to_owned())
}

impl Vfs {
    /// Opens a folder, or a file ending in `.zip` / `.pinbundle`, with the
    /// default [`ZipLimits`].
    pub fn open(path: &Path) -> Result<Vfs, AssetError> {
        Vfs::open_with_limits(path, ZipLimits::default())
    }

    /// Opens a folder or archive with explicit limits. For a folder, only
    /// `max_file` applies (checked on every read).
    pub fn open_with_limits(path: &Path, limits: ZipLimits) -> Result<Vfs, AssetError> {
        let meta = std::fs::metadata(path)?;
        if meta.is_dir() {
            return Vfs::open_dir(path, limits);
        }
        if !has_archive_extension(path) {
            return Err(AssetError::Io(format!(
                "{}: not a folder, .zip or .pinbundle",
                path.display()
            )));
        }
        // A stored archive is slightly larger than its contents; anything far
        // beyond the uncompressed cap cannot be valid.
        let cap = limits
            .max_total_uncompressed
            .saturating_mul(2)
            .saturating_add(1 << 20);
        if meta.len() > cap {
            return Err(AssetError::TotalTooLarge(meta.len()));
        }
        let bytes = std::fs::read(path)?;
        Vfs::from_zip_bytes(bytes, limits)
    }

    /// Validates an in-memory archive. Nothing is decompressed until [`read`](Self::read).
    pub fn from_zip_bytes(bytes: Vec<u8>, limits: ZipLimits) -> Result<Vfs, AssetError> {
        let mut index = ZipIndex::new(bytes, limits)?;
        if !index.contains(ROOT_MARKER)
            && let Some(dir) = wrapper_folder(index.names())
        {
            index.strip_prefix(&dir);
        }
        Ok(Vfs {
            backend: Backend::Zip(index),
        })
    }

    fn open_dir(path: &Path, limits: ZipLimits) -> Result<Vfs, AssetError> {
        let mut root = std::fs::canonicalize(path)?;
        if !root.join(ROOT_MARKER).is_file() {
            // A single top-level folder wrapping everything.
            let mut children = std::fs::read_dir(&root)?;
            if let (Some(Ok(only)), None) = (children.next(), children.next()) {
                let candidate = only.path();
                if only.file_type().is_ok_and(|t| t.is_dir())
                    && candidate.join(ROOT_MARKER).is_file()
                {
                    root = std::fs::canonicalize(candidate)?;
                }
            }
        }
        Ok(Vfs {
            backend: Backend::Dir {
                root,
                max_file: limits.max_file,
            },
        })
    }

    /// Resolves a folder path, refusing anything outside the root.
    fn resolve_dir(root: &Path, path: &str, original: &str) -> Result<PathBuf, AssetError> {
        let mut full = root.to_path_buf();
        full.extend(path.split('/'));
        let canonical = match std::fs::canonicalize(&full) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(AssetError::NotFound(original.to_owned()));
            }
            Err(e) => return Err(e.into()),
        };
        if !canonical.starts_with(root) {
            return Err(AssetError::PathTraversal(original.to_owned()));
        }
        Ok(canonical)
    }

    /// Reads a whole file. `path` is `/`-separated and relative to the root.
    pub fn read(&self, path: &str) -> Result<Vec<u8>, AssetError> {
        let norm = normalize_path(path)?;
        if norm.is_empty() {
            return Err(AssetError::NotFound(path.to_owned()));
        }
        match &self.backend {
            Backend::Zip(index) => index.read(&norm).map_err(|e| match e {
                AssetError::NotFound(_) => AssetError::NotFound(path.to_owned()),
                other => other,
            }),
            Backend::Dir { root, max_file } => {
                let full = Vfs::resolve_dir(root, &norm, path)?;
                let meta = std::fs::metadata(&full)?;
                if !meta.is_file() {
                    return Err(AssetError::NotFound(path.to_owned()));
                }
                if meta.len() > *max_file {
                    return Err(AssetError::TooLarge {
                        path: norm,
                        size: meta.len(),
                    });
                }
                Ok(std::fs::read(&full)?)
            }
        }
    }

    /// True when `path` names a readable file inside the bundle.
    pub fn exists(&self, path: &str) -> bool {
        let Ok(norm) = normalize_path(path) else {
            return false;
        };
        if norm.is_empty() {
            return false;
        }
        match &self.backend {
            Backend::Zip(index) => index.contains(&norm),
            Backend::Dir { root, .. } => {
                Vfs::resolve_dir(root, &norm, path).is_ok_and(|p| p.is_file())
            }
        }
    }

    /// Every file in the bundle, sorted, `/`-separated, without directories.
    /// In a folder bundle, symlinks are listed only when they resolve to a file
    /// inside the root, and symlinked folders are not descended into.
    pub fn list(&self) -> Vec<String> {
        match &self.backend {
            Backend::Zip(index) => index.names().map(str::to_owned).collect(),
            Backend::Dir { root, .. } => {
                let mut out = Vec::new();
                walk(root, root, "", 0, &mut out);
                out.sort();
                out
            }
        }
    }
}

fn walk(root: &Path, dir: &Path, prefix: &str, depth: usize, out: &mut Vec<String>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue; // not addressable by a UTF-8 path
        };
        let rel = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            walk(root, &entry.path(), &rel, depth + 1, out);
        } else if kind.is_file() {
            out.push(rel);
        } else if kind.is_symlink()
            && std::fs::canonicalize(entry.path())
                .is_ok_and(|target| target.starts_with(root) && target.is_file())
        {
            out.push(rel);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapper_detection() {
        let names = ["pkg/table.toml", "pkg/a.lua"];
        assert_eq!(wrapper_folder(names.into_iter()), Some("pkg".to_owned()));
        let names = ["pkg/a.lua", "pkg/table.toml"];
        assert_eq!(wrapper_folder(names.into_iter()), Some("pkg".to_owned()));
        let names = ["pkg/table.toml", "other/a.lua"];
        assert_eq!(wrapper_folder(names.into_iter()), None);
        let names = ["pkg/table.toml", "x.lua"];
        assert_eq!(wrapper_folder(names.into_iter()), None);
        let names = ["pkg/sub/table.toml"];
        assert_eq!(wrapper_folder(names.into_iter()), None);
        assert_eq!(wrapper_folder(core::iter::empty()), None);
        assert_eq!(wrapper_folder(["a.lua"].into_iter()), None);
    }

    #[test]
    fn extension_check() {
        assert!(has_archive_extension(Path::new("x.ZIP")));
        assert!(has_archive_extension(Path::new("x.pinbundle")));
        assert!(!has_archive_extension(Path::new("x.toml")));
        assert!(!has_archive_extension(Path::new("x")));
    }
}
