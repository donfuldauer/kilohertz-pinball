//! A minimal, deterministic binary glTF 2.0 writer: one scene of meshes
//! with node `extras`, flat-colour metallic-roughness materials, one buffer
//! in the GLB `BIN` chunk. The JSON is written by hand in a fixed key order,
//! so the same nodes always give the same bytes.

use super::mesh::Mesh;
use core::fmt::Write as _;

/// A flat-colour PBR material.
#[derive(Clone, Debug, PartialEq)]
pub struct FlatMaterial {
    /// Name (the physics material name for colliders, e.g. `wood`).
    pub name: String,
    /// Linear RGBA.
    pub base_color: [f32; 4],
    /// Metallic factor.
    pub metallic: f32,
    /// Roughness factor.
    pub roughness: f32,
}

/// Node `extras` (the Blender custom properties `pin_assets` reads).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Extras {
    /// `collider`.
    pub collider: bool,
    /// `collision_layer`.
    pub collision_layer: u8,
    /// `lamp`.
    pub lamp: Option<String>,
    /// Further string-valued keys (ignored by `pin_assets`), in order.
    pub strings: Vec<(&'static str, String)>,
}

/// One node with its own mesh (a single primitive).
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    /// Node and mesh name.
    pub name: String,
    /// Geometry, metres.
    pub mesh: Mesh,
    /// Index into the material list.
    pub material: usize,
    /// Extras.
    pub extras: Extras,
}

/// JSON string literal.
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

/// JSON number for a finite `f32` (shortest round-trip form).
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
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (lo, hi)
}

fn pad4(buf: &mut Vec<u8>, fill: u8) {
    while !buf.len().is_multiple_of(4) {
        buf.push(fill);
    }
}

/// Serialises `nodes` (meshes with at least one triangle) and `materials`
/// to GLB bytes. Every float must be finite.
pub fn write(nodes: &[Node], materials: &[FlatMaterial]) -> Vec<u8> {
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
        let mut pos = Vec::with_capacity(m.positions.len() * 12);
        for p in &m.positions {
            for v in p {
                pos.extend_from_slice(&v.to_le_bytes());
            }
        }
        let mut nrm = Vec::with_capacity(m.normals.len() * 12);
        for n in &m.normals {
            for v in n {
                nrm.extend_from_slice(&v.to_le_bytes());
            }
        }
        let mut idx = Vec::with_capacity(m.indices.len() * 4);
        for &k in &m.indices {
            idx.extend_from_slice(&k.to_le_bytes());
        }
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
            ",\"mesh\":{i},\"extras\":{{\"collider\":{}",
            node.extras.collider
        );
        let _ = write!(
            json_nodes,
            ",\"collision_layer\":{}",
            node.extras.collision_layer
        );
        if let Some(lamp) = &node.extras.lamp {
            json_nodes.push_str(",\"lamp\":");
            json_str(&mut json_nodes, lamp);
        }
        for (k, v) in &node.extras.strings {
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
        json_str(&mut mats, &mat.name);
        mats.push_str(",\"pbrMetallicRoughness\":{\"baseColorFactor\":");
        json_f32s(&mut mats, &mat.base_color);
        mats.push_str(",\"metallicFactor\":");
        json_f32(&mut mats, mat.metallic);
        mats.push_str(",\"roughnessFactor\":");
        json_f32(&mut mats, mat.roughness);
        mats.push('}');
        if mat.base_color[3] < 1.0 {
            mats.push_str(",\"alphaMode\":\"BLEND\"");
        }
        mats.push_str(",\"doubleSided\":true}");
    }
    pad4(&mut bin, 0);
    let scene_nodes: Vec<String> = (0..nodes.len()).map(|i| i.to_string()).collect();
    let mut json = String::new();
    json.push_str(
        "{\"asset\":{\"version\":\"2.0\",\"generator\":\"pin_tablegen (Kilohertz Pinball)\"},\"scene\":0,",
    );
    let _ = write!(
        json,
        "\"scenes\":[{{\"name\":\"playfield\",\"nodes\":[{}]}}],\"nodes\":[{json_nodes}],\"meshes\":[{meshes}],\
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
    fn json_escapes() {
        let mut s = String::new();
        json_str(&mut s, "a\"b\\c\n");
        assert_eq!(s, "\"a\\\"b\\\\c\\u000a\"");
        let mut s = String::new();
        json_f32(&mut s, -0.0);
        json_f32(&mut s, 0.25);
        assert_eq!(s, "0.00.25");
    }

    #[test]
    fn header_and_alignment() {
        let mut mesh = Mesh::default();
        mesh.quad(
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        );
        let node = Node {
            name: "q".into(),
            mesh,
            material: 0,
            extras: Extras {
                collider: true,
                collision_layer: 3,
                lamp: Some("l".into()),
                strings: vec![("material", "wood".into())],
            },
        };
        let mat = FlatMaterial {
            name: "wood".into(),
            base_color: [1.0, 1.0, 1.0, 0.5],
            metallic: 0.0,
            roughness: 0.5,
        };
        let glb = write(&[node], &[mat]);
        assert_eq!(&glb[..4], b"glTF");
        assert_eq!(glb.len() % 4, 0);
        assert_eq!(
            u32::from_le_bytes(glb[8..12].try_into().unwrap()) as usize,
            glb.len()
        );
        let text = String::from_utf8_lossy(&glb);
        assert!(text.contains("\"collision_layer\":3"));
        assert!(text.contains("\"alphaMode\":\"BLEND\""));
    }
}
