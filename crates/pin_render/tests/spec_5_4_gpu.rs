//! Independent GPU spec tests for Subphase 5.4 (hardware ray-query reflections), under the
//! image test policy D30: analytic pixel assertions first; the regression snapshot is
//! compared (and can only be stored) after the analytic assertions (1)–(5) pass in the
//! same test.
//!
//! Written from the 5.4 brief alone, without reading the implementation
//! (`src/raytrace/**`) or the Implementer's tests. The calls into the 5.4 API are
//! gathered in the "API under test" section below; the brief fixes the names but not
//! every parameter list (see DECISIONS "5.4-T — assumed ray-query API and featureless
//! context").
//!
//! # Scene (world z up, SI)
//!
//! A playfield (`PlayfieldWood`, clear coat roughness 0.06) at z = 0, two chrome balls of
//! radius 13.5 mm resting on it 120 mm apart (A left at x = −60 mm, B right at
//! x = +60 mm), a red emissive panel 32 mm above the playfield over A and a blue one over
//! B (120 mm squares facing down). No point lights and a black environment, so every
//! colour in the image comes from the two emitters by reflection. The cameras are above
//! the panels, so they see the panels' culled back faces and look through them.
//!
//! Each ball is a mirror. In the zoomed view of A (camera above, on B's side) there is
//! a patch of A whose mirror ray hits B, and B's mirror ray from there hits the blue
//! panel (computed per pixel here by ray casting): a renderer that traces reflections
//! shows the other ball there, carrying the other ball's colour (blue for A, red for B).
//! A probe captured at A's centre sees B shaded with the placeholder environment
//! (black: probe captures do not recurse), so the probe-only render shows no colour
//! there. That is the "each ball is reflected in the other" criterion (PLAN 5.4).
//!
//! Snapshots: `tests/fixtures/spec_5_4/<name>_<w>x<h>.rgba8` (raw sRGB RGBA8 of the
//! linear HDR image). Set `SPEC_5_4_BLESS=1` to (re)write them; they are written only
//! after the analytic checks.
//!
//! Skips: ray-mode tests skip with a printed note when the adapter lacks
//! `EXPERIMENTAL_RAY_QUERY`; with `PIN_REQUIRE_GPU=1` they fail instead on an adapter
//! known to report it (lavapipe, DECISIONS Q3) or when the adapter reports it but the
//! device was created without it. `PIN_REQUIRE_RAY_QUERY=1` makes any skip a failure.
#![cfg(feature = "gpu-tests")]
#![allow(clippy::needless_range_loop)] // channel/axis indices read clearer as ranges
#![allow(clippy::needless_update)] // `..RayQueryConfig::default()` stays valid whatever fields the config grows

use pin_render::gpu::{GpuContext, GpuOptions, require_gpu, wgpu};
use pin_render::pbr::math::{Mat4, PbrCamera};
use pin_render::pbr::{
    DrawItem, Environment, FrameDesc, HdrImage, MaterialKind, MaterialParams, MeshGeometry, MeshId,
    PbrRenderer,
};
use pin_render::raytrace::{
    RayQueryConfig, RayQueryReflections, RayStats, ReflectionQuality, select_reflection_source,
};
use pin_render::reflections::{OctahedralProbes, ProbeConfig, ReflectionSource, ReflectiveBall};

const RQ: wgpu::Features = wgpu::Features::EXPERIMENTAL_RAY_QUERY;

// ---------------------------------------------------------------------------------------
// API under test (the only calls into `pin_render::raytrace` with a parameter list the
// brief does not fix; see the module docs)
// ---------------------------------------------------------------------------------------

/// The static meshes of the scene (sphere for the balls, quad for playfield and panels),
/// as the `(MeshId, geometry)` list the real API takes: the same geometry `new_pbr`
/// uploads, under the ids it got.
fn static_meshes(meshes: &Meshes) -> [(MeshId, MeshGeometry); 2] {
    [
        (meshes.sphere, MeshGeometry::uv_sphere(R, 96, 48)),
        (meshes.quad, MeshGeometry::quad(1.0)),
    ]
}

/// `select_reflection_source` for `pbr`'s main pass.
fn select(
    ctx: &GpuContext,
    pbr: &PbrRenderer,
    meshes: &Meshes,
    quality: ReflectionQuality,
    config: RayQueryConfig,
) -> Box<dyn ReflectionSource> {
    let owned = static_meshes(meshes);
    let refs: Vec<(MeshId, &MeshGeometry)> = owned.iter().map(|(i, g)| (*i, g)).collect();
    select_reflection_source(ctx, quality, pbr, &refs, &config)
}

/// `RayQueryReflections` creation; `Err` carries a printable reason.
fn new_ray(
    ctx: &GpuContext,
    pbr: &PbrRenderer,
    meshes: &Meshes,
    quality: ReflectionQuality,
    config: RayQueryConfig,
) -> Result<RayQueryReflections, String> {
    let owned = static_meshes(meshes);
    let refs: Vec<(MeshId, &MeshGeometry)> = owned.iter().map(|(i, g)| (*i, g)).collect();
    RayQueryReflections::new(ctx, pbr, quality, &refs, &config).map_err(|e| format!("{e:?}"))
}

/// BLAS builds and TLAS refits so far.
fn ray_counts(src: &RayQueryReflections) -> (u64, u64) {
    let s: RayStats = src.ray_stats();
    (s.blas_builds, s.tlas_refits)
}

// ---------------------------------------------------------------------------------------
// Contexts
// ---------------------------------------------------------------------------------------

fn new_context() -> Option<GpuContext> {
    match GpuContext::new_headless_with_features(&GpuOptions::from_env(), RQ) {
        Ok(ctx) => {
            let info = ctx.adapter_info();
            println!(
                "[spec_5_4] adapter {} ({:?}); adapter ray query {}, device ray query {}",
                info.name,
                info.backend,
                ctx.adapter().features().contains(RQ),
                ctx.device().features().contains(RQ)
            );
            Some(ctx)
        }
        Err(e) => {
            if require_gpu() {
                panic!("PIN_REQUIRE_GPU=1 but no GPU context: {e}");
            }
            println!("SKIP (no GPU adapter, PIN_REQUIRE_GPU not set): {e}");
            None
        }
    }
}

/// One context per test process, shared by every test (lavapipe crashes when many
/// tests each create their own device; DECISIONS "Wave 5 gate fix").
fn gpu() -> Option<&'static GpuContext> {
    static CTX: std::sync::OnceLock<Option<GpuContext>> = std::sync::OnceLock::new();
    CTX.get_or_init(new_context).as_ref()
}

fn require_ray_query() -> bool {
    std::env::var("PIN_REQUIRE_RAY_QUERY").as_deref() == Ok("1")
}

/// The shared context if its device has `EXPERIMENTAL_RAY_QUERY`, else a skip note (or a
/// failure, see the module docs).
fn ray_ctx(test: &str) -> Option<&'static GpuContext> {
    let ctx = gpu()?;
    if ctx.device().features().contains(RQ) {
        return Some(ctx);
    }
    let info = ctx.adapter_info();
    let adapter_has = ctx.adapter().features().contains(RQ);
    let known_capable = info.name.to_lowercase().contains("llvmpipe");
    let why = if adapter_has {
        "the adapter reports EXPERIMENTAL_RAY_QUERY but the device was created without it"
    } else {
        "the adapter lacks EXPERIMENTAL_RAY_QUERY"
    };
    if require_ray_query() || (require_gpu() && (adapter_has || known_capable)) {
        panic!("[{test}] {why} (adapter {}, {:?})", info.name, info.backend);
    }
    println!(
        "[{test}] SKIP: {why} (adapter {}, {:?})",
        info.name, info.backend
    );
    None
}

/// A context whose device lacks `EXPERIMENTAL_RAY_QUERY`: the shared one when it lacks
/// the feature, else a GL context (wgpu's GL backend never reports the feature,
/// DECISIONS Q3). `None` (with a note) when neither exists.
fn featureless_ctx(test: &str) -> Option<&'static GpuContext> {
    let ctx = gpu()?;
    if !ctx.device().features().contains(RQ) {
        return Some(ctx);
    }
    static GL: std::sync::OnceLock<Option<GpuContext>> = std::sync::OnceLock::new();
    let gl = GL
        .get_or_init(|| {
            let opts = GpuOptions {
                force_fallback_adapter: false,
                backends: wgpu::Backends::GL,
            };
            match GpuContext::new_headless(&opts) {
                Ok(c) if !c.device().features().contains(RQ) => {
                    println!("[spec_5_4] featureless context: {}", c.adapter_info().name);
                    Some(c)
                }
                Ok(c) => {
                    println!(
                        "[spec_5_4] GL adapter {} reports ray query",
                        c.adapter_info().name
                    );
                    None
                }
                Err(e) => {
                    println!("[spec_5_4] no GL adapter: {e}");
                    None
                }
            }
        })
        .as_ref();
    if gl.is_none() {
        if require_ray_query() {
            panic!("[{test}] no context without EXPERIMENTAL_RAY_QUERY to test the fallback on");
        }
        println!("[{test}] SKIP (feature-absent part): no context without the feature");
    }
    gl
}

// ---------------------------------------------------------------------------------------
// Vector helpers and analytic geometry
// ---------------------------------------------------------------------------------------

type V3 = [f32; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm(a: V3) -> V3 {
    scale(a, 1.0 / dot(a, a).sqrt())
}
fn reflect(d: V3, n: V3) -> V3 {
    sub(d, scale(n, 2.0 * dot(d, n)))
}

/// Distance along the unit ray (o, d) to the sphere (c, r), if hit in front of `o`.
fn hit_sphere(o: V3, d: V3, c: V3, r: f32) -> Option<f32> {
    let oc = sub(o, c);
    let b = dot(oc, d);
    let cc = dot(oc, oc) - r * r;
    let disc = b * b - cc;
    if disc <= 0.0 {
        return None;
    }
    let s = -b - disc.sqrt();
    (s > 1e-6).then_some(s)
}

/// A one-sided square: centre, unit front normal, in-plane axes, half side.
#[derive(Copy, Clone)]
struct Quad {
    c: V3,
    n: V3,
    e1: V3,
    e2: V3,
    half: f32,
}

impl Quad {
    /// Object → world for `MeshGeometry::quad(1.0)` (z = 0 plane, facing +z), scaled to
    /// the side.
    fn model(&self) -> Mat4 {
        let s = self.half * 2.0;
        [
            [self.e1[0] * s, self.e1[1] * s, self.e1[2] * s, 0.0],
            [self.e2[0] * s, self.e2[1] * s, self.e2[2] * s, 0.0],
            [self.n[0], self.n[1], self.n[2], 0.0],
            [self.c[0], self.c[1], self.c[2], 1.0],
        ]
    }
    /// Distance to the front face along (o, d) within `frac` of the half side.
    fn hit_t(&self, o: V3, d: V3, frac: f32) -> Option<f32> {
        let denom = dot(d, self.n);
        if denom > -1e-4 {
            return None;
        }
        let t = dot(sub(self.c, o), self.n) / denom;
        if t <= 0.0 {
            return None;
        }
        let l = sub(add(o, scale(d, t)), self.c);
        (dot(l, self.e1).abs() < frac * self.half && dot(l, self.e2).abs() < frac * self.half)
            .then_some(t)
    }
    fn hit(&self, o: V3, d: V3, frac: f32) -> bool {
        self.hit_t(o, d, frac).is_some()
    }
}

// ---------------------------------------------------------------------------------------
// The scene
// ---------------------------------------------------------------------------------------

/// Ball radius (27 mm ball).
const R: f32 = 0.0135;
/// Left ball A and right ball B, 120 mm apart, resting on the playfield.
const BALL_A: V3 = [-0.06, 0.0, R];
const BALL_B: V3 = [0.06, 0.0, R];
/// Emitter panels: height above the playfield and side.
const PANEL_Z: f32 = 0.032;
const PANEL: f32 = 0.12;
const PLAYFIELD: f32 = 0.5;
/// Emitted radiance of the red (over A) and blue (over B) panels.
const RED_L: V3 = [2.0, 0.06, 0.04];
const BLUE_L: V3 = [0.04, 0.1, 2.0];

fn panel(over: V3) -> Quad {
    Quad {
        c: [over[0], over[1], PANEL_Z],
        n: [0.0, 0.0, -1.0],
        e1: [1.0, 0.0, 0.0],
        e2: [0.0, -1.0, 0.0],
        half: PANEL * 0.5,
    }
}
fn red_panel() -> Quad {
    panel(BALL_A)
}
fn blue_panel() -> Quad {
    panel(BALL_B)
}
fn playfield() -> Quad {
    Quad {
        c: [0.0; 3],
        n: [0.0, 0.0, 1.0],
        e1: [1.0, 0.0, 0.0],
        e2: [0.0, 1.0, 0.0],
        half: PLAYFIELD * 0.5,
    }
}

/// Chrome reflectance (metal F0 = base colour; Schlick only raises it off normal
/// incidence, so `base` is a lower bound of the reflectance per bounce).
fn chrome() -> V3 {
    let b = MaterialKind::ChromeSteel.params().base_color;
    [b[0], b[1], b[2]]
}

/// What varies between the test scenes.
#[derive(Copy, Clone)]
struct Spec {
    a: V3,
    b: V3,
    /// Ball perceptual roughness (`None` = the chrome default, 0.06).
    roughness: Option<f32>,
}

const SCENE: Spec = Spec {
    a: BALL_A,
    b: BALL_B,
    roughness: None,
};

struct Meshes {
    sphere: MeshId,
    quad: MeshId,
}

fn emitter(l: V3) -> MaterialParams {
    MaterialParams {
        emissive: l,
        ..MaterialParams::lambert([0.0; 3])
    }
}

/// Draws: 0 ball A, 1 ball B, 2 playfield, 3 red panel, 4 blue panel.
fn draws(m: &Meshes, s: &Spec) -> [DrawItem; 5] {
    let ball = |c: V3| {
        let mut d = DrawItem::new(
            m.sphere,
            pin_render::pbr::math::translation(c),
            MaterialKind::ChromeSteel,
        );
        if let Some(r) = s.roughness {
            d.material.roughness = r;
        }
        d
    };
    let quad = |q: Quad, kind: MaterialKind| DrawItem::new(m.quad, q.model(), kind);
    let mut red = quad(red_panel(), MaterialKind::PlayfieldWood);
    red.material = emitter(RED_L);
    let mut blue = quad(blue_panel(), MaterialKind::PlayfieldWood);
    blue.material = emitter(BLUE_L);
    [
        ball(s.a),
        ball(s.b),
        quad(playfield(), MaterialKind::PlayfieldWood),
        red,
        blue,
    ]
}

/// A square pinhole camera, z up.
#[derive(Copy, Clone)]
struct View {
    eye: V3,
    target: V3,
    fov_y: f32,
    size: u32,
}

impl View {
    fn pbr(&self) -> PbrCamera {
        PbrCamera::look_at(
            self.eye,
            self.target,
            [0.0, 0.0, 1.0],
            self.fov_y,
            1.0,
            0.01,
            5.0,
        )
    }
    /// Unit ray through the centre of pixel (x, y) (row 0 at the top).
    fn ray(&self, x: u32, y: u32) -> V3 {
        let f = norm(sub(self.target, self.eye));
        let s = norm(cross(f, [0.0, 0.0, 1.0]));
        let u = cross(s, f);
        let t = (self.fov_y * 0.5).tan();
        let nx = (x as f32 + 0.5) / self.size as f32 * 2.0 - 1.0;
        let ny = 1.0 - (y as f32 + 0.5) / self.size as f32 * 2.0;
        norm(add(f, add(scale(s, nx * t), scale(u, ny * t))))
    }
}

/// The whole scene from above, on the −y side.
const OVERVIEW: View = View {
    eye: [0.0, -0.22, 0.25],
    target: [0.0, 0.0, 0.01],
    fov_y: 0.62,
    size: 160,
};

/// Zoomed on the patch of `me` that mirrors `other`, camera 250 mm away at 45° elevation
/// on `other`'s side.
fn zoom(me: V3, other: V3) -> View {
    let side = if other[0] > me[0] { 1.0 } else { -1.0 };
    let e = std::f32::consts::FRAC_1_SQRT_2;
    let v = [side * e, 0.0, e];
    let n_mid = norm(add([side, 0.0, 0.0], v));
    View {
        eye: add(me, scale(v, 0.25)),
        target: add(me, scale(n_mid, R)),
        fov_y: 0.02,
        size: 128,
    }
}

/// Pixels of ball `me` that show the ball `other` carrying `other_panel`'s colour:
///
/// * the mirror ray from the surface point hits `other`, and `other`'s mirror ray from
///   there hits the middle of `other_panel` (0.55 of its half side), both from the exact
///   hit point and from `other`'s centre (what a probe of `other` would return);
/// * the mirror ray, cast from `me`'s surface point or centre (what `me`'s own probe
///   sees), misses both panels enlarged 1.6×, so neither panel shows there directly;
/// * not the rim (n · view < 0.35).
fn mutual_region(
    view: &View,
    me: V3,
    other: V3,
    other_panel: &Quad,
    my_panel: &Quad,
) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    for y in 0..view.size {
        for x in 0..view.size {
            let d = view.ray(x, y);
            let Some(s) = hit_sphere(view.eye, d, me, R) else {
                continue;
            };
            let p = add(view.eye, scale(d, s));
            let n = norm(sub(p, me));
            if dot(n, scale(d, -1.0)) < 0.35 {
                continue;
            }
            let r = reflect(d, n);
            let direct = [other_panel, my_panel]
                .iter()
                .any(|q| q.hit(p, r, 1.6) || q.hit(me, r, 1.6));
            if direct {
                continue;
            }
            let Some(s2) = hit_sphere(p, r, other, R) else {
                continue;
            };
            let q = add(p, scale(r, s2));
            let r2 = reflect(r, norm(sub(q, other)));
            if other_panel.hit(q, r2, 0.55) && other_panel.hit(other, r2, 0.55) {
                out.push((x, y));
            }
        }
    }
    out
}

/// Pixels of ball `me` whose mirror ray hits the middle of `panel` from the surface
/// point and from the centre, and misses the other ball (the panel shows directly: any
/// reflection source, probes included, shows its colour there).
fn direct_region(view: &View, me: V3, other: V3, panel: &Quad) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    for y in 0..view.size {
        for x in 0..view.size {
            let d = view.ray(x, y);
            let Some(s) = hit_sphere(view.eye, d, me, R) else {
                continue;
            };
            let p = add(view.eye, scale(d, s));
            let n = norm(sub(p, me));
            if dot(n, scale(d, -1.0)) < 0.35 {
                continue;
            }
            let r = reflect(d, n);
            if panel.hit(p, r, 0.55)
                && panel.hit(me, r, 0.55)
                && hit_sphere(p, r, other, R * 1.3).is_none()
                && hit_sphere(me, r, other, R * 1.3).is_none()
            {
                out.push((x, y));
            }
        }
    }
    out
}

/// What the primary ray of a pixel sees (panels are seen from behind: culled).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Prim {
    Ball,
    Playfield,
    Background,
}

fn primary(view: &View, s: &Spec, x: u32, y: u32) -> (Prim, V3, V3) {
    let d = view.ray(x, y);
    let ta = hit_sphere(view.eye, d, s.a, R);
    let tb = hit_sphere(view.eye, d, s.b, R);
    let tball = match (ta, tb) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    let tp = playfield().hit_t(view.eye, d, 1.0);
    match (tball, tp) {
        (Some(tb), Some(tp)) if tp < tb => (Prim::Playfield, add(view.eye, scale(d, tp)), d),
        (Some(tb), _) => (Prim::Ball, add(view.eye, scale(d, tb)), d),
        (None, Some(tp)) => (Prim::Playfield, add(view.eye, scale(d, tp)), d),
        (None, None) => (Prim::Background, [0.0; 3], d),
    }
}

// ---------------------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------------------

fn new_pbr(ctx: &GpuContext, size: u32) -> (PbrRenderer, Meshes) {
    let mut pbr = PbrRenderer::new(ctx, size, size).expect("PbrRenderer::new");
    // Upload order fixed: identical mesh ids in every renderer.
    let sphere = pbr
        .upload_mesh(&MeshGeometry::uv_sphere(R, 96, 48))
        .expect("sphere");
    let quad = pbr.upload_mesh(&MeshGeometry::quad(1.0)).expect("quad");
    (pbr, Meshes { sphere, quad })
}

fn assert_finite(img: &HdrImage, what: &str) {
    for (i, p) in img.pixels.iter().enumerate() {
        assert!(
            p.iter().all(|c| c.is_finite()),
            "{what}: pixel {i} is not finite: {p:?} (NaN/Inf)"
        );
        assert!(
            p[0] >= 0.0 && p[1] >= 0.0 && p[2] >= 0.0,
            "{what}: pixel {i} is negative: {p:?}"
        );
    }
}

/// One frame: the source prepares, then the main pass binds it. Checks (4) on every
/// image: no NaN, no Inf, no negative radiance.
fn render(
    ctx: &GpuContext,
    pbr: &mut PbrRenderer,
    m: &Meshes,
    src: &mut dyn ReflectionSource,
    view: &View,
    s: &Spec,
    what: &str,
) -> HdrImage {
    let cam = view.pbr();
    let draws = draws(m, s);
    let mut frame = FrameDesc::new(&cam, &[], &draws);
    frame.environment = Environment::BLACK;
    frame.clear = [0.0, 0.0, 0.0, 1.0];
    let balls = [
        ReflectiveBall {
            position: s.a,
            radius: R,
            draw: 0,
        },
        ReflectiveBall {
            position: s.b,
            radius: R,
            draw: 1,
        },
    ];
    src.prepare(ctx, pbr, &frame, &balls)
        .unwrap_or_else(|e| panic!("{what}: prepare: {e}"));
    let binding = src.binding();
    let img = pbr
        .render_reflected_to_image(ctx, &frame, Some(&binding))
        .unwrap_or_else(|e| panic!("{what}: render: {e}"));
    assert_eq!((img.width, img.height), (view.size, view.size), "{what}");
    assert_finite(&img, what);
    img
}

/// Which reflection source a rig uses.
#[derive(Copy, Clone, Debug)]
enum Source {
    /// `OctahedralProbes` with the default config: the probe path.
    Probes,
    /// `select_reflection_source(quality, config)`.
    Select(ReflectionQuality, RayQueryConfig),
}

/// A renderer with its own reflection source.
struct Rig {
    ctx: &'static GpuContext,
    pbr: PbrRenderer,
    meshes: Meshes,
    src: Box<dyn ReflectionSource>,
}

impl Rig {
    fn new(ctx: &'static GpuContext, size: u32, source: Source) -> Rig {
        let (pbr, meshes) = new_pbr(ctx, size);
        let src: Box<dyn ReflectionSource> = match source {
            Source::Probes => {
                Box::new(OctahedralProbes::new(ctx, &pbr, ProbeConfig::default()).expect("probes"))
            }
            Source::Select(q, c) => select(ctx, &pbr, &meshes, q, c),
        };
        Rig {
            ctx,
            pbr,
            meshes,
            src,
        }
    }
    fn frame(&mut self, view: &View, s: &Spec, what: &str) -> HdrImage {
        render(
            self.ctx,
            &mut self.pbr,
            &self.meshes,
            self.src.as_mut(),
            view,
            s,
            what,
        )
    }
}

fn ray(q: ReflectionQuality) -> Source {
    Source::Select(q, RayQueryConfig::default())
}

fn mean_rgb(img: &HdrImage, pts: &[(u32, u32)]) -> V3 {
    assert!(!pts.is_empty(), "empty region");
    let mut s = [0.0f64; 3];
    for &(x, y) in pts {
        let p = img.at(x, y);
        for c in 0..3 {
            s[c] += f64::from(p[c]);
        }
    }
    s.map(|v| (v / pts.len() as f64) as f32)
}

fn bits(img: &HdrImage) -> Vec<u32> {
    img.pixels
        .iter()
        .flat_map(|p| p.iter().map(|c| c.to_bits()))
        .collect()
}

fn assert_identical(a: &HdrImage, b: &HdrImage, what: &str) {
    assert_eq!((a.width, a.height), (b.width, b.height), "{what}: size");
    let (ba, bb) = (bits(a), bits(b));
    let diff = ba.iter().zip(&bb).filter(|(x, y)| x != y).count();
    if diff > 0 {
        let first = ba.iter().zip(&bb).position(|(x, y)| x != y).unwrap_or(0);
        let px = (first / 4) as u32;
        panic!(
            "{what}: {diff} channels differ; first at pixel ({}, {}): {:?} vs {:?}",
            px % a.width,
            px / a.width,
            a.pixels[px as usize],
            b.pixels[px as usize]
        );
    }
}

// ---------------------------------------------------------------------------------------
// (1) Fallback identity
// ---------------------------------------------------------------------------------------

/// The scene sequence for the identity checks: static, static, then both balls moved.
fn identity_frames() -> [Spec; 3] {
    let moved = Spec {
        a: [-0.045, 0.012, R],
        b: [0.07, -0.01, R],
        roughness: None,
    };
    [SCENE, SCENE, moved]
}

/// Renders the identity sequence with the probe path and with `source` on `ctx`; the
/// readbacks must be byte-identical frame by frame. Returns the probe images.
fn check_identical_to_probes(
    ctx: &'static GpuContext,
    source: Source,
    what: &str,
) -> Vec<HdrImage> {
    let mut probes = Rig::new(ctx, OVERVIEW.size, Source::Probes);
    let mut other = Rig::new(ctx, OVERVIEW.size, source);
    assert_eq!(
        other.src.name(),
        probes.src.name(),
        "{what}: the fallback must be the probe source"
    );
    assert_ne!(other.src.name(), "ray_query", "{what}");
    let mut out = Vec::new();
    for (i, s) in identity_frames().iter().enumerate() {
        let want = probes.frame(&OVERVIEW, s, "probe path");
        let got = other.frame(&OVERVIEW, s, what);
        assert_identical(&got, &want, &format!("{what}, frame {i}"));
        assert_eq!(
            other.src.stats(),
            probes.src.stats(),
            "{what}, frame {i}: probe counters"
        );
        out.push(want);
    }
    out
}

/// The probe image is not trivially dark: ball A shows the red panel above it, ball B
/// the blue one (so the identity above compares real probe reflections).
fn check_probe_image_reflects_panels(img: &HdrImage) {
    let chrome = chrome();
    for (me, other, panel, l, ch, name) in [
        (BALL_A, BALL_B, red_panel(), RED_L, 0usize, "A / red"),
        (BALL_B, BALL_A, blue_panel(), BLUE_L, 2usize, "B / blue"),
    ] {
        let reg = direct_region(&OVERVIEW, me, other, &panel);
        assert!(
            reg.len() >= 15,
            "{name}: direct region too small ({})",
            reg.len()
        );
        let m = mean_rgb(img, &reg);
        let want = chrome[ch] * l[ch];
        println!(
            "probe image {name}: direct region {} px, mean {m:?}, F·L {want}",
            reg.len()
        );
        assert!(
            m[ch] > 0.5 * want && m[ch] < 1.5 * want,
            "{name}: panel reflection {} vs F·L {want}",
            m[ch]
        );
        let others = (0..3)
            .filter(|&c| c != ch)
            .map(|c| m[c])
            .fold(0.0, f32::max);
        assert!(m[ch] > 4.0 * others, "{name}: wrong colour {m:?}");
    }
}

fn check_fallback_identity() {
    let Some(ctx) = gpu() else {
        return;
    };
    // Off on whatever device we have: the probe path, byte for byte.
    let imgs = check_identical_to_probes(ctx, ray(ReflectionQuality::Off), "quality Off");
    check_probe_image_reflects_panels(&imgs[0]);
    // Feature absent: every quality falls back to the probe path, byte for byte, and the
    // ray-query source refuses to be created (an error, not a panic).
    if let Some(bare) = featureless_ctx("fallback_identity") {
        for q in [
            ReflectionQuality::Off,
            ReflectionQuality::BallsOnly,
            ReflectionQuality::BallsAndClearCoat,
        ] {
            check_identical_to_probes(bare, ray(q), &format!("feature absent, {q}"));
        }
        let (pbr, bare_meshes) = new_pbr(bare, 16);
        for q in [
            ReflectionQuality::BallsOnly,
            ReflectionQuality::BallsAndClearCoat,
        ] {
            let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                new_ray(bare, &pbr, &bare_meshes, q, RayQueryConfig::default()).is_err()
            }));
            assert!(
                matches!(made, Ok(true)),
                "RayQueryReflections::new on a device without the feature must return an error \
                 without panicking ({q}): {}",
                if made.is_ok() { "created" } else { "panicked" }
            );
        }
    }
}

#[test]
fn fallback_is_byte_identical_to_the_probe_path() {
    check_fallback_identity();
}

// ---------------------------------------------------------------------------------------
// (2) Two-ball mutual reflection
// ---------------------------------------------------------------------------------------

/// Lean of a region towards the other ball's colour: (other channel − own channel) over
/// the expected two-bounce radiance `F_chrome² · L_other` (in the other channel).
///
/// Margins (documented derivation): a renderer that traces the A → B → panel path shows
/// `F_A · F_B · L` with F ≥ the chrome base colour, so its lean is ≈ 1 (the own channel
/// is ~0: no path reaches the own panel by construction of the region). We require
/// ≥ 0.5, which leaves room for MSAA/edge filtering, f16 storage and any probe-based
/// shading of the second bounce. A probe-only render shows B shaded with the black
/// placeholder environment there (≈ 0); we require ≤ 0.1 (a tenth of the expected
/// signal) so that a probe path cannot pass the ray criterion by accident.
fn lean(m: V3, other_ch: usize, own_ch: usize, other_l: V3) -> f32 {
    let f = chrome()[other_ch];
    (m[other_ch] - m[own_ch]) / (f * f * other_l[other_ch])
}

/// One ball's view in the two-ball scene: the region, the channels and the expected
/// colour.
struct Mutual {
    name: &'static str,
    view: View,
    region: Vec<(u32, u32)>,
    other_ch: usize,
    own_ch: usize,
    other_l: V3,
}

fn mutual_views() -> [Mutual; 2] {
    let va = zoom(BALL_A, BALL_B);
    let vb = zoom(BALL_B, BALL_A);
    let ra = mutual_region(&va, BALL_A, BALL_B, &blue_panel(), &red_panel());
    let rb = mutual_region(&vb, BALL_B, BALL_A, &red_panel(), &blue_panel());
    for (n, r) in [("A", &ra), ("B", &rb)] {
        assert!(
            r.len() >= 100,
            "test geometry: ball {n}'s mutual region has only {} px",
            r.len()
        );
    }
    [
        Mutual {
            name: "ball A (left) shows ball B (blue)",
            view: va,
            region: ra,
            other_ch: 2,
            own_ch: 0,
            other_l: BLUE_L,
        },
        Mutual {
            name: "ball B (right) shows ball A (red)",
            view: vb,
            region: rb,
            other_ch: 0,
            own_ch: 2,
            other_l: RED_L,
        },
    ]
}

/// Ray mode (BallsOnly) shows each ball in the other with the other ball's colour; the
/// probe path does not. Returns the ray-mode images (A view, B view).
fn check_mutual_reflection(ctx: &'static GpuContext) -> Vec<HdrImage> {
    let mut out = Vec::new();
    for m in mutual_views() {
        let mut rays = Rig::new(ctx, m.view.size, ray(ReflectionQuality::BallsOnly));
        assert_eq!(rays.src.name(), "ray_query");
        let mut probes = Rig::new(ctx, m.view.size, Source::Probes);
        // Second frame of each (any one-off setup has happened).
        rays.frame(&m.view, &SCENE, m.name);
        let ray_img = rays.frame(&m.view, &SCENE, m.name);
        probes.frame(&m.view, &SCENE, m.name);
        let probe_img = probes.frame(&m.view, &SCENE, m.name);
        let mr = mean_rgb(&ray_img, &m.region);
        let mp = mean_rgb(&probe_img, &m.region);
        let (lr, lp) = (
            lean(mr, m.other_ch, m.own_ch, m.other_l),
            lean(mp, m.other_ch, m.own_ch, m.other_l),
        );
        println!(
            "{}: {} px; ray {mr:?} lean {lr:.3}; probes {mp:?} lean {lp:.3}",
            m.name,
            m.region.len()
        );
        assert!(lr >= 0.5, "{}: ray mode lean {lr} < 0.5 ({mr:?})", m.name);
        assert!(
            mr[m.other_ch] > 4.0 * mr[m.own_ch],
            "{}: ray mode region not dominated by the other ball's colour: {mr:?}",
            m.name
        );
        assert!(lp <= 0.1, "{}: probe-only lean {lp} > 0.1 ({mp:?})", m.name);
        let f = chrome()[m.other_ch];
        assert!(
            mp[m.other_ch] <= 0.1 * f * f * m.other_l[m.other_ch],
            "{}: probe-only render shows the other ball's colour: {mp:?}",
            m.name
        );
        out.push(ray_img);
    }
    out
}

#[test]
fn ray_mode_shows_each_ball_in_the_other_and_probes_do_not() {
    let Some(ctx) = ray_ctx("mutual_reflection") else {
        return;
    };
    check_mutual_reflection(ctx);
}

// ---------------------------------------------------------------------------------------
// Roughness cutoff
// ---------------------------------------------------------------------------------------

/// Below the cutoff the ray result is used, above it the probe result.
#[test]
fn roughness_cutoff_switches_between_ray_and_probe_results() {
    let Some(ctx) = ray_ctx("roughness_cutoff") else {
        return;
    };
    let [m, _] = mutual_views();
    let cfg = |c: f32| RayQueryConfig {
        ray_roughness_cutoff: c,
        ..RayQueryConfig::default()
    };
    // (ball roughness, cutoff, ray expected). Ray-traced leans at higher roughness may
    // be lower if the implementation blurs rough reflections (GGX α = ρ²: 0.04 at 0.2,
    // 0.12 at 0.35), hence the lower bounds 0.3 / 0.2; the probe result stays ≤ 0.1.
    let cases = [
        (0.2, 0.3, true),
        (0.35, 0.3, false),
        (0.6, 0.3, false),
        (0.35, 0.5, true),
    ];
    for (rough, cutoff, want_ray) in cases {
        let s = Spec {
            roughness: Some(rough),
            ..SCENE
        };
        let what = format!("roughness {rough}, cutoff {cutoff}");
        let mut rays = Rig::new(
            ctx,
            m.view.size,
            Source::Select(ReflectionQuality::BallsOnly, cfg(cutoff)),
        );
        let mut probes = Rig::new(ctx, m.view.size, Source::Probes);
        rays.frame(&m.view, &s, &what);
        let ri = rays.frame(&m.view, &s, &what);
        probes.frame(&m.view, &s, &what);
        let pi = probes.frame(&m.view, &s, &what);
        let (mr, mp) = (mean_rgb(&ri, &m.region), mean_rgb(&pi, &m.region));
        let lr = lean(mr, m.other_ch, m.own_ch, m.other_l);
        println!("{what}: ray {mr:?} lean {lr:.3}; probes {mp:?}");
        if want_ray {
            let min = if rough <= 0.2 { 0.3 } else { 0.2 };
            assert!(
                lr >= min,
                "{what}: below the cutoff the ray result must show (lean {lr})"
            );
        } else {
            assert!(
                lr <= 0.1,
                "{what}: above the cutoff the probe result must be used (lean {lr})"
            );
            let f = chrome()[m.other_ch];
            let tol = 0.05 * f * f * m.other_l[m.other_ch] + 1e-3;
            for c in 0..3 {
                assert!(
                    (mr[c] - mp[c]).abs() <= tol,
                    "{what}: channel {c}: ray-mode region {} vs probe path {} (tolerance {tol})",
                    mr[c],
                    mp[c]
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// (3) Determinism, (4) finite output
// ---------------------------------------------------------------------------------------

/// Two renders of the same frame are identical: consecutive frames of one renderer, and
/// the same frame number of a second renderer. Returns the BallsAndClearCoat overview.
fn check_deterministic(ctx: &'static GpuContext) -> HdrImage {
    let mut keep = None;
    for q in [
        ReflectionQuality::BallsOnly,
        ReflectionQuality::BallsAndClearCoat,
    ] {
        let what = format!("deterministic {q}");
        let mut a = Rig::new(ctx, OVERVIEW.size, ray(q));
        let mut b = Rig::new(ctx, OVERVIEW.size, ray(q));
        a.frame(&OVERVIEW, &SCENE, &what);
        let a2 = a.frame(&OVERVIEW, &SCENE, &what);
        let a3 = a.frame(&OVERVIEW, &SCENE, &what);
        b.frame(&OVERVIEW, &SCENE, &what);
        let b2 = b.frame(&OVERVIEW, &SCENE, &what);
        assert_identical(&a3, &a2, &format!("{what}: frames 2 and 3"));
        assert_identical(&b2, &a2, &format!("{what}: two renderers, frame 2"));
        keep = Some(a2);
    }
    keep.expect("rendered")
}

#[test]
fn ray_mode_renders_are_deterministic() {
    let Some(ctx) = ray_ctx("deterministic") else {
        return;
    };
    check_deterministic(ctx);
}

/// (4) Every quality, a moved scene, rough and polished balls and two coincident balls
/// render without NaN / Inf / negative values (checked in `render`).
#[test]
fn ray_mode_output_is_finite() {
    let Some(ctx) = ray_ctx("finite") else {
        return;
    };
    let specs = [
        SCENE,
        identity_frames()[2],
        Spec {
            roughness: Some(0.0),
            ..SCENE
        },
        Spec {
            roughness: Some(1.0),
            ..SCENE
        },
        // Both balls at one place: rays start inside the other sphere.
        Spec {
            a: BALL_A,
            b: BALL_A,
            roughness: None,
        },
        // Ball sunk halfway into the playfield, ball touching a panel.
        Spec {
            a: [-0.06, 0.0, 0.0],
            b: [0.06, 0.0, PANEL_Z - R],
            roughness: None,
        },
    ];
    for q in [
        ReflectionQuality::Off,
        ReflectionQuality::BallsOnly,
        ReflectionQuality::BallsAndClearCoat,
    ] {
        let mut rig = Rig::new(ctx, OVERVIEW.size, ray(q));
        for (i, s) in specs.iter().enumerate() {
            rig.frame(&OVERVIEW, s, &format!("finite {q} scene {i}"));
        }
        let zv = zoom(BALL_A, BALL_B);
        let mut z = Rig::new(ctx, zv.size, ray(q));
        z.frame(&zv, &SCENE, &format!("finite {q} zoom"));
    }
}

// ---------------------------------------------------------------------------------------
// (5) Quality ordering
// ---------------------------------------------------------------------------------------

/// Overview pixel classes: `pf` = the playfield fills the pixel and its 5×5
/// neighbourhood (half-resolution clear coat and edge coverage stay inside), `other` =
/// a ball or the background fills the 5×5 neighbourhood.
struct Masks {
    pf: Vec<bool>,
    other: Vec<bool>,
}

fn masks(view: &View, s: &Spec) -> Masks {
    let n = view.size as i64;
    let class: Vec<Prim> = (0..n * n)
        .map(|i| primary(view, s, (i % n) as u32, (i / n) as u32).0)
        .collect();
    let mut pf = vec![false; class.len()];
    let mut other = vec![false; class.len()];
    for y in 0..n {
        for x in 0..n {
            let c = class[(y * n + x) as usize];
            let mut uniform = true;
            for dy in -2..=2 {
                for dx in -2..=2 {
                    let (xx, yy) = (x + dx, y + dy);
                    if xx < 0 || yy < 0 || xx >= n || yy >= n {
                        continue;
                    }
                    let cc = class[(yy * n + xx) as usize];
                    let same = (cc == Prim::Playfield) == (c == Prim::Playfield);
                    uniform &= same;
                }
            }
            if uniform {
                let i = (y * n + x) as usize;
                if c == Prim::Playfield {
                    pf[i] = true;
                } else {
                    other[i] = true;
                }
            }
        }
    }
    Masks { pf, other }
}

/// Playfield pixels (inside `pf`) whose mirror ray hits the middle of `panel` without
/// passing a ball, with the clear coat's expected reflection `F_cc(θ) · L` (F0 0.04,
/// Schlick).
fn clearcoat_region(
    view: &View,
    s: &Spec,
    m: &Masks,
    panel: &Quad,
    l: V3,
) -> (Vec<(u32, u32)>, V3) {
    let mut pts = Vec::new();
    let mut want = [0.0f64; 3];
    for y in 0..view.size {
        for x in 0..view.size {
            if !m.pf[(y * view.size + x) as usize] {
                continue;
            }
            let (_, p, d) = primary(view, s, x, y);
            let r = reflect(d, [0.0, 0.0, 1.0]);
            if !panel.hit(p, r, 0.55) {
                continue;
            }
            if hit_sphere(p, r, s.a, R * 1.2).is_some() || hit_sphere(p, r, s.b, R * 1.2).is_some()
            {
                continue;
            }
            let cos = r[2].clamp(0.0, 1.0);
            let f = 0.04 + 0.96 * (1.0 - cos).powi(5);
            for c in 0..3 {
                want[c] += f64::from(f * l[c]);
            }
            pts.push((x, y));
        }
    }
    let k = pts.len().max(1) as f64;
    (pts, want.map(|v| (v / k) as f32))
}

/// BallsAndClearCoat differs from BallsOnly only on the playfield, where it adds the
/// clear-coat reflection of the panels. Returns the BallsAndClearCoat image.
fn check_quality_ordering(ctx: &'static GpuContext) -> HdrImage {
    let mut balls = Rig::new(ctx, OVERVIEW.size, ray(ReflectionQuality::BallsOnly));
    let mut coat = Rig::new(
        ctx,
        OVERVIEW.size,
        ray(ReflectionQuality::BallsAndClearCoat),
    );
    balls.frame(&OVERVIEW, &SCENE, "BallsOnly");
    let bo = balls.frame(&OVERVIEW, &SCENE, "BallsOnly");
    coat.frame(&OVERVIEW, &SCENE, "BallsAndClearCoat");
    let bc = coat.frame(&OVERVIEW, &SCENE, "BallsAndClearCoat");
    let m = masks(&OVERVIEW, &SCENE);
    let n_other = m.other.iter().filter(|&&b| b).count();
    let n_pf = m.pf.iter().filter(|&&b| b).count();
    assert!(
        n_other > 200 && n_pf > 5000,
        "test geometry: masks {n_other} / {n_pf}"
    );
    // Outside the playfield (balls, background): identical.
    let mut diff = 0;
    let mut first = None;
    for (i, p) in bo.pixels.iter().enumerate() {
        if m.other[i] && p.map(f32::to_bits) != bc.pixels[i].map(f32::to_bits) {
            diff += 1;
            first.get_or_insert(i);
        }
    }
    assert!(
        diff == 0,
        "BallsAndClearCoat changed {diff} pixels outside the playfield; first {:?}: {:?} vs {:?}",
        first.map(|i| (i as u32 % OVERVIEW.size, i as u32 / OVERVIEW.size)),
        first.map(|i| bo.pixels[i]),
        first.map(|i| bc.pixels[i])
    );
    // On the playfield: the clear coat now mirrors each panel in its colour.
    let mut changed = 0usize;
    for (i, p) in bo.pixels.iter().enumerate() {
        if m.pf[i] && p.map(f32::to_bits) != bc.pixels[i].map(f32::to_bits) {
            changed += 1;
        }
    }
    assert!(
        changed > 0,
        "BallsAndClearCoat does not change the playfield at all"
    );
    for (panel, l, ch, name) in [
        (red_panel(), RED_L, 0usize, "red"),
        (blue_panel(), BLUE_L, 2usize, "blue"),
    ] {
        let (pts, want) = clearcoat_region(&OVERVIEW, &SCENE, &m, &panel, l);
        assert!(
            pts.len() >= 100,
            "test geometry: {name} clear-coat region {} px",
            pts.len()
        );
        let (mo, mc) = (mean_rgb(&bo, &pts), mean_rgb(&bc, &pts));
        println!(
            "clear coat {name}: {} px; BallsOnly {mo:?}, BallsAndClearCoat {mc:?}, F_cc·L {want:?}",
            pts.len()
        );
        assert!(
            mc[ch] - mo[ch] >= 0.3 * want[ch],
            "{name}: clear-coat reflection adds {} < 0.3 · F_cc·L = {}",
            mc[ch] - mo[ch],
            0.3 * want[ch]
        );
        let others = (0..3)
            .filter(|&c| c != ch)
            .map(|c| mc[c] - mo[c])
            .fold(0.0, f32::max);
        assert!(
            mc[ch] - mo[ch] > 4.0 * others.max(0.0),
            "{name}: clear-coat reflection has the wrong colour: {mo:?} → {mc:?}"
        );
    }
    bc
}

#[test]
fn clear_coat_quality_changes_only_the_playfield() {
    let Some(ctx) = ray_ctx("quality_ordering") else {
        return;
    };
    check_quality_ordering(ctx);
}

// ---------------------------------------------------------------------------------------
// Acceleration-structure counters
// ---------------------------------------------------------------------------------------

/// BLAS: one per static mesh, built once (this scene has two meshes: sphere and quad).
/// TLAS: one refit per frame.
#[test]
fn blas_built_once_per_mesh_and_tlas_refit_per_frame() {
    let Some(ctx) = ray_ctx("ray_stats") else {
        return;
    };
    for q in [
        ReflectionQuality::BallsOnly,
        ReflectionQuality::BallsAndClearCoat,
    ] {
        let (mut pbr, meshes) = new_pbr(ctx, 64);
        let mut src = new_ray(ctx, &pbr, &meshes, q, RayQueryConfig::default())
            .unwrap_or_else(|e| panic!("RayQueryReflections::new with the feature present: {e}"));
        assert_eq!(src.name(), "ray_query");
        let view = View {
            size: 64,
            ..OVERVIEW
        };
        let frames = identity_frames();
        render(ctx, &mut pbr, &meshes, &mut src, &view, &frames[0], "stats");
        let (blas1, tlas1) = ray_counts(&src);
        println!("{q}: after frame 1: {blas1} BLAS builds, {tlas1} TLAS refits");
        assert!(
            (1..=2).contains(&blas1),
            "{q}: {blas1} BLAS builds for 2 meshes after the first frame"
        );
        assert!(tlas1 <= 1, "{q}: {tlas1} TLAS refits in the first frame");
        let mut tlas = tlas1;
        for i in 1..6 {
            // Balls move every other frame.
            let s = &frames[1 + i % 2];
            render(ctx, &mut pbr, &meshes, &mut src, &view, s, "stats");
            let (b, t) = ray_counts(&src);
            assert_eq!(b, blas1, "{q}: frame {}: BLAS rebuilt", i + 1);
            assert_eq!(
                t,
                tlas + 1,
                "{q}: frame {}: TLAS refits {tlas} → {t}",
                i + 1
            );
            tlas = t;
        }
    }
}

#[test]
fn selector_picks_ray_query_when_the_feature_is_present() {
    let Some(ctx) = ray_ctx("selector") else {
        return;
    };
    let (pbr, meshes) = new_pbr(ctx, 16);
    for q in [
        ReflectionQuality::BallsOnly,
        ReflectionQuality::BallsAndClearCoat,
    ] {
        assert_eq!(
            select(ctx, &pbr, &meshes, q, RayQueryConfig::default()).name(),
            "ray_query",
            "{q}"
        );
    }
    let probes = OctahedralProbes::new(ctx, &pbr, ProbeConfig::default()).expect("probes");
    assert_eq!(
        select(
            ctx,
            &pbr,
            &meshes,
            ReflectionQuality::Off,
            RayQueryConfig::default()
        )
        .name(),
        probes.name(),
        "Off selects the probe path"
    );
    assert_eq!(
        select(
            ctx,
            &pbr,
            &meshes,
            ReflectionQuality::default(),
            RayQueryConfig::default()
        )
        .name(),
        probes.name(),
        "the default setting (D33) selects the probe path"
    );
}

// ---------------------------------------------------------------------------------------
// (6) Regression snapshot, only after (1)–(5)
// ---------------------------------------------------------------------------------------

/// Compares (or with `SPEC_5_4_BLESS=1` stores) a raw sRGB RGBA8 snapshot of `img`.
/// Secondary evidence only (D30). Per-backend tolerance: stored on lavapipe (llvmpipe);
/// other rasterisers / ray-query implementations differ at silhouettes and in the last
/// bits of the shading.
fn snapshot(ctx: &GpuContext, name: &str, img: &HdrImage) {
    let bytes = img.to_srgb8();
    assert_eq!(bytes.len(), (img.width * img.height * 4) as usize);
    let dir = format!("{}/tests/fixtures/spec_5_4", env!("CARGO_MANIFEST_DIR"));
    let path = format!("{dir}/{name}_{}x{}.rgba8", img.width, img.height);
    if std::env::var("SPEC_5_4_BLESS").as_deref() == Ok("1") {
        std::fs::create_dir_all(&dir).expect("fixture dir");
        std::fs::write(&path, &bytes).expect("write snapshot");
        println!("blessed {path}");
        return;
    }
    let want = std::fs::read(&path).unwrap_or_else(|e| {
        panic!("snapshot {path} missing ({e}); run once with SPEC_5_4_BLESS=1")
    });
    assert_eq!(want.len(), bytes.len(), "snapshot {name}: size changed");
    let info = ctx.adapter_info();
    let same = info.name.to_lowercase().contains("llvmpipe");
    let (max_mean, max_outliers) = if same { (0.5, 0.005) } else { (3.0, 0.05) };
    let mut sum = 0u64;
    let mut outliers = 0usize;
    for (a, b) in bytes.iter().zip(&want) {
        let d = u64::from(a.abs_diff(*b));
        sum += d;
        if d > 12 {
            outliers += 1;
        }
    }
    let mean = sum as f64 / bytes.len() as f64;
    let frac = outliers as f64 / bytes.len() as f64;
    assert!(
        mean <= max_mean && frac <= max_outliers,
        "snapshot {name} on {}: mean abs diff {mean:.3} (max {max_mean}), {:.2} % channels off \
         by > 12 (max {:.2} %)",
        info.name,
        frac * 100.0,
        max_outliers * 100.0
    );
}

#[test]
fn regression_snapshot_after_the_analytic_checks() {
    let Some(ctx) = ray_ctx("snapshot") else {
        return;
    };
    // (1)–(5) first, on the very images that are then snapshotted.
    check_fallback_identity();
    let views = check_mutual_reflection(ctx);
    let overview = check_deterministic(ctx);
    let coat = check_quality_ordering(ctx);
    assert_identical(
        &coat,
        &overview,
        "the snapshotted overview is the deterministic one",
    );
    snapshot(ctx, "overview_balls_and_clearcoat", &coat);
    snapshot(ctx, "zoom_a_balls", &views[0]);
    snapshot(ctx, "zoom_b_balls", &views[1]);
}
