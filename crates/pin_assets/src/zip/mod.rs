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

/// Archive-level ratio rule (N-b): the declared uncompressed sizes of all
/// entries together may exceed `max_ratio` × the archive length only while
/// they stay at or below this floor, so a tiny archive of text still loads.
/// Entries under [`RATIO_FLOOR`] escape the per-entry check; this bounds
/// their sum: at most `max(ARCHIVE_RATIO_FLOOR, max_ratio × archive length)`
/// bytes can be read from any archive.
pub const ARCHIVE_RATIO_FLOOR: u64 = 8 << 20;

/// `path` of the [`AssetError::CompressionRatio`] raised by the archive-level
/// rule (no single entry is to blame).
pub const ARCHIVE_PATH: &str = "(whole archive)";

/// Largest archive (bytes) worth reading: its uncompressed cap plus 1 MiB of
/// headers. Checked by [`Vfs::open`](crate::Vfs::open) before reading the
/// file and by [`Vfs::from_zip_bytes`](crate::Vfs::from_zip_bytes).
pub(crate) fn archive_cap(limits: &ZipLimits) -> u64 {
    limits.max_total_uncompressed.saturating_add(1 << 20)
}

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

/// Most end-of-central-directory candidates with a Zip64 locator in front of
/// them that an archive may have (a real archive has one).
const MAX_ZIP64_LOCATORS: usize = 16;

/// Size of a central-directory file header without its variable parts.
const CENTRAL_HEADER_MIN: u64 = 46;

/// The entry count a Zip64 locator at `loc` leads to. The Zip64 end record
/// must be exactly where the locator says, end exactly at the locator, and be
/// one the zip reader accepts as it is (single disk, equal counts, room for
/// its records before it). Otherwise the reader would go looking for another
/// record anywhere before the locator (for archives with prepended data),
/// which this pre-check cannot follow cheaply, so such an archive is
/// refused instead: a bundle has no reason to carry prepended data.
fn zip64_count(b: &[u8], loc: usize, max_entries: usize) -> Result<u64, AssetError> {
    let bad = |why: &str| AssetError::Zip(format!("Zip64 end record {why}"));
    let off = le_u64(b, loc + 8)
        .and_then(|o| usize::try_from(o).ok())
        .filter(|&o| o < loc)
        .ok_or_else(|| bad("offset is outside the archive"))?;
    if le_u32(b, off) != Some(u64::from(ZIP64_EOCD_SIG)) {
        return Err(bad("is not where its locator says"));
    }
    let size = le_u64(b, off + 4).ok_or_else(|| bad("is truncated"))?;
    if (off as u64).checked_add(12).and_then(|e| e.checked_add(size)) != Some(loc as u64) {
        return Err(bad("does not end at its locator"));
    }
    let (disk, cd_disk) = (le_u32(b, off + 16), le_u32(b, off + 20));
    let (on_disk, total) = (le_u64(b, off + 24), le_u64(b, off + 32));
    let cd_offset = le_u64(b, off + 48).ok_or_else(|| bad("is truncated"))?;
    if disk != Some(0) || cd_disk != le_u32(b, loc + 4) || on_disk != total {
        return Err(bad("describes more than one disk"));
    }
    let total = total.ok_or_else(|| bad("is truncated"))?;
    let room = total
        .checked_mul(CENTRAL_HEADER_MIN)
        .and_then(|n| n.checked_add(cd_offset));
    if room.is_none_or(|r| r > off as u64) {
        // The reader would reject this record and search for another.
        return Err(if total > max_entries as u64 {
            AssetError::TooManyEntries(usize::try_from(total).unwrap_or(usize::MAX))
        } else {
            bad("declares more records than fit before it")
        });
    }
    Ok(total)
}

/// The largest entry count any end-of-central-directory candidate (or the
/// Zip64 end record its locator leads to) declares. Read before the central
/// directory is parsed, so a directory with millions of records is refused
/// without walking it. O(1) per candidate: no scan of the archive.
///
/// The Zip64 record is consulted whenever a locator precedes a candidate,
/// whatever its 16-bit count says (the zip reader switches to the Zip64
/// record when the directory size or offset is saturated, too); see
/// [`zip64_count`] for what it must look like.
fn declared_entry_count(b: &[u8], max_entries: usize) -> Result<u64, AssetError> {
    let tail_start = b.len().saturating_sub(22 + 0xFFFF);
    let mut max = 0;
    let mut locators = 0;
    let mut at = b.len().saturating_sub(22);
    loop {
        if at < tail_start {
            break;
        }
        if le_u32(b, at) == Some(u64::from(EOCD_SIG)) {
            max = max.max(le_u16(b, at + 10).unwrap_or(0));
            max = max.max(le_u16(b, at + 8).unwrap_or(0));
            if let Some(loc) = at.checked_sub(20)
                && le_u32(b, loc) == Some(u64::from(ZIP64_LOCATOR_SIG))
            {
                locators += 1;
                if locators > MAX_ZIP64_LOCATORS {
                    return Err(AssetError::Zip(
                        "too many Zip64 end-of-central-directory locators".to_owned(),
                    ));
                }
                max = max.max(zip64_count(b, loc, max_entries)?);
            }
        }
        if at == 0 {
            break;
        }
        at -= 1;
    }
    Ok(max)
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
        let size = bytes.len() as u64;
        if size > archive_cap(&limits) {
            return Err(AssetError::TotalTooLarge(size));
        }
        let declared = declared_entry_count(&bytes, limits.max_entries)?;
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
            if total > ARCHIVE_RATIO_FLOOR && total / len.max(1) > limits.max_ratio {
                return Err(AssetError::CompressionRatio {
                    path: ARCHIVE_PATH.to_owned(),
                    ratio: total / len.max(1),
                });
            }
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

    /// Declared uncompressed size of an entry (`path` already normalised).
    pub(crate) fn size(&self, path: &str) -> Option<u64> {
        self.entries.get(path).map(|e| e.size)
    }

    /// Decompresses one entry (`path` already normalised). Output never
    /// exceeds the declared size, and an entry over [`RATIO_FLOOR`] is
    /// aborted as soon as output / compressed bytes actually consumed exceeds
    /// the ratio cap.
    pub(crate) fn read(&self, path: &str) -> Result<Vec<u8>, AssetError> {
        self.read_prefix(path, u64::MAX)
    }

    /// The first `max` bytes of an entry (all of it when shorter): only
    /// those are decompressed. The CRC-32 is checked when the whole entry
    /// is read.
    pub(crate) fn read_prefix(&self, path: &str, max: u64) -> Result<Vec<u8>, AssetError> {
        let entry = *self
            .entries
            .get(path)
            .ok_or_else(|| AssetError::NotFound(path.to_owned()))?;
        let whole = max >= entry.size;
        let limit = entry.size.min(max);
        let raw = &self.bytes[entry.data_start..entry.data_start + entry.compressed];
        let out = if entry.deflated {
            let mut decoder = flate2::bufread::DeflateDecoder::new(raw);
            let capacity = usize::try_from(limit.min(RATIO_FLOOR)).unwrap_or(0);
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
                if !whole && produced >= limit {
                    break;
                }
            }
            out
        } else {
            raw[..usize::try_from(limit).unwrap_or(raw.len()).min(raw.len())].to_vec()
        };
        if !whole {
            let mut out = out;
            if (out.len() as u64) < limit {
                return Err(AssetError::Zip(format!(
                    "{path:?}: decompressed size is below the declared {}",
                    entry.size
                )));
            }
            out.truncate(limit as usize);
            return Ok(out);
        }
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
