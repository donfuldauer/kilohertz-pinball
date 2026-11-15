//! An immutable in-memory copy of a table bundle (Subphase 6.0 review
//! finding: validate and simulate the same bytes).
//!
//! [`BundleSnapshot::capture`] opens a bundle (folder, `.zip` or
//! `.pinbundle`), reads every listed file exactly once and keeps the bytes
//! in memory as an uncompressed ("stored") zip archive served by
//! [`Vfs::from_zip_bytes`]. Everything read through
//! [`BundleSnapshot::vfs`] afterwards comes from that copy, so a bundle
//! replaced or edited on disk after the capture cannot change what a
//! validator saw or what [`Simulation::load_snapshot`](crate::Simulation::load_snapshot)
//! builds. The archive keeps the sorted file names and contents of the
//! source, so [`bundle_hash`](super::bundle_hash) of the snapshot equals the
//! hash of the bundle it was taken from at capture time.

use crate::sim::SimError;
use pin_assets::zip::ZipLimits;
use pin_assets::{AssetError, Vfs};
use std::path::Path;

/// CRC-32 (IEEE 802.3, reflected, polynomial `0xEDB88320`) lookup table.
const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

/// CRC-32 of `bytes` as zip stores it.
fn crc32(bytes: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in bytes {
        c = CRC_TABLE[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

/// DOS date 1980-01-01 (the zip epoch; the snapshot carries no times).
const DOS_DATE: u16 = (1 << 5) | 1;
/// General-purpose flag bit 11: names are UTF-8.
const UTF8_FLAG: u16 = 1 << 11;

fn too_big(what: &str) -> SimError {
    SimError::Asset(AssetError::Zip(format!(
        "bundle snapshot: {what} exceeds the zip32 range"
    )))
}

fn u32_of(n: usize, what: &str) -> Result<u32, SimError> {
    u32::try_from(n).map_err(|_| too_big(what))
}

/// Local file header (30 bytes) plus central directory record (46 bytes)
/// per entry, before the name (written twice).
const ENTRY_OVERHEAD: u64 = 30 + 46;
/// End of central directory record, bytes.
const EOCD_LEN: u64 = 22;

/// Exact size in bytes of the archive [`stored_zip`] writes for entries of
/// the given (name length, data length), or `None` on `u64` overflow.
fn stored_zip_len(entries: impl IntoIterator<Item = (usize, usize)>) -> Option<u64> {
    entries.into_iter().try_fold(EOCD_LEN, |acc, (name, data)| {
        let name = u64::try_from(name).ok()?;
        let data = u64::try_from(data).ok()?;
        acc.checked_add(ENTRY_OVERHEAD)?
            .checked_add(name.checked_mul(2)?)?
            .checked_add(data)
    })
}

/// Writes `files` (name, bytes) as a stored zip archive. The output size is
/// computed first and refused (zip32 range) before anything is copied.
fn stored_zip(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>, SimError> {
    let total = stored_zip_len(files.iter().map(|(n, d)| (n.len(), d.len())));
    if total.is_none_or(|t| t > u64::from(u32::MAX)) {
        return Err(too_big("archive size"));
    }
    let mut out = Vec::new();
    let mut central = Vec::new();
    let count = u16::try_from(files.len()).map_err(|_| too_big("file count"))?;
    for (name, data) in files {
        let offset = u32_of(out.len(), "archive size")?;
        let name_len = u16::try_from(name.len()).map_err(|_| too_big("file name"))?;
        let size = u32_of(data.len(), "file size")?;
        let crc = crc32(data);
        // Local file header.
        out.extend_from_slice(&0x0403_4B50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&UTF8_FLAG.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // stored
        out.extend_from_slice(&0u16.to_le_bytes()); // time
        out.extend_from_slice(&DOS_DATE.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        // Central directory record.
        central.extend_from_slice(&0x0201_4B50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // made by (MS-DOS)
        central.extend_from_slice(&20u16.to_le_bytes()); // version needed
        central.extend_from_slice(&UTF8_FLAG.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // stored
        central.extend_from_slice(&0u16.to_le_bytes()); // time
        central.extend_from_slice(&DOS_DATE.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&name_len.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra
        central.extend_from_slice(&0u16.to_le_bytes()); // comment
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
        central.extend_from_slice(&0u32.to_le_bytes()); // external attributes
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let cd_offset = u32_of(out.len(), "archive size")?;
    let cd_size = u32_of(central.len(), "central directory")?;
    out.extend_from_slice(&central);
    // End of central directory.
    out.extend_from_slice(&0x0605_4B50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    u32_of(out.len(), "archive size")?;
    Ok(out)
}

/// [`AssetError::TooLarge`] when file `name` of `size` bytes exceeds
/// `limits.max_file`.
fn check_file(name: &str, size: u64, limits: &ZipLimits) -> Result<(), SimError> {
    if size > limits.max_file {
        return Err(AssetError::TooLarge {
            path: name.to_owned(),
            size,
        }
        .into());
    }
    Ok(())
}

/// An immutable in-memory copy of a table bundle (see the
/// [module docs](self)). Validate it and build the simulation from it
/// ([`Simulation::load_snapshot`](crate::Simulation::load_snapshot)) so
/// both see the same bytes.
#[derive(Debug)]
pub struct BundleSnapshot {
    vfs: Vfs,
    hash: u64,
    files: usize,
}

impl BundleSnapshot {
    /// Opens `bundle` (folder, `.zip` or `.pinbundle`) and copies every
    /// file into memory, reading each once.
    ///
    /// # Errors
    /// [`SimError::Asset`] when the bundle cannot be opened or a file cannot
    /// be read (or the copy exceeds the zip32 range or [`ZipLimits`]).
    pub fn capture(bundle: &Path) -> Result<BundleSnapshot, SimError> {
        let vfs = Vfs::open(bundle)?;
        BundleSnapshot::from_vfs(&vfs)
    }

    /// Copies every file of an open `vfs` into memory, reading each once.
    ///
    /// The [`ZipLimits::default`] entry-count, per-file and aggregate limits
    /// are enforced while capturing, before data beyond them is retained.
    ///
    /// # Errors
    /// As [`capture`](Self::capture).
    pub fn from_vfs(vfs: &Vfs) -> Result<BundleSnapshot, SimError> {
        BundleSnapshot::from_vfs_with_limits(vfs, &ZipLimits::default())
    }

    /// [`from_vfs`](Self::from_vfs) with explicit `limits` (the snapshot
    /// archive is validated with the same limits). Limits are checked before
    /// data is retained beyond them: the entry count before any read, the
    /// declared sizes ([`Vfs::file_size`]) before any read, and the bytes
    /// actually read after each file, so a failing capture holds at most
    /// `max_total_uncompressed` plus one file.
    ///
    /// # Errors
    /// [`AssetError::TooManyEntries`] when the bundle lists more than
    /// `max_entries` files, [`AssetError::TooLarge`] when a file exceeds
    /// `max_file`, [`AssetError::TotalTooLarge`] when the sum of the sizes
    /// exceeds `max_total_uncompressed` (all wrapped in [`SimError::Asset`]),
    /// and the other [`capture`](Self::capture) errors.
    #[doc(hidden)]
    pub fn from_vfs_with_limits(vfs: &Vfs, limits: &ZipLimits) -> Result<BundleSnapshot, SimError> {
        let mut names = vfs.list();
        names.sort();
        if names.len() > limits.max_entries {
            return Err(AssetError::TooManyEntries(names.len()).into());
        }
        // Declared sizes first: refuse an oversized bundle before reading.
        let mut declared = 0u64;
        for name in &names {
            let size = vfs.file_size(name)?;
            check_file(name, size, limits)?;
            declared = declared.saturating_add(size);
            if declared > limits.max_total_uncompressed {
                return Err(AssetError::TotalTooLarge(declared).into());
            }
        }
        // Files may change between the size check and the read: count the
        // bytes actually read and stop at the first overrun.
        let mut files = Vec::new();
        let mut total = 0u64;
        for name in names {
            let bytes = vfs.read(&name)?;
            let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
            check_file(&name, size, limits)?;
            total = total.saturating_add(size);
            if total > limits.max_total_uncompressed {
                return Err(AssetError::TotalTooLarge(total).into());
            }
            files.push((name, bytes));
        }
        let count = files.len();
        let archive = stored_zip(&files)?;
        drop(files);
        let vfs = Vfs::from_zip_bytes(archive, *limits)?;
        let hash = super::bundle_hash(&vfs)?;
        Ok(BundleSnapshot {
            vfs,
            hash,
            files: count,
        })
    }

    /// The snapshot's files (read-only, in memory).
    #[must_use]
    pub fn vfs(&self) -> &Vfs {
        &self.vfs
    }

    /// [`bundle_hash`](super::bundle_hash) of the snapshot: equal to
    /// [`Simulation::table_hash`](crate::Simulation::table_hash) of a
    /// simulation built from it.
    #[must_use]
    pub fn bundle_hash(&self) -> u64 {
        self.hash
    }

    /// Number of files in the snapshot.
    #[must_use]
    pub fn file_count(&self) -> usize {
        self.files
    }

    /// Reads file `path` of the snapshot.
    ///
    /// # Errors
    /// [`AssetError::NotFound`] and the other [`Vfs::read`] errors.
    pub fn read(&self, path: &str) -> Result<Vec<u8>, AssetError> {
        self.vfs.read(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_the_reference_value() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn stored_zip_round_trips_through_the_vfs() {
        let files = vec![
            ("a.txt".to_owned(), b"hello".to_vec()),
            ("dir/b.bin".to_owned(), vec![0u8, 1, 2, 255]),
            ("empty".to_owned(), Vec::new()),
        ];
        let vfs = Vfs::from_zip_bytes(stored_zip(&files).unwrap(), ZipLimits::default()).unwrap();
        assert_eq!(vfs.list(), vec!["a.txt", "dir/b.bin", "empty"]);
        for (n, b) in &files {
            assert_eq!(&vfs.read(n).unwrap(), b, "{n}");
        }
        let snap = BundleSnapshot::from_vfs(&vfs).unwrap();
        assert_eq!(snap.file_count(), 3);
        assert_eq!(snap.bundle_hash(), super::super::bundle_hash(&vfs).unwrap());
        assert_eq!(snap.read("dir/b.bin").unwrap(), vec![0u8, 1, 2, 255]);
    }

    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A fresh folder bundle under the system temp dir holding `files`
    /// (flat names). Removed by the returned guard.
    struct TempBundle(std::path::PathBuf);

    impl Drop for TempBundle {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn folder(files: &[(String, Vec<u8>)]) -> TempBundle {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pin_sim_snapshot_limits_{}_{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (n, b) in files {
            std::fs::write(dir.join(n), b).unwrap();
        }
        TempBundle(dir)
    }

    fn files_of(sizes: &[usize]) -> Vec<(String, Vec<u8>)> {
        sizes
            .iter()
            .enumerate()
            .map(|(i, &n)| (format!("f{i:03}.bin"), vec![(i % 251) as u8; n]))
            .collect()
    }

    fn limits(max_entries: usize, max_file: u64, max_total: u64) -> ZipLimits {
        ZipLimits {
            max_total_uncompressed: max_total,
            max_file,
            max_ratio: 100,
            max_entries,
        }
    }

    #[test]
    fn too_many_entries_is_refused_before_any_read() {
        let files = files_of(&[8, 8, 8, 8]);
        let dir = folder(&files);
        // The folder Vfs refuses every read (8 bytes > its max_file of 4):
        // reaching a read would give TooLarge, not TooManyEntries.
        let vfs = Vfs::open_with_limits(&dir.0, limits(10_000, 4, 1 << 20)).unwrap();
        let err =
            BundleSnapshot::from_vfs_with_limits(&vfs, &limits(3, 1 << 20, 1 << 20)).unwrap_err();
        assert!(
            matches!(err, SimError::Asset(AssetError::TooManyEntries(4))),
            "{err:?}"
        );
    }

    #[test]
    fn aggregate_over_the_limit_is_refused_before_any_read() {
        // Each file is under the per-file limit, the sum (32 * 128) is not.
        let files = files_of(&[128; 32]);
        let dir = folder(&files);
        let vfs = Vfs::open_with_limits(&dir.0, limits(10_000, 4, 1 << 20)).unwrap();
        let err =
            BundleSnapshot::from_vfs_with_limits(&vfs, &limits(10_000, 256, 512)).unwrap_err();
        assert!(
            matches!(err, SimError::Asset(AssetError::TotalTooLarge(n)) if n > 512 && n <= 512 + 128),
            "{err:?}"
        );
    }

    #[test]
    fn aggregate_over_the_limit_from_a_readable_folder_is_the_size_error() {
        let files = files_of(&[128; 32]);
        let dir = folder(&files);
        let vfs = Vfs::open(&dir.0).unwrap();
        let err =
            BundleSnapshot::from_vfs_with_limits(&vfs, &limits(10_000, 256, 512)).unwrap_err();
        assert!(
            matches!(err, SimError::Asset(AssetError::TotalTooLarge(_))),
            "{err:?}"
        );
    }

    #[test]
    fn a_file_over_the_per_file_limit_is_refused() {
        let files = files_of(&[8, 300, 8]);
        let dir = folder(&files);
        let vfs = Vfs::open(&dir.0).unwrap();
        let err =
            BundleSnapshot::from_vfs_with_limits(&vfs, &limits(10_000, 256, 1 << 20)).unwrap_err();
        assert!(
            matches!(&err, SimError::Asset(AssetError::TooLarge { path, size: 300 }) if path == "f001.bin"),
            "{err:?}"
        );
    }

    #[test]
    fn a_bundle_exactly_at_the_limits_is_captured() {
        let files = files_of(&[128; 4]);
        let dir = folder(&files);
        let vfs = Vfs::open(&dir.0).unwrap();
        let snap = BundleSnapshot::from_vfs_with_limits(&vfs, &limits(4, 128, 512)).unwrap();
        assert_eq!(snap.file_count(), 4);
        assert_eq!(snap.bundle_hash(), super::super::bundle_hash(&vfs).unwrap());
        assert_eq!(
            snap.bundle_hash(),
            BundleSnapshot::from_vfs(&vfs).unwrap().bundle_hash()
        );
        for (n, b) in &files {
            assert_eq!(&snap.read(n).unwrap(), b, "{n}");
        }
    }

    #[test]
    fn a_folder_bundle_within_the_default_limits_is_captured() {
        let files = files_of(&[0, 1, 1000, 17]);
        let dir = folder(&files);
        let snap = BundleSnapshot::capture(&dir.0).unwrap();
        assert_eq!(snap.file_count(), 4);
        let vfs = Vfs::open(&dir.0).unwrap();
        assert_eq!(snap.bundle_hash(), super::super::bundle_hash(&vfs).unwrap());
    }

    #[test]
    fn stored_zip_len_matches_the_written_archive_and_overflows_to_none() {
        let files = files_of(&[0, 5, 300]);
        let len = stored_zip_len(files.iter().map(|(n, d)| (n.len(), d.len())));
        assert_eq!(len, Some(stored_zip(&files).unwrap().len() as u64));
        assert_eq!(stored_zip_len([]), Some(EOCD_LEN));
        assert_eq!(stored_zip_len([(usize::MAX, 0)]), None);
        assert_eq!(stored_zip_len([(0, usize::MAX), (0, usize::MAX)]), None);
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(128))]
        /// Ok iff count <= max_entries, every size <= max_file and the sum
        /// <= max_total; otherwise the matching size error. Never panics.
        #[test]
        fn capture_succeeds_iff_within_the_limits(
            sizes in proptest::collection::vec(0usize..64, 0..12),
            max_entries in 0usize..12,
            max_file in 0u64..64,
            max_total in 0u64..400,
        ) {
            let files = files_of(&sizes);
            let vfs = Vfs::from_zip_bytes(stored_zip(&files).unwrap(), ZipLimits::default())
                .unwrap();
            let lim = limits(max_entries, max_file, max_total);
            let sum: u64 = sizes.iter().map(|&n| n as u64).sum();
            let within = sizes.len() <= max_entries
                && sizes.iter().all(|&n| n as u64 <= max_file)
                && sum <= max_total;
            match BundleSnapshot::from_vfs_with_limits(&vfs, &lim) {
                Ok(snap) => {
                    proptest::prop_assert!(within);
                    proptest::prop_assert_eq!(snap.file_count(), files.len());
                    proptest::prop_assert_eq!(
                        snap.bundle_hash(),
                        super::super::bundle_hash(&vfs).unwrap()
                    );
                }
                Err(err) => {
                    proptest::prop_assert!(!within, "{:?}", err);
                    proptest::prop_assert!(
                        matches!(
                            err,
                            SimError::Asset(
                                AssetError::TooManyEntries(_)
                                    | AssetError::TooLarge { .. }
                                    | AssetError::TotalTooLarge(_)
                            )
                        ),
                        "{:?}",
                        err
                    );
                }
            }
        }
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(64))]
        #[test]
        fn any_file_set_round_trips(
            contents in proptest::collection::vec(
                proptest::collection::vec(proptest::prelude::any::<u8>(), 0..512),
                0..8,
            )
        ) {
            let files: Vec<(String, Vec<u8>)> = contents
                .into_iter()
                .enumerate()
                .map(|(i, b)| (format!("f{i:02}.bin"), b))
                .collect();
            let vfs = Vfs::from_zip_bytes(stored_zip(&files).unwrap(), ZipLimits::default())
                .unwrap();
            let snap = BundleSnapshot::from_vfs(&vfs).unwrap();
            proptest::prop_assert_eq!(snap.file_count(), files.len());
            for (n, b) in &files {
                proptest::prop_assert_eq!(&snap.read(n).unwrap(), b);
            }
        }
    }
}
