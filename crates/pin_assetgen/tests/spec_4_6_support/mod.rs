//! Shared helpers for the Subphase 4.6 Independent Tester spec tests
//! (black box: written from the brief's FIXED PUBLIC API only).
#![allow(dead_code, unreachable_patterns, clippy::needless_range_loop)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use pin_assetgen::{AssetKind, KitManifest, TableMeta, generate_kit};

/// −40 dBFS.
pub const RMS_FLOOR: f64 = 0.01;
/// −0.1 dBFS as the brief rounds it ("≈ 0.989").
pub const PEAK_CEIL: f64 = 0.989;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A fresh, empty directory under the cargo test tmp dir.
pub fn tmp(name: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("spec_4_6")
        .join(format!("{name}_{}_{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn meta(title: &str, palette: &[[u8; 3]], seed: u64) -> TableMeta {
    TableMeta {
        title: title.to_owned(),
        palette: palette.to_vec(),
        seed,
    }
}

/// Appendix A example metadata: "Apex Circuit", palette #0b1d3a #f2a900 #e63946.
pub const APEX_PALETTE: [[u8; 3]; 3] = [[0x0b, 0x1d, 0x3a], [0xf2, 0xa9, 0x00], [0xe6, 0x39, 0x46]];

pub fn apex_meta() -> TableMeta {
    meta("Apex Circuit", &APEX_PALETTE, 1)
}

pub fn generate(m: &TableMeta, name: &str) -> (PathBuf, KitManifest) {
    let dir = tmp(name);
    let man = generate_kit(m, &dir).unwrap_or_else(|e| panic!("generate_kit failed: {e}"));
    (dir, man)
}

/// Every file under `dir`: bundle-relative '/' path → bytes.
pub fn read_tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
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
                out.insert(rel, std::fs::read(&p).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

pub fn kind_name(k: &AssetKind) -> &'static str {
    match k {
        AssetKind::Sound => "Sound",
        AssetKind::Music => "Music",
        AssetKind::Motif => "Motif",
        AssetKind::Font => "Font",
        AssetKind::Image => "Image",
        _ => "Other",
    }
}

pub fn is_audio(k: &AssetKind) -> bool {
    matches!(k, AssetKind::Sound | AssetKind::Music | AssetKind::Motif)
}

/// id → (path, kind name, hash), asserting ids are unique.
pub fn index(man: &KitManifest) -> BTreeMap<String, (String, &'static str, String)> {
    let mut out = BTreeMap::new();
    for e in &man.entries {
        let prev = out.insert(
            e.id.clone(),
            (e.path.clone(), kind_name(&e.kind), e.sha256_or_hash.clone()),
        );
        assert!(prev.is_none(), "duplicate manifest id {:?}", e.id);
    }
    out
}

pub fn file_bytes(dir: &Path, rel: &str) -> Vec<u8> {
    std::fs::read(dir.join(rel)).unwrap_or_else(|e| panic!("manifest path {rel:?}: {e}"))
}

// ---------------------------------------------------------------- audio

/// (format tag, channels, sample rate, bits per sample) of a RIFF/WAVE file.
pub fn wav_info(bytes: &[u8]) -> (u16, u16, u32, u16) {
    assert!(
        bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE",
        "not a RIFF/WAVE file"
    );
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        if id == b"fmt " {
            let b = &bytes[pos + 8..pos + 8 + len];
            return (
                u16::from_le_bytes([b[0], b[1]]),
                u16::from_le_bytes([b[2], b[3]]),
                u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
                u16::from_le_bytes([b[14], b[15]]),
            );
        }
        pos += 8 + len + (len & 1);
    }
    panic!("WAV has no fmt chunk");
}

/// Decodes with pin_audio's decoder at 48 kHz: (left, right).
pub fn decode48(bytes: &[u8]) -> (Vec<f32>, Vec<f32>) {
    let s = pin_audio::mixer::decode(bytes, 48_000)
        .unwrap_or_else(|e| panic!("pin_audio decode failed: {e:?}"));
    let d = s.data();
    assert_eq!(d.len(), 2 * s.frames());
    (
        d.iter().step_by(2).copied().collect(),
        d.iter().skip(1).step_by(2).copied().collect(),
    )
}

pub fn rms(s: &[f32]) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    (s.iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() / s.len() as f64).sqrt()
}

pub fn peak(s: &[f32]) -> f64 {
    s.iter().fold(0.0f64, |m, &v| m.max(f64::from(v).abs()))
}

/// Largest |s[i+1] - s[i]| inside the buffer.
pub fn max_step(s: &[f32]) -> f64 {
    s.windows(2).fold(0.0f64, |m, w| {
        m.max((f64::from(w[1]) - f64::from(w[0])).abs())
    })
}

/// |first - last|: the jump a looping player makes at the wrap point.
pub fn seam(s: &[f32]) -> f64 {
    (f64::from(s[0]) - f64::from(*s.last().unwrap())).abs()
}

// ---------------------------------------------------------------- images

pub const PNG_SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// Loads a PNG through pin_assets' image path (a glTF whose base colour
/// texture is the external image, resolved through the VFS).
pub fn load_png_via_assets(png: &[u8], name: &str) -> pin_assets::ImageData {
    let dir = tmp(name);
    let json = r#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],
"nodes":[{"name":"n","mesh":0}],
"meshes":[{"name":"n","primitives":[{"attributes":{"POSITION":0},"material":0}]}],
"materials":[{"name":"m","pbrMetallicRoughness":{"baseColorTexture":{"index":0}}}],
"textures":[{"source":0}],
"images":[{"uri":"img.png"}],
"accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0.0,0.0,0.0],"max":[0.25,0.5,0.125]}],
"bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":36}],
"buffers":[{"byteLength":36,"uri":"tri.bin"}]}"#;
    let tri: [f32; 9] = [0.0, 0.0, 0.0, 0.25, 0.0, 0.0, 0.0, 0.5, 0.125];
    let mut bin = Vec::new();
    for v in tri {
        bin.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(dir.join("scene.gltf"), json).unwrap();
    std::fs::write(dir.join("tri.bin"), bin).unwrap();
    std::fs::write(dir.join("img.png"), png).unwrap();
    let vfs = pin_assets::Vfs::open(&dir).unwrap();
    let scene = pin_assets::load_gltf(&vfs, "scene.gltf")
        .unwrap_or_else(|e| panic!("pin_assets failed to load the PNG: {e}"));
    assert_eq!(scene.images.len(), 1);
    let img = scene.images.into_iter().next().unwrap();
    let _ = std::fs::remove_dir_all(dir);
    img
}

// ---------------------------------------------------------------- sha256

pub fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(chunk[4 * i..4 * i + 4].try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7]
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [
                t1.wrapping_add(t2),
                v[0],
                v[1],
                v[2],
                v[3].wrapping_add(t1),
                v[4],
                v[5],
                v[6],
            ];
        }
        for i in 0..8 {
            h[i] = h[i].wrapping_add(v[i]);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}
