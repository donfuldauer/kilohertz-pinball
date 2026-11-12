//! Shared helpers for the pin_assets implementer tests: a raw zip writer that
//! can produce hostile archives, a GLB builder, and the mock bundle.
#![allow(dead_code)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

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

/// One raw (stored) zip entry with every hostile knob exposed.
#[derive(Clone)]
pub struct RawEntry {
    pub name: Vec<u8>,
    pub data: Vec<u8>,
    pub flags: u16,
    /// Upper byte of "version made by" (3 = Unix).
    pub system: u8,
    pub external_attrs: u32,
    pub declared_size: Option<u32>,
    pub declared_compressed: Option<u32>,
}

impl RawEntry {
    pub fn file(name: &str, data: &[u8]) -> RawEntry {
        RawEntry {
            name: name.as_bytes().to_vec(),
            data: data.to_vec(),
            flags: 0x0800, // UTF-8 names
            system: 3,
            external_attrs: 0o100644 << 16,
            declared_size: None,
            declared_compressed: None,
        }
    }

    pub fn symlink(name: &str, target: &str) -> RawEntry {
        RawEntry {
            external_attrs: 0o120777 << 16,
            ..RawEntry::file(name, target.as_bytes())
        }
    }
}

/// Writes a stored zip archive exactly as described (no sanitising).
pub fn raw_zip(entries: &[RawEntry]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for e in entries {
        let offset = out.len() as u32;
        let crc = crc32(&e.data);
        let size = e.declared_size.unwrap_or(e.data.len() as u32);
        let csize = e.declared_compressed.unwrap_or(e.data.len() as u32);
        // Local header.
        out.extend(0x0403_4b50u32.to_le_bytes());
        out.extend(20u16.to_le_bytes());
        out.extend(e.flags.to_le_bytes());
        out.extend(0u16.to_le_bytes()); // stored
        out.extend(0u32.to_le_bytes()); // time/date
        out.extend(crc.to_le_bytes());
        out.extend(csize.to_le_bytes());
        out.extend(size.to_le_bytes());
        out.extend((e.name.len() as u16).to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(&e.name);
        out.extend(&e.data);
        // Central record.
        central.extend(0x0201_4b50u32.to_le_bytes());
        central.extend((u16::from(e.system) << 8 | 20).to_le_bytes());
        central.extend(20u16.to_le_bytes());
        central.extend(e.flags.to_le_bytes());
        central.extend(0u16.to_le_bytes());
        central.extend(0u32.to_le_bytes());
        central.extend(crc.to_le_bytes());
        central.extend(csize.to_le_bytes());
        central.extend(size.to_le_bytes());
        central.extend((e.name.len() as u16).to_le_bytes());
        central.extend(0u16.to_le_bytes()); // extra
        central.extend(0u16.to_le_bytes()); // comment
        central.extend(0u16.to_le_bytes()); // disk
        central.extend(0u16.to_le_bytes()); // internal attrs
        central.extend(e.external_attrs.to_le_bytes());
        central.extend(offset.to_le_bytes());
        central.extend(&e.name);
    }
    let cd_offset = out.len() as u32;
    out.extend(&central);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend((central.len() as u32).to_le_bytes());
    out.extend(cd_offset.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

/// A stored zip of `(name, bytes)` files.
pub fn stored_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let entries: Vec<RawEntry> = files.iter().map(|(n, d)| RawEntry::file(n, d)).collect();
    raw_zip(&entries)
}

/// A deflated zip written with the `zip` crate.
pub fn deflated_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in files {
        w.start_file(*name, opts).unwrap();
        w.write_all(data).unwrap();
    }
    w.finish().unwrap().into_inner()
}

/// Wraps JSON and a BIN chunk into a GLB container.
pub fn glb(json: &serde_json::Value, bin: &[u8]) -> Vec<u8> {
    let mut j = serde_json::to_vec(json).unwrap();
    while j.len() % 4 != 0 {
        j.push(b' ');
    }
    let mut b = bin.to_vec();
    while b.len() % 4 != 0 {
        b.push(0);
    }
    let total = 12 + 8 + j.len() + if b.is_empty() { 0 } else { 8 + b.len() };
    let mut out = Vec::with_capacity(total);
    out.extend(b"glTF");
    out.extend(2u32.to_le_bytes());
    out.extend((total as u32).to_le_bytes());
    out.extend((j.len() as u32).to_le_bytes());
    out.extend(0x4E4F_534Au32.to_le_bytes());
    out.extend(&j);
    if !b.is_empty() {
        out.extend((b.len() as u32).to_le_bytes());
        out.extend(0x004E_4942u32.to_le_bytes());
        out.extend(&b);
    }
    out
}

pub fn f32_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// A 2×1 RGBA PNG: one red and one green pixel (stored zlib, valid CRCs).
pub fn tiny_png() -> Vec<u8> {
    fn chunk(out: &mut Vec<u8>, ty: &[u8], data: &[u8]) {
        out.extend((data.len() as u32).to_be_bytes());
        let mut c = ty.to_vec();
        c.extend(data);
        out.extend(&c);
        out.extend(crc32(&c).to_be_bytes());
    }
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend(2u32.to_be_bytes());
    ihdr.extend(1u32.to_be_bytes());
    ihdr.extend([8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    let raw = [0u8, 255, 0, 0, 255, 0, 255, 0, 255];
    // zlib stored block.
    let mut z = vec![0x78, 0x01, 0x01];
    z.extend((raw.len() as u16).to_le_bytes());
    z.extend((!(raw.len() as u16)).to_le_bytes());
    z.extend(raw);
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + u32::from(x)) % 65521;
        b = (b + a) % 65521;
    }
    z.extend(((b << 16) | a).to_be_bytes());
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// The test playfield: one triangle mesh used by two nodes with extras, a
/// parent/child transform, a material with factors and an embedded PNG.
pub fn playfield_glb() -> Vec<u8> {
    let positions = [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
    let mut bin = f32_bytes(&positions);
    let idx_off = bin.len();
    bin.extend([0u16, 1, 2].iter().flat_map(|i| i.to_le_bytes()));
    bin.extend([0, 0]); // pad to 4
    let png_off = bin.len();
    let png = tiny_png();
    bin.extend(&png);
    let json = serde_json::json!({
        "asset": {"version": "2.0"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [
            {"name": "ramp", "mesh": 0, "translation": [1.0, 2.0, 3.0],
             "children": [1],
             "extras": {"collision_layer": 2, "collider": true, "lamp": "overdrive_ready"}},
            {"name": "orbit_opto", "mesh": 0, "scale": [2.0, 2.0, 2.0],
             "extras": {"switch": "right_orbit_opto", "unknown_key": 5}}
        ],
        "meshes": [{"name": "tri", "primitives": [{
            "attributes": {"POSITION": 0}, "indices": 1, "material": 0}],
            "extras": {"collision_layer": 5, "switch": "mesh_level"}}],
        "materials": [{"name": "chrome",
            "pbrMetallicRoughness": {"baseColorFactor": [0.5, 0.25, 1.0, 1.0],
                "metallicFactor": 0.75, "roughnessFactor": 0.125,
                "baseColorTexture": {"index": 0}},
            "emissiveFactor": [0.1, 0.2, 0.3]}],
        "textures": [{"source": 0}],
        "images": [{"bufferView": 2, "mimeType": "image/png"}],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3",
             "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 0.0]},
            {"bufferView": 1, "componentType": 5123, "count": 3, "type": "SCALAR"}
        ],
        "bufferViews": [
            {"buffer": 0, "byteOffset": 0, "byteLength": 36},
            {"buffer": 0, "byteOffset": idx_off, "byteLength": 6},
            {"buffer": 0, "byteOffset": png_off, "byteLength": png.len()}
        ],
        "buffers": [{"byteLength": bin.len()}]
    });
    glb(&json, &bin)
}

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A fresh empty directory under the system temp dir.
pub fn temp_dir(tag: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "pin_assets_{tag}_{}_{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/apex")
}

/// Every file of the mock bundle: the committed fixtures plus the generated
/// `playfield.glb` and `backbox.glb`. Sorted by path.
pub fn bundle_files() -> Vec<(String, Vec<u8>)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                out.push((rel, std::fs::read(&p).unwrap()));
            }
        }
    }
    let root = fixture_dir();
    let mut out = Vec::new();
    walk(&root, &root, &mut out);
    out.push(("playfield.glb".into(), playfield_glb()));
    out.push(("backbox.glb".into(), playfield_glb()));
    out.sort();
    out
}

/// Replaces a file's contents in a bundle file list.
pub fn with_file(files: &[(String, Vec<u8>)], name: &str, data: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<_> = files.iter().filter(|(n, _)| n != name).cloned().collect();
    out.push((name.to_owned(), data.to_vec()));
    out.sort();
    out
}

/// Writes the bundle into `dir`.
pub fn write_dir(dir: &Path, files: &[(String, Vec<u8>)]) {
    for (name, data) in files {
        let p = dir.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, data).unwrap();
    }
}

pub fn zip_of(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let refs: Vec<(&str, &[u8])> = files.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect();
    deflated_zip(&refs)
}

pub fn table_text() -> String {
    std::fs::read_to_string(fixture_dir().join("table.toml")).unwrap()
}

pub fn backglass_text() -> String {
    std::fs::read_to_string(fixture_dir().join("backglass.toml")).unwrap()
}
