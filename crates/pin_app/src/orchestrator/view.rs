//! The table as the render thread draws it (Subphase 6.0 wiring).
//!
//! * [`TableVisuals`] (GPU-free): the playfield glTF's meshes with their
//!   materials (5.1 [`MaterialKind`] by glTF material name, plain dielectric
//!   factors otherwise), lamp-tagged meshes, the flipper outlines, the
//!   table bounds, and the display assets of `backglass.toml` and the
//!   `[[scenes]]` ([`DisplayAssets`]), all read from the validated bundle
//!   snapshot.
//! * [`TableRenderer`] (GPU): the 5.1 [`PbrRenderer`] draws the table,
//!   balls and flippers of each [`FrameStateSnapshot`] into its HDR target;
//!   the 5.2 [`PostChain`] tone-maps it into the playfield target; the 5.3
//!   [`DisplaySet`] draws the backglass and score display (DMD / LCD) into
//!   their viewports (PiPs on the playfield window or their own windows);
//!   the 1.4 debug lines are an optional overlay
//!   ([`TableRenderOptions::debug_overlay`], default off). The displays may
//!   be built for a different layer format than the target (the non-sRGB
//!   twin of an `*Srgb` target, issue #10: [`TableRenderer::with_layer_format`]),
//!   and are then drawn through a [`LayerCompositor`]
//!   ([`TableRenderer::encode_composited`]).
//! * [`TableRenderer::render_to_image`]: the same frame offscreen, read
//!   back (analytic pixel tests, D30).

use super::hud::HudOverlay;
use super::table::{DisplayCommand, FlipperOutline, ValidatedTable, table_lines};
use super::threads::FLIPPER_LINE_COLOR;
use pin_assets::IdKind;
use pin_core::LampId;
use pin_core::schema::table::LampKind;
use pin_core::snapshot::FrameStateSnapshot;
use pin_render::debug::{Camera, DebugLine, DebugLines, DebugRenderer, snapshot_lines};
use pin_render::displays::{
    BackglassDesc, BackglassRenderer, DisplayRoute, DisplaySet, RgbaImage, SceneLibrary,
    ScoreDisplayConfig, ScoreDisplayModel, ScoreRenderer,
};
use pin_render::gpu::{GpuContext, GpuError, LayerCompositor, LayerTarget, wgpu};
use pin_render::pbr::{
    DrawItem, Environment, FrameDesc, MaterialKind, MaterialParams, MeshGeometry, MeshId,
    PbrCamera, PbrRenderer, PointLight,
};
use pin_render::post::{MotionBall, PostChain, PostConfig, PostFrame};
use pin_render::topology::{Rect, SurfaceRole};

/// Column-major 4×4 (`m[column][row]`), as `pin_render::pbr::math`.
pub type Mat4 = [[f32; 4]; 4];

/// One mesh of the table.
#[derive(Clone, Debug, PartialEq)]
pub struct VisualMesh {
    /// Node name.
    pub name: String,
    /// Object-space positions.
    pub positions: Vec<[f32; 3]>,
    /// Object-space unit normals (computed from the faces when the glTF
    /// has none).
    pub normals: Vec<[f32; 3]>,
    /// Triangle list.
    pub indices: Vec<u32>,
    /// Object → table frame.
    pub model: Mat4,
    /// Surface parameters.
    pub material: MaterialParams,
    /// Drawn in the blended pass.
    pub transparent: bool,
    /// Lamp (`LampId` index) whose level drives the mesh's emission
    /// (glTF extras `lamp`).
    pub lamp: Option<usize>,
    /// Whether opaque draws render both front and back faces
    /// (glTF material `doubleSided`).
    pub double_sided: bool,
}

/// The backglass and score display content of a table.
#[derive(Clone, Debug)]
pub struct DisplayAssets {
    /// DMD or LCD, from `backglass.toml` `[score_display]`.
    pub score: ScoreDisplayConfig,
    /// The `[[scenes]]` (missing files fall back to placeholders).
    pub scenes: SceneLibrary,
    /// The decoded images of the scenes' `kind = "image"` layers, by
    /// bundle path (each file once, path order). A file that cannot be
    /// read or decoded is missing here (a warning says why) and its layers
    /// are not drawn.
    pub scene_images: Vec<(String, RgbaImage)>,
    /// The translite layers; `None` when they could not be loaded (the
    /// reason is in [`TableVisuals::warnings`]).
    pub backglass: Option<BackglassDesc>,
}

/// Everything the render thread draws of a table (GPU-free).
#[derive(Clone, Debug)]
pub struct TableVisuals {
    /// Table meshes.
    pub meshes: Vec<VisualMesh>,
    /// Flipper outlines (`FlipperId` order).
    pub flippers: Vec<FlipperOutline>,
    /// Table bounds (min, max), m.
    pub bounds: ([f32; 3], [f32; 3]),
    /// Displays; `None` when `[score_display]` is invalid.
    pub displays: Option<DisplayAssets>,
    /// Collider outlines for the optional debug overlay.
    pub debug_lines: Vec<DebugLine>,
    /// Non-fatal problems (missing images, invalid display settings).
    pub warnings: Vec<String>,
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-12 && l.is_finite() {
        [v[0] / l, v[1] / l, v[2] / l]
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// Area-weighted vertex normals of a triangle list (`+z` for vertices of
/// degenerate triangles only).
pub fn vertex_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut acc = vec![[0.0f32; 3]; positions.len()];
    for t in indices.chunks_exact(3) {
        let [a, b, c] = [t[0], t[1], t[2]].map(|i| i as usize);
        if a >= positions.len() || b >= positions.len() || c >= positions.len() {
            continue;
        }
        let n = cross(
            sub(positions[b], positions[a]),
            sub(positions[c], positions[a]),
        );
        for i in [a, b, c] {
            for k in 0..3 {
                acc[i][k] += n[k];
            }
        }
    }
    acc.into_iter().map(normalize).collect()
}

fn transform(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
    let mut o = [0.0f32; 3];
    for (r, v) in o.iter_mut().enumerate() {
        *v = m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r];
    }
    o
}

fn decode_image(bytes: &[u8]) -> Result<RgbaImage, String> {
    let img = image::load_from_memory(bytes)
        .map_err(|e| e.to_string())?
        .to_rgba8();
    let (w, h) = img.dimensions();
    RgbaImage::from_rgba(w, h, img.into_raw()).ok_or_else(|| "bad image size".to_owned())
}

/// Decodes every image the first `count` scenes of `scenes` reference
/// (`kind = "image"` layers; `SceneId(i)` for `i < count`, as
/// [`SceneLibrary::load`] numbers them), each file once, in path order.
/// `read` returns a bundle file's bytes; a file that cannot be read or
/// decoded adds a warning and is left out.
pub fn load_scene_images(
    scenes: &SceneLibrary,
    count: usize,
    mut read: impl FnMut(&str) -> Result<Vec<u8>, String>,
    warnings: &mut Vec<String>,
) -> Vec<(String, RgbaImage)> {
    let mut files: Vec<&str> = (0..count.min(usize::from(u16::MAX) + 1))
        .filter_map(|i| scenes.get(pin_core::SceneId(i as u16)))
        .flat_map(|s| s.image_files())
        .collect();
    files.sort_unstable();
    files.dedup();
    let mut out = Vec::with_capacity(files.len());
    for f in files {
        match read(f).and_then(|b| decode_image(&b)) {
            Ok(img) => out.push((f.to_owned(), img)),
            Err(e) => warnings.push(format!("scene image {f}: {e}")),
        }
    }
    out
}

/// Most collider outline lines kept for the debug overlay.
pub const MAX_OVERLAY_TABLE_LINES: usize = 8192;

impl TableVisuals {
    /// Builds the visuals of `checked` (meshes from its glTF, displays from
    /// its snapshot).
    pub fn of(checked: &ValidatedTable) -> TableVisuals {
        let scene = &checked.scene;
        let table = &checked.table;
        let mut warnings = Vec::new();
        let lamp_of = |name: &str| {
            table
                .ids
                .entries(IdKind::Lamp)
                .iter()
                .find(|(n, _)| n == name)
        };
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        let mut meshes = Vec::new();
        for m in &scene.meshes {
            if m.positions.is_empty() || m.indices.len() < 3 {
                continue;
            }
            for &p in &m.positions {
                let w = transform(&m.transform, p);
                for k in 0..3 {
                    if w[k].is_finite() {
                        min[k] = min[k].min(w[k]);
                        max[k] = max[k].max(w[k]);
                    }
                }
            }
            let mat = m.material.and_then(|i| scene.materials.get(i));
            let kind = match mat {
                Some(g) => render_kind(&g.name, m.extras.lamp.is_some()),
                None => MaterialKind::PlayfieldWood,
            };
            let material = match mat {
                Some(g) => MaterialParams::from_gltf_factors(
                    kind,
                    g.base_color,
                    Some(g.metallic),
                    Some(g.roughness),
                    g.emissive,
                ),
                None => kind.params(),
            };
            let normals = if m.normals.len() == m.positions.len() {
                m.normals.clone()
            } else {
                vertex_normals(&m.positions, &m.indices)
            };
            let lamp = m
                .extras
                .lamp
                .as_deref()
                .and_then(lamp_of)
                .map(|&(_, i)| usize::from(i));
            let double_sided = mat.is_some_and(|g| g.double_sided);
            meshes.push(VisualMesh {
                name: m.name.clone(),
                positions: m.positions.clone(),
                normals,
                indices: m.indices.clone(),
                model: m.transform,
                material,
                transparent: kind.is_transparent(),
                lamp,
                double_sided,
            });
        }
        for f in &table.flippers {
            let p = f.base_position_mm.map(|v| v * 1.0e-3);
            for k in 0..3 {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
            }
        }
        if min[0] > max[0] {
            min = [0.0; 3];
            max = [0.52, 1.07, 0.0];
        }
        let displays = match ScoreDisplayConfig::from_manifest(&table.backglass.score_display) {
            Ok(score) => {
                let snap = &checked.snapshot;
                let scenes = match SceneLibrary::load(&table.manifest.scenes, |f| {
                    snap.read(f).map_err(|e| e.to_string())
                }) {
                    Ok(l) => l,
                    Err(e) => {
                        warnings.push(format!("display scenes: {e}"));
                        SceneLibrary::new()
                    }
                };
                let scene_images = load_scene_images(
                    &scenes,
                    table.manifest.scenes.len(),
                    |f| snap.read(f).map_err(|e| e.to_string()),
                    &mut warnings,
                );
                let backglass = BackglassDesc::from_manifest(
                    &table.backglass,
                    |name| {
                        let id = lamp_of(name)?.1;
                        let kind = table
                            .manifest
                            .lamps
                            .iter()
                            .find(|l| l.id == name)
                            .map_or(LampKind::Insert, |l| l.kind);
                        Some((LampId(id), kind))
                    },
                    |f| {
                        snap.read(f)
                            .map_err(|e| e.to_string())
                            .and_then(|b| decode_image(&b))
                    },
                );
                let backglass = match backglass {
                    Ok(b) => Some(b),
                    Err(e) => {
                        warnings.push(format!("backglass: {e}"));
                        None
                    }
                };
                Some(DisplayAssets {
                    score,
                    scenes,
                    scene_images,
                    backglass,
                })
            }
            Err(e) => {
                warnings.push(format!("score display: {e}"));
                None
            }
        };
        TableVisuals {
            meshes,
            flippers: FlipperOutline::of(table),
            bounds: (min, max),
            displays,
            debug_lines: table_lines(scene, MAX_OVERLAY_TABLE_LINES),
            warnings,
        }
    }

    /// The playfield rectangle `(min_xy, max_xy)` in plan view.
    pub fn plan_bounds(&self) -> ([f32; 2], [f32; 2]) {
        let (a, b) = self.bounds;
        ([a[0], a[1]], [b[0], b[1]])
    }
}

/// Options of a [`TableRenderer`] (default: post defaults, overlay off).
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct TableRenderOptions {
    /// 5.2 post settings (MSAA, bloom, tone map).
    pub post: PostConfig,
    /// Draw the 1.4 debug lines (collider outlines, balls, flippers) over
    /// the PBR image. Default off.
    pub debug_overlay: bool,
    /// Draw the profiler HUD ([`HudOverlay`]) over the playfield, with the
    /// lines given to [`TableRenderer::set_hud_lines`] (`pin_app
    /// --profiler-hud`). Default off: no HUD is built and frames are
    /// unchanged.
    pub profiler_hud: bool,
}

/// The render material of a glTF material.
///
/// An exact `pbr_*` shader name wins; otherwise a lamp-tagged mesh (glTF
/// extras `lamp`, e.g. a generated insert) is a [`MaterialKind::LightInsert`]
/// so it brightens with its lamp; otherwise the physical material the name
/// denotes (the lookup `pin_sim` uses for colliders) picks acrylic and steel;
/// everything else is printed wood. Names are never matched by prefix.
pub(crate) fn render_kind(gltf_name: &str, lamp_tagged: bool) -> MaterialKind {
    if let Some(k) = MaterialKind::from_shader_name(gltf_name) {
        return k;
    }
    if lamp_tagged {
        return MaterialKind::LightInsert;
    }
    match pin_core::Material::from_name(gltf_name) {
        Some(pin_core::Material::Acrylic) => MaterialKind::ClearAcrylic,
        Some(pin_core::Material::Steel) => MaterialKind::ChromeSteel,
        _ => MaterialKind::PlayfieldWood,
    }
}

/// The perspective camera over a table: looking straight down the table
/// normal at the centre of the plan bounds from a height that shows the
/// whole table at `aspect` (width / height) with a 3 % margin.
pub fn table_camera(min_xy: [f32; 2], max_xy: [f32; 2], aspect: f32) -> PbrCamera {
    let fov = 40f32.to_radians();
    let cx = 0.5 * (min_xy[0] + max_xy[0]);
    let cy = 0.5 * (min_xy[1] + max_xy[1]);
    let half_h = 0.5 * (max_xy[1] - min_xy[1]).abs().max(0.01) * 1.03;
    let half_w = 0.5 * (max_xy[0] - min_xy[0]).abs().max(0.01) * 1.03;
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        1.0
    };
    let half = half_h.max(half_w / aspect);
    let height = half / (0.5 * fov).tan();
    PbrCamera::look_at(
        [cx, cy, height],
        [cx, cy, 0.0],
        [0.0, 1.0, 0.0],
        fov,
        aspect,
        0.05,
        height + 1.0,
    )
}

/// A unit bat: a box from `(0, −½, 0)` to `(1, ½, 1)`.
fn unit_box() -> MeshGeometry {
    let mut g = MeshGeometry::default();
    let faces: [([f32; 3], [[f32; 3]; 4]); 6] = [
        (
            [0.0, 0.0, 1.0],
            [
                [0.0, -0.5, 1.0],
                [1.0, -0.5, 1.0],
                [1.0, 0.5, 1.0],
                [0.0, 0.5, 1.0],
            ],
        ),
        (
            [0.0, 0.0, -1.0],
            [
                [0.0, -0.5, 0.0],
                [0.0, 0.5, 0.0],
                [1.0, 0.5, 0.0],
                [1.0, -0.5, 0.0],
            ],
        ),
        (
            [1.0, 0.0, 0.0],
            [
                [1.0, -0.5, 0.0],
                [1.0, 0.5, 0.0],
                [1.0, 0.5, 1.0],
                [1.0, -0.5, 1.0],
            ],
        ),
        (
            [-1.0, 0.0, 0.0],
            [
                [0.0, -0.5, 0.0],
                [0.0, -0.5, 1.0],
                [0.0, 0.5, 1.0],
                [0.0, 0.5, 0.0],
            ],
        ),
        (
            [0.0, 1.0, 0.0],
            [
                [0.0, 0.5, 0.0],
                [0.0, 0.5, 1.0],
                [1.0, 0.5, 1.0],
                [1.0, 0.5, 0.0],
            ],
        ),
        (
            [0.0, -1.0, 0.0],
            [
                [0.0, -0.5, 0.0],
                [1.0, -0.5, 0.0],
                [1.0, -0.5, 1.0],
                [0.0, -0.5, 1.0],
            ],
        ),
    ];
    for (n, quad) in faces {
        let base = g.positions.len() as u32;
        for p in quad {
            g.positions.push(p);
            g.normals.push(n);
        }
        g.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    g
}

/// Bat width and height (m) of the flipper draw.
const BAT_WIDTH_M: f32 = 0.012;
const BAT_HEIGHT_M: f32 = 0.02;

fn flipper_model(f: &FlipperOutline, angle_rad: f32) -> Mat4 {
    let a = f.rest_rad + angle_rad;
    let (s, c) = a.sin_cos();
    let [x, y, z] = f.base_m;
    [
        [c * f.length_m, s * f.length_m, 0.0, 0.0],
        [-s * BAT_WIDTH_M, c * BAT_WIDTH_M, 0.0, 0.0],
        [0.0, 0.0, BAT_HEIGHT_M, 0.0],
        [x, y, z, 1.0],
    ]
}

fn ball_model(p: [f32; 3], r: f32) -> Mat4 {
    [
        [r, 0.0, 0.0, 0.0],
        [0.0, r, 0.0, 0.0],
        [0.0, 0.0, r, 0.0],
        [p[0], p[1], p[2], 1.0],
    ]
}

/// The ball material of built-in profile `id` (its `shader` name; chrome
/// steel otherwise).
fn ball_material(id: pin_core::ProfileId) -> MaterialKind {
    pin_core::profiles::builtin_profiles()
        .nth(id.index())
        .and_then(|p| MaterialKind::from_shader_name(&p.shader))
        .unwrap_or(MaterialKind::ChromeSteel)
}

/// Key lights over the table: four warm-white points 0.5 m above the
/// playfield, spread over its length.
fn key_lights(min: [f32; 3], max: [f32; 3]) -> Vec<PointLight> {
    let cx = 0.5 * (min[0] + max[0]);
    let len = max[1] - min[1];
    (0..4)
        .map(|i| {
            let y = min[1] + len * (0.125 + 0.25 * i as f32);
            PointLight::new([cx, y, max[2] + 0.5], [1.0, 0.95, 0.88], 0.35, 4.0)
        })
        .collect()
}

/// Draws a table frame into any colour target (see the [module
/// docs](self)).
pub struct TableRenderer {
    format: wgpu::TextureFormat,
    layer_format: wgpu::TextureFormat,
    size: (u32, u32),
    pbr: PbrRenderer,
    post: PostChain,
    camera: PbrCamera,
    plan: ([f32; 2], [f32; 2]),
    lights: Vec<PointLight>,
    table_draws: Vec<(DrawItem, Option<usize>)>,
    ball_mesh: MeshId,
    bat_mesh: MeshId,
    flippers: Vec<FlipperOutline>,
    draws: Vec<DrawItem>,
    motion: Vec<MotionBall>,
    displays: DisplaySet,
    score: Option<ScoreDisplayModel>,
    overlay: Option<(DebugRenderer, DebugLines, Vec<DebugLine>, Camera)>,
    hud: Option<HudOverlay>,
    display_commands: u64,
}

impl TableRenderer {
    /// Uploads `visuals` for `width`×`height` targets of `format` (8-bit
    /// RGBA / BGRA, sRGB or not); the displays are drawn in `format` too.
    ///
    /// # Errors
    /// Invalid sizes, formats or meshes ([`GpuError`]).
    pub fn new(
        ctx: &GpuContext,
        format: wgpu::TextureFormat,
        size: (u32, u32),
        visuals: &TableVisuals,
        opts: TableRenderOptions,
    ) -> Result<TableRenderer, GpuError> {
        TableRenderer::with_layer_format(ctx, format, format, size, visuals, opts)
    }

    /// Like [`new`](Self::new), but the display renderers (score display,
    /// backglass) are built for `layer_format`
    /// ([`LayerTarget::layer_format`]: the non-sRGB twin of an `*Srgb`
    /// target, issue #10) while the post chain and the debug overlay stay
    /// on `format`. Draw such a renderer with
    /// [`encode_composited`](Self::encode_composited).
    ///
    /// # Errors
    /// Invalid sizes, formats or meshes ([`GpuError`]); a `layer_format`
    /// that is neither `format` nor (for an `*Srgb` `format`) its non-sRGB
    /// twin (an `*Srgb` layer on a non-sRGB target would blend in linear
    /// space and copy the bytes back unchanged).
    pub fn with_layer_format(
        ctx: &GpuContext,
        format: wgpu::TextureFormat,
        layer_format: wgpu::TextureFormat,
        (width, height): (u32, u32),
        visuals: &TableVisuals,
        opts: TableRenderOptions,
    ) -> Result<TableRenderer, GpuError> {
        if !layer_format_allowed(format, layer_format) {
            return Err(GpuError::InvalidArgument(format!(
                "display layer format {layer_format:?} is not allowed for target {format:?} \
                 (equal formats, or an *Srgb target with its non-sRGB twin)"
            )));
        }
        let mut pbr = PbrRenderer::new(ctx, width, height)?;
        pbr.set_msaa_samples(opts.post.aa.samples())?;
        let mut post = PostChain::new(ctx, width, height, format)?;
        post.set_config(opts.post);
        let mut table_draws = Vec::with_capacity(visuals.meshes.len());
        for m in &visuals.meshes {
            let id = pbr.upload_mesh_data(&m.positions, &m.normals, &m.indices)?;
            table_draws.push((
                DrawItem {
                    mesh: id,
                    model: m.model,
                    material: m.material,
                    transparent: m.transparent,
                    emission: 0.0,
                    double_sided: m.double_sided,
                },
                m.lamp,
            ));
        }
        let ball_mesh = pbr.upload_mesh(&MeshGeometry::uv_sphere(1.0, 24, 12))?;
        let bat_mesh = pbr.upload_mesh(&unit_box())?;
        let plan = visuals.plan_bounds();
        let mut displays = DisplaySet::default();
        let mut score = None;
        if let Some(d) = &visuals.displays {
            displays.score = Some(ScoreRenderer::new(ctx, layer_format, d.score));
            if let Some(b) = &d.backglass {
                displays.backglass = Some(BackglassRenderer::new(ctx, layer_format, b)?);
            }
            let mut model = ScoreDisplayModel::new(d.score, d.scenes.clone());
            for (file, img) in &d.scene_images {
                model.images_mut().insert(file, img.clone());
                if let Some(ScoreRenderer::Lcd(lcd)) = &mut displays.score
                    && let Err(e) = lcd.set_image(ctx, file, img)
                {
                    eprintln!("pin_app: LCD scene image {file}: {e}");
                }
            }
            score = Some(model);
        }
        let overlay = opts.debug_overlay.then(|| {
            (
                DebugRenderer::new(ctx.device(), format),
                DebugLines::new(4096 + visuals.debug_lines.len() + visuals.flippers.len()),
                visuals.debug_lines.clone(),
                Camera::ortho_top_down_with_margin(plan.0, plan.1, 0.01),
            )
        });
        let hud = opts
            .profiler_hud
            .then(|| HudOverlay::new(ctx.device(), format));
        Ok(TableRenderer {
            format,
            layer_format,
            size: (width, height),
            camera: table_camera(plan.0, plan.1, width as f32 / height.max(1) as f32),
            pbr,
            post,
            plan,
            lights: key_lights(visuals.bounds.0, visuals.bounds.1),
            table_draws,
            ball_mesh,
            bat_mesh,
            flippers: visuals.flippers.clone(),
            draws: Vec::new(),
            motion: Vec::new(),
            displays,
            score,
            overlay,
            hud,
            display_commands: 0,
        })
    }

    /// Target format (post chain, debug overlay).
    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// The format the display renderers are built for (equal to
    /// [`format`](Self::format) unless built by
    /// [`with_layer_format`](Self::with_layer_format)).
    pub fn layer_format(&self) -> wgpu::TextureFormat {
        self.layer_format
    }

    /// Target size.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The camera of the playfield view.
    pub fn camera(&self) -> &PbrCamera {
        &self.camera
    }

    /// Resizes the targets (no-op when unchanged).
    ///
    /// # Errors
    /// A zero or oversized size.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), GpuError> {
        if (width, height) == self.size {
            return Ok(());
        }
        self.pbr.resize(width, height)?;
        self.post.resize(width, height)?;
        self.size = (width, height);
        self.camera = table_camera(
            self.plan.0,
            self.plan.1,
            width as f32 / height.max(1) as f32,
        );
        if let Some(s) = &mut self.displays.score {
            s.mark_dirty();
        }
        if let Some(b) = &mut self.displays.backglass {
            b.mark_dirty();
        }
        Ok(())
    }

    /// Applies one display command from the sim (`now_ms`: the simulated
    /// time of the frame, the snapshot tick).
    pub fn apply_display_command(&mut self, cmd: DisplayCommand, now_ms: u64) {
        self.display_commands += 1;
        let Some(model) = &mut self.score else {
            return;
        };
        match cmd {
            DisplayCommand::Scene(id) => model.scenes_mut().show(id, now_ms),
            DisplayCommand::Score { player, score } => {
                let p = usize::from(player);
                let s = model.scores_mut();
                if p < s.scores.len() {
                    s.scores[p] = score;
                    s.players = s.players.max(player + 1);
                }
            }
        }
    }

    /// Shows `lines` in the profiler HUD from the next playfield frame
    /// (ignored without [`TableRenderOptions::profiler_hud`]).
    pub fn set_hud_lines(&mut self, lines: &[String]) {
        if let Some(h) = &mut self.hud {
            h.set_lines(lines);
        }
    }

    /// The profiler HUD (`Some` with [`TableRenderOptions::profiler_hud`]).
    pub fn hud(&self) -> Option<&HudOverlay> {
        self.hud.as_ref()
    }

    /// Paints the profiler HUD over a playfield frame (`view`, this
    /// renderer's format), last, over the displays.
    fn encode_hud(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        playfield: bool,
    ) {
        if playfield && let Some(h) = &mut self.hud {
            h.paint(queue, encoder, view, self.size);
        }
    }

    /// Display commands applied so far.
    pub fn display_commands(&self) -> u64 {
        self.display_commands
    }

    /// The score display model (scores, the active scene).
    pub fn score_model(&self) -> Option<&ScoreDisplayModel> {
        self.score.as_ref()
    }

    /// Whether the table has a backglass / score display renderer.
    pub fn has_displays(&self) -> (bool, bool) {
        (
            self.displays.backglass.is_some(),
            self.displays.score.is_some(),
        )
    }

    /// Records one frame of `snap` into `view` (this renderer's format,
    /// [`size`](Self::size) pixels): if `playfield`, the PBR table through
    /// the post chain over the whole target (and the debug overlay when
    /// enabled), then every display of `routes` on `window` into its pixel
    /// viewport, then (if `playfield`) the profiler HUD when enabled.
    /// Returns the displays drawn. The displays are drawn through
    /// `view` too, so this is for a renderer whose
    /// [`layer_format`](Self::layer_format) equals its
    /// [`format`](Self::format); otherwise use
    /// [`encode_composited`](Self::encode_composited).
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        playfield: bool,
        window: usize,
        routes: &[DisplayRoute],
        snap: &FrameStateSnapshot,
    ) -> usize {
        if playfield {
            self.encode_playfield(queue, encoder, view, snap);
        }
        let drawn = self.encode_displays(queue, encoder, view, window, routes, snap);
        self.encode_hud(queue, encoder, view, playfield);
        drawn
    }

    /// Records one frame of `snap` into `frame` (a texture of this
    /// renderer's [`format`](Self::format) whose default view is `view`,
    /// created with `layers`' target's view formats and extra usages): if
    /// `playfield`, the PBR table through the post chain and the debug
    /// overlay into `view`; then every display of `routes` on `window`
    /// into the view [`LayerCompositor::begin`] returns (`view` itself
    /// when it returns `None`, the [`LayerTarget::Direct`] path), and
    /// [`LayerCompositor::end`]. With [`LayerTarget::TwinView`] or
    /// [`LayerTarget::Intermediate`] the display layers blend in
    /// display-encoded space, on an `*Srgb` target too (issue #10). With
    /// [`LayerTarget::Direct`] on a non-sRGB target they blend in
    /// display-encoded space likewise; on an `*Srgb` target (the last
    /// resort when neither a twin view nor copy usages are available) the
    /// display shaders encode their output and fixed-function blending
    /// happens in linear space. Last, if `playfield`, the profiler HUD
    /// when enabled ([`TableRenderOptions::profiler_hud`]) into `view`. On
    /// the direct path this is exactly [`encode`](Self::encode). Returns
    /// the displays drawn.
    ///
    /// # Panics
    /// If `layers`' target formats are not this renderer's
    /// ([`compositor_matches`]): a wiring error, never a runtime condition.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_composited(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: &wgpu::Texture,
        view: &wgpu::TextureView,
        layers: &mut LayerCompositor,
        playfield: bool,
        window: usize,
        routes: &[DisplayRoute],
        snap: &FrameStateSnapshot,
    ) -> usize {
        assert!(
            compositor_matches(self.format, self.layer_format, layers.target()),
            "encode_composited: compositor {:?} does not match the renderer (target {:?}, \
             display layer format {:?})",
            layers.target(),
            self.format,
            self.layer_format
        );
        if playfield {
            self.encode_playfield(queue, encoder, view, snap);
        }
        // No display on this window: nothing to composite (an intermediate
        // target would otherwise copy the frame out and back for nothing).
        if !routes.iter().any(|r| r.window == window) {
            let drawn = self.encode_displays(queue, encoder, view, window, routes, snap);
            self.encode_hud(queue, encoder, view, playfield);
            return drawn;
        }
        let layer_view = layers.begin(device, encoder, frame);
        let drawn = self.encode_displays(
            queue,
            encoder,
            layer_view.as_ref().unwrap_or(view),
            window,
            routes,
            snap,
        );
        layers.end(encoder, frame);
        self.encode_hud(queue, encoder, view, playfield);
        drawn
    }

    /// The playfield part of [`encode`](Self::encode): PBR table, post
    /// chain and the optional debug overlay into `view` (this renderer's
    /// [`format`](Self::format)).
    fn encode_playfield(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        snap: &FrameStateSnapshot,
    ) {
        self.draws.clear();
        for (d, lamp) in &self.table_draws {
            let mut d = *d;
            if let Some(l) = lamp {
                d.emission = snap.lamps.get(*l).copied().unwrap_or(0.0).clamp(0.0, 1.0);
            }
            self.draws.push(d);
        }
        self.motion.clear();
        for b in snap.balls() {
            let r = super::threads::default_ball_radius(b.profile);
            self.draws.push(DrawItem::new(
                self.ball_mesh,
                ball_model(b.position_m, r),
                ball_material(b.profile),
            ));
            self.motion.push(MotionBall::from_state(b, r));
        }
        for st in snap.flippers() {
            if let Some(f) = self.flippers.get(usize::from(st.id.0)) {
                self.draws.push(DrawItem::new(
                    self.bat_mesh,
                    flipper_model(f, st.angle_rad),
                    MaterialKind::CeramicPearl,
                ));
            }
        }
        let mut frame = FrameDesc::new(&self.camera, &self.lights, &self.draws);
        frame.environment = Environment::STUDIO;
        frame.gi = snap.gi;
        self.pbr.encode(encoder, &frame);
        let pf = PostFrame {
            camera: &self.camera,
            balls: &self.motion,
        };
        self.post.encode(encoder, self.pbr.hdr_view(), &pf, view);
        if let Some((r, lines, table, cam)) = &mut self.overlay {
            lines.clear();
            for l in table.iter() {
                lines.push(*l);
            }
            for st in snap.flippers() {
                if let Some(f) = self.flippers.get(usize::from(st.id.0)) {
                    lines.push(f.line(st.angle_rad, FLIPPER_LINE_COLOR));
                }
            }
            snapshot_lines(snap, &super::threads::default_ball_radius, lines);
            r.draw(encoder, queue, view, cam, lines);
        }
    }

    /// The display part of [`encode`](Self::encode): every display of
    /// `routes` on `window` into `layer_view` (a view of this renderer's
    /// [`layer_format`](Self::layer_format)). Returns the displays drawn.
    fn encode_displays(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        layer_view: &wgpu::TextureView,
        window: usize,
        routes: &[DisplayRoute],
        snap: &FrameStateSnapshot,
    ) -> usize {
        if let (Some(model), Some(sr)) = (&self.score, &mut self.displays.score) {
            let content = model.compose(snap.tick);
            sr.update(queue, &content);
        }
        self.displays
            .encode_window(window, routes, encoder, layer_view, self.size, &snap.lamps)
    }

    /// Renders one frame of `snap` offscreen (playfield + the displays of
    /// `routes` on window 0) and reads it back: RGBA8 (the stored bytes),
    /// row 0 at the top. The displays go through
    /// [`encode_composited`](Self::encode_composited) with
    /// [`offscreen_layer_target`]: when the layer format differs from the
    /// target format, a twin-format scratch texture
    /// ([`LayerTarget::Intermediate`]).
    ///
    /// # Errors
    /// Readback failures.
    pub fn render_to_image(
        &mut self,
        ctx: &GpuContext,
        routes: &[DisplayRoute],
        snap: &FrameStateSnapshot,
    ) -> Result<RgbaImage, GpuError> {
        let (w, h) = self.size;
        let layers = offscreen_layer_target(self.format, self.layer_format);
        let tex = ctx.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("table frame"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | layers.extra_usages(),
            view_formats: &[],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        let mut enc = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("table frame"),
            });
        let mut comp = LayerCompositor::new(layers);
        self.encode_composited(
            ctx.device(),
            ctx.queue(),
            &mut enc,
            &tex,
            &view,
            &mut comp,
            true,
            0,
            routes,
            snap,
        );
        ctx.queue().submit([enc.finish()]);
        read_rgba8(ctx, &tex, self.format)
    }
}

/// The layer target of an offscreen texture of `format` (created without
/// view formats) for display renderers built for `layer_format`:
/// [`LayerTarget::Direct`] when they are equal, else
/// [`LayerTarget::Intermediate`] (copies work on every backend, unlike
/// texture view formats).
pub fn offscreen_layer_target(
    format: wgpu::TextureFormat,
    layer_format: wgpu::TextureFormat,
) -> LayerTarget {
    if format == layer_format {
        LayerTarget::Direct(format)
    } else {
        LayerTarget::Intermediate {
            target: format,
            twin: layer_format,
        }
    }
}

/// Reads an 8-bit RGBA / BGRA texture back as RGBA (bounded wait).
fn read_rgba8(
    ctx: &GpuContext,
    tex: &wgpu::Texture,
    format: wgpu::TextureFormat,
) -> Result<RgbaImage, GpuError> {
    let device = ctx.device();
    let (w, h) = (tex.width(), tex.height());
    let row = w as usize * 4;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
    let padded = row.div_ceil(align) * align;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("table readback"),
        size: (padded * h as usize) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("table readback"),
    });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded as u32),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    ctx.queue().submit([enc.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    buffer.map_async(wgpu::MapMode::Read, .., move |r| {
        let _ = tx.send(r);
    });
    pin_render::gpu::wait_for_device(device, pin_render::gpu::READBACK_WAIT_TIMEOUT)
        .map_err(|e| GpuError::Readback(format!("poll: {e}")))?;
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| GpuError::Readback(format!("map: {e}")))?
        .map_err(|e| GpuError::Readback(format!("map: {e:?}")))?;
    let bgra = matches!(
        format,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
    );
    let mut out = Vec::with_capacity(row * h as usize);
    {
        let mapped = buffer
            .get_mapped_range(..)
            .map_err(|e| GpuError::Readback(format!("get_mapped_range: {e:?}")))?;
        for y in 0..h as usize {
            for p in mapped[y * padded..y * padded + row].chunks_exact(4) {
                if bgra {
                    out.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
                } else {
                    out.extend_from_slice(p);
                }
            }
        }
    }
    buffer.unmap();
    RgbaImage::from_rgba(w, h, out).ok_or_else(|| GpuError::Readback("image size".into()))
}

/// The display routes of one window from its viewports (logical
/// coordinates, the window's own role first, PiPs after), scaled to
/// `pixel_size`: a backglass / score viewport becomes a route on `window`
/// (dedicated when it is the window's own role).
pub fn routes_for_window(
    window: usize,
    viewports: &[(SurfaceRole, Rect)],
    logical_size: (u32, u32),
    pixel_size: (u32, u32),
) -> Vec<DisplayRoute> {
    let sx = if logical_size.0 > 0 {
        pixel_size.0 as f32 / logical_size.0 as f32
    } else {
        1.0
    };
    let sy = if logical_size.1 > 0 {
        pixel_size.1 as f32 / logical_size.1 as f32
    } else {
        1.0
    };
    viewports
        .iter()
        .enumerate()
        .filter(|(_, (role, _))| *role != SurfaceRole::Playfield)
        .map(|(i, &(role, r))| DisplayRoute {
            role,
            window,
            viewport: if i == 0 {
                Rect::new(0, 0, pixel_size.0, pixel_size.1)
            } else {
                Rect::new(
                    (r.x as f32 * sx).round() as i32,
                    (r.y as f32 * sy).round() as i32,
                    (r.w as f32 * sx).round() as u32,
                    (r.h as f32 * sy).round() as u32,
                )
            },
            dedicated: i == 0,
        })
        .collect()
}

/// Whether display layers of `layer` format may be drawn for a `target`
/// format texture: equal formats, or an `*Srgb` target with its non-sRGB
/// twin. An `*Srgb` layer on a non-sRGB target is refused (the layers would
/// blend in linear space in a scratch texture, then be copied back
/// unchanged).
pub(crate) fn layer_format_allowed(
    target: wgpu::TextureFormat,
    layer: wgpu::TextureFormat,
) -> bool {
    layer == target
        || (target.is_srgb() && !layer.is_srgb() && layer == target.remove_srgb_suffix())
}

/// Whether a compositor for `compositor` suits a renderer built for
/// `target` / `layer`: its layer and target formats must equal the
/// renderer's (else the display pipelines and the view they draw into
/// would disagree, which wgpu rejects).
pub(crate) fn compositor_matches(
    target: wgpu::TextureFormat,
    layer: wgpu::TextureFormat,
    compositor: LayerTarget,
) -> bool {
    compositor.target_format() == target && compositor.layer_format() == layer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_kind_precedence() {
        for k in MaterialKind::ALL {
            assert_eq!(render_kind(k.shader_name(), false), k);
            // an exact pbr_ name wins over the lamp tag
            assert_eq!(render_kind(k.shader_name(), true), k);
        }
        assert_eq!(
            render_kind("insert #ff0000", true),
            MaterialKind::LightInsert
        );
        assert_eq!(render_kind("anything", true), MaterialKind::LightInsert);
        // No name-shaped special cases: the prefix alone does nothing.
        assert_eq!(
            render_kind("insert #ff0000", false),
            MaterialKind::PlayfieldWood
        );
        assert_eq!(render_kind("acrylic", false), MaterialKind::ClearAcrylic);
        assert_eq!(render_kind("wood", false), MaterialKind::PlayfieldWood);
        assert_eq!(render_kind("steel", false), MaterialKind::ChromeSteel);
        assert_eq!(render_kind("mystery", false), MaterialKind::PlayfieldWood);
        assert_eq!(render_kind("", false), MaterialKind::PlayfieldWood);
    }

    #[test]
    fn layer_format_pairs() {
        use wgpu::TextureFormat as F;
        assert!(layer_format_allowed(F::Rgba8Unorm, F::Rgba8Unorm));
        assert!(layer_format_allowed(F::Rgba8UnormSrgb, F::Rgba8UnormSrgb));
        assert!(layer_format_allowed(F::Rgba8UnormSrgb, F::Rgba8Unorm));
        assert!(layer_format_allowed(F::Bgra8UnormSrgb, F::Bgra8Unorm));
        assert!(layer_format_allowed(F::Bgra8Unorm, F::Bgra8Unorm));
        assert!(!layer_format_allowed(F::Rgba8Unorm, F::Rgba8UnormSrgb));
        assert!(!layer_format_allowed(F::Bgra8Unorm, F::Bgra8UnormSrgb));
        assert!(!layer_format_allowed(F::Rgba8Unorm, F::Bgra8Unorm));
        assert!(!layer_format_allowed(F::Rgba8UnormSrgb, F::Bgra8Unorm));
        assert!(!layer_format_allowed(F::Rgba16Float, F::Rgba8Unorm));
    }

    #[test]
    fn compositor_must_match_renderer_formats() {
        use wgpu::TextureFormat as F;
        let twin = LayerTarget::TwinView {
            target: F::Rgba8UnormSrgb,
            twin: F::Rgba8Unorm,
        };
        assert!(compositor_matches(F::Rgba8UnormSrgb, F::Rgba8Unorm, twin));
        // Renderer built with an sRGB layer format: compositor wants the twin.
        assert!(!compositor_matches(
            F::Rgba8UnormSrgb,
            F::Rgba8UnormSrgb,
            twin
        ));
        assert!(!compositor_matches(F::Bgra8UnormSrgb, F::Rgba8Unorm, twin));
        let direct = LayerTarget::Direct(F::Rgba8UnormSrgb);
        assert!(compositor_matches(
            F::Rgba8UnormSrgb,
            F::Rgba8UnormSrgb,
            direct
        ));
        // Non-sRGB layer pipelines with a Direct sRGB compositor: rejected.
        assert!(!compositor_matches(
            F::Rgba8UnormSrgb,
            F::Rgba8Unorm,
            direct
        ));
    }

    #[test]
    fn normals_of_a_flat_quad_point_up() {
        let p = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ];
        let n = vertex_normals(&p, &[0, 1, 2, 0, 2, 3]);
        assert!(n.iter().all(|v| (v[2] - 1.0).abs() < 1e-6), "{n:?}");
        assert_eq!(vertex_normals(&p, &[0, 0, 0]), vec![[0.0, 0.0, 1.0]; 4]);
    }

    #[test]
    fn camera_sees_the_whole_table() {
        let cam = table_camera([0.0, 0.0], [0.52, 1.07], 0.5);
        let vp = cam.view_proj();
        for p in [[0.0, 0.0, 0.0], [0.52, 1.07, 0.0], [0.0, 1.07, 0.0]] {
            let clip: [f32; 4] = core::array::from_fn(|r| {
                vp[0][r] * p[0] + vp[1][r] * p[1] + vp[2][r] * p[2] + vp[3][r]
            });
            assert!(clip[3] > 0.0, "{p:?} behind the camera");
            let c = [clip[0] / clip[3], clip[1] / clip[3]];
            assert!(c[0].abs() <= 1.0 && c[1].abs() <= 1.0, "{p:?} -> {c:?}");
        }
    }

    #[test]
    fn routes_scale_pips_and_mark_dedicated_windows() {
        let vps = [
            (SurfaceRole::Playfield, Rect::new(0, 0, 100, 200)),
            (SurfaceRole::Backglass, Rect::new(10, 20, 30, 40)),
            (SurfaceRole::Score, Rect::new(0, 100, 50, 10)),
        ];
        let r = routes_for_window(2, &vps, (100, 200), (200, 400));
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].role, SurfaceRole::Backglass);
        assert_eq!(r[0].viewport, Rect::new(20, 40, 60, 80));
        assert!(!r[0].dedicated && r[0].window == 2);
        let r = routes_for_window(
            1,
            &[(SurfaceRole::Score, Rect::new(0, 0, 9, 9))],
            (9, 9),
            (18, 18),
        );
        assert_eq!(r[0].viewport, Rect::new(0, 0, 18, 18));
        assert!(r[0].dedicated);
    }

    #[test]
    fn offscreen_layers_are_direct_unless_the_layer_format_is_a_twin() {
        use wgpu::TextureFormat as F;
        for f in [
            F::Rgba8Unorm,
            F::Bgra8Unorm,
            F::Rgba8UnormSrgb,
            F::Bgra8UnormSrgb,
        ] {
            let t = offscreen_layer_target(f, f);
            assert_eq!(t, LayerTarget::Direct(f));
            assert!(t.extra_usages().is_empty() && t.view_formats().is_empty());
        }
        for (srgb, twin) in [
            (F::Rgba8UnormSrgb, F::Rgba8Unorm),
            (F::Bgra8UnormSrgb, F::Bgra8Unorm),
        ] {
            let t = offscreen_layer_target(srgb, twin);
            assert_eq!(t, LayerTarget::Intermediate { target: srgb, twin });
            assert_eq!(t.layer_format(), twin);
            assert!(
                t.view_formats().is_empty(),
                "no texture view formats needed"
            );
            assert_eq!(
                t.extra_usages(),
                wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST
            );
        }
    }

    #[test]
    fn flipper_model_maps_the_bat_tip() {
        let f = FlipperOutline {
            base_m: [0.1, 0.2, 0.0],
            length_m: 0.07,
            rest_rad: 0.0,
        };
        let m = flipper_model(&f, std::f32::consts::FRAC_PI_2);
        let tip = transform(&m, [1.0, 0.0, 0.0]);
        assert!(
            (tip[0] - 0.1).abs() < 1e-6 && (tip[1] - 0.27).abs() < 1e-6,
            "{tip:?}"
        );
    }
}
