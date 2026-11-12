//! glTF 2.0 loader and extras binding.
//!
//! Owner: Subphase 4.1.
//!
//! glTF files come from untrusted bundles (D25). The `gltf` crate is used
//! only to deserialize and index-validate the JSON document (its `import`
//! feature is off: it would read the real filesystem). The GLB container,
//! buffers (GLB `BIN`, `data:` URIs, or bundle files resolved through the
//! [`Vfs`]), accessors and images are read here with explicit bounds checks,
//! so a hostile file yields [`AssetError::Gltf`], never a panic or an
//! out-of-bundle read. Images are decoded to RGBA8 with the `image` crate
//! under explicit size and allocation limits.

mod check;
mod data;

use crate::validate::{IdKind, TableIds, ValidationError};
use crate::{AssetError, Vfs};
use ::gltf::json;
use ::gltf::{Document, Semantic};
use data::{Buffers, read_f32s, read_indices};

/// Largest decoded image side, pixels.
const MAX_IMAGE_SIDE: u32 = 16_384;
/// Largest decoder allocation per image, bytes.
const MAX_IMAGE_ALLOC: u64 = 512 * 1024 * 1024;
/// Node hierarchy depth limit.
const MAX_NODE_DEPTH: usize = 256;
/// Highest collision layer an extras tag may name.
const MAX_LAYER: u8 = 7;

/// Everything loaded from one glTF file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GltfScene {
    /// One entry per (node, triangle primitive) of the default scene.
    pub meshes: Vec<MeshData>,
    /// Materials, indexed by [`MeshData::material`].
    pub materials: Vec<PbrMaterial>,
    /// Decoded images, indexed by the `*_image` fields of [`PbrMaterial`].
    pub images: Vec<ImageData>,
}

/// One triangle primitive placed by its node.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshData {
    /// Node name, else mesh name, else `node<index>`.
    pub name: String,
    /// Object-space positions.
    pub positions: Vec<[f32; 3]>,
    /// Object-space normals (computed from the triangles when absent).
    pub normals: Vec<[f32; 3]>,
    /// `TEXCOORD_0` (empty when absent).
    pub uvs: Vec<[f32; 2]>,
    /// Triangle list indices (generated when the primitive is not indexed).
    pub indices: Vec<u32>,
    /// Index into [`GltfScene::materials`].
    pub material: Option<usize>,
    /// Node world transform, column-major.
    pub transform: [[f32; 4]; 4],
    /// Blender custom properties.
    pub extras: NodeExtras,
}

/// Blender custom properties from node (or mesh) `extras`. Node keys take
/// precedence over mesh keys; unknown keys are ignored.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct NodeExtras {
    /// `collision_layer` (0..=7).
    pub collision_layer: Option<u8>,
    /// `switch`: switch name from `table.toml`.
    pub switch: Option<String>,
    /// `lamp`: lamp name from `table.toml` (lamp insert).
    pub lamp: Option<String>,
    /// `collider` (bool, or 0/1 as Blender exports booleans).
    pub collider: bool,
}

/// Metallic-roughness PBR material.
#[derive(Clone, Debug, PartialEq)]
pub struct PbrMaterial {
    /// Name (may be empty).
    pub name: String,
    /// Linear RGBA factor.
    pub base_color: [f32; 4],
    /// Metallic factor.
    pub metallic: f32,
    /// Roughness factor.
    pub roughness: f32,
    /// Emissive factor.
    pub emissive: [f32; 3],
    /// Base colour texture image.
    pub base_color_image: Option<usize>,
    /// Metallic-roughness texture image.
    pub metallic_roughness_image: Option<usize>,
    /// Normal map image.
    pub normal_image: Option<usize>,
    /// Emissive texture image.
    pub emissive_image: Option<usize>,
}

/// A decoded image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageData {
    /// Pixels.
    pub width: u32,
    /// Pixels.
    pub height: u32,
    /// `width * height * 4` bytes, row-major, top row first.
    pub rgba8: Vec<u8>,
}

fn gerr(m: impl Into<String>) -> AssetError {
    AssetError::Gltf(m.into())
}

const IDENTITY: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// Column-major `a * b`.
fn mat_mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut out = [[0.0; 4]; 4];
    for (c, col) in out.iter_mut().enumerate() {
        for (r, v) in col.iter_mut().enumerate() {
            *v = (0..4).map(|k| a[k][r] * b[c][k]).sum();
        }
    }
    out
}

/// Splits a GLB container into its JSON chunk and optional BIN chunk.
fn split_glb(bytes: &[u8]) -> Result<(&[u8], Option<&[u8]>), AssetError> {
    let u32_at = |off: usize| -> Result<u32, AssetError> {
        bytes
            .get(off..off + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or_else(|| gerr("truncated GLB"))
    };
    if u32_at(4)? != 2 {
        return Err(gerr(format!("unsupported GLB version {}", u32_at(4)?)));
    }
    let total = u32_at(8)? as usize;
    if total < 12 || total > bytes.len() {
        return Err(gerr(format!(
            "GLB length {total} does not match the file ({} bytes)",
            bytes.len()
        )));
    }
    let bytes = &bytes[..total];
    let chunk = |off: usize| -> Result<(u32, &[u8], usize), AssetError> {
        let len = u32_at(off)? as usize;
        let ty = u32_at(off + 4)?;
        let start = off + 8;
        let end = start
            .checked_add(len)
            .filter(|&e| e <= bytes.len())
            .ok_or_else(|| gerr("GLB chunk runs past the end of the file"))?;
        Ok((ty, &bytes[start..end], end))
    };
    const JSON: u32 = 0x4E4F_534A;
    const BIN: u32 = 0x004E_4942;
    let (ty, json_chunk, next) = chunk(12)?;
    if ty != JSON {
        return Err(gerr("first GLB chunk is not JSON"));
    }
    let bin = if next + 8 <= bytes.len() {
        let (ty, bin, _) = chunk(next)?;
        (ty == BIN).then_some(bin)
    } else {
        None
    };
    Ok((json_chunk, bin))
}

fn parse_document(bytes: &[u8]) -> Result<(Document, Option<Vec<u8>>), AssetError> {
    let (json_bytes, bin) = if bytes.starts_with(b"glTF") {
        split_glb(bytes)?
    } else {
        (bytes, None)
    };
    let root: json::Root =
        json::deserialize::from_slice(json_bytes).map_err(|e| gerr(e.to_string()))?;
    check::prevalidate(&root)?;
    let document = Document::from_json(root).map_err(|e| gerr(e.to_string()))?;
    Ok((document, bin.map(<[u8]>::to_vec)))
}

/// Reads one extras object into `out`, overriding keys that are present.
fn apply_extras(
    extras: &json::Extras,
    owner: &str,
    out: &mut NodeExtras,
) -> Result<(), AssetError> {
    let Some(raw) = extras else {
        return Ok(());
    };
    let value: serde_json::Value =
        serde_json::from_str(raw.get()).map_err(|e| gerr(format!("{owner}: extras: {e}")))?;
    let Some(map) = value.as_object() else {
        return Ok(()); // non-object extras carry no tags
    };
    if let Some(v) = map.get("collision_layer") {
        let layer = v
            .as_u64()
            .or_else(|| {
                v.as_f64()
                    .filter(|f| f.fract() == 0.0 && *f >= 0.0)
                    .map(|f| f as u64)
            })
            .filter(|&l| l <= u64::from(MAX_LAYER))
            .ok_or_else(|| {
                gerr(format!(
                    "{owner}: extras.collision_layer must be an integer 0..={MAX_LAYER}, got {v}"
                ))
            })?;
        out.collision_layer = Some(layer as u8);
    }
    for (key, slot) in [("switch", &mut out.switch), ("lamp", &mut out.lamp)] {
        if let Some(v) = map.get(key) {
            let s = v
                .as_str()
                .ok_or_else(|| gerr(format!("{owner}: extras.{key} must be a string, got {v}")))?;
            *slot = Some(s.to_owned());
        }
    }
    if let Some(v) = map.get("collider") {
        out.collider = match v {
            serde_json::Value::Bool(b) => *b,
            serde_json::Value::Number(n) if n.as_f64() == Some(0.0) => false,
            serde_json::Value::Number(n) if n.as_f64() == Some(1.0) => true,
            _ => {
                return Err(gerr(format!(
                    "{owner}: extras.collider must be a boolean, got {v}"
                )));
            }
        };
    }
    Ok(())
}

/// Smooth normals accumulated from triangle faces.
fn compute_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut normals = vec![[0.0f32; 3]; positions.len()];
    for tri in indices.chunks_exact(3) {
        let [a, b, c] = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
        let (pa, pb, pc) = (positions[a], positions[b], positions[c]);
        let u = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
        let v = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
        let n = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        for i in [a, b, c] {
            for k in 0..3 {
                normals[i][k] += n[k];
            }
        }
    }
    for n in &mut normals {
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len > 0.0 && len.is_finite() {
            for v in n.iter_mut() {
                *v /= len;
            }
        } else {
            *n = [0.0, 0.0, 1.0];
        }
    }
    normals
}

fn decode_image(bytes: &[u8], what: &str) -> Result<ImageData, AssetError> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| gerr(format!("{what}: {e}")))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_SIDE);
    limits.max_image_height = Some(MAX_IMAGE_SIDE);
    limits.max_alloc = Some(MAX_IMAGE_ALLOC);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|e| gerr(format!("{what}: {e}")))?
        .to_rgba8();
    Ok(ImageData {
        width: decoded.width(),
        height: decoded.height(),
        rgba8: decoded.into_raw(),
    })
}

/// Loads meshes, PBR materials, images and Blender extras from a `.glb` or
/// `.gltf` in the bundle. External buffers and images are resolved relative to
/// `path` through `vfs` only; `data:` URIs are decoded inline.
///
/// Hostile input is rejected by explicit checks; as defence in depth, a panic
/// inside the pinned `gltf` crate is also caught and reported as
/// [`AssetError::Gltf`] (DECISIONS.md, 4.1).
pub fn load_gltf(vfs: &Vfs, path: &str) -> Result<GltfScene, AssetError> {
    let bytes = vfs.read(path)?;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| load_bytes(vfs, path, &bytes)))
        .unwrap_or_else(|_| Err(gerr(format!("{path}: malformed glTF (loader panicked)"))))
}

fn load_bytes(vfs: &Vfs, path: &str, bytes: &[u8]) -> Result<GltfScene, AssetError> {
    let (document, bin) = parse_document(bytes)?;
    let base = path.rsplit_once(['/', '\\']).map_or("", |(dir, _)| dir);
    let buffers = Buffers::load(&document, bin, vfs, base)?;

    // Images (only those a texture uses are decoded; the rest stay empty).
    let mut images = Vec::with_capacity(document.images().len());
    for image in document.images() {
        let what = format!("image {}", image.index());
        let encoded = match image.source() {
            ::gltf::image::Source::View { view, .. } => buffers.view(&view)?.to_vec(),
            ::gltf::image::Source::Uri { uri, .. } => data::resolve_uri(uri, vfs, base)?,
        };
        images.push(decode_image(&encoded, &what)?);
    }

    let materials = document
        .materials()
        .map(|m| {
            let pbr = m.pbr_metallic_roughness();
            let tex = |t: Option<::gltf::Texture<'_>>| t.map(|t| t.source().index());
            PbrMaterial {
                name: m.name().unwrap_or_default().to_owned(),
                base_color: pbr.base_color_factor(),
                metallic: pbr.metallic_factor(),
                roughness: pbr.roughness_factor(),
                emissive: m.emissive_factor(),
                base_color_image: tex(pbr.base_color_texture().map(|i| i.texture())),
                metallic_roughness_image: tex(pbr
                    .metallic_roughness_texture()
                    .map(|i| i.texture())),
                normal_image: tex(m.normal_texture().map(|i| i.texture())),
                emissive_image: tex(m.emissive_texture().map(|i| i.texture())),
            }
        })
        .collect();

    // Nodes of the default scene (else the first scene, else every root node).
    let roots: Vec<::gltf::Node<'_>> = match document
        .default_scene()
        .or_else(|| document.scenes().next())
    {
        Some(scene) => scene.nodes().collect(),
        None => {
            let mut is_child = vec![false; document.nodes().len()];
            for n in document.nodes() {
                for c in n.children() {
                    is_child[c.index()] = true;
                }
            }
            document.nodes().filter(|n| !is_child[n.index()]).collect()
        }
    };
    let mut meshes = Vec::new();
    let mut visited = vec![false; document.nodes().len()];
    let mut stack: Vec<(::gltf::Node<'_>, [[f32; 4]; 4], usize)> =
        roots.into_iter().rev().map(|n| (n, IDENTITY, 0)).collect();
    while let Some((node, parent, depth)) = stack.pop() {
        if depth > MAX_NODE_DEPTH {
            return Err(gerr("node hierarchy is too deep"));
        }
        if std::mem::replace(&mut visited[node.index()], true) {
            return Err(gerr(format!(
                "node {} appears more than once in the scene graph",
                node.index()
            )));
        }
        let local = node.transform().matrix();
        if local.iter().flatten().any(|v| !v.is_finite()) {
            return Err(gerr(format!("node {}: non-finite transform", node.index())));
        }
        let world = mat_mul(&parent, &local);
        if let Some(mesh) = node.mesh() {
            let name = node
                .name()
                .or(mesh.name())
                .map_or_else(|| format!("node{}", node.index()), str::to_owned);
            let mut extras = NodeExtras::default();
            apply_extras(mesh.extras(), &format!("mesh {name:?}"), &mut extras)?;
            apply_extras(node.extras(), &format!("node {name:?}"), &mut extras)?;
            for prim in mesh.primitives() {
                if prim.mode() != ::gltf::mesh::Mode::Triangles {
                    continue;
                }
                let what = format!("mesh {name:?} primitive {}", prim.index());
                let Some(pos) = prim.get(&Semantic::Positions) else {
                    return Err(gerr(format!("{what}: no POSITION attribute")));
                };
                let positions: Vec<[f32; 3]> = read_f32s::<3>(&buffers, &pos, &what)?;
                let count = positions.len();
                let indices = match prim.indices() {
                    Some(acc) => read_indices(&buffers, &acc, &what)?,
                    None => (0..count as u32).collect(),
                };
                if indices.len() % 3 != 0 {
                    return Err(gerr(format!("{what}: index count is not a multiple of 3")));
                }
                if let Some(&bad) = indices.iter().find(|&&i| i as usize >= count) {
                    return Err(gerr(format!(
                        "{what}: index {bad} out of range ({count} vertices)"
                    )));
                }
                let normals = match prim.get(&Semantic::Normals) {
                    Some(acc) => read_f32s::<3>(&buffers, &acc, &what)?,
                    None => compute_normals(&positions, &indices),
                };
                let uvs = match prim.get(&Semantic::TexCoords(0)) {
                    Some(acc) => read_f32s::<2>(&buffers, &acc, &what)?,
                    None => Vec::new(),
                };
                if normals.len() != count || !(uvs.is_empty() || uvs.len() == count) {
                    return Err(gerr(format!(
                        "{what}: attribute counts differ from POSITION ({count})"
                    )));
                }
                meshes.push(MeshData {
                    name: name.clone(),
                    positions,
                    normals,
                    uvs,
                    indices,
                    material: prim.material().index(),
                    transform: world,
                    extras: extras.clone(),
                });
            }
        }
        for child in node.children().collect::<Vec<_>>().into_iter().rev() {
            stack.push((child, world, depth + 1));
        }
    }
    Ok(GltfScene {
        meshes,
        materials,
        images,
    })
}

/// [`validate_gltf_bindings_in`] with the file reported as `"glTF"`.
pub fn validate_gltf_bindings(scene: &GltfScene, ids: &TableIds) -> Vec<ValidationError> {
    validate_gltf_bindings_in(scene, ids, "glTF")
}

/// Checks that every `switch` / `lamp` named in extras exists in the table.
/// Errors are reported against `file` at `meshes[i].extras.switch` (or `.lamp`).
pub fn validate_gltf_bindings_in(
    scene: &GltfScene,
    ids: &TableIds,
    file: &str,
) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    for (i, mesh) in scene.meshes.iter().enumerate() {
        for (key, kind, name) in [
            ("switch", IdKind::Switch, &mesh.extras.switch),
            ("lamp", IdKind::Lamp, &mesh.extras.lamp),
        ] {
            let Some(name) = name else { continue };
            if ids.get(kind, name).is_some() {
                continue;
            }
            let hint = crate::validate::suggest_name(ids, kind, name)
                .map(|s| format!(" (did you mean {s:?}?)"))
                .unwrap_or_default();
            errors.push(ValidationError::new(
                file,
                format!("meshes[{i}].extras.{key}"),
                format!(
                    "mesh {:?}: unknown {} {name:?}{hint}",
                    mesh.name,
                    kind.label()
                ),
            ));
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_product() {
        let mut t = IDENTITY;
        t[3] = [1.0, 2.0, 3.0, 1.0];
        let mut s = IDENTITY;
        s[0][0] = 2.0;
        let m = mat_mul(&t, &s);
        assert_eq!(m[0][0], 2.0);
        assert_eq!(m[3], [1.0, 2.0, 3.0, 1.0]);
        assert_eq!(mat_mul(&IDENTITY, &t), t);
    }

    #[test]
    fn glb_split_rejects_garbage() {
        assert!(split_glb(b"glTF").is_err());
        let mut v = b"glTF".to_vec();
        v.extend(3u32.to_le_bytes());
        v.extend(12u32.to_le_bytes());
        assert!(split_glb(&v).is_err());
        let mut v = b"glTF".to_vec();
        v.extend(2u32.to_le_bytes());
        v.extend(4u32.to_le_bytes());
        assert!(split_glb(&v).is_err());
        let mut v = b"glTF".to_vec();
        v.extend(2u32.to_le_bytes());
        v.extend(20u32.to_le_bytes());
        v.extend(100u32.to_le_bytes());
        v.extend(0x4E4F_534Au32.to_le_bytes());
        assert!(split_glb(&v).is_err());
        let mut v = b"glTF".to_vec();
        v.extend(2u32.to_le_bytes());
        v.extend(20u32.to_le_bytes());
        v.extend(0u32.to_le_bytes());
        v.extend(7u32.to_le_bytes());
        assert!(split_glb(&v).is_err());
    }

    #[test]
    fn extras_parsing() {
        let raw = |s: &str| -> json::Extras {
            Some(serde_json::value::RawValue::from_string(s.to_owned()).unwrap())
        };
        let mut e = NodeExtras::default();
        apply_extras(
            &raw(r#"{"collision_layer":2,"switch":"s","lamp":"l","collider":1,"x":5}"#),
            "n",
            &mut e,
        )
        .unwrap();
        assert_eq!(
            e,
            NodeExtras {
                collision_layer: Some(2),
                switch: Some("s".into()),
                lamp: Some("l".into()),
                collider: true
            }
        );
        apply_extras(
            &raw(r#"{"collider":false,"collision_layer":3.0}"#),
            "n",
            &mut e,
        )
        .unwrap();
        assert!(!e.collider);
        assert_eq!(e.collision_layer, Some(3));
        apply_extras(&raw("[1,2]"), "n", &mut e).unwrap();
        apply_extras(&None, "n", &mut e).unwrap();
        for bad in [
            r#"{"collision_layer":8}"#,
            r#"{"collision_layer":-1}"#,
            r#"{"collision_layer":"1"}"#,
            r#"{"switch":3}"#,
            r#"{"lamp":null}"#,
            r#"{"collider":"yes"}"#,
            r#"{"collider":2}"#,
        ] {
            assert!(
                matches!(
                    apply_extras(&raw(bad), "n", &mut e),
                    Err(AssetError::Gltf(_))
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn normals_from_faces() {
        let p = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [5.0, 5.0, 5.0],
        ];
        let n = compute_normals(&p, &[0, 1, 2]);
        assert_eq!(n[0], [0.0, 0.0, 1.0]);
        assert_eq!(n[3], [0.0, 0.0, 1.0]);
    }

    #[test]
    fn bad_images_are_errors() {
        assert!(decode_image(b"nope", "i").is_err());
    }
}
