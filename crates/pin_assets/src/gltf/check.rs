//! Pre-validation of the raw glTF JSON.
//!
//! `gltf` / `gltf-json` 1.4.1 dereference some indices without checking them
//! (the POSITION min/max validation hook indexes `accessors` directly;
//! `Image::source` unwraps `mimeType` / `uri`; the wrapper types `unwrap` every
//! `nth(index)`). This pass checks every index the loader or the crate can
//! dereference, plus the image source rules, *before* the document is handed
//! to `gltf`'s own validation, so a hostile file is an error, not a panic.

use super::gerr;
use crate::AssetError;
use ::gltf::json::{Index, Root};

fn check<T>(index: &Index<T>, len: usize, what: &dyn Fn() -> String) -> Result<(), AssetError> {
    if index.value() < len {
        Ok(())
    } else {
        Err(gerr(format!(
            "{}: index {} out of range ({len} available)",
            what(),
            index.value()
        )))
    }
}

/// Checks cross-references of the raw document.
pub(super) fn prevalidate(root: &Root) -> Result<(), AssetError> {
    let (n_acc, n_views, n_bufs) = (
        root.accessors.len(),
        root.buffer_views.len(),
        root.buffers.len(),
    );
    for (v, view) in root.buffer_views.iter().enumerate() {
        check(&view.buffer, n_bufs, &|| format!("bufferViews[{v}].buffer"))?;
    }
    for (a, acc) in root.accessors.iter().enumerate() {
        if let Some(view) = &acc.buffer_view {
            check(view, n_views, &|| format!("accessors[{a}].bufferView"))?;
        }
        if let Some(sparse) = &acc.sparse {
            check(&sparse.indices.buffer_view, n_views, &|| {
                format!("accessors[{a}].sparse.indices.bufferView")
            })?;
            check(&sparse.values.buffer_view, n_views, &|| {
                format!("accessors[{a}].sparse.values.bufferView")
            })?;
        }
    }
    for (m, mesh) in root.meshes.iter().enumerate() {
        for (p, prim) in mesh.primitives.iter().enumerate() {
            let at = |field: &str| format!("meshes[{m}].primitives[{p}].{field}");
            for index in prim.attributes.values() {
                check(index, n_acc, &|| at("attributes"))?;
            }
            if let Some(i) = &prim.indices {
                check(i, n_acc, &|| at("indices"))?;
            }
            if let Some(mat) = &prim.material {
                check(mat, root.materials.len(), &|| at("material"))?;
            }
            for target in prim.targets.iter().flatten() {
                for i in [&target.positions, &target.normals, &target.tangents]
                    .into_iter()
                    .flatten()
                {
                    check(i, n_acc, &|| at("targets"))?;
                }
            }
        }
    }
    for (i, image) in root.images.iter().enumerate() {
        match (&image.uri, &image.buffer_view) {
            (Some(_), None) => {}
            (None, Some(view)) => {
                check(view, n_views, &|| format!("images[{i}].bufferView"))?;
                if image.mime_type.is_none() {
                    return Err(gerr(format!(
                        "images[{i}]: an image in a bufferView needs a mimeType"
                    )));
                }
            }
            _ => {
                return Err(gerr(format!(
                    "images[{i}]: exactly one of uri and bufferView is required"
                )));
            }
        }
    }
    for (t, texture) in root.textures.iter().enumerate() {
        check(&texture.source, root.images.len(), &|| {
            format!("textures[{t}].source")
        })?;
        if let Some(s) = &texture.sampler {
            check(s, root.samplers.len(), &|| format!("textures[{t}].sampler"))?;
        }
    }
    let n_tex = root.textures.len();
    for (m, mat) in root.materials.iter().enumerate() {
        let pbr = &mat.pbr_metallic_roughness;
        let refs = [
            pbr.base_color_texture.as_ref().map(|t| &t.index),
            pbr.metallic_roughness_texture.as_ref().map(|t| &t.index),
            mat.normal_texture.as_ref().map(|t| &t.index),
            mat.occlusion_texture.as_ref().map(|t| &t.index),
            mat.emissive_texture.as_ref().map(|t| &t.index),
        ];
        for index in refs.into_iter().flatten() {
            check(index, n_tex, &|| format!("materials[{m}] texture"))?;
        }
    }
    let n_nodes = root.nodes.len();
    for (n, node) in root.nodes.iter().enumerate() {
        if let Some(mesh) = &node.mesh {
            check(mesh, root.meshes.len(), &|| format!("nodes[{n}].mesh"))?;
        }
        if let Some(cam) = &node.camera {
            check(cam, root.cameras.len(), &|| format!("nodes[{n}].camera"))?;
        }
        if let Some(skin) = &node.skin {
            check(skin, root.skins.len(), &|| format!("nodes[{n}].skin"))?;
        }
        for c in node.children.iter().flatten() {
            check(c, n_nodes, &|| format!("nodes[{n}].children"))?;
        }
    }
    for (s, scene) in root.scenes.iter().enumerate() {
        for node in &scene.nodes {
            check(node, n_nodes, &|| format!("scenes[{s}].nodes"))?;
        }
    }
    if let Some(scene) = &root.scene {
        check(scene, root.scenes.len(), &|| "scene".to_owned())?;
    }
    Ok(())
}
