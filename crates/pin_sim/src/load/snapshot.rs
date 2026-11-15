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

/// Writes `files` (name, bytes) as a stored zip archive.
fn stored_zip(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>, SimError> {
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
    /// # Errors
    /// As [`capture`](Self::capture).
    pub fn from_vfs(vfs: &Vfs) -> Result<BundleSnapshot, SimError> {
        let mut names = vfs.list();
        names.sort();
        let mut files = Vec::with_capacity(names.len());
        for name in names {
            let bytes = vfs.read(&name)?;
            files.push((name, bytes));
        }
        let count = files.len();
        let archive = stored_zip(&files)?;
        drop(files);
        let vfs = Vfs::from_zip_bytes(archive, ZipLimits::default())?;
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
