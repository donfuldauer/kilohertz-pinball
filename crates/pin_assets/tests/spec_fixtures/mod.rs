//! Shared helpers for the Subphase 4.1 Independent Tester spec tests.
//!
//! Everything here is built from the brief alone: a complete, valid mock bundle
//! (Appendix A manifests with the undeclared names of the example filled in),
//! a byte-exact raw zip writer (so hostile central directories can be forged),
//! a `.glb` builder, a 1x1 PNG encoder and a temp-dir guard.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use pin_assets::{AssetError, ValidationError, Vfs, ZipLimits};

// ---------------------------------------------------------------------------
// Temp directories (no wall-clock, no /tmp literal).
// ---------------------------------------------------------------------------

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A unique directory under the OS temp dir, removed on drop.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!(
            "pin_assets_spec_{}_{}_{}",
            std::process::id(),
            tag,
            n
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("create temp dir");
        TempDir(p)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes `files` ('/'-separated relative names) below `root`.
pub fn write_tree(root: &Path, files: &BTreeMap<String, Vec<u8>>) {
    for (name, data) in files {
        let mut p = root.to_path_buf();
        for part in name.split('/') {
            p.push(part);
        }
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, data).unwrap();
    }
}

// ---------------------------------------------------------------------------
// Checksums and tiny encoders.
// ---------------------------------------------------------------------------

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            let mask = (!(crc & 1)).wrapping_add(1);
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + u32::from(x)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// A valid 1x1 RGBA PNG (zlib stored block).
pub fn png_1x1(rgba: [u8; 4]) -> Vec<u8> {
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        let mut c = kind.to_vec();
        c.extend_from_slice(body);
        out.extend_from_slice(&c);
        out.extend_from_slice(&crc32(&c).to_be_bytes());
    }
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&1u32.to_be_bytes());
    ihdr.extend_from_slice(&1u32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    let raw = [0u8, rgba[0], rgba[1], rgba[2], rgba[3]];
    let mut z = vec![0x78, 0x01, 0x01];
    z.extend_from_slice(&(raw.len() as u16).to_le_bytes());
    z.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
    z.extend_from_slice(&raw);
    z.extend_from_slice(&adler32(&raw).to_be_bytes());
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        s.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    s
}

// ---------------------------------------------------------------------------
// Zip writers.
// ---------------------------------------------------------------------------

/// One entry of a hand-written (stored, method 0) zip archive.
#[derive(Clone)]
pub struct RawEntry {
    pub name: Vec<u8>,
    pub data: Vec<u8>,
    /// General-purpose bit flags (bit 0 = encrypted, bit 11 = UTF-8 name).
    pub flags: u16,
    /// External file attributes (unix mode in the high 16 bits).
    pub external_attrs: u32,
    /// "Version made by" (high byte 3 = unix).
    pub made_by: u16,
}

impl RawEntry {
    pub fn file(name: &str, data: &[u8]) -> RawEntry {
        RawEntry {
            name: name.as_bytes().to_vec(),
            data: data.to_vec(),
            flags: 0x0800,
            external_attrs: 0o100644 << 16,
            made_by: (3 << 8) | 20,
        }
    }

    pub fn symlink(name: &str, target: &str) -> RawEntry {
        RawEntry {
            external_attrs: 0o120777 << 16,
            ..RawEntry::file(name, target.as_bytes())
        }
    }

    pub fn encrypted(name: &str, data: &[u8]) -> RawEntry {
        RawEntry {
            flags: 0x0801,
            ..RawEntry::file(name, data)
        }
    }
}

/// Writes a stored-only zip with exactly the given names, flags and attributes.
pub fn raw_zip(entries: &[RawEntry]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for e in entries {
        let offset = out.len() as u32;
        let crc = crc32(&e.data);
        let size = e.data.len() as u32;
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&e.flags.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // stored
        out.extend_from_slice(&0u16.to_le_bytes()); // time
        out.extend_from_slice(&0x0021u16.to_le_bytes()); // date 1980-01-01
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&e.name);
        out.extend_from_slice(&e.data);

        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&e.made_by.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&e.flags.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0x0021u16.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra
        central.extend_from_slice(&0u16.to_le_bytes()); // comment
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        central.extend_from_slice(&e.external_attrs.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(&e.name);
    }
    let cd_offset = out.len() as u32;
    let cd_size = central.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// Stored zip of plain files.
pub fn stored_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let entries: Vec<RawEntry> = files.iter().map(|(n, d)| RawEntry::file(n, d)).collect();
    raw_zip(&entries)
}

/// Deflate-compressed zip written by the `zip` crate (what a table author's tool makes).
pub fn deflated_zip<'a, I>(files: I) -> Vec<u8>
where
    I: IntoIterator<Item = (&'a str, &'a [u8])>,
{
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in files {
        w.start_file(name, opts).expect("start_file");
        w.write_all(data).expect("write");
    }
    w.finish().expect("finish").into_inner()
}

pub fn zip_of(files: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    deflated_zip(files.iter().map(|(n, d)| (n.as_str(), d.as_slice())))
}

/// Limits far above anything the tests build, so only the cap under test trips.
pub fn roomy() -> ZipLimits {
    ZipLimits {
        max_total_uncompressed: 1 << 30,
        max_file: 1 << 30,
        max_ratio: 1_000_000,
        max_entries: 1_000_000,
    }
}

pub fn vfs_from(files: &BTreeMap<String, Vec<u8>>) -> Vfs {
    Vfs::from_zip_bytes(zip_of(files), ZipLimits::default()).expect("valid bundle zip opens")
}

// ---------------------------------------------------------------------------
// glTF builders.
// ---------------------------------------------------------------------------

/// Wraps a JSON chunk and a BIN chunk into a binary glTF 2.0 container.
pub fn glb(json: &str, bin: &[u8]) -> Vec<u8> {
    let mut j = json.as_bytes().to_vec();
    while !j.len().is_multiple_of(4) {
        j.push(b' ');
    }
    let mut b = bin.to_vec();
    while !b.len().is_multiple_of(4) {
        b.push(0);
    }
    let total = 12 + 8 + j.len() + if b.is_empty() { 0 } else { 8 + b.len() };
    let mut out = Vec::new();
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(j.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&j);
    if !b.is_empty() {
        out.extend_from_slice(&(b.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(&b);
    }
    out
}

pub const TRI_POS: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [0.25, 0.0, 0.0], [0.0, 0.5, 0.125]];
pub const TRI_NRM: [[f32; 3]; 3] = [[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]];
pub const TRI_UV: [[f32; 2]; 3] = [[0.0, 0.0], [1.0, 0.0], [0.25, 0.75]];
pub const TRI_IDX: [u16; 3] = [0, 2, 1];
pub const QUAD_POS: [[f32; 3]; 4] = [
    [-1.0, -1.0, 0.0],
    [1.0, -1.0, 0.0],
    [1.0, 1.0, 0.5],
    [-1.0, 1.0, 0.5],
];
pub const QUAD_IDX: [u32; 6] = [0, 1, 2, 0, 2, 3];

/// The binary buffer of the standard test scene, and the offset/length of its
/// two embedded PNG images.
pub fn scene_bin() -> (Vec<u8>, [(usize, usize); 2]) {
    let mut b = Vec::new();
    for p in TRI_POS {
        for c in p {
            b.extend_from_slice(&c.to_le_bytes());
        }
    }
    for p in TRI_NRM {
        for c in p {
            b.extend_from_slice(&c.to_le_bytes());
        }
    }
    for p in TRI_UV {
        for c in p {
            b.extend_from_slice(&c.to_le_bytes());
        }
    }
    for i in TRI_IDX {
        b.extend_from_slice(&i.to_le_bytes());
    }
    b.extend_from_slice(&[0, 0]);
    assert_eq!(b.len(), 104);
    for p in QUAD_POS {
        for c in p {
            b.extend_from_slice(&c.to_le_bytes());
        }
    }
    for i in QUAD_IDX {
        b.extend_from_slice(&i.to_le_bytes());
    }
    assert_eq!(b.len(), 176);
    let mut imgs = [(0, 0); 2];
    for (k, rgba) in [[255, 0, 0, 255], [0, 255, 0, 255]].into_iter().enumerate() {
        let png = png_1x1(rgba);
        imgs[k] = (b.len(), png.len());
        b.extend_from_slice(&png);
        while !b.len().is_multiple_of(4) {
            b.push(0);
        }
    }
    (b, imgs)
}

/// How the standard scene refers to its buffer.
pub enum BufferRef<'a> {
    /// GLB-embedded BIN chunk (no uri).
    Embedded,
    /// `"uri": "<this>"`.
    Uri(&'a str),
}

/// JSON of the standard test scene.
///
/// Nodes: `deck` (root, translation, node extras that conflict with its mesh
/// extras), `insert` (child of deck, translation + scale, node extras),
/// `post` (root, rotation + translation, extras only on its mesh).
/// `deck_extras` / `insert_extras` are JSON object texts for the two node extras.
pub fn scene_json(buffer: BufferRef<'_>, deck_extras: &str, insert_extras: &str) -> String {
    let (bin, imgs) = scene_bin();
    let buf = match buffer {
        BufferRef::Embedded => format!(r#"{{"byteLength":{}}}"#, bin.len()),
        BufferRef::Uri(u) => format!(r#"{{"byteLength":{},"uri":"{}"}}"#, bin.len(), u),
    };
    format!(
        r#"{{
"asset":{{"version":"2.0"}},
"scene":0,
"scenes":[{{"nodes":[0,2]}}],
"nodes":[
 {{"name":"deck","children":[1],"translation":[1.0,2.0,3.0],"mesh":0,"extras":{deck_extras}}},
 {{"name":"insert","translation":[10.0,0.0,0.0],"scale":[2.0,2.0,2.0],"mesh":1,"extras":{insert_extras}}},
 {{"name":"post","translation":[0.0,5.0,0.0],"rotation":[0.0,0.0,0.70710678,0.70710678],"mesh":2}}
],
"meshes":[
 {{"name":"deck","primitives":[{{"attributes":{{"POSITION":0,"NORMAL":1,"TEXCOORD_0":2}},"indices":3,"material":0}}],"extras":{{"collision_layer":5}}}},
 {{"name":"insert","primitives":[{{"attributes":{{"POSITION":4}},"indices":5,"material":1}}]}},
 {{"name":"post","primitives":[{{"attributes":{{"POSITION":4}},"indices":5}}],"extras":{{"collision_layer":7,"collider":true,"unknown_key":[1,2,3]}}}}
],
"materials":[
 {{"name":"wood","pbrMetallicRoughness":{{"baseColorFactor":[0.8,0.6,0.4,0.5],"metallicFactor":0.25,"roughnessFactor":0.75,"baseColorTexture":{{"index":0}}}},"emissiveFactor":[0.1,0.2,0.3],"emissiveTexture":{{"index":1}}}},
 {{"name":"glow"}}
],
"textures":[{{"source":1}},{{"source":0}}],
"images":[{{"bufferView":6,"mimeType":"image/png"}},{{"bufferView":7,"mimeType":"image/png"}}],
"accessors":[
 {{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0.0,0.0,0.0],"max":[0.25,0.5,0.125]}},
 {{"bufferView":1,"componentType":5126,"count":3,"type":"VEC3"}},
 {{"bufferView":2,"componentType":5126,"count":3,"type":"VEC2"}},
 {{"bufferView":3,"componentType":5123,"count":3,"type":"SCALAR"}},
 {{"bufferView":4,"componentType":5126,"count":4,"type":"VEC3","min":[-1.0,-1.0,0.0],"max":[1.0,1.0,0.5]}},
 {{"bufferView":5,"componentType":5125,"count":6,"type":"SCALAR"}}
],
"bufferViews":[
 {{"buffer":0,"byteOffset":0,"byteLength":36}},
 {{"buffer":0,"byteOffset":36,"byteLength":36}},
 {{"buffer":0,"byteOffset":72,"byteLength":24}},
 {{"buffer":0,"byteOffset":96,"byteLength":6}},
 {{"buffer":0,"byteOffset":104,"byteLength":48}},
 {{"buffer":0,"byteOffset":152,"byteLength":24}},
 {{"buffer":0,"byteOffset":{},"byteLength":{}}},
 {{"buffer":0,"byteOffset":{},"byteLength":{}}}
],
"buffers":[{buf}]
}}"#,
        imgs[0].0, imgs[0].1, imgs[1].0, imgs[1].1
    )
}

pub const DECK_EXTRAS: &str = r#"{"collision_layer":2,"switch":"right_orbit_opto","collider":true,"blender_junk":{"a":[1,2]},"some_number":4.5}"#;
pub const INSERT_EXTRAS: &str = r#"{"lamp":"overdrive_ready","collision_layer":0}"#;

/// The standard scene as a `.glb` whose extras bind to names of [`TABLE_TOML`].
pub fn playfield_glb() -> Vec<u8> {
    glb(
        &scene_json(BufferRef::Embedded, DECK_EXTRAS, INSERT_EXTRAS),
        &scene_bin().0,
    )
}

/// The standard scene as a `.glb` with caller-chosen node extras.
pub fn glb_with_extras(deck_extras: &str, insert_extras: &str) -> Vec<u8> {
    glb(
        &scene_json(BufferRef::Embedded, deck_extras, insert_extras),
        &scene_bin().0,
    )
}

// ---------------------------------------------------------------------------
// The mock bundle.
// ---------------------------------------------------------------------------

/// PLAN.md Appendix A.1 with every name the example references declared
/// (`lower_left_eos`, `bumper_1_coil`, `bumper_1_ring`, the two ramp optos,
/// `backglass_logo`) plus a diverter mech, so that it validates cleanly.
pub const TABLE_TOML: &str = r##"schema_version = 1

[meta]
id = "apex_circuit"
name = "Apex Circuit"
author = "Reference"
version = "1.0.0"
script_entry = "rules.lua"
playfield_mesh = "playfield.glb"
pitch_deg = 6.5
players_max = 4
palette = ["#0b1d3a", "#f2a900", "#e63946"]

[balls]
profiles = ["standard_steel", "ceramic_powerball"]
restrict = [ { profile = "jumbo_xl_resin", layers = [0] } ]

[physics]

[cabinet_switches]
left_flipper = 1
right_flipper = 2
left_upper = 3
right_upper = 4
action = 5
start = 6
launch = 7
tilt = 8

[[switches]]
id = "right_orbit_opto"
number = 24
kind = "opto"
layer = 0
position = [410.0, 720.0, 0.0]
radius = 12.0

[[switches]]
id = "lower_left_eos"
number = 25
kind = "eos"

[[switches]]
id = "bumper_1_ring"
number = 26
kind = "ring"

[[switches]]
id = "ramp_entry_opto"
number = 27
kind = "opto"

[[switches]]
id = "ramp_made_opto"
number = 28
kind = "opto"

[[coils]]
id = "left_flipper_coil"
number = 1
kind = "flipper"

[[coils]]
id = "bumper_1_coil"
number = 2
kind = "bumper"

[[coils]]
id = "ramp_diverter_coil"
number = 3
kind = "diverter"

[[lamps]]
id = "overdrive_ready"
number = 40
kind = "insert"
color = "#ff7a00"
response = "led"

[[lamps]]
id = "backglass_logo"
number = 41
kind = "gi"

[[magnets]]
id = "orbit_magnet"
number = 1
position = [400.0, 700.0, 0.0]
k_coil = 2.0e-6
d_min = 8.0
range = 60.0
tau_ms = 60.0

[[flippers]]
id = "lower_left"
coil = "left_flipper_coil"
eos_switch = "lower_left_eos"
collision_layer = 0
base_position = [180.0, 120.0, 0.0]
surface_normal = [0.0, 0.0, 1.0]
rest_angle_deg = -30.0
sweep_angle_deg = 50.0
eos_angle_fraction = 0.92
shape = { type = "StraightCapsule", length = 76.0, base_radius = 11.0, tip_radius = 6.5 }
staging = { type = "DedicatedButton", input = "left_flipper" }

[[splines]]
id = "return_wireform"
kind = "wireform"
layer = 2
gauge = 19.0
points = [[60.0, 900.0, 40.0], [40.0, 600.0, 55.0], [150.0, 260.0, 20.0]]
roll_deg = [0.0, 8.0, 0.0]

[[layer_transitions]]
id = "ramp_entry"
from_layer = 0
to_layer = 2
zone = { min = [240.0, 640.0, 0.0], max = [290.0, 680.0, 30.0] }

[[dispensers]]
id = "power_chamber"
number = 1
exit_position = [300.0, 880.0, 10.0]
exit_velocity = [0.0, -0.6, 0.0]
contents = ["standard_steel", "standard_steel", "standard_steel", "standard_steel", "ceramic_powerball"]

[[mechs]]
id = "bumper_1"
kind = "bumper"
coil = "bumper_1_coil"
switch = "bumper_1_ring"
position = [260.0, 820.0, 0.0]

[[mechs]]
id = "ramp_diverter"
kind = "diverter"
coil = "ramp_diverter_coil"

[[shots]]
id = "cross_shot_ramp"
sequence = ["ramp_entry_opto", "ramp_made_opto"]
window_ms = 2500

[[modes]]
id = "powerball_mania"
start_scene = "scene_powerball_mania"

[[sounds]]
id = "sfx_bumper"
file = "audio/bumper.ogg"
bus = "sfx"

[[scenes]]
id = "scene_powerball_mania"
file = "display/powerball_mania.toml"

[persist]
keys = ["high_score_1", "high_score_2", "games_played"]

[dof]
rom_name = "apexcircuit"
map = [ { coil = "left_flipper_coil", type = "S", number = 1 } ]
"##;

/// PLAN.md Appendix A.2 verbatim (comments removed).
pub const BACKGLASS_TOML: &str = r##"schema_version = 1

[translite]
layers = [
  { file = "backglass/base.png", depth = 0 },
  { file = "backglass/logo.png", depth = 1, lamp_mask = "bg_logo" },
]

[[lamp_masks]]
id = "bg_logo"
lamp = "backglass_logo"
file = "backglass/logo_mask.png"

[score_display]
mode = "dmd"
dmd = { width = 128, height = 32, color = "orange" }
lcd = { width = 1920, height = 1080 }

[backbox]
scene = "backbox.glb"
camera = { position = [0.0, -600.0, 300.0], look_at = [0.0, 0.0, 300.0], fov_deg = 35.0 }
collision_layer = 4
"##;

/// A backbox scene with no table bindings.
pub fn backbox_glb() -> Vec<u8> {
    glb_with_extras("{}", "{}")
}

/// Every file of the mock bundle, keyed by '/'-separated bundle path.
pub fn bundle_with(table: &str, backglass: &str) -> BTreeMap<String, Vec<u8>> {
    let mut m = BTreeMap::new();
    let mut put = |k: &str, v: Vec<u8>| {
        m.insert(k.to_owned(), v);
    };
    put("table.toml", table.as_bytes().to_vec());
    put("backglass.toml", backglass.as_bytes().to_vec());
    put("playfield.glb", playfield_glb());
    put("backbox.glb", backbox_glb());
    put(
        "rules.lua",
        b"-- mock rules\nfunction on_switch(id, closed) end\n".to_vec(),
    );
    put(
        "audio/bumper.ogg",
        b"OggS\0\x02mock-vorbis-payload".to_vec(),
    );
    put(
        "display/powerball_mania.toml",
        b"schema_version = 1\n".to_vec(),
    );
    put("backglass/base.png", png_1x1([10, 20, 30, 255]));
    put("backglass/logo.png", png_1x1([200, 100, 0, 255]));
    put("backglass/logo_mask.png", png_1x1([255, 255, 255, 255]));
    m
}

pub fn bundle() -> BTreeMap<String, Vec<u8>> {
    bundle_with(TABLE_TOML, BACKGLASS_TOML)
}

/// Replaces exactly one occurrence of `from` in `text`; panics otherwise so a
/// mutation can never silently be a no-op.
pub fn sub(text: &str, from: &str, to: &str) -> String {
    assert_eq!(
        text.matches(from).count(),
        1,
        "fixture mutation pattern {from:?} must occur exactly once"
    );
    text.replacen(from, to, 1)
}

// ---------------------------------------------------------------------------
// Validation helpers.
// ---------------------------------------------------------------------------

/// Loads the mock bundle with the given manifests and returns the validation
/// errors; panics when the load succeeds or fails with a different error kind.
pub fn invalid(files: &BTreeMap<String, Vec<u8>>) -> Vec<ValidationError> {
    let vfs = vfs_from(files);
    match pin_assets::load_table(&vfs) {
        Err(AssetError::Invalid(errs)) => {
            assert!(!errs.is_empty(), "Invalid must carry at least one error");
            errs
        }
        Err(other) => panic!("expected AssetError::Invalid, got {other:?}"),
        Ok(t) => panic!(
            "expected validation errors but the bundle loaded (warnings: {:?})",
            t.warnings
        ),
    }
}

pub fn table_errors(table: &str) -> Vec<ValidationError> {
    invalid(&bundle_with(table, BACKGLASS_TOML))
}

pub fn backglass_errors(backglass: &str) -> Vec<ValidationError> {
    invalid(&bundle_with(TABLE_TOML, backglass))
}

/// Asserts that `errs` holds an error for `file` at exactly one of `paths`
/// and returns it. Each error must also Display as "<file>: <path>: <message>".
pub fn expect_at<'a>(
    errs: &'a [ValidationError],
    file: &str,
    paths: &[&str],
) -> &'a ValidationError {
    for e in errs {
        assert_eq!(
            e.to_string(),
            format!("{}: {}: {}", e.file, e.path, e.message),
            "Display must be \"<file>: <path>: <message>\""
        );
    }
    errs.iter()
        .find(|e| e.file == file && paths.contains(&e.path.as_str()))
        .unwrap_or_else(|| {
            panic!(
                "no validation error for {file} at any of {paths:?}; got:\n{}",
                errs.iter()
                    .map(|e| format!("  {e}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        })
}

/// Like [`expect_at`] but accepts any path that equals `prefix` or extends it
/// with an index/field (for rules whose brief wording does not pin the leaf).
pub fn expect_under<'a>(
    errs: &'a [ValidationError],
    file: &str,
    prefix: &str,
) -> &'a ValidationError {
    errs.iter()
        .find(|e| {
            e.file == file
                && (e.path == prefix
                    || e.path.starts_with(&format!("{prefix}["))
                    || e.path.starts_with(&format!("{prefix}.")))
        })
        .unwrap_or_else(|| {
            panic!(
                "no validation error for {file} under {prefix}; got:\n{}",
                errs.iter()
                    .map(|e| format!("  {e}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        })
}
