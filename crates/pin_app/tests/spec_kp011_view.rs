//! KP-011 / #91 Independent Tester GPU spec tests: Apex Circuit player-view
//! two-sidedness acceptance criterion and `TableRenderer` end-to-end oblique
//! back-face propagation (REQ-VIS-001, docs/SPEC.md §7.4, D30a).
#![cfg(feature = "gpu-tests")]
#![allow(clippy::needless_range_loop)]

mod apex_support;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use apex_support::{apex_bundle, on_big_stack, scratch};
use pin_app::orchestrator::{
    TableRenderOptions, TableRenderer, TableVisuals, VisualMesh, load_checked, validate_table,
};
use pin_core::snapshot::FrameStateSnapshot;
use pin_render::displays::RgbaImage;
use pin_render::gpu::{GpuContext, context_for_tests, wgpu};
use pin_render::pbr::math::PbrCamera;
use pin_render::pbr::{DrawItem, Environment, FrameDesc, MeshId, PointLight};
use pin_render::post::{FrameRenderer, PostConfig};
use pin_sim::SimConfig;

const VIEW_W: u32 = 400;
const VIEW_H: u32 = 800;

fn gpu() -> &'static GpuContext {
    static GPU: OnceLock<Option<GpuContext>> = OnceLock::new();
    GPU.get_or_init(|| context_for_tests("spec_kp011_view"))
        .as_ref()
        .expect(
            "spec_kp011_view needs a GPU adapter (PIN_REQUIRE_GPU=1 and a working \
             Vulkan/Metal/DX12 adapter, e.g. lavapipe); it never skips",
        )
}

fn apex_visuals() -> (TableVisuals, FrameStateSnapshot) {
    let dir = apex_bundle().to_path_buf();
    on_big_stack(move || {
        let checked = validate_table(&dir).unwrap_or_else(|e| panic!("validate_table: {e}"));
        let sim = load_checked(&checked, SimConfig::default())
            .unwrap_or_else(|e| panic!("load_checked: {e}"));
        (TableVisuals::of(&checked), *sim.snapshot())
    })
}

/// Replaces every `VisualMesh`'s triangle winding (`[a, b, c]` -> `[a, c, b]`)
/// and negates normals while preserving all other fields.
fn with_replaced_reversed_winding(visuals: &TableVisuals) -> TableVisuals {
    let meshes = visuals
        .meshes
        .iter()
        .map(|m| {
            let mut rev_indices = Vec::with_capacity(m.indices.len());
            for tri in m.indices.chunks_exact(3) {
                rev_indices.push(tri[0]);
                rev_indices.push(tri[2]);
                rev_indices.push(tri[1]);
            }
            let rev_normals: Vec<[f32; 3]> =
                m.normals.iter().map(|n| [-n[0], -n[1], -n[2]]).collect();
            VisualMesh {
                name: format!("{}_rev", m.name),
                positions: m.positions.clone(),
                normals: rev_normals,
                indices: rev_indices,
                model: m.model,
                material: m.material,
                transparent: m.transparent,
                lamp: m.lamp,
                double_sided: m.double_sided,
            }
        })
        .collect();
    TableVisuals {
        meshes,
        flippers: visuals.flippers.clone(),
        bounds: visuals.bounds,
        displays: None,
        debug_lines: Vec::new(),
        warnings: Vec::new(),
    }
}

/// Appends a reversed-winding copy (`[a, c, b]` with negated normals) alongside the
/// original for every opaque `VisualMesh` (`!m.transparent`). Transparent `ClearAcrylic`
/// meshes keep a single copy because they already render two-pass back+front with depth
/// writes disabled (duplicating them would double-blend the same translucent layer;
/// see DECISIONS.md "KP-011 (#91)").
fn with_opaque_reversed_winding_copies(visuals: &TableVisuals) -> TableVisuals {
    let mut meshes = Vec::with_capacity(visuals.meshes.len() * 2);
    for m in &visuals.meshes {
        meshes.push(m.clone());
        if !m.transparent {
            let mut rev_indices = Vec::with_capacity(m.indices.len());
            for tri in m.indices.chunks_exact(3) {
                rev_indices.push(tri[0]);
                rev_indices.push(tri[2]);
                rev_indices.push(tri[1]);
            }
            let rev_normals: Vec<[f32; 3]> =
                m.normals.iter().map(|n| [-n[0], -n[1], -n[2]]).collect();
            meshes.push(VisualMesh {
                name: format!("{}_rev", m.name),
                positions: m.positions.clone(),
                normals: rev_normals,
                indices: rev_indices,
                model: m.model,
                material: m.material,
                transparent: m.transparent,
                lamp: m.lamp,
                double_sided: m.double_sided,
            });
        }
    }
    TableVisuals {
        meshes,
        flippers: visuals.flippers.clone(),
        bounds: visuals.bounds,
        displays: None,
        debug_lines: Vec::new(),
        warnings: Vec::new(),
    }
}

fn with_double_sided(visuals: &TableVisuals, double_sided: bool) -> TableVisuals {
    TableVisuals {
        meshes: visuals
            .meshes
            .iter()
            .map(|m| VisualMesh {
                double_sided,
                ..m.clone()
            })
            .collect(),
        flippers: visuals.flippers.clone(),
        bounds: visuals.bounds,
        displays: None,
        debug_lines: Vec::new(),
        warnings: Vec::new(),
    }
}

fn render_player_view(
    ctx: &GpuContext,
    visuals: &TableVisuals,
    width: u32,
    height: u32,
) -> (pin_render::pbr::HdrImage, pin_render::post::LdrImage) {
    let mut fr =
        FrameRenderer::new(ctx, width, height, PostConfig::default()).expect("FrameRenderer::new");
    let mesh_ids: Vec<MeshId> = visuals
        .meshes
        .iter()
        .map(|m| {
            fr.pbr_mut()
                .upload_mesh_data(&m.positions, &m.normals, &m.indices)
                .expect("upload_mesh_data")
        })
        .collect();
    let draws: Vec<DrawItem> = visuals
        .meshes
        .iter()
        .zip(&mesh_ids)
        .map(|(m, &mesh)| DrawItem {
            mesh,
            model: m.model,
            material: m.material,
            transparent: m.transparent,
            emission: 0.0,
            double_sided: m.double_sided,
        })
        .collect();

    let (min_b, max_b) = visuals.bounds;
    let cx = 0.5 * (min_b[0] + max_b[0]);
    let cy = 0.5 * (min_b[1] + max_b[1]);
    // Angled player perspective from below the front/apron looking up toward the backbox,
    // so vertical walls and curved lanes are seen obliquely and from behind.
    let cam = PbrCamera::look_at(
        [cx, min_b[1] - 0.35, 0.55],
        [cx, cy, 0.02],
        [0.0, 0.0, 1.0],
        45.0f32.to_radians(),
        width as f32 / height as f32,
        0.02,
        10.0,
    );
    let lights = [
        PointLight::new([cx, cy, 0.75], [1.0, 0.98, 0.95], 1.5, 5.0),
        PointLight::new(
            [cx - 0.15, min_b[1] + 0.15, 0.45],
            [0.9, 0.95, 1.0],
            0.8,
            5.0,
        ),
    ];
    let mut frame = FrameDesc::new(&cam, &lights, &draws);
    frame.environment = Environment::STUDIO;
    frame.gi = [0.2, 0.2, 0.25, 0.3];
    frame.clear = [0.02, 0.02, 0.04, 1.0];

    let out = fr.render(ctx, &frame, &[]).expect("FrameRenderer::render");
    (out.hdr, out.ldr)
}

fn diff_pct(
    a_hdr: &pin_render::pbr::HdrImage,
    b_hdr: &pin_render::pbr::HdrImage,
    a_ldr: &pin_render::post::LdrImage,
    b_ldr: &pin_render::post::LdrImage,
) -> (usize, f64, usize, f64) {
    let total = a_ldr.pixels.len();
    let ldr_diff = a_ldr
        .pixels
        .iter()
        .zip(&b_ldr.pixels)
        .filter(|(p, q)| (0..3).any(|c| p[c].abs_diff(q[c]) > 2))
        .count();
    let hdr_diff = a_hdr
        .pixels
        .iter()
        .zip(&b_hdr.pixels)
        .filter(|(p, q)| {
            (0..3).any(|c| (p[c] - q[c]).abs() > 2.0e-3 + 2.0e-2 * p[c].abs().max(q[c].abs()))
        })
        .count();
    (
        ldr_diff,
        ldr_diff as f64 * 100.0 / total as f64,
        hdr_diff,
        hdr_diff as f64 * 100.0 / total as f64,
    )
}

/// REQ-VIS-001: Every `VisualMesh` generated from Apex Circuit's `playfield.glb`
/// has `double_sided == true`, and an angled player-view render of Apex Circuit
/// changes fewer than 0.1 % of pixels both when reversed-winding copies of all
/// opaque meshes are appended alongside the originals and when every mesh (opaque
/// and transparent) has its winding replaced by the reverse.
#[test]
fn req_vis_001_apex_player_view_matches_with_reversed_winding_copies() {
    let ctx = gpu();
    let (visuals, _snap) = apex_visuals();
    assert!(
        visuals.warnings.is_empty(),
        "Apex Circuit visuals warnings: {:?}",
        visuals.warnings
    );
    assert!(!visuals.meshes.is_empty(), "Apex Circuit must have meshes");

    for m in &visuals.meshes {
        assert!(
            m.double_sided,
            "Apex Circuit playfield mesh {:?} must have double_sided == true",
            m.name
        );
    }

    let opaque_doubled = with_opaque_reversed_winding_copies(&visuals);
    let all_replaced = with_replaced_reversed_winding(&visuals);
    let (hdr_orig, ldr_orig) = render_player_view(ctx, &visuals, VIEW_W, VIEW_H);
    let (hdr_opq_dbl, ldr_opq_dbl) = render_player_view(ctx, &opaque_doubled, VIEW_W, VIEW_H);
    let (hdr_all_rep, ldr_all_rep) = render_player_view(ctx, &all_replaced, VIEW_W, VIEW_H);

    // (a) Analytic sanity (D30a): the angled player view is genuinely lit and non-uniform.
    let mut lums = Vec::with_capacity(hdr_orig.pixels.len());
    for p in &hdr_orig.pixels {
        assert!(
            p.iter().all(|c| c.is_finite() && *c >= 0.0),
            "non-finite HDR pixel: {p:?}"
        );
        lums.push(0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]);
    }
    let n = lums.len() as f32;
    let mean_lum = lums.iter().sum::<f32>() / n;
    let stddev_lum = (lums.iter().map(|&l| (l - mean_lum).powi(2)).sum::<f32>() / n).sqrt();
    assert!(
        mean_lum > 0.03,
        "player view is too dark/blank: mean_lum={mean_lum}"
    );
    assert!(
        stddev_lum > 0.01,
        "player view is uniform/flat: stddev_lum={stddev_lum}"
    );

    // (b) Fewer than 0.1 % of pixels differ for opaque appended copies and for all-mesh
    // replaced-winding copies.
    let total_pixels = (VIEW_W * VIEW_H) as usize;
    let (opq_ldr, opq_ldr_pct, opq_hdr, opq_hdr_pct) =
        diff_pct(&hdr_orig, &hdr_opq_dbl, &ldr_orig, &ldr_opq_dbl);
    let (rep_ldr, rep_ldr_pct, rep_hdr, rep_hdr_pct) =
        diff_pct(&hdr_orig, &hdr_all_rep, &ldr_orig, &ldr_all_rep);

    println!(
        "Apex player view ({VIEW_W}x{VIEW_H}): opaque-appended LDR {opq_ldr}/{total_pixels} ({opq_ldr_pct:.4}%), HDR {opq_hdr}/{total_pixels} ({opq_hdr_pct:.4}%)"
    );
    println!(
        "Apex player view ({VIEW_W}x{VIEW_H}): all-replaced LDR {rep_ldr}/{total_pixels} ({rep_ldr_pct:.4}%), HDR {rep_hdr}/{total_pixels} ({rep_hdr_pct:.4}%)"
    );

    assert!(
        opq_ldr_pct < 0.1 && opq_hdr_pct < 0.1,
        "Apex player view changed {opq_ldr_pct:.4}% LDR / {opq_hdr_pct:.4}% HDR (>= 0.1%) when opaque reversed-winding copies were added"
    );
    assert!(
        rep_ldr_pct < 0.1 && rep_hdr_pct < 0.1,
        "Apex player view changed {rep_ldr_pct:.4}% LDR / {rep_hdr_pct:.4}% HDR (>= 0.1%) when all meshes had reversed winding"
    );

    // Discriminating control: with `double_sided = false`, both comparisons change > 5% of pixels,
    // proving the camera view exposes many single-winding back faces.
    let ss_orig = with_double_sided(&visuals, false);
    let ss_opq_dbl = with_double_sided(&opaque_doubled, false);
    let ss_all_rep = with_double_sided(&all_replaced, false);
    let (hdr_ss_orig, ldr_ss_orig) = render_player_view(ctx, &ss_orig, VIEW_W, VIEW_H);
    let (hdr_ss_dbl, ldr_ss_dbl) = render_player_view(ctx, &ss_opq_dbl, VIEW_W, VIEW_H);
    let (hdr_ss_rep, ldr_ss_rep) = render_player_view(ctx, &ss_all_rep, VIEW_W, VIEW_H);
    let (_, ss_dbl_pct, _, _) = diff_pct(&hdr_ss_orig, &hdr_ss_dbl, &ldr_ss_orig, &ldr_ss_dbl);
    let (_, ss_rep_pct, _, _) = diff_pct(&hdr_ss_orig, &hdr_ss_rep, &ldr_ss_orig, &ldr_ss_rep);
    assert!(
        ss_dbl_pct > 5.0 && ss_rep_pct > 50.0,
        "player view is not sufficiently discriminating with double_sided=false: opq_dbl={ss_dbl_pct:.2}%, rep={ss_rep_pct:.2}%"
    );
}

/// Builds a minimal valid binary glTF 2.0 (`.glb`) containing one single-winding
/// vertical wall quad near `x = 0.45 m` (inside a 0.514 m x 1.070 m table) whose
/// front face normal points in `+x` (away from the playfield centre at `x = 0.257 m`),
/// so `TableRenderer`'s perspective camera sees ONLY its back face (`-x`).
/// Also includes a dark playfield floor quad at `z = 0` so `TableVisuals::of` has
/// valid table bounds.
fn build_oblique_wall_glb(wall_double_sided: bool) -> Vec<u8> {
    // Wall quad at x = 0.46 m, spanning y = 0.30..0.77 m, z = 0.0..0.22 m.
    // Front face CCW looking from +x toward -x:
    // v0=(0.46, 0.30, 0.0), v1=(0.46, 0.77, 0.0), v2=(0.46, 0.77, 0.22), v3=(0.46, 0.30, 0.22)
    // Edge (v1-v0) x (v2-v0) = (0, 0.47, 0) x (0, 0.47, 0.22) = (+0.1034, 0, 0) -> normal +X!
    // Viewed from table centre (x ~ 0.257), camera looks from -x toward +x, seeing the BACK face!
    let wall_pos: [[f32; 3]; 4] = [
        [0.46, 0.30, 0.0],
        [0.46, 0.77, 0.0],
        [0.46, 0.77, 0.22],
        [0.46, 0.30, 0.22],
    ];
    let wall_nrm: [[f32; 3]; 4] = [[1.0, 0.0, 0.0]; 4];
    let idx: [u32; 6] = [0, 1, 2, 0, 2, 3];

    // Playfield floor quad at z = 0.0, facing +z, spanning [0, 0.514] x [0, 1.070].
    let floor_pos: [[f32; 3]; 4] = [
        [0.0, 0.0, 0.0],
        [0.514, 0.0, 0.0],
        [0.514, 1.070, 0.0],
        [0.0, 1.070, 0.0],
    ];
    let floor_nrm: [[f32; 3]; 4] = [[0.0, 0.0, 1.0]; 4];

    let mut bin = Vec::new();
    let push_v3 = |b: &mut Vec<u8>, data: &[[f32; 3]]| -> (usize, usize) {
        let off = b.len();
        for v in data {
            for c in v {
                b.extend_from_slice(&c.to_le_bytes());
            }
        }
        (off, b.len() - off)
    };
    let push_idx = |b: &mut Vec<u8>, data: &[u32]| -> (usize, usize) {
        let off = b.len();
        for v in data {
            b.extend_from_slice(&v.to_le_bytes());
        }
        (off, b.len() - off)
    };

    let (wp_off, wp_len) = push_v3(&mut bin, &wall_pos);
    let (wn_off, wn_len) = push_v3(&mut bin, &wall_nrm);
    let (wi_off, wi_len) = push_idx(&mut bin, &idx);
    let (fp_off, fp_len) = push_v3(&mut bin, &floor_pos);
    let (fn_off, fn_len) = push_v3(&mut bin, &floor_nrm);
    let (fi_off, fi_len) = push_idx(&mut bin, &idx);

    // Bright red/white wall material vs dark wood floor so the wall's back face stands out clearly.
    let json = format!(
        r#"{{
  "asset": {{"version": "2.0"}},
  "scene": 0,
  "scenes": [{{"nodes": [0, 1]}}],
  "nodes": [
    {{"name": "playfield", "mesh": 0}},
    {{"name": "oblique_wall", "mesh": 1}}
  ],
  "meshes": [
    {{"name": "playfield", "primitives": [{{"attributes": {{"POSITION": 3, "NORMAL": 4}}, "indices": 5, "material": 0}}]}},
    {{"name": "oblique_wall", "primitives": [{{"attributes": {{"POSITION": 0, "NORMAL": 1}}, "indices": 2, "material": 1}}]}}
  ],
  "materials": [
    {{"name": "wood", "pbrMetallicRoughness": {{"baseColorFactor": [0.05, 0.05, 0.05, 1.0], "metallicFactor": 0.0, "roughnessFactor": 0.9}}, "doubleSided": false}},
    {{"name": "wall_bright", "pbrMetallicRoughness": {{"baseColorFactor": [0.9, 0.1, 0.1, 1.0], "metallicFactor": 0.0, "roughnessFactor": 0.5}}, "emissiveFactor": [0.6, 0.1, 0.1], "doubleSided": {wall_double_sided}}}
  ],
  "accessors": [
    {{"bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3", "min": [0.46, 0.30, 0.0], "max": [0.46, 0.77, 0.22]}},
    {{"bufferView": 1, "componentType": 5126, "count": 4, "type": "VEC3"}},
    {{"bufferView": 2, "componentType": 5125, "count": 6, "type": "SCALAR"}},
    {{"bufferView": 3, "componentType": 5126, "count": 4, "type": "VEC3", "min": [0.0, 0.0, 0.0], "max": [0.514, 1.070, 0.0]}},
    {{"bufferView": 4, "componentType": 5126, "count": 4, "type": "VEC3"}},
    {{"bufferView": 5, "componentType": 5125, "count": 6, "type": "SCALAR"}}
  ],
  "bufferViews": [
    {{"buffer": 0, "byteOffset": {wp_off}, "byteLength": {wp_len}}},
    {{"buffer": 0, "byteOffset": {wn_off}, "byteLength": {wn_len}}},
    {{"buffer": 0, "byteOffset": {wi_off}, "byteLength": {wi_len}}},
    {{"buffer": 0, "byteOffset": {fp_off}, "byteLength": {fp_len}}},
    {{"buffer": 0, "byteOffset": {fn_off}, "byteLength": {fn_len}}},
    {{"buffer": 0, "byteOffset": {fi_off}, "byteLength": {fi_len}}}
  ],
  "buffers": [{{"byteLength": {}}}]
}}"#,
        bin.len()
    );

    let mut j = json.into_bytes();
    while !j.len().is_multiple_of(4) {
        j.push(b' ');
    }
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let total = 12 + 8 + j.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(j.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&j);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    out
}

fn minimal_table_toml() -> &'static str {
    r##"schema_version = 1

[meta]
id = "kp011_oblique_wall"
name = "KP011 Oblique Wall"
author = "Test"
version = "1.0.0"
script_entry = "rules.lua"
playfield_mesh = "playfield.glb"
pitch_deg = 6.5
players_max = 1
palette = ["#101010", "#202020", "#ff3030"]

[balls]
profiles = ["standard_steel"]

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
id = "trough_1"
number = 10
kind = "trough"
layer = 0
position = [257.0, 20.0, 0.0]
radius = 30.0

[[coils]]
id = "left_flipper_coil"
number = 1
kind = "flipper"

[[switches]]
id = "left_flipper_eos"
number = 12
kind = "eos"

[[flippers]]
id = "left_flipper"
coil = "left_flipper_coil"
eos_switch = "left_flipper_eos"
collision_layer = 0
base_position = [180.0, 120.0, 0.0]
surface_normal = [0.0, 0.0, 1.0]
rest_angle_deg = -30.0
sweep_angle_deg = 50.0
eos_angle_fraction = 0.92
shape = { type = "StraightCapsule", length = 76.0, base_radius = 11.0, tip_radius = 6.5 }
staging = { type = "DedicatedButton", input = "left_flipper" }

[persist]
keys = []
"##
}

fn minimal_backglass_toml() -> &'static str {
    r#"schema_version = 1

[translite]
layers = []

[score_display]
mode = "dmd"
dmd = { width = 128, height = 32, color = "orange" }
lcd = { width = 1920, height = 1080 }
"#
}

fn minimal_glass_toml() -> &'static str {
    "schema_version = 1\nheight = 112.0\n"
}

fn write_synthetic_bundle(tag: &str, wall_double_sided: bool) -> PathBuf {
    let dir = scratch(tag);
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    files.insert(
        "table.toml".to_owned(),
        minimal_table_toml().as_bytes().to_vec(),
    );
    files.insert(
        "backglass.toml".to_owned(),
        minimal_backglass_toml().as_bytes().to_vec(),
    );
    files.insert(
        "glass.toml".to_owned(),
        minimal_glass_toml().as_bytes().to_vec(),
    );
    files.insert(
        "rules.lua".to_owned(),
        b"function on_tick(ctx) end\nfunction on_switch(ctx, id, closed) end\n".to_vec(),
    );
    files.insert(
        "playfield.glb".to_owned(),
        build_oblique_wall_glb(wall_double_sided),
    );
    for (name, data) in files {
        std::fs::write(dir.join(name), data).expect("write synthetic bundle file");
    }
    dir
}

fn project_table_px(r: &TableRenderer, p: [f32; 3], w: u32, h: u32) -> (i32, i32) {
    let m = pin_render::pbr::model_view_proj(r.camera(), &pin_render::pbr::math::IDENTITY);
    let c = |row: usize| (0..3).map(|k| m[k][row] * p[k]).sum::<f32>() + m[3][row];
    let (x, y, ww) = (c(0), c(1), c(3));
    let (nx, ny) = (x / ww, y / ww);
    (
        ((nx * 0.5 + 0.5) * w as f32).round() as i32,
        ((0.5 - ny * 0.5) * h as f32).round() as i32,
    )
}

fn render_synthetic_table(
    ctx: &GpuContext,
    tag: &str,
    wall_double_sided: bool,
) -> (TableVisuals, RgbaImage, (i32, i32), (i32, i32)) {
    let dir = write_synthetic_bundle(tag, wall_double_sided);
    let (visuals, snap) = on_big_stack(move || {
        let checked = validate_table(&dir).unwrap_or_else(|e| panic!("validate_table: {e}"));
        let sim = load_checked(&checked, SimConfig::default())
            .unwrap_or_else(|e| panic!("load_checked: {e}"));
        (TableVisuals::of(&checked), *sim.snapshot())
    });
    let mut r = TableRenderer::new(
        ctx,
        wgpu::TextureFormat::Rgba8Unorm,
        (200, 400),
        &visuals,
        TableRenderOptions::default(),
    )
    .expect("TableRenderer::new");
    let img = r
        .render_to_image(ctx, &[], &snap)
        .expect("TableRenderer::render_to_image");
    // Sample midpoint of the wall's back face projected onto the 200x400 image:
    // Wall spans x=0.46, y=0.30..0.77, z=0.0..0.22 -> midpoint (0.46, 0.535, 0.11).
    let p_top = project_table_px(&r, [0.46, 0.535, 0.21], 200, 400);
    let p_bot = project_table_px(&r, [0.46, 0.535, 0.01], 200, 400);
    (visuals, img, p_bot, p_top)
}

/// REQ-VIS-001: End-to-end `TableRenderer` test verifying `TableVisuals::of` and
/// `TableRenderer::new` propagate `double_sided` from glTF to GPU draw items.
/// A single-winding vertical wall near the right edge of `table_camera`'s perspective
/// frustum has its back face angled toward the camera:
/// - with `"doubleSided": false`, the wall's back face is culled (shows dark floor/clear);
/// - with `"doubleSided": true`, the wall's back face is drawn (bright red wall pixels appear).
#[test]
fn req_vis_001_table_renderer_propagates_double_sided_for_oblique_back_face() {
    let ctx = gpu();

    let (vis_false, img_false, p_bot, p_top) =
        render_synthetic_table(ctx, "kp011_wall_false", false);
    let (vis_true, img_true, _, _) = render_synthetic_table(ctx, "kp011_wall_true", true);

    let wall_false = vis_false
        .meshes
        .iter()
        .find(|m| m.name == "oblique_wall")
        .expect("oblique_wall mesh (false)");
    assert!(
        !wall_false.double_sided,
        "doubleSided: false in glTF must set VisualMesh::double_sided = false"
    );

    let wall_true = vis_true
        .meshes
        .iter()
        .find(|m| m.name == "oblique_wall")
        .expect("oblique_wall mesh (true)");
    assert!(
        wall_true.double_sided,
        "doubleSided: true in glTF must set VisualMesh::double_sided = true"
    );

    // The wall's bottom (z=0.01) and top (z=0.21) project to distinct x coordinates
    // because the perspective camera sits above the table centre looking down.
    let x_min = p_bot.0.min(p_top.0).max(0) as u32;
    let x_max = p_bot.0.max(p_top.0).min(199) as u32;
    let y_mid = ((p_bot.1 + p_top.1) / 2).clamp(10, 389) as u32;
    assert!(
        x_max > x_min + 2,
        "perspective wall projection must span multiple pixels horizontally: bot={p_bot:?}, top={p_top:?}"
    );

    let mut max_red_false = 0u8;
    let mut max_red_true = 0u8;
    let mut red_band_pixels = 0usize;
    for y in (y_mid - 15)..=(y_mid + 15) {
        for x in x_min..=x_max {
            let pf = img_false.pixel(x, y);
            let pt = img_true.pixel(x, y);
            max_red_false = max_red_false.max(pf[0]);
            max_red_true = max_red_true.max(pt[0]);
            if pt[0] > pf[0].saturating_add(40) && pt[0] > pt[1].saturating_add(30) {
                red_band_pixels += 1;
            }
        }
    }

    println!(
        "Oblique wall back-face projection x={x_min}..={x_max}, y_mid={y_mid}: \
         max_red_false={max_red_false}, max_red_true={max_red_true}, red_band_pixels={red_band_pixels}"
    );
    assert!(
        max_red_true > max_red_false + 50,
        "TableRenderer failed to draw double-sided oblique wall back face: \
         max_red_false={max_red_false}, max_red_true={max_red_true}"
    );
    assert!(
        red_band_pixels >= 20,
        "expected >= 20 lit red back-face wall pixels in TableRenderer output, got {red_band_pixels}"
    );
}
