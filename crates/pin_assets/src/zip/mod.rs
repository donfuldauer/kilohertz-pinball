//! Hardened zip extraction (no absolute paths, `..`, symlinks; size and ratio caps).
//!
//! Owner: Subphase 4.1.
//!
//! A bundle archive is untrusted (D25, §3.5). [`ZipIndex::new`] scans the whole
//! central directory at open time and rejects the archive if any entry is
//! hostile; nothing is ever extracted to disk. Entries are decompressed into
//! memory on demand, and a decompressed entry may not exceed the size its
//! central-directory record declared (which the caps were checked against).

use crate::AssetError;
use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::sync::Mutex;
use zip::CompressionMethod;
use zip::ZipArchive;
use zip::result::ZipError;

/// Caps applied to an archive at open time.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ZipLimits {
    /// Sum of the uncompressed sizes of all entries, bytes (default 512 MiB).
    pub max_total_uncompressed: u64,
    /// Uncompressed size of a single entry, bytes (default 256 MiB).
    pub max_file: u64,
    /// Largest allowed `uncompressed / compressed` ratio of an entry (default 100).
    pub max_ratio: u64,
    /// Number of central-directory entries, directories included (default 10 000).
    pub max_entries: usize,
}

impl Default for ZipLimits {
    fn default() -> Self {
        ZipLimits {
            max_total_uncompressed: 512 * 1024 * 1024,
            max_file: 256 * 1024 * 1024,
            max_ratio: 100,
            max_entries: 10_000,
        }
    }
}

/// Normalises a bundle-relative path: `\` becomes `/`, empty and `.`
/// components are dropped. Absolute paths (leading `/` or `\`, UNC
/// `\\server\x`, drive prefixes `C:`) and any `..` component are rejected, as
/// are NUL characters. The result may be empty (the root).
pub fn normalize_path(name: &str) -> Result<String, AssetError> {
    if name.contains('\0') {
        return Err(AssetError::BadName);
    }
    let s = name.replace('\\', "/");
    let b = s.as_bytes();
    if b.first() == Some(&b'/') || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':') {
        return Err(AssetError::AbsolutePath(name.to_owned()));
    }
    let mut out = String::with_capacity(s.len());
    for component in s.split('/') {
        match component {
            "" | "." => {}
            ".." => return Err(AssetError::PathTraversal(name.to_owned())),
            c => {
                if !out.is_empty() {
                    out.push('/');
                }
                out.push_str(c);
            }
        }
    }
    Ok(out)
}

#[derive(Copy, Clone, Debug)]
struct Entry {
    index: usize,
    size: u64,
}

/// A validated archive held in memory.
pub(crate) struct ZipIndex {
    archive: Mutex<ZipArchive<Cursor<Vec<u8>>>>,
    entries: BTreeMap<String, Entry>,
}

impl core::fmt::Debug for ZipIndex {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ZipIndex")
            .field("entries", &self.entries.len())
            .finish()
    }
}

fn zip_error(e: ZipError) -> AssetError {
    AssetError::Zip(e.to_string())
}

impl ZipIndex {
    /// Scans and validates the whole central directory.
    pub(crate) fn new(bytes: Vec<u8>, limits: ZipLimits) -> Result<ZipIndex, AssetError> {
        let archive = ZipArchive::new(Cursor::new(bytes)).map_err(zip_error)?;
        if archive.len() > limits.max_entries {
            return Err(AssetError::TooManyEntries(archive.len()));
        }
        let mut entries = BTreeMap::new();
        let mut total: u64 = 0;
        for index in 0..archive.len() {
            let file = archive.by_index_data(index).map_err(zip_error)?;
            let raw = file.name_raw();
            let name = core::str::from_utf8(raw).map_err(|_| AssetError::BadName)?;
            let path = normalize_path(name)?;
            let is_dir = name.ends_with('/') || name.ends_with('\\') || file.is_dir();
            if file.is_symlink() {
                return Err(AssetError::Symlink(name.to_owned()));
            }
            if file.encrypted() {
                return Err(AssetError::Encrypted(name.to_owned()));
            }
            if is_dir {
                continue;
            }
            if path.is_empty() {
                return Err(AssetError::BadName);
            }
            match file.compression() {
                CompressionMethod::Stored | CompressionMethod::Deflated => {}
                other => {
                    return Err(AssetError::Zip(format!(
                        "{name:?}: unsupported compression method {other:?}"
                    )));
                }
            }
            let size = file.size();
            if size > limits.max_file {
                return Err(AssetError::TooLarge {
                    path: name.to_owned(),
                    size,
                });
            }
            let compressed = file.compressed_size();
            let ratio = if compressed == 0 {
                size
            } else {
                size / compressed
            };
            if ratio > limits.max_ratio {
                return Err(AssetError::CompressionRatio {
                    path: name.to_owned(),
                    ratio,
                });
            }
            total = total.saturating_add(size);
            if total > limits.max_total_uncompressed {
                return Err(AssetError::TotalTooLarge(total));
            }
            if entries.insert(path.clone(), Entry { index, size }).is_some() {
                return Err(AssetError::DuplicateEntry(path));
            }
        }
        if archive.has_duplicate_names() {
            // The zip reader keeps one record per exact name; the raw count tells.
            return Err(AssetError::DuplicateEntry(
                "(identical names in the central directory)".to_owned(),
            ));
        }
        Ok(ZipIndex {
            archive: Mutex::new(archive),
            entries,
        })
    }

    /// Removes a single wrapping folder `prefix/` from every entry.
    pub(crate) fn strip_prefix(&mut self, prefix: &str) {
        let wrapped = format!("{prefix}/");
        self.entries = core::mem::take(&mut self.entries)
            .into_iter()
            .filter_map(|(k, v)| k.strip_prefix(&wrapped).map(|s| (s.to_owned(), v)))
            .collect();
    }

    pub(crate) fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    pub(crate) fn contains(&self, path: &str) -> bool {
        self.entries.contains_key(path)
    }

    /// Decompresses one entry (`path` already normalised).
    pub(crate) fn read(&self, path: &str) -> Result<Vec<u8>, AssetError> {
        let entry = *self
            .entries
            .get(path)
            .ok_or_else(|| AssetError::NotFound(path.to_owned()))?;
        let mut archive = self
            .archive
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let file = archive.by_index(entry.index).map_err(zip_error)?;
        // Never trust the stream to stop where the directory said it would.
        let capacity = usize::try_from(entry.size.min(1 << 20)).unwrap_or(0);
        let mut out = Vec::with_capacity(capacity);
        file.take(entry.size.saturating_add(1))
            .read_to_end(&mut out)
            .map_err(|e| AssetError::Zip(format!("{path:?}: {e}")))?;
        if out.len() as u64 != entry.size {
            return Err(AssetError::Zip(format!(
                "{path:?}: decompressed size {} differs from the declared {}",
                out.len(),
                entry.size
            )));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_accepts_and_cleans() {
        assert_eq!(normalize_path("a/b").unwrap(), "a/b");
        assert_eq!(normalize_path("a//b").unwrap(), "a/b");
        assert_eq!(normalize_path("./a/b").unwrap(), "a/b");
        assert_eq!(normalize_path("a\\b").unwrap(), "a/b");
        assert_eq!(normalize_path("a/b/").unwrap(), "a/b");
        assert_eq!(normalize_path("").unwrap(), "");
        assert_eq!(normalize_path("a:b").unwrap(), "a:b");
    }

    #[test]
    fn normalize_rejects() {
        for p in ["/etc/x", "C:\\x", "C:/x", "c:x", "\\\\server\\x", "\\x"] {
            assert_eq!(
                normalize_path(p),
                Err(AssetError::AbsolutePath(p.to_owned())),
                "{p}"
            );
        }
        for p in ["../x", "a/../../x", "..\\x", "a/..", "a/../b"] {
            assert_eq!(
                normalize_path(p),
                Err(AssetError::PathTraversal(p.to_owned())),
                "{p}"
            );
        }
        assert_eq!(normalize_path("a\0b"), Err(AssetError::BadName));
    }

    #[test]
    fn default_limits() {
        let l = ZipLimits::default();
        assert_eq!(l.max_total_uncompressed, 512 << 20);
        assert_eq!(l.max_file, 256 << 20);
        assert_eq!(l.max_ratio, 100);
        assert_eq!(l.max_entries, 10_000);
    }

    #[test]
    fn garbage_is_a_zip_error() {
        assert!(matches!(
            ZipIndex::new(b"not a zip".to_vec(), ZipLimits::default()),
            Err(AssetError::Zip(_))
        ));
    }
}
