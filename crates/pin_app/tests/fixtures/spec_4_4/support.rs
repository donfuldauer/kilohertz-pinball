//! Shared helpers for the Subphase 4.4 spec tests (Independent Tester).
//!
//! `base/` is a complete, valid folder bundle (PLAN Appendix A manifests, the
//! same mock table `pin_assets` validates in its own tests, plus two small
//! GLB files). The helpers copy it into fresh temp directories and derive
//! valid, renamed and broken variants from it.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A temp directory removed on drop.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("pin_app_spec44_{tag}_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn join(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn base_bundle() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/spec_4_4/base")
}

pub fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let p = e.path();
        let dst = to.join(e.file_name());
        if p.is_dir() {
            copy_dir(&p, &dst);
        } else {
            std::fs::copy(&p, &dst).unwrap();
        }
    }
}

/// Writes a valid bundle folder at `dir` whose `[meta]` id / name / author
/// are the given ones.
pub fn write_valid_bundle(dir: &Path, id: &str, name: &str, author: &str) {
    copy_dir(&base_bundle(), dir);
    let manifest = dir.join("table.toml");
    let text = std::fs::read_to_string(&manifest).unwrap();
    let text = text
        .replace("id = \"apex_circuit\"", &format!("id = \"{id}\""))
        .replace("name = \"Apex Circuit\"", &format!("name = \"{name}\""))
        .replace("author = \"Reference\"", &format!("author = \"{author}\""));
    assert!(text.contains(&format!("name = \"{name}\"")));
    std::fs::write(&manifest, text).unwrap();
}

/// A bundle whose `table.toml` is not TOML at all.
pub fn write_garbage_manifest_bundle(dir: &Path) {
    copy_dir(&base_bundle(), dir);
    std::fs::write(dir.join("table.toml"), b"this is [not toml = \x00\xff\xfe").unwrap();
}

/// A bundle whose `table.toml` parses but fails validation (schema version
/// from the future).
pub fn write_bad_schema_bundle(dir: &Path) {
    copy_dir(&base_bundle(), dir);
    let manifest = dir.join("table.toml");
    let text = std::fs::read_to_string(&manifest).unwrap();
    let text = text.replace("schema_version = 1", "schema_version = 9999");
    std::fs::write(&manifest, text).unwrap();
}

/// A bundle with `table.toml` but without `backglass.toml`.
pub fn write_missing_backglass_bundle(dir: &Path) {
    copy_dir(&base_bundle(), dir);
    std::fs::remove_file(dir.join("backglass.toml")).unwrap();
}

/// A bundle whose manifest references a script file that does not exist.
pub fn write_missing_script_bundle(dir: &Path) {
    copy_dir(&base_bundle(), dir);
    std::fs::remove_file(dir.join("rules.lua")).unwrap();
}

/// A file with an archive extension that is not a zip.
pub fn write_fake_archive(path: &Path) {
    std::fs::write(path, b"PK\x03\x04 definitely not a zip archive \xff\xff\xff\xff").unwrap();
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            walk(root, &p, out);
        } else {
            let rel = p
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, std::fs::read(&p).unwrap()));
        }
    }
}

/// Packs a bundle folder into a stored (uncompressed) zip archive at `out`.
pub fn zip_dir(dir: &Path, out: &Path) {
    let mut files = Vec::new();
    walk(dir, dir, &mut files);
    files.sort();
    let mut data = Vec::new();
    let mut central = Vec::new();
    for (name, bytes) in &files {
        let offset = data.len() as u32;
        let crc = crc32(bytes);
        let size = bytes.len() as u32;
        let header = |sig: u32, central_dir: bool, out: &mut Vec<u8>| {
            out.extend(sig.to_le_bytes());
            if central_dir {
                out.extend(((3u16 << 8) | 20).to_le_bytes()); // made by: Unix, 2.0
            }
            out.extend(20u16.to_le_bytes()); // version needed
            out.extend(0x0800u16.to_le_bytes()); // UTF-8 names
            out.extend(0u16.to_le_bytes()); // stored
            out.extend(0u16.to_le_bytes()); // time
            out.extend(0x21u16.to_le_bytes()); // date 1980-01-01
            out.extend(crc.to_le_bytes());
            out.extend(size.to_le_bytes());
            out.extend(size.to_le_bytes());
            out.extend((name.len() as u16).to_le_bytes());
            out.extend(0u16.to_le_bytes()); // extra len
        };
        header(0x0403_4b50, false, &mut data);
        data.extend(name.as_bytes());
        data.extend(bytes);
        header(0x0201_4b50, true, &mut central);
        central.extend(0u16.to_le_bytes()); // comment len
        central.extend(0u16.to_le_bytes()); // disk
        central.extend(0u16.to_le_bytes()); // internal attrs
        central.extend((0o100644u32 << 16).to_le_bytes());
        central.extend(offset.to_le_bytes());
        central.extend(name.as_bytes());
    }
    let cd_offset = data.len() as u32;
    let cd_size = central.len() as u32;
    data.extend(central);
    data.extend(0x0605_4b50u32.to_le_bytes());
    data.extend(0u16.to_le_bytes());
    data.extend(0u16.to_le_bytes());
    data.extend((files.len() as u16).to_le_bytes());
    data.extend((files.len() as u16).to_le_bytes());
    data.extend(cd_size.to_le_bytes());
    data.extend(cd_offset.to_le_bytes());
    data.extend(0u16.to_le_bytes());
    std::fs::write(out, data).unwrap();
}

/// File names directly inside `dir`, sorted.
pub fn dir_names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}
