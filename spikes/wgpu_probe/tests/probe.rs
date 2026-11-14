//! Q3 (ray query), Q5 (offscreen render) and Q4 (present modes).
//!
//! Q3/Q4 never fail. Q5 asserts pixels whenever it gets an adapter, and fails
//! on "no adapter" only when `SPIKE_REQUIRE_GPU=1` (set in CI).

use wgpu_probe::{
    AdapterReport, CLEAR_RGBA8, DRAW_RGBA8, REQUIRE_ENV, SIZE, all_adapters, ci_target, pixel,
    render_offscreen, request_ci_adapter,
};

fn main() {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    // ---- Q3: adapters + EXPERIMENTAL_RAY_QUERY -------------------------
    let adapters = all_adapters();
    println!("[wgpu Q3] {os}/{arch}: {} adapter(s)", adapters.len());
    for a in &adapters {
        println!("[wgpu Q3]   {}", AdapterReport::of(a));
    }

    // ---- Q5: offscreen render on lavapipe / WARP / Metal ---------------
    let required = std::env::var(REQUIRE_ENV).is_ok_and(|v| v == "1");
    let target = ci_target();
    match request_ci_adapter(&target) {
        Ok(adapter) => {
            let report = AdapterReport::of(&adapter);
            println!("[wgpu Q5] target {} -> {report}", target.label);
            if required && let Some(want) = target.expected_name {
                assert!(
                    report.name.contains(want),
                    "CI expected the {} adapter ({want:?}), got {:?}",
                    target.label,
                    report.name
                );
            }
            let data = render_offscreen(&adapter).expect("offscreen render");
            let left = pixel(&data, SIZE / 4, SIZE / 2);
            let right = pixel(&data, 3 * SIZE / 4, SIZE / 2);
            println!("[wgpu Q5] left(draw)={left:?} right(clear)={right:?}");
            assert_eq!(left, DRAW_RGBA8, "drawn half");
            assert_eq!(right, CLEAR_RGBA8, "cleared half");
            println!(
                "[wgpu Q5] RESULT: offscreen render + readback OK on {}",
                target.label
            );
        }
        Err(e) if !required => {
            println!(
                "[wgpu Q5] SKIP: no {} adapter ({e}); set {REQUIRE_ENV}=1 to fail instead",
                target.label
            );
        }
        Err(e) => panic!("[wgpu Q5] no {} adapter: {e}", target.label),
    }

    // ---- Q4: present modes ---------------------------------------------
    present_modes::probe(&adapters);
}

mod present_modes {
    use wgpu::{Adapter, Backends, SurfaceTargetUnsafe};
    use wgpu_probe::{AdapterReport, instance};

    /// SDL video drivers whose raw window handles wgpu can consume.
    const NATIVE_DRIVERS: [&str; 4] = ["windows", "cocoa", "x11", "wayland"];

    pub fn probe(adapters: &[Adapter]) {
        let sdl = match sdl3::init() {
            Ok(s) => s,
            Err(e) => return println!("[wgpu Q4] no surface available: SDL_Init failed: {e}"),
        };
        let video = match sdl.video() {
            Ok(v) => v,
            Err(e) => {
                return println!(
                    "[wgpu Q4] no surface available headless: SDL video init failed: {e}"
                );
            }
        };
        let driver = video.current_video_driver();
        if !NATIVE_DRIVERS.contains(&driver) {
            return println!(
                "[wgpu Q4] no surface available headless: SDL video driver {driver:?} has no native window"
            );
        }
        let window = match video.window("present-mode probe", 64, 64).hidden().build() {
            Ok(w) => w,
            Err(e) => {
                return println!(
                    "[wgpu Q4] no surface available: window creation failed on {driver}: {e}"
                );
            }
        };
        // One instance for the surface; adapters must come from the same instance.
        let instance = instance(Backends::all());
        // SAFETY: `window` outlives `surface` (both dropped at end of scope, surface first).
        let surface =
            match unsafe { SurfaceTargetUnsafe::from_display_and_window(&window, &window) }
                .map_err(|e| e.to_string())
                .and_then(|t| {
                    unsafe { instance.create_surface_unsafe(t) }.map_err(|e| e.to_string())
                }) {
                Ok(s) => s,
                Err(e) => {
                    return println!(
                        "[wgpu Q4] no surface available: create_surface on {driver}: {e}"
                    );
                }
            };
        let surface_adapters = pollster::block_on(instance.enumerate_adapters(Backends::all()));
        println!(
            "[wgpu Q4] SDL driver {driver}: {} adapter(s) (Q3 saw {})",
            surface_adapters.len(),
            adapters.len()
        );
        for a in &surface_adapters {
            let caps = surface.get_capabilities(a);
            let r = AdapterReport::of(a);
            if caps.present_modes.is_empty() {
                println!(
                    "[wgpu Q4]   {:?} | {} | surface unsupported",
                    r.backend, r.name
                );
            } else {
                println!(
                    "[wgpu Q4]   {:?} | {} | present modes: {:?}",
                    r.backend, r.name, caps.present_modes
                );
            }
        }
        drop(surface);
        drop(window);
    }
}
