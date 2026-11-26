//! A minimal, deterministic binary glTF 2.0 writer for `cabinet.glb`: one
//! scene of nodes (one triangle primitive each with `POSITION`, `NORMAL` and
//! `TEXCOORD_0`, or no mesh at all: a node kept in the stable node list
//! with nothing to draw), an optional node `translation` and string `extras`, named
//! flat-colour metallic-roughness materials (optionally alpha-blended) and
//! one buffer in the GLB `BIN` chunk. The JSON is written by hand in a fixed
//! key order and every float goes through one printer (no `-0`), so the same
//! nodes always give the same bytes. Same conventions as the backbox writer,
//! which has no UVs or node transforms.

use core::fmt::Write as _;
use pin_core::fmath::{max_f32, min_f32};

/// An indexed triangle mesh, metres.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Mesh {
    /// Positions.
    pub positions: Vec<[f32; 3]>,
    /// Unit normals, one per position.
    pub normals: Vec<[f32; 3]>,
    /// `TEXCOORD_0`, one per position, in `[0, 1]`.
    pub uvs: Vec<[f32; 2]>,
    /// Triangle list (counter-clockwise front faces).
    pub indices: Vec<u32>,
}

/// A flat-colour PBR material.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Material {
    /// Stable name.
    pub name: &'static str,
    /// Linear RGBA.
    pub base_color: [f32; 4],
    /// Metallic factor.
    pub metallic: f32,
    /// Roughness factor.
    pub roughness: f32,
    /// `alphaMode: BLEND` (otherwise the glTF default, opaque).
    pub blend: bool,
}

/// One node with its own mesh.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Node {
    /// Node and mesh name.
    pub name: &'static str,
    /// Geometry, metres, in the node's frame (`None`: a node without a
    /// mesh, written with no `mesh` property).
    pub mesh: Option<Mesh>,
    /// Index into the material list.
    pub material: usize,
    /// Node translation, metres (`None`: identity transform).
    pub translation: Option<[f32; 3]>,
    /// String-valued extras, in order (none: no `extras` object).
    pub extras: Vec<(&'static str, String)>,
}

impl Node {
    /// Every float of the node (positions, normals, UVs, translation).
    pub(crate) fn all_finite(&self) -> bool {
        self.mesh.as_ref().is_none_or(|m| {
            m.positions.iter().flatten().all(|v| v.is_finite())
                && m.normals.iter().flatten().all(|v| v.is_finite())
                && m.uvs.iter().flatten().all(|v| v.is_finite())
        }) && self
            .translation
            .is_none_or(|t| t.iter().all(|v| v.is_finite()))
    }
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

/// The one float printer: shortest round-trip form, never `-0`.
fn json_f32(out: &mut String, v: f32) {
    let v = if v == 0.0 { 0.0 } else { v };
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

fn le_f32s<'a>(values: impl Iterator<Item = &'a f32>) -> Vec<u8> {
    values.flat_map(|v| v.to_le_bytes()).collect()
}

/// Serialises `nodes` (each with no mesh, or a mesh with at least one
/// triangle and finite floats) and `materials` as one scene named `scene` to GLB bytes.
pub(crate) fn write(scene: &str, nodes: &[Node], materials: &[Material]) -> Vec<u8> {
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
    let mut mesh_count = 0usize;
    for (i, node) in nodes.iter().enumerate() {
        if i > 0 {
            json_nodes.push(',');
        }
        json_nodes.push_str("{\"name\":");
        json_str(&mut json_nodes, node.name);
        if let Some(m) = &node.mesh {
            let mi = mesh_count;
            mesh_count += 1;
            let pos = le_f32s(m.positions.iter().flatten());
            let nrm = le_f32s(m.normals.iter().flatten());
            let uv = le_f32s(m.uvs.iter().flatten());
            let idx: Vec<u8> = m.indices.iter().flat_map(|k| k.to_le_bytes()).collect();
            let vp = add_view(&mut bin, &pos, 34962, &mut views);
            let vn = add_view(&mut bin, &nrm, 34962, &mut views);
            let vt = add_view(&mut bin, &uv, 34962, &mut views);
            let vi = add_view(&mut bin, &idx, 34963, &mut views);
            let (lo, hi) = bounds(&m.positions);
            if mi > 0 {
                accessors.push(',');
                meshes.push(',');
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
                 {{\"bufferView\":{vt},\"componentType\":5126,\"count\":{count},\"type\":\"VEC2\"}},\
                 {{\"bufferView\":{vi},\"componentType\":5125,\"count\":{},\"type\":\"SCALAR\"}}",
                m.indices.len()
            );
            meshes.push_str("{\"name\":");
            json_str(&mut meshes, node.name);
            let _ = write!(
                meshes,
                ",\"primitives\":[{{\"attributes\":{{\"POSITION\":{},\"NORMAL\":{},\"TEXCOORD_0\":{}}},\
                 \"indices\":{},\"material\":{},\"mode\":4}}]}}",
                4 * mi,
                4 * mi + 1,
                4 * mi + 2,
                4 * mi + 3,
                node.material
            );
            let _ = write!(json_nodes, ",\"mesh\":{mi}");
        }
        if let Some(t) = node.translation {
            json_nodes.push_str(",\"translation\":");
            json_f32s(&mut json_nodes, &t);
        }
        if !node.extras.is_empty() {
            json_nodes.push_str(",\"extras\":{");
            for (k, (key, value)) in node.extras.iter().enumerate() {
                if k > 0 {
                    json_nodes.push(',');
                }
                json_str(&mut json_nodes, key);
                json_nodes.push(':');
                json_str(&mut json_nodes, value);
            }
            json_nodes.push('}');
        }
        json_nodes.push('}');
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
        mats.push('}');
        if mat.blend {
            mats.push_str(",\"alphaMode\":\"BLEND\",\"doubleSided\":true");
        }
        mats.push('}');
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
        json_f32s(&mut n, &[-0.0, 1.5, 1e-7]);
        assert_eq!(n, "[0.0,1.5,1e-7]");
    }

    #[test]
    fn header_chunks_alignment_translation_extras_and_blend() {
        let mesh = Mesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            indices: vec![0, 1, 2],
        };
        let node = Node {
            name: "n\"x",
            mesh: Some(mesh),
            material: 0,
            translation: Some([0.5, -0.0, 2.0]),
            extras: vec![("moving", "true".into()), ("travel_axis", "+x".into())],
        };
        assert!(node.all_finite());
        let mut bad = node.clone();
        bad.translation = Some([f32::NAN, 0.0, 0.0]);
        assert!(!bad.all_finite());
        let mat = Material {
            name: "glass",
            base_color: [1.0, 1.0, 1.0, 0.2],
            metallic: 0.0,
            roughness: 0.05,
            blend: true,
        };
        let glb = write("cabinet", &[node], &[mat]);
        assert_eq!(&glb[0..4], b"glTF");
        let total = u32::from_le_bytes(glb[8..12].try_into().unwrap()) as usize;
        assert_eq!(total, glb.len());
        assert_eq!(glb.len() % 4, 0);
        let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        assert_eq!(json_len % 4, 0);
        let json = core::str::from_utf8(&glb[20..20 + json_len]).unwrap();
        assert!(json.contains("\"translation\":[0.5,0.0,2.0]"));
        assert!(json.contains("\"extras\":{\"moving\":\"true\",\"travel_axis\":\"+x\"}"));
        assert!(json.contains("\"alphaMode\":\"BLEND\""));
        assert!(json.contains("\"TEXCOORD_0\":2"));
        assert!(json.contains("\"name\":\"cabinet\""));
    }
}
