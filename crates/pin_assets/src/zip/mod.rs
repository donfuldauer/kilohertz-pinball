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
use std::io::{Read, Seek, SeekFrom};
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
    // The result must normalise to itself (`list` and `exists` agree): a
    // drive letter revealed by dropping `.` components (`.\a:`) is absolute
    // too (4.3 fix round 1, O1).
    let o = out.as_bytes();
    if o.len() >= 2 && o[0].is_ascii_alphabetic() && o[1] == b':' {
        return Err(AssetError::AbsolutePath(name.to_owned()));
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
const CENTRAL_HEADER_SIG: u32 = 0x0201_4b50;

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

/// Size of a central-directory file header without its variable parts.
const CENTRAL_HEADER_MIN: u64 = 46;
/// Fixed size of the end-of-central-directory record.
const EOCD_LEN: usize = 22;
/// Fixed size of the Zip64 end record (its length field counts from byte 12).
const ZIP64_EOCD_LEN: u64 = 56;
/// Largest Zip64 extensible data sector accepted (the reader allocates it).
pub const MAX_ZIP64_EXTENSIBLE: u64 = 64 << 10;

/// The end of the central directory as the zip reader will see it.
#[derive(Copy, Clone, Debug)]
struct End {
    /// Offset of the end-of-central-directory record.
    eocd: usize,
    /// Number of central-directory records.
    count: u64,
}

/// Finds and fully checks the one end-of-central-directory record a bundle
/// may have, before the zip reader runs (N-a, review round 2 B1/B2).
///
/// The record is the last `PK\x05\x06` signature whose comment ends exactly
/// at the end of the archive (no trailing bytes); the reader is then shown the
/// archive up to that record with an empty comment ([`EndView`]) and opened
/// with `ArchiveOffset::Known(0)`, so it never searches for other candidates,
/// for prepended data, or for a moved Zip64 record. That only holds if the
/// reader accepts this record as it is, so every check it would make before
/// falling back is made here, and failing one refuses the archive:
///
/// * single disk, equal per-disk and total counts;
/// * Zip64 (when the reader would use it: a saturated field and a locator in
///   front): the record is exactly where the locator says, ends at the
///   locator, has an extensible sector of at most [`MAX_ZIP64_EXTENSIBLE`],
///   and has room for its records before it;
/// * the central directory ends exactly at the (Zip64) end record and starts
///   with a central-header signature.
///
/// The count is checked against `max_entries` here, before any record is
/// parsed. Archives with prepended data (self-extractors) are refused. All
/// checks are O(1) apart from the backwards search of the last 64 KiB.
fn find_end(b: &[u8], max_entries: usize) -> Result<End, AssetError> {
    let bad = |why: &str| AssetError::Zip(format!("end of central directory: {why}"));
    let len = b.len();
    let lowest = len.saturating_sub(EOCD_LEN + 0xFFFF);
    let mut at = len
        .checked_sub(EOCD_LEN)
        .ok_or_else(|| bad("archive too short"))?;
    let eocd = loop {
        if le_u32(b, at) == Some(u64::from(EOCD_SIG))
            && le_u16(b, at + 20).map(|c| at as u64 + EOCD_LEN as u64 + c) == Some(len as u64)
        {
            break at;
        }
        if at == lowest {
            return Err(bad("no end record whose comment ends the archive"));
        }
        at -= 1;
    };
    let field16 = |o: usize| le_u16(b, eocd + o).unwrap_or(0);
    let field32 = |o: usize| le_u32(b, eocd + o).unwrap_or(0);
    let (disk, cd_disk, on_disk, count) = (field16(4), field16(6), field16(8), field16(10));
    let (cd_size, cd_offset) = (field32(12), field32(16));
    let saturated = count == 0xFFFF || cd_size == 0xFFFF_FFFF || cd_offset == 0xFFFF_FFFF;
    let locator = eocd
        .checked_sub(20)
        .filter(|&l| le_u32(b, l) == Some(u64::from(ZIP64_LOCATOR_SIG)));
    let (count, cd_offset, cd_size, cd_end) = match locator {
        Some(loc) if saturated => {
            let (count, cd_offset, cd_size, record) = zip64_end(b, loc, max_entries)?;
            (count, cd_offset, cd_size, record)
        }
        _ => {
            if disk != 0 || cd_disk != 0 || on_disk != count {
                return Err(bad("multi-disk archives are not supported"));
            }
            (count, cd_offset, cd_size, eocd as u64)
        }
    };
    if count > max_entries as u64 {
        return Err(AssetError::TooManyEntries(
            usize::try_from(count).unwrap_or(usize::MAX),
        ));
    }
    if cd_offset.checked_add(cd_size) != Some(cd_end) {
        return Err(bad(
            "the central directory does not end at the end record (prepended or trailing data)",
        ));
    }
    if count > 0
        && usize::try_from(cd_offset).ok().and_then(|o| le_u32(b, o))
            != Some(u64::from(CENTRAL_HEADER_SIG))
    {
        return Err(bad("no central directory header where the end record says"));
    }
    Ok(End { eocd, count })
}

/// The Zip64 end record the locator at `loc` leads to: (count, directory
/// offset, directory size, record offset). See [`find_end`].
fn zip64_end(b: &[u8], loc: usize, max_entries: usize) -> Result<(u64, u64, u64, u64), AssetError> {
    let bad = |why: &str| AssetError::Zip(format!("Zip64 end record {why}"));
    if le_u32(b, loc + 16).is_none_or(|disks| disks > 1) || le_u32(b, loc + 4) != Some(0) {
        return Err(bad("describes more than one disk"));
    }
    let off = le_u64(b, loc + 8)
        .and_then(|o| usize::try_from(o).ok())
        .filter(|&o| o < loc)
        .ok_or_else(|| bad("offset is outside the archive"))?;
    if le_u32(b, off) != Some(u64::from(ZIP64_EOCD_SIG)) {
        return Err(bad("is not where its locator says"));
    }
    let size = le_u64(b, off + 4).ok_or_else(|| bad("is truncated"))?;
    if size.checked_add(12) != Some((loc - off) as u64) || size + 12 < ZIP64_EOCD_LEN {
        return Err(bad("does not end at its locator"));
    }
    if size + 12 - ZIP64_EOCD_LEN > MAX_ZIP64_EXTENSIBLE {
        return Err(bad("has an oversized extensible data sector"));
    }
    let field = |o: usize| le_u64(b, off + o).unwrap_or(u64::MAX);
    let (disk, cd_disk) = (le_u32(b, off + 16), le_u32(b, off + 20));
    let (on_disk, total) = (field(24), field(32));
    let (cd_size, cd_offset) = (field(40), field(48));
    if disk != Some(0) || cd_disk != Some(0) || on_disk != total {
        return Err(bad("describes more than one disk"));
    }
    if total > max_entries as u64 {
        return Err(AssetError::TooManyEntries(
            usize::try_from(total).unwrap_or(usize::MAX),
        ));
    }
    let room = total
        .checked_mul(CENTRAL_HEADER_MIN)
        .and_then(|n| n.checked_add(cd_offset));
    if room.is_none_or(|r| r > off as u64) {
        return Err(bad("declares more records than fit before it"));
    }
    Ok((total, cd_offset, cd_size, off as u64))
}

/// The archive as the zip reader sees it: everything up to the chosen end
/// record, which is shown with an empty comment and nothing after it, so the
/// record is the reader's first and only candidate at the end of the file.
struct EndView<'a> {
    /// The archive up to the comment-length field of the end record.
    head: &'a [u8],
    pos: u64,
}

impl EndView<'_> {
    fn len(&self) -> u64 {
        self.head.len() as u64 + 2
    }
}

impl Read for EndView<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut n = 0;
        while n < buf.len() && self.pos < self.len() {
            let available = if let Some(rest) = usize::try_from(self.pos)
                .ok()
                .and_then(|p| self.head.get(p..))
                .filter(|r| !r.is_empty())
            {
                let k = rest.len().min(buf.len() - n);
                buf[n..n + k].copy_from_slice(&rest[..k]);
                k
            } else {
                buf[n] = 0; // the empty comment's length field
                1
            };
            n += available;
            self.pos += available as u64;
        }
        Ok(n)
    }
}

impl Seek for EndView<'_> {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let target = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.len().checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos = target.ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "seek before the start")
        })?;
        Ok(self.pos)
    }
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
        let end = find_end(&bytes, limits.max_entries)?;
        let view = EndView {
            head: &bytes[..end.eocd + EOCD_LEN - 2],
            pos: 0,
        };
        let config = zip::read::Config {
            archive_offset: zip::read::ArchiveOffset::Known(0),
        };
        let archive = ZipArchive::with_config(config, view).map_err(zip_error)?;
        // Duplicate names make the reader keep fewer records (reported below).
        if archive.len() as u64 > end.count {
            return Err(AssetError::Zip(format!(
                "the central directory holds {} records, the end record says {}",
                archive.len(),
                end.count
            )));
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

    /// 4.3 fix round 1, O1: every accepted name normalises to itself, so
    /// `Vfs::list` and `Vfs::exists` agree (`.\\a:` hid the drive letter).
    #[test]
    fn normalisation_is_idempotent() {
        for p in [".\\a:", "./a:", "./c:x", "././z:/y"] {
            assert_eq!(
                normalize_path(p),
                Err(AssetError::AbsolutePath(p.to_owned())),
                "{p}"
            );
        }
        let names = [
            "a", "./a", "ab:", ".\\ab:", "x/./y", "\u{e9}/:", "1:", "_:x",
        ];
        for p in names {
            let once = normalize_path(p).unwrap();
            assert_eq!(normalize_path(&once).unwrap(), once, "{p}");
        }
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
