//! The wasm-bindgen surface of the S9 engine module (wasm32 only):
//! [`WebEngine`] (bundle loading, fixed-step stepping, live input, parity
//! status), [`WebGpu`] (wgpu on a canvas: WebGPU or WebGL2, the S3 table
//! renderer) and, with feature `harness`, the S1/S2/S4 probes run inside
//! the combined module.

use crate::engine::{self, Engine, LoadOpts};
use pin_render::displays::DisplayRoute;
use pin_render::gpu::{GpuContext, GpuOptions, wgpu};
use pin_render::post::{AaMode, PostConfig};
use pin_render::topology::{Rect, SurfaceRole};
use s4_rules::run::Design;
use serde_json::json;
use std::cell::RefCell;
use std::sync::Arc;
use wasm_bindgen::prelude::*;
use web_probe::renderer::TableRenderer;
use web_probe::scene::TableVisuals;
use web_sys::HtmlCanvasElement;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = performance, js_name = now)]
    fn perf_now() -> f64;
}

thread_local! {
    static GPU_ERRORS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static LOG_LINES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

struct ConsoleLogger;

impl log::Log for ConsoleLogger {
    fn enabled(&self, m: &log::Metadata<'_>) -> bool {
        m.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!("[{} {}] {}", record.level(), record.target(), record.args());
        match record.level() {
            log::Level::Error => web_sys::console::error_1(&line.clone().into()),
            log::Level::Warn => web_sys::console::warn_1(&line.clone().into()),
            _ => web_sys::console::log_1(&line.clone().into()),
        }
        if record.level() <= log::Level::Warn {
            LOG_LINES.with(|l| l.borrow_mut().push(line));
        }
    }

    fn flush(&self) {}
}

static LOGGER: ConsoleLogger = ConsoleLogger;

fn err(e: impl core::fmt::Display) -> JsValue {
    JsValue::from_str(&e.to_string())
}

/// Module start: a panic hook (message to `console.error` and to
/// `globalThis.__pinWebPanic`, so the page can show it after the trap) and
/// the console logger.
#[wasm_bindgen(start)]
pub fn start() {
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("pin_web panic: {info}");
        web_sys::console::error_1(&msg.clone().into());
        let _ = js_sys::Reflect::set(&js_sys::global(), &"__pinWebPanic".into(), &msg.into());
    }));
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Warn);
    }
}

/// What this module was built with (JSON).
#[wasm_bindgen]
pub fn build_info() -> String {
    json!({
        "crate": "pin_web",
        "target_feature_simd128": cfg!(target_feature = "simd128"),
        "target_feature_exception_handling": cfg!(target_feature = "exception-handling"),
        "feature_webgpu": cfg!(feature = "webgpu"),
        "feature_webgl": cfg!(feature = "webgl"),
        "feature_harness": cfg!(feature = "harness"),
        "wgpu_enabled_backend_features": format!("{:?}", wgpu::Instance::enabled_backend_features()),
        "panic": if cfg!(panic = "abort") { "abort" } else { "unwind" },
        "stack_layout": s1_harness::probes::stack_layout(),
        "web_max_bundle_bytes": engine::WEB_MAX_BUNDLE_BYTES,
    })
    .to_string()
}

/// Heap figures of the global allocator (pin_sim's counting allocator via
/// S4's peak tracker): `{current, peak}` bytes.
#[wasm_bindgen]
pub fn heap_stats() -> String {
    json!({ "current": s4_rules::peak::current(), "peak": s4_rules::peak::peak() }).to_string()
}

/// Resets the heap high-water mark.
#[wasm_bindgen]
pub fn heap_reset_peak() {
    s4_rules::peak::reset();
}

/// Most ticks one [`WebEngine::step`] call may run (10 s of simulated time).
pub const MAX_TICKS_PER_CALL: u32 = 10_000;

/// The engine: one loaded table, stepped at 1 kHz simulated time.
#[wasm_bindgen]
#[derive(Default)]
pub struct WebEngine {
    eng: Option<Engine>,
    trap_at: Option<u64>,
    measure_stack: bool,
    stack: Vec<(String, usize)>,
}

impl WebEngine {
    fn measured<R>(&mut self, what: &str, f: impl FnOnce(&mut Self) -> R) -> R {
        if !self.measure_stack {
            return f(self);
        }
        let (r, used, _) = s1_harness::probes::stack_highwater(usize::MAX, || f(&mut *self));
        self.note_stack(what, used);
        r
    }

    fn note_stack(&mut self, what: &str, used: usize) {
        match self.stack.iter_mut().find(|(w, _)| w == what) {
            Some((_, u)) => *u = (*u).max(used),
            None => self.stack.push((what.to_owned(), used)),
        }
    }

    fn engine(&self) -> Result<&Engine, JsValue> {
        self.eng.as_ref().ok_or_else(|| err("no table loaded"))
    }

    fn info(e: &Engine) -> String {
        json!({
            "rules": e.rules,
            "entry": e.entry,
            "table_hash": format!("{:016x}", e.sim.table_hash()),
            "files": e.vfs.list().len(),
            "bundle_bytes": e.bundle_bytes,
            "tick": e.sim.tick(),
        })
        .to_string()
    }
}

#[wasm_bindgen]
impl WebEngine {
    /// An empty engine.
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> WebEngine {
        WebEngine::default()
    }

    /// Loads a bundle for live play (`bytes` moved in: one copy from the JS
    /// `ArrayBuffer`). On error the previously loaded table stays loaded and
    /// usable; the error text is the loader's own.
    ///
    /// # Errors
    /// The loader's error text.
    pub fn load_bundle(&mut self, bytes: Vec<u8>, seed: f64) -> Result<String, JsValue> {
        let design = Design::WasmiInstrDepth;
        let r = self.measured("load_bundle", |_| {
            Engine::load(
                bytes,
                LoadOpts {
                    seed: seed as u64,
                    scr: None,
                    design,
                },
            )
        });
        let e = r.map_err(err)?;
        let info = Self::info(&e);
        self.eng = Some(e);
        Ok(info)
    }

    /// Loads a bundle for a replay of S1's scenario `scn` (its seed), with
    /// S1's script stream `scr` (Lua: S2's per-step checker) and native
    /// reference hashes `refs` (S9REF), and compares tick 0.
    ///
    /// # Errors
    /// Loader or file errors.
    pub fn load_replay(
        &mut self,
        bundle: Vec<u8>,
        scn: Vec<u8>,
        scr: Option<Vec<u8>>,
        refs: Option<Vec<u8>>,
    ) -> Result<String, JsValue> {
        let seed = Engine::scenario_seed(&scn).map_err(err)?;
        let r = self.measured("load_replay", |_| {
            let mut e = Engine::load(
                bundle,
                LoadOpts {
                    seed,
                    scr,
                    design: Design::WasmiInstrDepth,
                },
            )?;
            e.attach_replay(&scn, refs.as_deref())?;
            Ok::<Engine, String>(e)
        });
        let e = r.map_err(err)?;
        let info = Self::info(&e);
        self.eng = Some(e);
        Ok(info)
    }

    /// Whether a table is loaded.
    #[must_use]
    pub fn loaded(&self) -> bool {
        self.eng.is_some()
    }

    /// Runs up to `n` ticks; returns the ticks run. A tick armed with
    /// [`arm_trap`](Self::arm_trap) panics (wasm `unreachable`: the module
    /// is then terminated, as any engine trap).
    ///
    /// # Errors
    /// No table, or a replay operation failed.
    pub fn step(&mut self, n: u32) -> Result<u32, JsValue> {
        // A negative JS count arrives as 2^32 - k (wasm-bindgen converts with `>>> 0`); refuse
        // anything beyond 10 s of simulated time per call instead of freezing the page.
        if n > MAX_TICKS_PER_CALL {
            return Err(err(format!(
                "step({n}): more than {MAX_TICKS_PER_CALL} ticks per call"
            )));
        }
        let trap_at = self.trap_at;
        self.measured("step", |s| {
            let e = s.eng.as_mut().ok_or_else(|| err("no table loaded"))?;
            if let Some(t) = trap_at
                && e.sim.tick() + u64::from(n) >= t
            {
                let pre = u32::try_from(t.saturating_sub(e.sim.tick() + 1)).unwrap_or(0);
                e.step(pre).map_err(err)?;
                panic!("S9 injected engine trap at tick {}", e.sim.tick() + 1);
            }
            e.step(n).map_err(err)
        })
    }

    /// Queues a live cabinet input (`left`, `right`, `launch`, `start`, ...).
    ///
    /// # Errors
    /// No table or an unknown input name.
    pub fn input(&mut self, name: &str, pressed: bool) -> Result<(), JsValue> {
        let i = engine::cabinet_input(name).ok_or_else(|| err(format!("unknown input {name}")))?;
        self.eng
            .as_mut()
            .ok_or_else(|| err("no table loaded"))?
            .push_input(i, pressed);
        Ok(())
    }

    /// Status (JSON): tick, hashes, rules state, replay comparison.
    ///
    /// # Errors
    /// No table.
    pub fn status(&self) -> Result<String, JsValue> {
        Ok(self.engine()?.status())
    }

    /// Whether the attached replay has finished.
    #[must_use]
    pub fn replay_done(&self) -> bool {
        self.eng.as_ref().is_some_and(Engine::replay_done)
    }

    /// Arms an engine trap (a Rust panic) at simulated tick `tick`
    /// (req. 9 iii).
    pub fn arm_trap(&mut self, tick: f64) {
        self.trap_at = Some(tick as u64);
    }

    /// Measures the shadow-stack high-water mark of every load/step and of
    /// [`WebGpu::build`]/[`WebGpu::render`] calls from now on.
    pub fn set_measure_stack(&mut self, on: bool) {
        self.measure_stack = on;
    }

    /// The measured stack high-water marks (JSON, bytes below the caller).
    #[must_use]
    pub fn stack_report(&self) -> String {
        let m: serde_json::Map<String, serde_json::Value> = self
            .stack
            .iter()
            .map(|(k, v)| (k.clone(), json!(v)))
            .collect();
        serde_json::Value::Object(m).to_string()
    }
}

/// wgpu on a canvas plus the S3 table renderer.
#[wasm_bindgen]
pub struct WebGpu {
    ctx: GpuContext,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Option<TableRenderer>,
    routes: Vec<DisplayRoute>,
    post: PostConfig,
    info: serde_json::Value,
    frames: u64,
    /// Set while a submitted frame has not completed on the GPU (frame pacing:
    /// the page does not queue a second frame behind a slow GPU).
    in_flight: Arc<std::sync::atomic::AtomicBool>,
    skipped: u64,
    /// The egui overlay (pin_render's menu theme + `OverlayPainter`), drawn on top of
    /// the table when enabled (`overlay: true`): the app's menu/HUD stack in the module.
    overlay: Option<Overlay>,
}

struct Overlay {
    ctx: pin_render::ui::egui::Context,
    painter: pin_render::ui::OverlayPainter,
}

impl Overlay {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Overlay {
        let ctx = pin_render::ui::egui::Context::default();
        pin_render::ui::overlay_style(&ctx);
        Overlay {
            ctx,
            painter: pin_render::ui::OverlayPainter::new(device, format),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint(
        &mut self,
        gpu: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        size: [u32; 2],
        lines: &[String],
    ) -> Vec<wgpu::CommandBuffer> {
        use pin_render::ui::{OverlayFrame, raw_input, show_overlay};
        let raw = raw_input(size, 1.0, Vec::new(), Some(0.0));
        let out = self.ctx.run_ui(raw, |ui| {
            show_overlay(ui, |ui| {
                ui.heading("Kilohertz Pinball (web)");
                ui.separator();
                for l in lines {
                    ui.label(l.as_str());
                }
            });
        });
        let frame = OverlayFrame::new(&self.ctx, out);
        self.painter
            .paint(gpu.device(), gpu.queue(), encoder, view, size, frame)
    }
}

/// Where the backglass and score-display PiPs go on a `w`x`h` canvas, keeping them off the
/// lower playfield (flippers): landscape (w >= 1.2 h) puts them in the side columns the
/// aspect-fitted table leaves empty; portrait puts the score display in a band at the top
/// (below the page's control bar, 9 % of the height) and a small backglass under it.
/// (S3's probe layout put the score display over the flippers.)
#[must_use]
pub fn page_routes(w: u32, h: u32) -> Vec<DisplayRoute> {
    let route = |role, x: u32, y: u32, rw: u32, rh: u32| DisplayRoute {
        role,
        window: 0,
        viewport: Rect {
            x: x as i32,
            y: y as i32,
            w: rw.max(4),
            h: rh.max(4),
        },
        dedicated: false,
    };
    if u64::from(w) * 10 >= u64::from(h) * 12 {
        // Table width ~ 0.55 h in the middle; columns left and right of it.
        let col = (w.saturating_sub(h * 11 / 20) / 2).max(8);
        let pw = col * 9 / 10;
        let x0 = (col - pw) / 2;
        let bh = pw * 3 / 4;
        let sh = pw / 4;
        vec![
            route(SurfaceRole::Backglass, x0, h / 5, pw, bh),
            route(SurfaceRole::Score, w - col + x0, h / 2 - sh / 2, pw, sh),
        ]
    } else {
        let top = h * 9 / 100;
        let sw = w * 9 / 10;
        let sh = sw / 5;
        let bw = w * 3 / 10;
        vec![
            route(SurfaceRole::Score, (w - sw) / 2, top, sw, sh),
            route(
                SurfaceRole::Backglass,
                w / 20,
                top + sh + h / 100,
                bw,
                bw * 3 / 4,
            ),
        ]
    }
}

fn backends_of(name: &str) -> Result<wgpu::Backends, JsValue> {
    match name {
        "webgpu" => Ok(wgpu::Backends::BROWSER_WEBGPU),
        "webgl2" => Ok(wgpu::Backends::GL),
        "auto" => Ok(wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL),
        _ => Err(err(format!("unknown backend {name}"))),
    }
}

fn post_config(o: &serde_json::Value) -> PostConfig {
    let mut c = PostConfig::default();
    if let Some(m) = o.get("msaa").and_then(serde_json::Value::as_bool) {
        c.aa = if m { AaMode::Msaa4 } else { AaMode::Off };
    }
    if let Some(b) = o.get("bloom").and_then(serde_json::Value::as_bool) {
        c.bloom.enabled = b;
    }
    if let Some(b) = o.get("blur").and_then(serde_json::Value::as_bool) {
        c.motion_blur.enabled = b;
    }
    c
}

/// Creates the GPU context on `canvas` (its current width/height) for
/// `backend` = `webgpu` | `webgl2` | `auto` (`auto`: S3's
/// `new_instance_with_webgpu_detection`, WebGPU when an adapter exists,
/// else WebGL2). `opts_json`: `{msaa, bloom, blur}`.
///
/// # Errors
/// No adapter/device, or no 8-bit unorm canvas format.
#[wasm_bindgen]
pub async fn create_gpu(
    canvas: HtmlCanvasElement,
    backend: String,
    opts_json: String,
) -> Result<WebGpu, JsValue> {
    let opts: serde_json::Value = serde_json::from_str(&opts_json).map_err(err)?;
    let t0 = perf_now();
    let backends = backends_of(&backend)?;
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = backends;
    let instance = if backend == "auto" {
        wgpu::util::new_instance_with_webgpu_detection(desc).await
    } else {
        wgpu::Instance::new(desc)
    };
    let (w, h) = (canvas.width().max(1), canvas.height().max(1));
    let surface = instance
        .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
        .map_err(err)?;
    let gopts = GpuOptions {
        force_fallback_adapter: false,
        backends,
    };
    let ctx = GpuContext::request_async(instance, &gopts, Some(&surface), None)
        .await
        .map_err(err)?;
    let device = ctx.device();
    device.on_uncaptured_error(Arc::new(|e: wgpu::Error| {
        let s = format!("uncaptured: {e}");
        web_sys::console::error_1(&s.clone().into());
        GPU_ERRORS.with(|u| u.borrow_mut().push(s));
    }));
    device.set_device_lost_callback(|reason, msg| {
        let s = format!("device lost: {reason:?} {msg}");
        web_sys::console::warn_1(&s.clone().into());
        GPU_ERRORS.with(|u| u.borrow_mut().push(s));
    });
    let caps = surface.get_capabilities(ctx.adapter());
    let format = caps
        .formats
        .iter()
        .copied()
        .find(|f| {
            matches!(
                f,
                wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Bgra8Unorm
            )
        })
        .ok_or_else(|| {
            err(format!(
                "no 8-bit unorm canvas format in {:?}",
                caps.formats
            ))
        })?;
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        color_space: Default::default(),
        width: w,
        height: h,
        present_mode: wgpu::PresentMode::Fifo,
        desired_maximum_frame_latency: 2,
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
    };
    surface.configure(device, &config);
    let ai = ctx.adapter_info();
    let lim = ctx.device().limits();
    let info = json!({
        "requested": backend,
        "backend": format!("{:?}", ai.backend),
        "adapter": ai.name,
        "vendor": ai.vendor,
        "device": ai.device,
        "driver": ai.driver,
        "driver_info": ai.driver_info,
        "device_type": format!("{:?}", ai.device_type),
        "format": format!("{format:?}"),
        "surface_formats": caps.formats.iter().map(|f| format!("{f:?}")).collect::<Vec<_>>(),
        "size": [w, h],
        "max_texture_dimension_2d": lim.max_texture_dimension_2d,
        "max_storage_buffers_per_shader_stage": lim.max_storage_buffers_per_shader_stage,
        "init_ms": perf_now() - t0,
    });
    let overlay = opts
        .get("overlay")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
        .then(|| Overlay::new(ctx.device(), format));
    Ok(WebGpu {
        ctx,
        surface,
        config,
        renderer: None,
        routes: page_routes(w, h),
        post: post_config(&opts),
        info,
        frames: 0,
        in_flight: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        skipped: 0,
        overlay,
    })
}

#[wasm_bindgen]
impl WebGpu {
    /// Adapter/surface facts (JSON).
    #[must_use]
    pub fn info(&self) -> String {
        self.info.to_string()
    }

    /// Builds the table renderer for the engine's loaded bundle (its
    /// visuals read from the same VFS).
    ///
    /// # Errors
    /// No table, visuals or renderer failures.
    pub fn build(&mut self, eng: &mut WebEngine) -> Result<String, JsValue> {
        let t0 = perf_now();
        let ctx = &self.ctx;
        let (format, size, post) = (
            self.config.format,
            (self.config.width, self.config.height),
            self.post,
        );
        let r = eng.measured("gpu_build", |s| {
            let e = s.engine()?;
            let visuals = TableVisuals::from_vfs(&e.vfs).map_err(err)?;
            let r = TableRenderer::new(ctx, format, size, &visuals, post).map_err(err)?;
            Ok::<_, JsValue>((r, visuals.meshes.len(), visuals.triangles()))
        });
        let (r, meshes, tris) = r?;
        self.renderer = Some(r);
        Ok(json!({ "meshes": meshes, "triangles": tris, "build_ms": perf_now() - t0 }).to_string())
    }

    /// Resizes the canvas surface and the renderer's targets.
    ///
    /// # Errors
    /// Renderer resize failure.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), JsValue> {
        let (w, h) = (width.max(1), height.max(1));
        self.config.width = w;
        self.config.height = h;
        self.surface.configure(self.ctx.device(), &self.config);
        self.routes = page_routes(w, h);
        if let Some(r) = &mut self.renderer {
            r.resize(w, h).map_err(err)?;
        }
        Ok(())
    }

    /// Draws the engine's current snapshot (playfield PBR + post chain +
    /// backglass and score PiPs) and presents it.
    ///
    /// # Errors
    /// No renderer/table, or no surface texture.
    pub fn render(&mut self, eng: &mut WebEngine) -> Result<(), JsValue> {
        let Some(r) = self.renderer.as_mut() else {
            return Err(err("renderer not built"));
        };
        let (ctx, surface, routes) = (&self.ctx, &self.surface, &self.routes);
        let size = [self.config.width, self.config.height];
        let backend = self.info["backend"].as_str().unwrap_or("?").to_owned();
        let overlay = &mut self.overlay;
        eng.measured("gpu_render", |s| {
            let e = s.engine()?;
            if let Some(v) = e.sim.score(0) {
                r.set_score(0, v);
            }
            for sc in e.sim.display_scenes() {
                r.show_scene(sc.0, e.sim.tick());
            }
            let tex = match surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(t)
                | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
                other => return Err(err(format!("get_current_texture: {other:?}"))),
            };
            let view = tex
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            let mut enc = ctx
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("pin_web frame"),
                });
            r.encode(ctx.queue(), &mut enc, &view, routes, e.snapshot());
            let mut extra = Vec::new();
            if let Some(o) = overlay.as_mut() {
                let lines = [
                    format!("Backend: {backend}"),
                    format!(
                        "Tick: {}  Score: {}",
                        e.sim.tick(),
                        e.sim.score(0).unwrap_or(0)
                    ),
                    format!(
                        "Rules: {}{}",
                        e.rules,
                        if e.sim.script_faulted() {
                            " (tilt)"
                        } else {
                            ""
                        }
                    ),
                ];
                extra = o.paint(ctx, &mut enc, &view, size, &lines);
            }
            extra.push(enc.finish());
            ctx.queue().submit(extra);
            ctx.queue().present(tex);
            Ok(())
        })?;
        // Arc + atomic, not Rc + Cell: with the WebGL2 backend compiled in, wgpu's
        // callbacks must be Send unless `fragile-send-sync-non-atomic-wasm` is on (S3
        // recommends against that feature).
        self.in_flight
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let flag = Arc::clone(&self.in_flight);
        self.ctx.queue().on_submitted_work_done(move || {
            flag.store(false, std::sync::atomic::Ordering::Relaxed);
        });
        self.frames += 1;
        Ok(())
    }

    /// The PiP rectangles in canvas pixels (JSON `[{role, x, y, w, h}]`).
    #[must_use]
    pub fn layout(&self) -> String {
        let v: Vec<serde_json::Value> = self
            .routes
            .iter()
            .map(|r| {
                json!({ "role": format!("{:?}", r.role), "x": r.viewport.x, "y": r.viewport.y,
                        "w": r.viewport.w, "h": r.viewport.h })
            })
            .collect();
        json!({ "canvas": [self.config.width, self.config.height], "pips": v }).to_string()
    }

    /// Frames rendered.
    #[must_use]
    pub fn frames(&self) -> f64 {
        self.frames as f64
    }

    /// Whether the last submitted frame is still running on the GPU (polls
    /// the device without blocking; WebGL2 runs its callbacks here). The page
    /// skips an animation frame while this is true and counts it.
    pub fn busy(&mut self) -> bool {
        let _ = self.ctx.device().poll(wgpu::PollType::Poll);
        let b = self.in_flight.load(std::sync::atomic::Ordering::Relaxed);
        if b {
            self.skipped += 1;
        }
        b
    }

    /// Animation frames skipped because the GPU was still busy.
    #[must_use]
    pub fn skipped(&self) -> f64 {
        self.skipped as f64
    }

    /// GPU errors seen so far (uncaptured validation/OOM errors, device
    /// loss, pin_render's deferred error-scope reports, warnings): JSON.
    #[must_use]
    pub fn errors(&self) -> String {
        let unc = GPU_ERRORS.with(|u| u.borrow().clone());
        let logs = LOG_LINES.with(|l| l.borrow().clone());
        let deferred = pin_render::gpu::take_deferred_scope_errors();
        json!({ "uncaptured": unc, "log": logs, "deferred_scope_errors": deferred }).to_string()
    }
}

/// S1/S2/S4 probes inside the combined module (feature `harness`).
#[cfg(feature = "harness")]
pub mod probes {
    use super::{err, perf_now};
    use s4_rules::run::Design;
    use wasm_bindgen::prelude::*;

    fn design(s: &str) -> Result<Design, JsValue> {
        Design::parse(s).ok_or_else(|| err(format!("unknown design {s}")))
    }

    /// S1: linear-memory layout (stack-first, stack size).
    #[wasm_bindgen]
    #[must_use]
    pub fn probe_stack_layout() -> String {
        s1_harness::probes::stack_layout()
    }

    /// S1 A28: recursion of `n` 1 KiB frames on the shadow stack.
    #[wasm_bindgen]
    #[must_use]
    pub fn probe_recurse_big(n: u32) -> f64 {
        s1_harness::probes::recurse_big(n) as f64
    }

    /// S2: the adversarial Lua sandbox cases whose name contains `filter`.
    #[wasm_bindgen]
    #[must_use]
    pub fn probe_lua_sandbox(filter: &str) -> String {
        s2_harness::sandbox::run_all(filter)
    }

    /// S2: shadow-stack use per sandbox case (incl. the deepest C
    /// recursion, `cstack_gsub`).
    #[wasm_bindgen]
    #[must_use]
    pub fn probe_lua_case_stack() -> String {
        s2_harness::probes::case_stack()
    }

    /// S2: the budget-edge probe.
    #[wasm_bindgen]
    #[must_use]
    pub fn probe_lua_budget_edge() -> String {
        s2_harness::sandbox::budget_edge()
    }

    /// S2: stack pointer after `n` rounds of Lua errors (longjmp = Wasm
    /// exception through C and Rust frames).
    #[wasm_bindgen]
    #[must_use]
    pub fn probe_sp_after_errors(n: u32) -> String {
        s2_harness::probes::sp_after_errors(n)
    }

    /// S4: call-depth limits per frame shape for `design`.
    ///
    /// # Errors
    /// Unknown design.
    #[wasm_bindgen]
    pub fn probe_rules_stack(d: &str, reps: u32) -> Result<String, JsValue> {
        Ok(s4_rules::adversarial::stack_probe(
            design(d)?,
            reps as usize,
        ))
    }

    /// S4: the adversarial rules corpus on `design` (`prod` = apex's
    /// production rules.wasm, the mutation base).
    ///
    /// # Errors
    /// Unknown design.
    #[wasm_bindgen]
    pub fn probe_rules_adversarial(d: &str, prod: &[u8]) -> Result<String, JsValue> {
        Ok(s4_rules::adversarial::run_all(design(d)?, prod))
    }

    /// S4 F-A2: budget-exhausting callbacks (a lost `auto-dispatch` would
    /// overflow the browser stack here).
    ///
    /// # Errors
    /// Unknown design.
    #[wasm_bindgen]
    pub fn probe_rules_worst(d: &str, reps: u32, only: &str) -> Result<String, JsValue> {
        let only = (!only.is_empty()).then_some(only);
        Ok(s4_rules::worst::run(
            design(d)?,
            perf_now,
            reps as usize,
            only,
        ))
    }

    /// S1: a platform probe by name (`glam_ops`, `nan_bits`, `sizes`, ...).
    #[wasm_bindgen]
    #[must_use]
    pub fn probe_s1(name: &str) -> String {
        s1_harness::probes::run_probe(name)
    }

    /// D5 allocation guard (S1/S4): allocations inside every `step_1ms` of
    /// a scenario replayed with the wasmi rules backend.
    ///
    /// # Errors
    /// Load or scenario failures.
    #[wasm_bindgen]
    pub fn probe_alloc_check_wasm(bundle: &[u8], scn: &[u8]) -> Result<String, JsValue> {
        s4_rules::run::alloc_check(bundle, scn, Design::WasmiInstrDepth).map_err(err)
    }

    /// S4: the bundle-hardening check (`Vfs::from_zip_bytes`, web limits).
    #[wasm_bindgen]
    #[must_use]
    pub fn probe_zip_check(bytes: &[u8]) -> String {
        s4_rules::zipcheck::check(bytes, true)
    }
}
