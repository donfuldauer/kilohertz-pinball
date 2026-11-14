//! Shared helpers for the 4.4 implementer tests (`ui_*.rs`): mock table
//! bundles built from the pin_assets reference fixture, a stored-zip writer,
//! PNG encoding and fresh temporary folders. Included by the other `ui_*`
//! tests with `#[path = "ui_fixtures.rs"] mod fixtures;`; built on its own it
//! only checks that the base bundle validates.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The pin_assets reference bundle (`apex_circuit`).
pub fn apex_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../pin_assets/tests/fixtures/apex")
}

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A fresh empty folder under the cargo test temp dir.
pub fn temp_dir(tag: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("ui_4_4_{tag}_{}_{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn crc32(data: &[u8]) -> u32 {
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

/// A stored (uncompressed) zip of `(name, bytes)` files.
pub fn stored_zip(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let len = data.len() as u32;
        out.extend(0x0403_4b50u32.to_le_bytes());
        out.extend(20u16.to_le_bytes());
        out.extend(0x0800u16.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(0u32.to_le_bytes());
        out.extend(crc.to_le_bytes());
        out.extend(len.to_le_bytes());
        out.extend(len.to_le_bytes());
        out.extend((name.len() as u16).to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(name.as_bytes());
        out.extend(data);
        central.extend(0x0201_4b50u32.to_le_bytes());
        central.extend((3u16 << 8 | 20).to_le_bytes());
        central.extend(20u16.to_le_bytes());
        central.extend(0x0800u16.to_le_bytes());
        central.extend(0u16.to_le_bytes());
        central.extend(0u32.to_le_bytes());
        central.extend(crc.to_le_bytes());
        central.extend(len.to_le_bytes());
        central.extend(len.to_le_bytes());
        central.extend((name.len() as u16).to_le_bytes());
        central.extend([0u8; 8]);
        central.extend((0o100644u32 << 16).to_le_bytes());
        central.extend(offset.to_le_bytes());
        central.extend(name.as_bytes());
    }
    let cd_offset = out.len() as u32;
    out.extend(&central);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0u8; 4]);
    out.extend((files.len() as u16).to_le_bytes());
    out.extend((files.len() as u16).to_le_bytes());
    out.extend((central.len() as u32).to_le_bytes());
    out.extend(cd_offset.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

/// Wraps glTF JSON and a BIN chunk into a GLB.
pub fn glb(json: &str, bin: &[u8]) -> Vec<u8> {
    let mut j = json.as_bytes().to_vec();
    while !j.len().is_multiple_of(4) {
        j.push(b' ');
    }
    let mut b = bin.to_vec();
    while !b.len().is_multiple_of(4) {
        b.push(0);
    }
    let total = 12 + 8 + j.len() + 8 + b.len();
    let mut out = Vec::with_capacity(total);
    out.extend(b"glTF");
    out.extend(2u32.to_le_bytes());
    out.extend((total as u32).to_le_bytes());
    out.extend((j.len() as u32).to_le_bytes());
    out.extend(0x4E4F_534Au32.to_le_bytes());
    out.extend(&j);
    out.extend((b.len() as u32).to_le_bytes());
    out.extend(0x004E_4942u32.to_le_bytes());
    out.extend(&b);
    out
}

/// A one-triangle GLB (playfield / backbox mesh stand-in).
pub fn triangle_glb() -> Vec<u8> {
    let mut bin: Vec<u8> = [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]
        .iter()
        .flat_map(|f| f.to_le_bytes())
        .collect();
    bin.extend([0u16, 1, 2].iter().flat_map(|i| i.to_le_bytes()));
    bin.extend([0, 0]);
    let json = r#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],
"nodes":[{"name":"tri","mesh":0}],
"meshes":[{"name":"tri","primitives":[{"attributes":{"POSITION":0},"indices":1}]}],
"accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0.0,0.0,0.0],"max":[1.0,1.0,0.0]},
{"bufferView":1,"componentType":5123,"count":3,"type":"SCALAR"}],
"bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":36},{"buffer":0,"byteOffset":36,"byteLength":6}],
"buffers":[{"byteLength":44}]}"#;
    glb(json, &bin)
}

/// PNG bytes of a `w`×`h` RGBA image filled by `f(x, y)`.
pub fn png(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba(f(x, y)));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

/// Every file of the reference bundle plus generated meshes, sorted by path.
pub fn bundle_files() -> Vec<(String, Vec<u8>)> {
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
    let root = apex_dir();
    let mut out = Vec::new();
    walk(&root, &root, &mut out);
    out.push(("playfield.glb".into(), triangle_glb()));
    out.push(("backbox.glb".into(), triangle_glb()));
    out.sort();
    out
}

/// Replaces (or adds) a file.
pub fn with_file(files: &[(String, Vec<u8>)], name: &str, data: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<_> = files.iter().filter(|(n, _)| n != name).cloned().collect();
    out.push((name.to_owned(), data.to_vec()));
    out.sort();
    out
}

/// Replaces text in `table.toml` (the text must be present).
pub fn edit_table(files: &[(String, Vec<u8>)], from: &str, to: &str) -> Vec<(String, Vec<u8>)> {
    let t = files
        .iter()
        .find(|(n, _)| n == "table.toml")
        .map(|(_, d)| String::from_utf8(d.clone()).unwrap())
        .unwrap();
    assert!(t.contains(from), "table.toml lacks {from:?}");
    with_file(files, "table.toml", t.replacen(from, to, 1).as_bytes())
}

/// Writes the files into `dir`.
pub fn write_dir(dir: &Path, files: &[(String, Vec<u8>)]) {
    for (name, data) in files {
        let p = dir.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, data).unwrap();
    }
}

#[test]
fn reference_bundle_validates() {
    let dir = temp_dir("fixture");
    write_dir(&dir, &bundle_files());
    let status = pin_app::ui::inspect_bundle(&dir);
    assert!(
        matches!(status, pin_app::ui::TableStatus::Ready(_)),
        "{status:?}"
    );
}
