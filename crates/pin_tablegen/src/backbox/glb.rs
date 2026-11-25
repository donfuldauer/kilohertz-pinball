//! A minimal, deterministic binary glTF 2.0 writer for the backbox scenes:
//! one scene of meshes (one triangle primitive each, identity transforms)
//! with node `extras`, flat-colour metallic-roughness materials and one
//! buffer in the GLB `BIN` chunk. The JSON is written by hand in a fixed key
//! order, so the same nodes always give the same bytes. (Same output
//! conventions as the playfield writer of `layout`, whose writer is private
//! to that module.)

use core::fmt::Write as _;
use pin_core::fmath::{max_f32, min_f32};

/// An indexed triangle mesh, metres.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    /// Positions.
    pub positions: Vec<[f32; 3]>,
    /// Unit normals, one per position.
    pub normals: Vec<[f32; 3]>,
    /// Triangle list (counter-clockwise front faces).
    pub indices: Vec<u32>,
}

impl Mesh {
    /// Adds a quad `a b c d` (counter-clockwise seen from its front) with
    /// its own four vertices and the face normal `n`.
    pub fn quad(&mut self, a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3], n: [f32; 3]) {
        let base = self.positions.len() as u32;
        for p in [a, b, c, d] {
            self.positions.push(p);
            self.normals.push(n);
        }
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

/// A flat-colour PBR material.
#[derive(Clone, Debug, PartialEq)]
pub struct FlatMaterial {
    /// Name.
    pub name: &'static str,
    /// Linear RGBA.
    pub base_color: [f32; 4],
    /// Metallic factor.
    pub metallic: f32,
    /// Roughness factor.
    pub roughness: f32,
}

/// One node with its own mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    /// Node and mesh name.
    pub name: String,
    /// Geometry, metres.
    pub mesh: Mesh,
    /// Index into the material list.
    pub material: usize,
    /// `extras.collider`.
    pub collider: bool,
    /// `extras.collision_layer`.
    pub collision_layer: u8,
    /// Further string-valued extras (ignored by `pin_assets`), in order.
    pub strings: Vec<(&'static str, String)>,
}

fn json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn json_f32(out: &mut String, v: f32) {
    let v = if v == 0.0 { 0.0 } else { v }; // no "-0"
    let _ = write!(out, "{v:?}");
}

fn json_f32s(out: &mut String, vs: &[f32]) {
    out.push('[');
    for (i, &v) in vs.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        json_f32(out, v);
    }
    out.push(']');
}

fn bounds(points: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for p in points {
        for k in 0..3 {
            lo[k] = min_f32(lo[k], p[k]);
            hi[k] = max_f32(hi[k], p[k]);
        }
    }
    (lo, hi)
}

fn pad4(buf: &mut Vec<u8>, fill: u8) {
    while !buf.len().is_multiple_of(4) {
        buf.push(fill);
    }
}

fn le_bytes(values: impl Iterator<Item = [u8; 4]>) -> Vec<u8> {
    values.flatten().collect()
}

/// Serialises `nodes` (each with at least one triangle, finite floats) and
/// `materials` as one scene named `scene` to GLB bytes.
pub fn write(scene: &str, nodes: &[Node], materials: &[FlatMaterial]) -> Vec<u8> {
    let mut bin: Vec<u8> = Vec::new();
    let mut views = String::new();
    let mut accessors = String::new();
    let mut meshes = String::new();
    let mut json_nodes = String::new();
    let mut view_count = 0usize;
    let mut add_view = |bin: &mut Vec<u8>, bytes: &[u8], target: u32, views: &mut String| {
        pad4(bin, 0);
        if view_count > 0 {
            views.push(',');
        }
        let _ = write!(
            views,
            "{{\"buffer\":0,\"byteOffset\":{},\"byteLength\":{},\"target\":{target}}}",
            bin.len(),
            bytes.len()
        );
        bin.extend_from_slice(bytes);
        view_count += 1;
        view_count - 1
    };
    for (i, node) in nodes.iter().enumerate() {
        let m = &node.mesh;
        let pos = le_bytes(m.positions.iter().flatten().map(|v| v.to_le_bytes()));
        let nrm = le_bytes(m.normals.iter().flatten().map(|v| v.to_le_bytes()));
        let idx = le_bytes(m.indices.iter().map(|k| k.to_le_bytes()));
        let vp = add_view(&mut bin, &pos, 34962, &mut views);
        let vn = add_view(&mut bin, &nrm, 34962, &mut views);
        let vi = add_view(&mut bin, &idx, 34963, &mut views);
        let (lo, hi) = bounds(&m.positions);
        if i > 0 {
            accessors.push(',');
            meshes.push(',');
            json_nodes.push(',');
        }
        let count = m.positions.len();
        let _ = write!(
            accessors,
            "{{\"bufferView\":{vp},\"componentType\":5126,\"count\":{count},\"type\":\"VEC3\",\"min\":"
        );
        json_f32s(&mut accessors, &lo);
        accessors.push_str(",\"max\":");
        json_f32s(&mut accessors, &hi);
        let _ = write!(
            accessors,
            "}},{{\"bufferView\":{vn},\"componentType\":5126,\"count\":{count},\"type\":\"VEC3\"}},\
             {{\"bufferView\":{vi},\"componentType\":5125,\"count\":{},\"type\":\"SCALAR\"}}",
            m.indices.len()
        );
        meshes.push_str("{\"name\":");
        json_str(&mut meshes, &node.name);
        let _ = write!(
            meshes,
            ",\"primitives\":[{{\"attributes\":{{\"POSITION\":{},\"NORMAL\":{}}},\"indices\":{},\"material\":{},\"mode\":4}}]}}",
            3 * i,
            3 * i + 1,
            3 * i + 2,
            node.material
        );
        json_nodes.push_str("{\"name\":");
        json_str(&mut json_nodes, &node.name);
        let _ = write!(
            json_nodes,
            ",\"mesh\":{i},\"extras\":{{\"collider\":{},\"collision_layer\":{}",
            node.collider, node.collision_layer
        );
        for (k, v) in &node.strings {
            json_nodes.push(',');
            json_str(&mut json_nodes, k);
            json_nodes.push(':');
            json_str(&mut json_nodes, v);
        }
        json_nodes.push_str("}}");
    }
    let mut mats = String::new();
    for (i, mat) in materials.iter().enumerate() {
        if i > 0 {
            mats.push(',');
        }
        mats.push_str("{\"name\":");
        json_str(&mut mats, mat.name);
        mats.push_str(",\"pbrMetallicRoughness\":{\"baseColorFactor\":");
        json_f32s(&mut mats, &mat.base_color);
        mats.push_str(",\"metallicFactor\":");
        json_f32(&mut mats, mat.metallic);
        mats.push_str(",\"roughnessFactor\":");
        json_f32(&mut mats, mat.roughness);
        mats.push_str("},\"doubleSided\":true}");
    }
    pad4(&mut bin, 0);
    let scene_nodes: Vec<String> = (0..nodes.len()).map(|i| i.to_string()).collect();
    let mut json = String::new();
    json.push_str(
        "{\"asset\":{\"version\":\"2.0\",\"generator\":\"pin_tablegen (Kilohertz Pinball)\"},\"scene\":0,",
    );
    json.push_str("\"scenes\":[{\"name\":");
    json_str(&mut json, scene);
    let _ = write!(
        json,
        ",\"nodes\":[{}]}}],\"nodes\":[{json_nodes}],\"meshes\":[{meshes}],\
         \"materials\":[{mats}],\"accessors\":[{accessors}],\"bufferViews\":[{views}],\
         \"buffers\":[{{\"byteLength\":{}}}]}}",
        scene_nodes.join(","),
        bin.len()
    );
    let mut json = json.into_bytes();
    pad4(&mut json, b' ');
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
    out.extend_from_slice(&json);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x004E_4942u32.to_le_bytes());
    out.extend_from_slice(&bin);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_escapes_and_numbers() {
        let mut s = String::new();
        json_str(&mut s, "a\"b\\c\u{1}");
        assert_eq!(s, "\"a\\\"b\\\\c\\u0001\"");
        let mut n = String::new();
        json_f32s(&mut n, &[-0.0, 1.5]);
        assert_eq!(n, "[0.0,1.5]");
    }

    #[test]
    fn header_chunks_and_alignment() {
        let mut mesh = Mesh::default();
        mesh.quad(
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
        );
        assert_eq!(mesh.indices, [0, 1, 2, 0, 2, 3]);
        let node = Node {
            name: "n\"x".into(),
            mesh,
            material: 0,
            collider: true,
            collision_layer: 4,
            strings: vec![("role", "panel".into())],
        };
        let mat = FlatMaterial {
            name: "wood",
            base_color: [0.5, 0.4, 0.3, 1.0],
            metallic: 0.0,
            roughness: 0.8,
        };
        let glb = write("backbox", &[node], &[mat]);
        assert_eq!(&glb[0..4], b"glTF");
        let total = u32::from_le_bytes(glb[8..12].try_into().unwrap()) as usize;
        assert_eq!(total, glb.len());
        let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        assert_eq!(json_len % 4, 0);
        let json = core::str::from_utf8(&glb[20..20 + json_len]).unwrap();
        assert!(json.contains("\"collision_layer\":4"));
        assert!(json.contains("\"role\":\"panel\""));
        assert!(json.contains("\"name\":\"backbox\""));
    }
}
