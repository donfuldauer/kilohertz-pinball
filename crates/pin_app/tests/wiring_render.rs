//! Subphase 6.0 wiring (PR #21 review #12): the loaded table is drawn with
//! the 5.1 PBR renderer (through the 5.2 post chain) and the sim's display
//! scenes drive the 5.3 score display and backglass, rendered offscreen on
//! the test adapter (lavapipe in CI). Analytic pixel assertions only (D30):
//! region means, luminance and differences between frames.
//!
//! Skips (with a note) without an adapter unless `PIN_REQUIRE_GPU=1`.

mod support;

use pin_app::orchestrator::{
    DisplayCommand, TABLE_LINE_COLOR, TableRenderOptions, TableRenderer, TableVisuals,
    load_checked, validate_table,
};
use pin_core::snapshot::{BallState, FrameStateSnapshot};
use pin_render::displays::{DisplayRoute, RgbaImage};
use pin_render::gpu::{GpuContext, wgpu};
use pin_render::topology::{Rect, SurfaceRole};
use pin_sim::SimConfig;
use std::sync::OnceLock;
use support::{greybox_bundle, on_big_stack};

const W: u32 = 200;
const H: u32 = 400;
/// Score display (128 × 32 DMD) PiP at the bottom, backglass PiP top-left.
const DMD: Rect = Rect {
    x: 10,
    y: 340,
    w: 180,
    h: 45,
};
const BACKGLASS: Rect = Rect {
    x: 10,
    y: 10,
    w: 60,
    h: 45,
};

fn gpu() -> Option<&'static GpuContext> {
    // One context per test binary (lavapipe and many devices, DECISIONS
    // "Wave 5 gate fix").
    static GPU: OnceLock<Option<GpuContext>> = OnceLock::new();
    GPU.get_or_init(|| pin_render::gpu::context_for_tests("wiring_render"))
        .as_ref()
}

fn routes() -> Vec<DisplayRoute> {
    vec![
        DisplayRoute {
            role: SurfaceRole::Backglass,
            window: 0,
            viewport: BACKGLASS,
            dedicated: false,
        },
        DisplayRoute {
            role: SurfaceRole::Score,
            window: 0,
            viewport: DMD,
            dedicated: false,
        },
    ]
}

/// Mean RGBA (0..255) over a rectangle.
fn mean(img: &RgbaImage, r: Rect) -> [f32; 4] {
    let mut acc = [0.0f64; 4];
    let mut n = 0.0f64;
    for y in r.y as u32..r.y as u32 + r.h {
        for x in r.x as u32..r.x as u32 + r.w {
            let p = img.pixel(x, y);
            for k in 0..4 {
                acc[k] += f64::from(p[k]);
            }
            n += 1.0;
        }
    }
    acc.map(|v| (v / n) as f32)
}

fn luma(c: [f32; 4]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// Standard deviation of the luminance over a rectangle (a flat fill is 0).
fn luma_stddev(img: &RgbaImage, r: Rect) -> f32 {
    let mut v = Vec::new();
    for y in r.y as u32..r.y as u32 + r.h {
        for x in r.x as u32..r.x as u32 + r.w {
            let p = img.pixel(x, y);
            v.push(luma([
                f32::from(p[0]),
                f32::from(p[1]),
                f32::from(p[2]),
                0.0,
            ]));
        }
    }
    let m = v.iter().sum::<f32>() / v.len() as f32;
    (v.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / v.len() as f32).sqrt()
}

/// Mean absolute per-channel difference over a rectangle.
fn diff(a: &RgbaImage, b: &RgbaImage, r: Rect) -> f32 {
    let mut acc = 0.0f64;
    let mut n = 0.0f64;
    for y in r.y as u32..r.y as u32 + r.h {
        for x in r.x as u32..r.x as u32 + r.w {
            let (p, q) = (a.pixel(x, y), b.pixel(x, y));
            for k in 0..3 {
                acc += f64::from(p[k].abs_diff(q[k]));
            }
            n += 3.0;
        }
    }
    (acc / n) as f32
}

/// Loads the grey-box table (on a big stack) and returns its visuals, the
/// load-time snapshot and the scene ids the tests use.
fn table() -> (TableVisuals, FrameStateSnapshot, [pin_core::SceneId; 2]) {
    let dir = greybox_bundle().to_path_buf();
    on_big_stack(move || {
        let checked = validate_table(&dir).unwrap();
        let sim = load_checked(&checked, SimConfig::default()).unwrap();
        let ids = &checked.table.ids;
        let scenes = [
            ids.scene("scene_attract").unwrap(),
            ids.scene("scene_game_over").unwrap(),
        ];
        (TableVisuals::of(&checked), *sim.snapshot(), scenes)
    })
}

fn renderer(ctx: &GpuContext, v: &TableVisuals, overlay: bool) -> TableRenderer {
    TableRenderer::new(
        ctx,
        wgpu::TextureFormat::Rgba8Unorm,
        (W, H),
        v,
        TableRenderOptions {
            debug_overlay: overlay,
            ..TableRenderOptions::default()
        },
    )
    .unwrap()
}

#[test]
fn playfield_is_pbr_shaded_and_the_dmd_follows_the_scenes() {
    let Some(ctx) = gpu() else { return };
    let (v, snap, [attract, over]) = table();
    assert!(v.warnings.is_empty(), "{:?}", v.warnings);
    assert!(!v.meshes.is_empty());
    let mut r = renderer(ctx, &v, false);
    assert_eq!(r.has_displays(), (true, true));
    r.apply_display_command(DisplayCommand::Scene(attract), 0);
    let mut s = snap;
    s.tick = 500;
    let img = r.render_to_image(ctx, &routes(), &s).unwrap();

    // Playfield: lit, not black, not the flat debug colour, and shaded
    // (light falls off away from the key lights: rows differ).
    let pf = Rect::new(40, 120, 120, 160);
    let m = mean(&img, pf);
    assert!(luma(m) > 20.0, "playfield too dark: {m:?}");
    let debug = TABLE_LINE_COLOR.map(|c| c * 255.0);
    let dist = (0..3).map(|k| (m[k] - debug[k]).abs()).sum::<f32>();
    assert!(dist > 20.0, "playfield looks like the debug colour: {m:?}");
    let sd = luma_stddev(&img, pf);
    assert!(sd > 1.0, "flat playfield (luminance std dev {sd})");

    // DMD: orange dots lit by the attract scene (red > blue), brighter than
    // the unlit grid.
    let dmd = mean(&img, DMD);
    assert!(dmd[0] > dmd[2] + 5.0, "DMD not orange: {dmd:?}");
    assert!(luma(dmd) > 5.0, "DMD dark: {dmd:?}");
    // Backglass: the translite art is drawn (not the playfield underneath).
    let bg_pf = renderer(ctx, &v, false)
        .render_to_image(ctx, &[], &s)
        .unwrap();
    assert!(
        diff(&img, &bg_pf, BACKGLASS) > 5.0,
        "backglass PiP not drawn"
    );
    assert!(diff(&img, &bg_pf, DMD) > 5.0, "DMD PiP not drawn");

    // A different scene changes the DMD and nothing else of the playfield.
    r.apply_display_command(DisplayCommand::Scene(over), 500);
    let img2 = r.render_to_image(ctx, &routes(), &s).unwrap();
    assert!(diff(&img, &img2, DMD) > 1.0, "DMD did not follow the scene");
    assert!(
        diff(&img, &img2, pf) < 0.5,
        "playfield changed with the scene"
    );
    assert_eq!(r.display_commands(), 2);
}

#[test]
fn a_ball_is_drawn_where_the_snapshot_puts_it() {
    let Some(ctx) = gpu() else { return };
    let (v, snap, _) = table();
    let mut r = renderer(ctx, &v, false);
    let without = r.render_to_image(ctx, &[], &snap).unwrap();
    let ((x0, y0), (x1, y1)) = (
        (v.bounds.0[0], v.bounds.0[1]),
        (v.bounds.1[0], v.bounds.1[1]),
    );
    let (cx, cy) = (0.5 * (x0 + x1), 0.5 * (y0 + y1));
    let mut s = snap;
    s.push_ball(BallState {
        position_m: [cx, cy, 0.0135],
        ..BallState::default()
    });
    let with = r.render_to_image(ctx, &[], &s).unwrap();
    // The camera looks straight down at the table centre: the ball is in
    // the middle of the image.
    let centre = Rect::new(W as i32 / 2 - 3, H as i32 / 2 - 3, 6, 6);
    assert!(
        diff(&with, &without, centre) > 5.0,
        "ball not visible at the centre"
    );
    let corner = Rect::new(150, 20, 30, 30);
    assert!(
        diff(&with, &without, corner) < 1.0,
        "ball changed a far corner"
    );
}

#[test]
fn the_debug_overlay_is_optional_and_off_by_default() {
    let Some(ctx) = gpu() else { return };
    let (v, snap, _) = table();
    assert!(!TableRenderOptions::default().debug_overlay);
    let plain = renderer(ctx, &v, false)
        .render_to_image(ctx, &[], &snap)
        .unwrap();
    let lines = renderer(ctx, &v, true)
        .render_to_image(ctx, &[], &snap)
        .unwrap();
    let all = Rect::new(0, 0, W, H);
    assert!(diff(&plain, &lines, all) > 0.05, "overlay drew nothing");
    // The overlay draws over the PBR image, it does not replace it.
    assert!(luma(mean(&lines, Rect::new(40, 120, 120, 160))) > 20.0);
}
