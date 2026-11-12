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

/// Entries at or below this uncompressed size skip the ratio check (honest
/// small files such as silent WAVs reach ~1000:1); the size caps bound them.
pub const RATIO_FLOOR: u64 = 1 << 20;

const LOCAL_HEADER_SIG: u32 = 0x0403_4b50;
const EOCD_SIG: u32 = 0x0605_4b50;
const ZIP64_LOCATOR_SIG: u32 = 0x0706_4b50;
const ZIP64_EOCD_SIG: u32 = 0x0606_4b50;

#[derive(Copy, Clone, Debug)]
struct Entry {
    /// Offset of the compressed data in the archive.
    data_start: usize,
    compressed: usize,
    size: u64,
    deflated: bool,
    crc32: u32,
}

/// A validated archive held in memory.
pub(crate) struct ZipIndex {
    bytes: Vec<u8>,
    entries: BTreeMap<String, Entry>,
    max_ratio: u64,
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

fn le_u16(b: &[u8], at: usize) -> Option<u64> {
    let s = b.get(at..at.checked_add(2)?)?;
    Some(u64::from(u16::from_le_bytes([s[0], s[1]])))
}

fn le_u32(b: &[u8], at: usize) -> Option<u64> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u64::from(u32::from_le_bytes([s[0], s[1], s[2], s[3]])))
}

fn le_u64(b: &[u8], at: usize) -> Option<u64> {
    let s = b.get(at..at.checked_add(8)?)?;
    let mut a = [0u8; 8];
    a.copy_from_slice(s);
    Some(u64::from_le_bytes(a))
}

/// The largest entry count any end-of-central-directory candidate (or its
/// Zip64 record) declares. Read before the central directory is parsed, so a
/// directory with millions of records is refused without walking it.
fn declared_entry_count(b: &[u8]) -> u64 {
    let tail_start = b.len().saturating_sub(22 + 0xFFFF);
    let mut max = 0;
    let mut at = b.len().saturating_sub(22);
    loop {
        if at < tail_start {
            break;
        }
        if le_u32(b, at) == Some(u64::from(EOCD_SIG)) {
            let count = le_u16(b, at + 10).unwrap_or(0);
            max = max.max(count);
            if count == 0xFFFF
                && let Some(loc) = at.checked_sub(20)
                && le_u32(b, loc) == Some(u64::from(ZIP64_LOCATOR_SIG))
                && let Some(off) = le_u64(b, loc + 8).and_then(|o| usize::try_from(o).ok())
                && le_u32(b, off) == Some(u64::from(ZIP64_EOCD_SIG))
            {
                max = max.max(le_u64(b, off.saturating_add(32)).unwrap_or(0));
            }
        }
        if at == 0 {
            break;
        }
        at -= 1;
    }
    max
}

/// Offset of an entry's data, from its local header.
fn local_data_start(b: &[u8], header: u64, name: &str) -> Result<u64, AssetError> {
    let bad = || AssetError::Zip(format!("{name:?}: bad local header"));
    let h = usize::try_from(header).map_err(|_| bad())?;
    if le_u32(b, h) != Some(u64::from(LOCAL_HEADER_SIG)) {
        return Err(bad());
    }
    let n = le_u16(b, h + 26).ok_or_else(bad)?;
    let m = le_u16(b, h + 28).ok_or_else(bad)?;
    Ok(header + 30 + n + m)
}

fn ratio_error(path: &str, out: u64, consumed: u64) -> AssetError {
    AssetError::CompressionRatio {
        path: path.to_owned(),
        ratio: out / consumed.max(1),
    }
}

impl ZipIndex {
    /// Scans and validates the whole central directory.
    pub(crate) fn new(bytes: Vec<u8>, limits: ZipLimits) -> Result<ZipIndex, AssetError> {
        let declared = declared_entry_count(&bytes);
        if declared > limits.max_entries as u64 {
            return Err(AssetError::TooManyEntries(
                usize::try_from(declared).unwrap_or(usize::MAX),
            ));
        }
        let archive = ZipArchive::new(Cursor::new(bytes.as_slice())).map_err(zip_error)?;
        if archive.len() > limits.max_entries {
            return Err(AssetError::TooManyEntries(archive.len()));
        }
        let len = bytes.len() as u64;
        let mut entries = BTreeMap::new();
        // (local header start, data end, name) of every record.
        let mut ranges: Vec<(u64, u64, String)> = Vec::with_capacity(archive.len());
        let mut central_start = len;
        let mut total: u64 = 0;
        let mut total_compressed: u64 = 0;
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
            // The data must really be in the archive, once.
            let mut record_range = || -> Result<(), AssetError> {
                let compressed = file.compressed_size();
                let data_start = local_data_start(&bytes, file.header_start(), name)?;
                let data_end = data_start
                    .checked_add(compressed)
                    .filter(|&e| e <= len)
                    .ok_or_else(|| {
                        AssetError::Zip(format!(
                            "{name:?}: compressed data runs past the end of the archive"
                        ))
                    })?;
                central_start = central_start.min(file.central_header_start());
                total_compressed = total_compressed.saturating_add(compressed);
                ranges.push((file.header_start(), data_end, name.to_owned()));
                Ok(())
            };
            if is_dir {
                record_range()?;
                continue;
            }
            if path.is_empty() {
                return Err(AssetError::BadName);
            }
            let deflated = match file.compression() {
                CompressionMethod::Stored => false,
                CompressionMethod::Deflated => true,
                other => {
                    return Err(AssetError::Zip(format!(
                        "{name:?}: unsupported compression method {other:?}"
                    )));
                }
            };
            let size = file.size();
            if size > limits.max_file {
                return Err(AssetError::TooLarge {
                    path: name.to_owned(),
                    size,
                });
            }
            let compressed = file.compressed_size();
            if !deflated && size != compressed {
                return Err(AssetError::Zip(format!(
                    "{name:?}: stored entry with different sizes"
                )));
            }
            if size > RATIO_FLOOR {
                // An empty compressed stream counts as one byte.
                let ratio = size / compressed.max(1);
                if ratio > limits.max_ratio {
                    return Err(AssetError::CompressionRatio {
                        path: name.to_owned(),
                        ratio,
                    });
                }
            }
            total = total.saturating_add(size);
            if total > limits.max_total_uncompressed {
                return Err(AssetError::TotalTooLarge(total));
            }
            record_range()?;
            let data_start = local_data_start(&bytes, file.header_start(), name)?;
            let entry = Entry {
                // Both fit: data_end <= len.
                data_start: data_start as usize,
                compressed: compressed as usize,
                size,
                deflated,
                crc32: file.crc32(),
            };
            if entries.insert(path.clone(), entry).is_some() {
                return Err(AssetError::DuplicateEntry(path));
            }
        }
        if total_compressed > len {
            return Err(AssetError::Zip(
                "declared compressed sizes exceed the archive".to_owned(),
            ));
        }
        ranges.sort_unstable();
        let mut previous_end = 0;
        for (start, end, name) in &ranges {
            if *start < previous_end || *end > central_start {
                return Err(AssetError::Zip(format!(
                    "{name:?}: entry data overlaps another entry or the central directory"
                )));
            }
            previous_end = *end;
        }
        if archive.has_duplicate_names() {
            // The zip reader keeps one record per exact name; the raw count tells.
            return Err(AssetError::DuplicateEntry(
                "(identical names in the central directory)".to_owned(),
            ));
        }
        drop(archive);
        Ok(ZipIndex {
            bytes,
            entries,
            max_ratio: limits.max_ratio,
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

    /// Drops entries the predicate refuses (OS metadata files).
    pub(crate) fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.entries.retain(|k, _| keep(k));
    }

    pub(crate) fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    pub(crate) fn contains(&self, path: &str) -> bool {
        self.entries.contains_key(path)
    }

    /// Decompresses one entry (`path` already normalised). Output never
    /// exceeds the declared size, and an entry over [`RATIO_FLOOR`] is
    /// aborted as soon as output / compressed bytes actually consumed exceeds
    /// the ratio cap.
    pub(crate) fn read(&self, path: &str) -> Result<Vec<u8>, AssetError> {
        let entry = *self
            .entries
            .get(path)
            .ok_or_else(|| AssetError::NotFound(path.to_owned()))?;
        let raw = &self.bytes[entry.data_start..entry.data_start + entry.compressed];
        let out = if entry.deflated {
            let mut decoder = flate2::bufread::DeflateDecoder::new(raw);
            let capacity = usize::try_from(entry.size.min(RATIO_FLOOR)).unwrap_or(0);
            let mut out = Vec::with_capacity(capacity);
            let mut chunk = vec![0u8; 64 * 1024];
            loop {
                let n = decoder
                    .read(&mut chunk)
                    .map_err(|e| AssetError::Zip(format!("{path:?}: {e}")))?;
                if n == 0 {
                    break;
                }
                out.extend_from_slice(&chunk[..n]);
                let produced = out.len() as u64;
                if produced > entry.size {
                    break; // reported below
                }
                let consumed = (raw.len() - decoder.get_ref().len()) as u64;
                if produced > RATIO_FLOOR
                    && produced > self.max_ratio.saturating_mul(consumed.max(1))
                {
                    return Err(ratio_error(path, produced, consumed));
                }
            }
            out
        } else {
            raw.to_vec()
        };
        if out.len() as u64 != entry.size {
            return Err(AssetError::Zip(format!(
                "{path:?}: decompressed size differs from the declared {}",
                entry.size
            )));
        }
        if crc32fast::hash(&out) != entry.crc32 {
            return Err(AssetError::Zip(format!("{path:?}: CRC mismatch")));
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
        assert_eq!(normalize_path("ab:c").unwrap(), "ab:c");
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
