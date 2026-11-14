//! Independent spec tests for Subphase 5.3 — GPU-free display model: images, DMD geometry and
//! shading reference, display specs, backglass description from the manifest, routing,
//! scene library / player timing, the score display model and change detection.
//!
//! Written from the brief (`docs/briefs/wave5/5.3.md`) and the published contract (DECISIONS
//! "5.3 — Displays public API") without reading the implementation.
#![allow(clippy::needless_range_loop)]

use pin_core::schema::backglass::{
    BackglassManifest, DmdColor, DmdConfig, LampMask, LcdConfig, ScoreDisplay, ScoreDisplayMode,
    Translite, TransliteLayer,
};
use pin_core::schema::table::{LampKind, SceneDef};
use pin_core::script::HostCommandQueue;
use pin_core::{LampId, SceneId};
use pin_render::displays::{
    BackglassDesc, ChangeTracker, DMD_128X32, DMD_192X64, DMD_BACKGROUND, DMD_DOT_RADIUS,
    DMD_UNLIT_LEVEL, DisplayContent, DmdFrame, DmdSpec, LAMP_REDRAW_EPSILON, LayerBlend, LcdItem,
    LcdSpec, PLACEHOLDER_SCENES, RgbaImage, Scene, SceneLibrary, ScenePlayer, ScoreDisplayConfig,
    ScoreDisplayModel, display_routes, dmd_dot_color, dmd_geometry, dmd_tint, fit_aspect,
    placeholder_scene,
};
use pin_render::topology::{
    Corner, DEFAULT_BACKGLASS_PIP, DEFAULT_SCORE_PIP, DisplayInfo, PipPlacement, Rect,
    SurfaceRole, TopologyMode, pip_rect, plan_topology,
};
use proptest::prelude::*;

// ---------------------------------------------------------------------------------------
// RgbaImage
// ---------------------------------------------------------------------------------------

#[test]
fn rgba_image_constructors() {
    let img = RgbaImage::new(3, 2);
    assert_eq!((img.width, img.height), (3, 2));
    assert_eq!(img.pixels.len(), 24);
    assert_eq!(img.pixel(2, 1), [0, 0, 0, 0], "new() is transparent black");

    let g = RgbaImage::from_gray(2, 2, vec![0, 85, 170, 255]).unwrap();
    assert_eq!(g.pixel(0, 0), [0, 0, 0, 255]);
    assert_eq!(g.pixel(1, 0), [85, 85, 85, 255]);
    assert_eq!(g.pixel(0, 1), [170, 170, 170, 255], "row 0 is the top");
    assert_eq!(g.pixel(1, 1), [255, 255, 255, 255]);

    let c = RgbaImage::from_rgb(2, 1, vec![1, 2, 3, 4, 5, 6]).unwrap();
    assert_eq!(c.pixel(0, 0), [1, 2, 3, 255]);
    assert_eq!(c.pixel(1, 0), [4, 5, 6, 255]);

    let a = RgbaImage::from_rgba(1, 2, vec![1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
    assert_eq!(a.pixel(0, 0), [1, 2, 3, 4]);
    assert_eq!(a.pixel(0, 1), [5, 6, 7, 8]);
    assert_eq!(a.pixels, vec![1, 2, 3, 4, 5, 6, 7, 8]);

    // Length mismatches and zero sizes.
    assert!(RgbaImage::from_gray(2, 2, vec![0; 3]).is_none());
    assert!(RgbaImage::from_gray(2, 2, vec![0; 5]).is_none());
    assert!(RgbaImage::from_rgb(2, 2, vec![0; 16]).is_none());
    assert!(RgbaImage::from_rgba(2, 2, vec![0; 12]).is_none());
    assert!(RgbaImage::from_rgba(0, 2, vec![]).is_none());
    assert!(RgbaImage::from_rgba(2, 0, vec![]).is_none());
    assert!(RgbaImage::from_gray(0, 0, vec![]).is_none());
    assert!(RgbaImage::from_rgb(0, 5, vec![]).is_none());
    // A size whose byte count overflows must be refused, not wrap.
    assert!(RgbaImage::from_rgba(u32::MAX, u32::MAX, vec![0; 4]).is_none());
    assert!(RgbaImage::from_rgba(1 << 31, 2, vec![]).is_none());
}

// ---------------------------------------------------------------------------------------
// DMD spec, geometry and shading reference
// ---------------------------------------------------------------------------------------

#[test]
fn dmd_spec_bounds_and_presets() {
    assert_eq!((DMD_128X32.width, DMD_128X32.height), (128, 32));
    assert_eq!((DMD_192X64.width, DMD_192X64.height), (192, 64));
    assert_eq!(DMD_128X32.color, DmdColor::Orange);
    assert_eq!(DMD_192X64.color, DmdColor::Orange);
    for c in [DmdColor::Orange, DmdColor::Red, DmdColor::Rgb] {
        let s = DmdSpec::new(128, 32, c).unwrap();
        assert_eq!((s.width, s.height, s.color), (128, 32, c));
        assert!(DmdSpec::new(192, 64, c).is_ok());
    }
    assert!(DmdSpec::new(16, 8, DmdColor::Red).is_ok());
    assert!(DmdSpec::new(512, 256, DmdColor::Red).is_ok());
    for (w, h) in [
        (15u32, 8u32),
        (16, 7),
        (513, 32),
        (128, 257),
        (0, 0),
        (0, 32),
        (128, 0),
        (u32::MAX, u32::MAX),
    ] {
        assert!(
            DmdSpec::new(w, h, DmdColor::Orange).is_err(),
            "{w}x{h} must be refused"
        );
    }
    assert!(LcdSpec::new(1920, 1080).is_ok());
    assert!(LcdSpec::new(16, 16).is_ok());
    assert!(LcdSpec::new(8192, 8192).is_ok());
    for (w, h) in [(15u32, 16u32), (16, 15), (8193, 16), (16, 8193), (0, 0)] {
        assert!(LcdSpec::new(w, h).is_err(), "LCD {w}x{h} must be refused");
    }
}

#[test]
fn dmd_shading_reference_constants() {
    assert_eq!(dmd_tint(DmdColor::Orange), [1.0, 0.42, 0.06]);
    assert_eq!(dmd_tint(DmdColor::Red), [1.0, 0.12, 0.08]);
    assert_eq!(dmd_tint(DmdColor::Rgb), [1.0, 1.0, 1.0]);
    assert!((DMD_UNLIT_LEVEL - 0.10).abs() < 1e-6);
    assert!((DMD_DOT_RADIUS - 0.40).abs() < 1e-6);
    assert_eq!(DMD_BACKGROUND, [0.0, 0.0, 0.0]);
    for c in [DmdColor::Orange, DmdColor::Red, DmdColor::Rgb] {
        let t = dmd_tint(c);
        let lit = dmd_dot_color(c, [255, 255, 255]);
        let unlit = dmd_dot_color(c, [0, 0, 0]);
        for k in 0..3 {
            assert!((lit[k] - t[k]).abs() < 1e-5, "{c:?}: full drive = tint");
            assert!(
                (unlit[k] - t[k] * DMD_UNLIT_LEVEL).abs() < 1e-5,
                "{c:?}: unlit = tint at 10 %"
            );
        }
    }
    // Orange is orange and red is red (visual checklist "the three colour modes look right").
    let o = dmd_tint(DmdColor::Orange);
    assert!(o[0] > o[1] && o[1] > o[2]);
    let r = dmd_tint(DmdColor::Red);
    assert!(r[1] < 0.2 && r[2] < 0.2);
}

proptest! {
    #[test]
    fn prop_dmd_dot_color_is_the_documented_formula(
        d in proptest::array::uniform3(any::<u8>()),
        color in prop_oneof![Just(DmdColor::Orange), Just(DmdColor::Red), Just(DmdColor::Rgb)],
    ) {
        let got = dmd_dot_color(color, d);
        let t = dmd_tint(color);
        for k in 0..3 {
            let want = t[k] * (DMD_UNLIT_LEVEL + (1.0 - DMD_UNLIT_LEVEL) * f32::from(d[k]) / 255.0);
            prop_assert!((got[k] - want).abs() < 1e-5, "{:?} {:?}: {:?} vs {}", color, d, got, want);
            prop_assert!((0.0..=1.0).contains(&got[k]));
        }
        // Monotone in the drive level.
        let mut brighter = d;
        for k in 0..3 { brighter[k] = brighter[k].saturating_add(40); }
        let b = dmd_dot_color(color, brighter);
        for k in 0..3 { prop_assert!(b[k] >= got[k]); }
    }

    #[test]
    fn prop_dmd_geometry_centres_the_grid_and_fits_the_viewport(
        w in 16u32..=512, h in 8u32..=256,
        vx in -2000i32..2000, vy in -2000i32..2000,
        vw in 16u32..4000, vh in 8u32..4000,
    ) {
        let spec = DmdSpec::new(w, h, DmdColor::Orange).unwrap();
        let vp = Rect::new(vx, vy, vw, vh);
        let g = dmd_geometry(&spec, vp);
        let want_pitch = (vw as f32 / w as f32).min(vh as f32 / h as f32);
        prop_assert!((g.pitch - want_pitch).abs() <= 1e-4 * want_pitch.max(1.0), "pitch {} vs {}", g.pitch, want_pitch);
        prop_assert!((g.radius - DMD_DOT_RADIUS * g.pitch).abs() <= 1e-4 * g.pitch.max(1.0));
        let tol = 2e-3 * (1.0 + vw.max(vh) as f32 / 100.0);
        // Even pitch along both axes.
        let c00 = g.dot_center(0, 0);
        let c10 = g.dot_center(1, 0);
        let c01 = g.dot_center(0, 1);
        let cl = g.dot_center(w - 1, h - 1);
        prop_assert!((c10[0] - c00[0] - g.pitch).abs() <= tol && (c10[1] - c00[1]).abs() <= tol);
        prop_assert!((c01[1] - c00[1] - g.pitch).abs() <= tol && (c01[0] - c00[0]).abs() <= tol);
        prop_assert!((cl[0] - c00[0] - (w - 1) as f32 * g.pitch).abs() <= tol * 4.0);
        prop_assert!((cl[1] - c00[1] - (h - 1) as f32 * g.pitch).abs() <= tol * 4.0);
        // Centred in the viewport.
        let cx = vx as f32 + vw as f32 * 0.5;
        let cy = vy as f32 + vh as f32 * 0.5;
        prop_assert!(((c00[0] + cl[0]) * 0.5 - cx).abs() <= tol * 4.0, "x centre");
        prop_assert!(((c00[1] + cl[1]) * 0.5 - cy).abs() <= tol * 4.0, "y centre");
        // The whole grid (cells of one pitch around each centre) lies inside the viewport.
        prop_assert!(c00[0] - g.pitch * 0.5 >= vx as f32 - tol * 4.0);
        prop_assert!(c00[1] - g.pitch * 0.5 >= vy as f32 - tol * 4.0);
        prop_assert!(cl[0] + g.pitch * 0.5 <= (vx as f32 + vw as f32) + tol * 4.0);
        prop_assert!(cl[1] + g.pitch * 0.5 <= (vy as f32 + vh as f32) + tol * 4.0);
    }
}

#[test]
fn dmd_geometry_known_values() {
    let spec = DmdSpec::new(128, 32, DmdColor::Orange).unwrap();
    // Exact fit at pitch 8.
    let g = dmd_geometry(&spec, Rect::new(0, 0, 1024, 256));
    assert!((g.pitch - 8.0).abs() < 1e-5);
    assert!((g.radius - 3.2).abs() < 1e-4);
    let c = g.dot_center(0, 0);
    assert!((c[0] - 4.0).abs() < 1e-4 && (c[1] - 4.0).abs() < 1e-4, "{c:?}");
    let c = g.dot_center(127, 31);
    assert!((c[0] - 1020.0).abs() < 1e-3 && (c[1] - 252.0).abs() < 1e-3, "{c:?}");
    // Offset viewport, wider than 4:1: letterboxed left/right. Pitch 2 → grid 256 × 64.
    let g = dmd_geometry(&spec, Rect::new(100, 50, 300, 64));
    assert!((g.pitch - 2.0).abs() < 1e-5);
    let c = g.dot_center(0, 0);
    assert!((c[0] - (100.0 + 22.0 + 1.0)).abs() < 1e-3, "{c:?}");
    assert!((c[1] - 51.0).abs() < 1e-3, "{c:?}");
    // 192 × 64 in the 4:1 score PiP: pitch limited by the height... 512/192 = 2.67, 128/64 = 2.
    let spec = DmdSpec::new(192, 64, DmdColor::Red).unwrap();
    let g = dmd_geometry(&spec, Rect::new(0, 0, 512, 128));
    assert!((g.pitch - 2.0).abs() < 1e-5);
    assert!((g.dot_center(0, 0)[0] - 65.0).abs() < 1e-3);
}

#[test]
fn dmd_frame_basics() {
    let mut f = DmdFrame::new(128, 32);
    assert_eq!((f.width, f.height), (128, 32));
    assert_eq!(f.dots.len(), 128 * 32);
    assert_eq!(f.lit_count(), 0);
    assert_eq!(f.get(127, 31), [0, 0, 0]);
    f.set(0, 0, [255, 255, 255]);
    f.set(127, 31, [17, 17, 17]);
    f.set(5, 1, [0, 0, 1]);
    assert_eq!(f.get(0, 0), [255, 255, 255]);
    assert_eq!(f.get(127, 31), [17, 17, 17]);
    assert_eq!(f.lit_count(), 3);
    // Row-major, row 0 at the top.
    assert_eq!(f.dots[0], [255, 255, 255]);
    assert_eq!(f.dots[128 + 5], [0, 0, 1]);
    assert_eq!(f.dots[128 * 32 - 1], [17, 17, 17]);
    f.set(5, 1, [0, 0, 0]);
    assert_eq!(f.lit_count(), 2);
}

// ---------------------------------------------------------------------------------------
// Score display config from the manifest
// ---------------------------------------------------------------------------------------

#[test]
fn score_display_config_from_manifest() {
    let sd = |mode, dmd, lcd| ScoreDisplay { mode, dmd, lcd };
    // Defaults.
    match ScoreDisplayConfig::from_manifest(&sd(ScoreDisplayMode::Dmd, None, None)).unwrap() {
        ScoreDisplayConfig::Dmd(s) => {
            assert_eq!((s.width, s.height, s.color), (128, 32, DmdColor::Orange))
        }
        ScoreDisplayConfig::Lcd(_) => panic!("mode dmd gave an LCD"),
    }
    match ScoreDisplayConfig::from_manifest(&sd(ScoreDisplayMode::Lcd, None, None)).unwrap() {
        ScoreDisplayConfig::Lcd(s) => {
            assert_eq!((s.width, s.height), (1920, 1080));
            assert_eq!(s.width * 9, s.height * 16, "the default LCD is 16:9");
        }
        ScoreDisplayConfig::Dmd(_) => panic!("mode lcd gave a DMD"),
    }
    // Explicit tables; the mode picks which one counts.
    let dmd = Some(DmdConfig {
        width: 192,
        height: 64,
        color: DmdColor::Rgb,
    });
    let lcd = Some(LcdConfig {
        width: 1280,
        height: 720,
    });
    match ScoreDisplayConfig::from_manifest(&sd(ScoreDisplayMode::Dmd, dmd, lcd)).unwrap() {
        ScoreDisplayConfig::Dmd(s) => {
            assert_eq!((s.width, s.height, s.color), (192, 64, DmdColor::Rgb))
        }
        ScoreDisplayConfig::Lcd(_) => panic!(),
    }
    match ScoreDisplayConfig::from_manifest(&sd(ScoreDisplayMode::Lcd, dmd, lcd)).unwrap() {
        ScoreDisplayConfig::Lcd(s) => assert_eq!((s.width, s.height), (1280, 720)),
        ScoreDisplayConfig::Dmd(_) => panic!(),
    }
    for color in [DmdColor::Orange, DmdColor::Red, DmdColor::Rgb] {
        let d = Some(DmdConfig {
            width: 128,
            height: 32,
            color,
        });
        match ScoreDisplayConfig::from_manifest(&sd(ScoreDisplayMode::Dmd, d, None)).unwrap() {
            ScoreDisplayConfig::Dmd(s) => assert_eq!(s.color, color),
            ScoreDisplayConfig::Lcd(_) => panic!(),
        }
    }
    // Out-of-range sizes are errors, not panics.
    let bad = Some(DmdConfig {
        width: 0,
        height: 0,
        color: DmdColor::Red,
    });
    assert!(ScoreDisplayConfig::from_manifest(&sd(ScoreDisplayMode::Dmd, bad, None)).is_err());
    let bad = Some(DmdConfig {
        width: 100_000,
        height: 32,
        color: DmdColor::Red,
    });
    assert!(ScoreDisplayConfig::from_manifest(&sd(ScoreDisplayMode::Dmd, bad, None)).is_err());
    let bad = Some(LcdConfig {
        width: 0,
        height: 1080,
    });
    assert!(ScoreDisplayConfig::from_manifest(&sd(ScoreDisplayMode::Lcd, None, bad)).is_err());
    // A bad table for the mode that is not active does not matter... or is rejected; either
    // way it must not panic.
    let _ = ScoreDisplayConfig::from_manifest(&sd(ScoreDisplayMode::Dmd, None, bad));
}

// ---------------------------------------------------------------------------------------
// Backglass description from backglass.toml
// ---------------------------------------------------------------------------------------

fn flat(c: [u8; 4]) -> RgbaImage {
    RgbaImage::from_rgba(2, 2, c.repeat(4)).unwrap()
}

fn layer(file: &str, depth: u8, mask: Option<&str>) -> TransliteLayer {
    TransliteLayer {
        file: file.to_string(),
        depth,
        lamp_mask: mask.map(str::to_string),
    }
}

fn manifest(layers: Vec<TransliteLayer>, lamp_masks: Vec<LampMask>) -> BackglassManifest {
    BackglassManifest {
        schema_version: 1,
        translite: Translite { layers },
        lamp_masks,
        score_display: ScoreDisplay {
            mode: ScoreDisplayMode::Dmd,
            dmd: None,
            lcd: None,
        },
        backbox: None,
    }
}

fn mask(id: &str, lamp: &str, file: &str) -> LampMask {
    LampMask {
        id: id.to_string(),
        lamp: lamp.to_string(),
        file: file.to_string(),
    }
}

fn resolve(name: &str) -> Option<(LampId, LampKind)> {
    match name {
        "l_logo" => Some((LampId(7), LampKind::Insert)),
        "l_gi" => Some((LampId(2), LampKind::Gi)),
        "l_flash" => Some((LampId(40), LampKind::Flasher)),
        _ => None,
    }
}

/// Every file is a flat image whose red channel encodes which file it is.
fn load(file: &str) -> Result<RgbaImage, String> {
    let tag = match file {
        "backglass/base.png" => 10,
        "backglass/logo.png" => 20,
        "backglass/logo_mask.png" => 30,
        "backglass/flash.png" => 40,
        "backglass/flash_mask.png" => 50,
        "backglass/gi.png" => 60,
        "backglass/top.png" => 70,
        other => return Err(format!("no such file {other}")),
    };
    Ok(flat([tag, 0, 0, 255]))
}

fn tag(img: &RgbaImage) -> u8 {
    img.pixel(0, 0)[0]
}

#[test]
fn backglass_desc_from_manifest_sorts_by_depth_and_maps_lamps() {
    // The A.2 example shape plus a flasher, a GI layer, and layers declared out of order.
    let m = manifest(
        vec![
            layer("backglass/flash.png", 3, Some("m_flash")),
            layer("backglass/logo.png", 1, Some("m_logo")),
            layer("backglass/base.png", 0, None),
            layer("backglass/gi.png", 2, Some("m_gi")),
            layer("backglass/top.png", 3, None), // same depth as flash: stable → after it
        ],
        vec![
            mask("m_logo", "l_logo", "backglass/logo_mask.png"),
            mask("m_flash", "l_flash", "backglass/flash_mask.png"),
            mask("m_gi", "l_gi", "backglass/logo_mask.png"),
        ],
    );
    let desc = BackglassDesc::from_manifest(&m, resolve, load).unwrap();
    let tags: Vec<u8> = desc.layers.iter().map(|l| tag(&l.image)).collect();
    assert_eq!(tags, [10, 20, 60, 40, 70], "back to front by depth, stable");

    assert!(desc.layers[0].lamp.is_none());
    let logo = desc.layers[1].lamp.as_ref().expect("logo layer is lamp-driven");
    assert_eq!(logo.lamp, LampId(7));
    assert_eq!(logo.blend, LayerBlend::Over);
    assert_eq!(tag(&logo.mask), 30, "the mask image of m_logo");
    let gi = desc.layers[2].lamp.as_ref().unwrap();
    assert_eq!((gi.lamp, gi.blend), (LampId(2), LayerBlend::Over));
    let flash = desc.layers[3].lamp.as_ref().unwrap();
    assert_eq!(flash.lamp, LampId(40));
    assert_eq!(
        flash.blend,
        LayerBlend::Additive,
        "a flasher lamp makes the layer additive"
    );
    assert_eq!(tag(&flash.mask), 50);
    assert!(desc.layers[4].lamp.is_none());
}

#[test]
fn backglass_desc_from_manifest_reports_errors() {
    // Unknown mask id.
    let m = manifest(
        vec![
            layer("backglass/base.png", 0, None),
            layer("backglass/logo.png", 1, Some("m_missing")),
        ],
        vec![mask("m_logo", "l_logo", "backglass/logo_mask.png")],
    );
    let e = BackglassDesc::from_manifest(&m, resolve, load);
    assert!(e.is_err(), "a layer naming an unknown lamp mask is an error");
    assert!(!e.unwrap_err().to_string().is_empty());
    // Unknown lamp name.
    let m = manifest(
        vec![
            layer("backglass/base.png", 0, None),
            layer("backglass/logo.png", 1, Some("m_logo")),
        ],
        vec![mask("m_logo", "l_nope", "backglass/logo_mask.png")],
    );
    assert!(BackglassDesc::from_manifest(&m, resolve, load).is_err());
    // Unreadable layer image / mask image.
    let m = manifest(vec![layer("backglass/nope.png", 0, None)], vec![]);
    assert!(BackglassDesc::from_manifest(&m, resolve, load).is_err());
    let m = manifest(
        vec![
            layer("backglass/base.png", 0, None),
            layer("backglass/logo.png", 1, Some("m_logo")),
        ],
        vec![mask("m_logo", "l_logo", "backglass/nope_mask.png")],
    );
    assert!(BackglassDesc::from_manifest(&m, resolve, load).is_err());
    // No layers at all: an error or an empty description, never a panic.
    let m = manifest(vec![], vec![]);
    if let Ok(d) = BackglassDesc::from_manifest(&m, resolve, load) {
        assert!(d.layers.is_empty());
    }
    // A plain, valid manifest is fine; unused masks are not loaded-or-failed on.
    let m = manifest(
        vec![layer("backglass/base.png", 0, None)],
        vec![mask("m_unused", "l_logo", "backglass/logo_mask.png")],
    );
    let d = BackglassDesc::from_manifest(&m, resolve, load).unwrap();
    assert_eq!(d.layers.len(), 1);
}

proptest! {
    #[test]
    fn prop_backglass_layers_are_sorted_stably(depths in proptest::collection::vec(0u8..6, 1..12)) {
        // Layer i loads an image tagged i.
        let layers: Vec<TransliteLayer> = depths
            .iter()
            .enumerate()
            .map(|(i, d)| layer(&format!("{i}"), *d, None))
            .collect();
        let m = manifest(layers, vec![]);
        let desc = BackglassDesc::from_manifest(
            &m,
            |_| None,
            |f| Ok(flat([f.parse::<u8>().unwrap(), 0, 0, 255])),
        )
        .unwrap();
        let got: Vec<usize> = desc.layers.iter().map(|l| tag(&l.image) as usize).collect();
        let mut want: Vec<usize> = (0..depths.len()).collect();
        want.sort_by_key(|&i| depths[i]); // stable
        prop_assert_eq!(got, want);
    }
}

// ---------------------------------------------------------------------------------------
// Routing: PiP regions vs dedicated windows; aspect fitting
// ---------------------------------------------------------------------------------------

fn display(index: usize, x: i32, w: u32, h: u32) -> DisplayInfo {
    DisplayInfo {
        index,
        bounds: Rect::new(x, 0, w, h),
        refresh_hz: 60.0,
        content_scale: 1.0,
    }
}

#[test]
fn routes_single_monitor_are_pip_regions() {
    let displays = [display(0, 0, 1920, 1080)];
    let mode = TopologyMode::SingleMonitor {
        display: 0,
        backglass: DEFAULT_BACKGLASS_PIP,
        score: DEFAULT_SCORE_PIP,
    };
    let plan = plan_topology(&displays, &mode).unwrap();
    let routes = display_routes(&plan);
    assert_eq!(routes.len(), 2, "backglass and score, never the playfield");
    assert_eq!(routes[0].role, SurfaceRole::Backglass);
    assert_eq!(routes[1].role, SurfaceRole::Score);
    for r in &routes {
        assert_eq!(r.window, 0);
        assert!(!r.dedicated, "{:?} is a PiP on the playfield window", r.role);
    }
    assert_eq!(
        routes[0].viewport,
        pip_rect(SurfaceRole::Backglass, &DEFAULT_BACKGLASS_PIP, 1920, 1080).unwrap()
    );
    assert_eq!(
        routes[1].viewport,
        pip_rect(SurfaceRole::Score, &DEFAULT_SCORE_PIP, 1920, 1080).unwrap()
    );
    assert_eq!(routes[0].viewport, Rect::new(1440, 0, 480, 360));
    assert_eq!(routes[1].viewport, Rect::new(1440, 960, 480, 120));
}

#[test]
fn routes_cabinet_are_dedicated_windows_or_pips() {
    let displays = [
        display(0, 0, 3840, 2160),
        display(1, 3840, 1920, 1080),
        display(2, 5760, 1280, 320),
    ];
    // Three screens.
    let plan = plan_topology(
        &displays,
        &TopologyMode::Cabinet {
            playfield: 0,
            backglass: Some(1),
            score: Some(2),
        },
    )
    .unwrap();
    let routes = display_routes(&plan);
    assert_eq!(routes.len(), 2);
    assert_eq!(
        (routes[0].role, routes[0].window, routes[0].dedicated),
        (SurfaceRole::Backglass, 1, true)
    );
    assert_eq!(
        (routes[1].role, routes[1].window, routes[1].dedicated),
        (SurfaceRole::Score, 2, true)
    );
    assert_eq!(routes[0].viewport, Rect::new(0, 0, 1920, 1080), "window-local");
    assert_eq!(routes[1].viewport, Rect::new(0, 0, 1280, 320));

    // Swapped displays: the window index follows the plan, not the display number.
    let plan = plan_topology(
        &displays,
        &TopologyMode::Cabinet {
            playfield: 1,
            backglass: Some(2),
            score: Some(0),
        },
    )
    .unwrap();
    for r in display_routes(&plan) {
        assert!(r.dedicated);
        assert_eq!(Some((r.window, r.viewport)), plan.find(r.role));
        assert_eq!(plan.windows[r.window].primary_role(), Some(r.role));
    }

    // Two screens: backglass dedicated, score PiP on the playfield — and the reverse.
    for (bg, sc) in [(Some(1), None), (None, Some(1))] {
        let plan = plan_topology(
            &displays,
            &TopologyMode::Cabinet {
                playfield: 0,
                backglass: bg,
                score: sc,
            },
        )
        .unwrap();
        let routes = display_routes(&plan);
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].role, SurfaceRole::Backglass);
        assert_eq!(routes[1].role, SurfaceRole::Score);
        assert_eq!(routes[0].dedicated, bg.is_some());
        assert_eq!(routes[1].dedicated, sc.is_some());
        assert_eq!(routes[0].window, usize::from(bg.is_some()));
        assert_eq!(routes[1].window, usize::from(sc.is_some()));
        for r in &routes {
            assert_eq!(Some((r.window, r.viewport)), plan.find(r.role));
        }
    }
    // One screen in cabinet mode: both PiPs.
    let plan = plan_topology(
        &displays,
        &TopologyMode::Cabinet {
            playfield: 0,
            backglass: None,
            score: None,
        },
    )
    .unwrap();
    let routes = display_routes(&plan);
    assert!(routes.iter().all(|r| !r.dedicated && r.window == 0));
    assert_eq!(routes.len(), 2);
}

proptest! {
    #[test]
    fn prop_routes_agree_with_the_plan(
        w in 320u32..4000, h in 240u32..3000,
        bf in 0.05f32..0.45, sf in 0.05f32..0.45,
        bc in 0usize..4, sc in 0usize..4,
        bg_dedicated in any::<bool>(), sc_dedicated in any::<bool>(), single in any::<bool>(),
    ) {
        let corners = [Corner::TopLeft, Corner::TopRight, Corner::BottomLeft, Corner::BottomRight];
        let displays = [display(0, 0, w, h), display(1, w as i32, 1024, 768), display(2, w as i32 + 1024, 800, 200)];
        let mode = if single {
            TopologyMode::SingleMonitor {
                display: 0,
                backglass: PipPlacement { corner: corners[bc], width_fraction: bf },
                score: PipPlacement { corner: corners[sc], width_fraction: sf },
            }
        } else {
            TopologyMode::Cabinet {
                playfield: 0,
                backglass: bg_dedicated.then_some(1),
                score: sc_dedicated.then_some(2),
            }
        };
        let Ok(plan) = plan_topology(&displays, &mode) else { return Ok(()); };
        let routes = display_routes(&plan);
        prop_assert_eq!(routes.len(), 2);
        prop_assert_eq!(routes[0].role, SurfaceRole::Backglass);
        prop_assert_eq!(routes[1].role, SurfaceRole::Score);
        for r in &routes {
            prop_assert_eq!(Some((r.window, r.viewport)), plan.find(r.role));
            let primary = plan.windows[r.window].primary_role();
            prop_assert_eq!(r.dedicated, primary == Some(r.role));
            if !r.dedicated {
                prop_assert_eq!(primary, Some(SurfaceRole::Playfield));
                prop_assert_eq!(r.window, 0);
            }
        }
    }

    #[test]
    fn prop_fit_aspect_is_the_largest_centred_rect(
        x in -3000i32..3000, y in -3000i32..3000,
        w in 1u32..5000, h in 1u32..5000,
        aw in 1u32..64, ah in 1u32..64,
    ) {
        let vp = Rect::new(x, y, w, h);
        let r = fit_aspect(vp, aw, ah);
        // Inside the viewport.
        prop_assert!(r.x >= vp.x && r.y >= vp.y);
        prop_assert!(r.right() <= vp.right() && r.bottom() <= vp.bottom());
        // Largest of that aspect, rounded down: one side spans the viewport, the other is
        // floor(side · aspect).
        let wide = u64::from(w) * u64::from(ah) >= u64::from(h) * u64::from(aw);
        if wide {
            prop_assert_eq!(r.h, h);
            prop_assert_eq!(u64::from(r.w), u64::from(h) * u64::from(aw) / u64::from(ah));
        } else {
            prop_assert_eq!(r.w, w);
            prop_assert_eq!(u64::from(r.h), u64::from(w) * u64::from(ah) / u64::from(aw));
        }
        // Centred (rounded down: the leftover splits evenly up to one pixel).
        let left = i64::from(r.x) - i64::from(vp.x);
        let right = vp.right() - r.right();
        let top = i64::from(r.y) - i64::from(vp.y);
        let bottom = vp.bottom() - r.bottom();
        prop_assert!((left - right).abs() <= 1, "h centring {} / {}", left, right);
        prop_assert!((top - bottom).abs() <= 1, "v centring {} / {}", top, bottom);
        prop_assert!(left <= right && top <= bottom, "rounded down");
    }
}

#[test]
fn fit_aspect_known_values() {
    assert_eq!(fit_aspect(Rect::new(0, 0, 1920, 1080), 16, 9), Rect::new(0, 0, 1920, 1080));
    assert_eq!(fit_aspect(Rect::new(0, 0, 1920, 1080), 4, 3), Rect::new(240, 0, 1440, 1080));
    assert_eq!(fit_aspect(Rect::new(10, 20, 400, 100), 16, 9), Rect::new(121, 20, 177, 100));
    assert_eq!(fit_aspect(Rect::new(0, 0, 480, 120), 16, 9), Rect::new(133, 0, 213, 120));
    assert_eq!(fit_aspect(Rect::new(0, 0, 100, 400), 4, 1), Rect::new(0, 187, 100, 25));
    // Degenerate aspect arguments must not panic (divide by zero).
    let _ = fit_aspect(Rect::new(0, 0, 100, 100), 0, 1);
    let _ = fit_aspect(Rect::new(0, 0, 100, 100), 1, 0);
    let _ = fit_aspect(Rect::new(0, 0, 0, 0), 4, 3);
    let _ = fit_aspect(Rect::new(i32::MAX, i32::MAX, u32::MAX, u32::MAX), u32::MAX, 1);
}

// ---------------------------------------------------------------------------------------
// Placeholder scenes and the scene library
// ---------------------------------------------------------------------------------------

const REQUIRED_SCENES: [&str; 4] = [
    "scene_attract",
    "scene_score",
    "scene_game_over",
    "scene_powerball_mania",
];

#[test]
fn placeholder_scenes_are_present_and_parse() {
    let ids: Vec<&str> = PLACEHOLDER_SCENES.iter().map(|(id, _)| *id).collect();
    for want in REQUIRED_SCENES {
        assert!(ids.contains(&want), "placeholder scene {want} missing ({ids:?})");
        assert!(placeholder_scene(want).is_some(), "{want} does not load");
    }
    // Ids are unique and every listed text parses to what `placeholder_scene` returns.
    for (i, (id, text)) in PLACEHOLDER_SCENES.iter().enumerate() {
        assert!(id.starts_with("scene_"), "{id}");
        assert!(!ids[..i].contains(id), "duplicate placeholder id {id}");
        let parsed = Scene::from_toml_str(text).unwrap_or_else(|e| panic!("{id}: {e}"));
        let via_bytes = Scene::from_bytes(text.as_bytes()).unwrap();
        assert_eq!(parsed, via_bytes, "{id}: from_bytes = from_toml_str");
        assert_eq!(Some(parsed.clone()), placeholder_scene(id), "{id}");
        assert!(!parsed.layers.is_empty(), "{id} has no layers");
    }
    assert!(placeholder_scene("scene_nope").is_none());
    assert!(placeholder_scene("").is_none());
}

#[test]
fn scene_library_placeholders_and_lookup() {
    let lib = SceneLibrary::placeholders();
    assert_eq!(lib.len(), PLACEHOLDER_SCENES.len());
    for (i, (id, _)) in PLACEHOLDER_SCENES.iter().enumerate() {
        let sid = lib.id_of(id).unwrap();
        assert_eq!(sid, SceneId(i as u16), "ids follow PLACEHOLDER_SCENES order");
        assert_eq!(lib.get(sid), placeholder_scene(id).as_ref());
    }
    assert!(lib.id_of("scene_unknown").is_none());
    assert!(lib.get(SceneId(PLACEHOLDER_SCENES.len() as u16)).is_none());
    assert!(lib.get(SceneId(u16::MAX)).is_none());

    let mut lib = SceneLibrary::new();
    assert_eq!(lib.len(), 0);
    assert!(lib.get(SceneId(0)).is_none());
    let s = placeholder_scene("scene_attract").unwrap();
    lib.insert(SceneId(9), "my_scene", s.clone());
    assert_eq!(lib.len(), 1);
    assert_eq!(lib.id_of("my_scene"), Some(SceneId(9)));
    assert_eq!(lib.get(SceneId(9)), Some(&s));
    assert!(lib.get(SceneId(0)).is_none());
    // Re-inserting an id replaces it.
    let t = placeholder_scene("scene_game_over").unwrap();
    lib.insert(SceneId(9), "my_scene", t.clone());
    assert_eq!(lib.len(), 1);
    assert_eq!(lib.get(SceneId(9)), Some(&t));
}

fn def(id: &str, file: &str) -> SceneDef {
    SceneDef {
        id: id.to_string(),
        file: file.to_string(),
    }
}

#[test]
fn scene_library_load_reads_files_and_falls_back_to_placeholders() {
    let attract_text = PLACEHOLDER_SCENES
        .iter()
        .find(|(id, _)| *id == "scene_attract")
        .unwrap()
        .1;
    let defs = [
        def("scene_custom", "scenes/custom.toml"),
        def("scene_powerball_mania", "scenes/powerball_mania.toml"), // file missing → placeholder
        def("scene_game_over", "scenes/game_over.toml"),             // file present → the file
    ];
    let mut asked = Vec::new();
    let lib = SceneLibrary::load(&defs, |path| {
        asked.push(path.to_string());
        match path {
            // A custom scene and an override of a placeholder id both use the attract text.
            "scenes/custom.toml" | "scenes/game_over.toml" => Ok(attract_text.as_bytes().to_vec()),
            other => Err(format!("{other}: not found")),
        }
    })
    .unwrap();
    assert_eq!(
        asked,
        ["scenes/custom.toml", "scenes/powerball_mania.toml", "scenes/game_over.toml"],
        "files are read in declaration order by their bundle path"
    );
    assert_eq!(lib.len(), 3);
    // SceneId(i) = declaration index i.
    assert_eq!(lib.id_of("scene_custom"), Some(SceneId(0)));
    assert_eq!(lib.id_of("scene_powerball_mania"), Some(SceneId(1)));
    assert_eq!(lib.id_of("scene_game_over"), Some(SceneId(2)));
    let attract = placeholder_scene("scene_attract").unwrap();
    assert_eq!(lib.get(SceneId(0)), Some(&attract));
    assert_eq!(
        lib.get(SceneId(1)),
        placeholder_scene("scene_powerball_mania").as_ref(),
        "a missing file falls back to the placeholder of the same id"
    );
    assert_eq!(
        lib.get(SceneId(2)),
        Some(&attract),
        "a file that exists wins over the placeholder"
    );

    // A missing file with no placeholder of that id is an error.
    let defs = [def("scene_not_a_placeholder", "scenes/x.toml")];
    let e = SceneLibrary::load(&defs, |p| Err(format!("{p}: not found")));
    assert!(e.is_err());
    assert!(!e.err().unwrap().to_string().is_empty());
    // A file that reads but does not parse is an error (no silent fallback to nothing).
    let defs = [def("scene_custom", "scenes/x.toml")];
    assert!(SceneLibrary::load(&defs, |_| Ok(b"this is = = not toml".to_vec())).is_err());
    assert!(SceneLibrary::load(&defs, |_| Ok(vec![0xff, 0xfe, 0x00])).is_err());
    // No scenes declared.
    let lib = SceneLibrary::load(&[], |_| Err("unused".to_string())).unwrap();
    assert_eq!(lib.len(), 0);
}

// ---------------------------------------------------------------------------------------
// Scene player timing (simulated ms, D8)
// ---------------------------------------------------------------------------------------

fn timed_library() -> SceneLibrary {
    let base = placeholder_scene("scene_powerball_mania").unwrap();
    let mut lib = SceneLibrary::new();
    let mut once = base.clone();
    once.duration_ms = 1000;
    once.repeat = false;
    lib.insert(SceneId(0), "once", once);
    let mut looped = base.clone();
    looped.duration_ms = 1000;
    looped.repeat = true;
    lib.insert(SceneId(1), "looped", looped);
    let mut forever = base;
    forever.duration_ms = 0;
    forever.repeat = false;
    lib.insert(SceneId(2), "forever", forever);
    lib
}

#[test]
fn scene_player_timing() {
    let lib = timed_library();
    let mut p = ScenePlayer::default();
    assert!(p.active(&lib, 0).is_none(), "nothing shown yet");
    assert!(p.active(&lib, 123_456).is_none());

    // One-shot: ends after its duration.
    p.show(SceneId(0), 100);
    let (scene, t) = p.active(&lib, 100).unwrap();
    assert_eq!(Some(scene), lib.get(SceneId(0)));
    assert_eq!(t, 0);
    assert_eq!(p.active(&lib, 600).unwrap().1, 500);
    assert_eq!(p.active(&lib, 1099).unwrap().1, 999);
    assert!(p.active(&lib, 1101).is_none(), "a one-shot scene ends after its duration");
    assert!(p.active(&lib, 1_000_000).is_none());
    // Asking is not mutating: the same instant gives the same answer again.
    assert_eq!(p.active(&lib, 600).unwrap().1, 500);

    // Repeat: elapsed wraps modulo the duration.
    p.show(SceneId(1), 2000);
    assert_eq!(p.active(&lib, 2000).unwrap().1, 0);
    assert_eq!(p.active(&lib, 2999).unwrap().1, 999);
    assert_eq!(p.active(&lib, 3000).unwrap().1, 0);
    assert_eq!(p.active(&lib, 4500).unwrap().1, 500);
    assert_eq!(p.active(&lib, 2000 + 1000 * 1_000_000 + 7).unwrap().1, 7);
    assert_eq!(Some(p.active(&lib, 4500).unwrap().0), lib.get(SceneId(1)));

    // Duration 0: shown until replaced.
    p.show(SceneId(2), 5000);
    assert_eq!(p.active(&lib, 5000).unwrap().1, 0);
    assert_eq!(p.active(&lib, 5000 + 86_400_000).unwrap().1, 86_400_000);

    // Showing a scene again restarts it.
    p.show(SceneId(0), 9000);
    assert_eq!(p.active(&lib, 9400).unwrap().1, 400);
    p.show(SceneId(0), 9400);
    assert_eq!(p.active(&lib, 9400).unwrap().1, 0);

    // Unknown ids show nothing; clear() shows nothing.
    p.show(SceneId(77), 9500);
    assert!(p.active(&lib, 9500).is_none());
    p.show(SceneId(2), 9600);
    assert!(p.active(&lib, 9700).is_some());
    p.clear();
    assert!(p.active(&lib, 9700).is_none());

    // A clock before the start time (replay rewind) must not panic.
    p.show(SceneId(1), 10_000);
    let _ = p.active(&lib, 5);
    p.show(SceneId(0), u64::MAX);
    let _ = p.active(&lib, 0);
    let _ = p.active(&lib, u64::MAX);
}

#[test]
fn scene_player_applies_host_commands_last_wins() {
    let lib = timed_library();
    let mut p = ScenePlayer::default();
    let mut q = HostCommandQueue::default();
    p.apply_commands(&q, 10);
    assert!(p.active(&lib, 10).is_none(), "an empty queue shows nothing");

    q.show_scene(SceneId(0)).unwrap();
    q.show_scene(SceneId(2)).unwrap();
    q.show_scene(SceneId(1)).unwrap();
    p.apply_commands(&q, 50);
    let (scene, t) = p.active(&lib, 70).unwrap();
    assert_eq!(Some(scene), lib.get(SceneId(1)), "the last display_scenes entry wins");
    assert_eq!(t, 20);

    // An empty queue leaves the running scene alone.
    q.clear();
    p.apply_commands(&q, 400);
    assert_eq!(p.active(&lib, 450).unwrap().1, 400);
    // Score-only commands do not restart it either.
    q.set_score(0, 5).unwrap();
    p.apply_commands(&q, 500);
    assert_eq!(p.active(&lib, 550).unwrap().1, 500);
}

proptest! {
    #[test]
    fn prop_scene_player_elapsed_model(
        duration in 0u64..5000, repeat in any::<bool>(),
        start in 0u64..1_000_000, dt in 0u64..20_000,
    ) {
        let mut scene = placeholder_scene("scene_attract").unwrap();
        scene.duration_ms = duration;
        scene.repeat = repeat;
        let mut lib = SceneLibrary::new();
        lib.insert(SceneId(3), "s", scene);
        let mut p = ScenePlayer::default();
        p.show(SceneId(3), start);
        let got = p.active(&lib, start + dt).map(|(_, t)| t);
        if duration == 0 {
            prop_assert_eq!(got, Some(dt));
        } else if repeat {
            prop_assert_eq!(got, Some(dt % duration));
        } else if dt < duration {
            prop_assert_eq!(got, Some(dt));
        } else if dt > duration {
            prop_assert_eq!(got, None);
        } else {
            // dt == duration: the boundary instant may be either the last frame or over.
            prop_assert!(got.is_none() || got == Some(dt));
        }
    }
}

// ---------------------------------------------------------------------------------------
// Score display model
// ---------------------------------------------------------------------------------------

fn dmd_model(w: u32, h: u32, color: DmdColor) -> ScoreDisplayModel {
    ScoreDisplayModel::new(
        ScoreDisplayConfig::Dmd(DmdSpec::new(w, h, color).unwrap()),
        SceneLibrary::placeholders(),
    )
}

fn lcd_model() -> ScoreDisplayModel {
    ScoreDisplayModel::new(
        ScoreDisplayConfig::Lcd(LcdSpec::new(1920, 1080).unwrap()),
        SceneLibrary::placeholders(),
    )
}

fn dmd(content: DisplayContent) -> DmdFrame {
    match content {
        DisplayContent::Dmd(f) => f,
        DisplayContent::Lcd(_) => panic!("expected a DMD frame"),
    }
}

fn lcd_texts(content: &DisplayContent) -> Vec<String> {
    match content {
        DisplayContent::Lcd(list) => list
            .items
            .iter()
            .filter_map(|i| match i {
                LcdItem::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect(),
        DisplayContent::Dmd(_) => panic!("expected an LCD draw list"),
    }
}

#[test]
fn model_composes_the_mode_it_was_configured_with() {
    for (w, h) in [(128u32, 32u32), (192, 64)] {
        let m = dmd_model(w, h, DmdColor::Orange);
        let f = dmd(m.compose(0));
        assert_eq!((f.width, f.height), (w, h));
        assert_eq!(f.dots.len(), (w * h) as usize);
        assert!(f.lit_count() > 0, "the built-in score view shows the score");
    }
    let m = lcd_model();
    match m.compose(0) {
        DisplayContent::Lcd(list) => {
            assert_eq!((list.width, list.height), (1920, 1080));
            assert!(!list.items.is_empty());
        }
        DisplayContent::Dmd(_) => panic!("expected an LCD draw list"),
    }
}

#[test]
fn model_mono_frames_are_quantised_to_sixteen_levels() {
    for color in [DmdColor::Orange, DmdColor::Red] {
        for (id, _) in PLACEHOLDER_SCENES {
            let mut m = dmd_model(128, 32, color);
            let sid = SceneLibrary::placeholders().id_of(id).unwrap();
            m.scenes_mut().show(sid, 0);
            for t in [0u64, 33, 250, 1000, 1700] {
                let f = dmd(m.compose(t));
                for (i, d) in f.dots.iter().enumerate() {
                    assert!(
                        d[0] == d[1] && d[1] == d[2],
                        "{id} {color:?} t={t}: mono dot {i} has unequal channels {d:?}"
                    );
                    assert_eq!(d[0] % 17, 0, "{id} {color:?} t={t}: dot {i} level {}", d[0]);
                }
            }
        }
    }
}

#[test]
fn model_lcd_score_view_shows_formatted_scores_and_status() {
    let mut m = lcd_model();
    {
        let s = m.scores_mut();
        s.scores = [1_234_560, 98_000, 0, 0];
        s.players = 2;
        s.current_player = 1;
        s.ball = 2;
        s.credits = 3;
    }
    let content = m.compose(0);
    let texts = lcd_texts(&content);
    let all = texts.join(" | ");
    assert!(all.contains("1,234,560"), "player 1 score missing: {all}");
    assert!(all.contains("98,000"), "player 2 score missing: {all}");
    assert!(all.contains("PLAYER 2  BALL 2"), "status line missing: {all}");
    // Everything lies on the 16:9 canvas.
    let DisplayContent::Lcd(list) = &content else {
        unreachable!()
    };
    for item in &list.items {
        match item {
            LcdItem::Text { origin, cell, text, .. } => {
                assert!(*cell > 0.0 && cell.is_finite());
                assert!(origin[0] >= 0.0 && origin[1] >= 0.0, "{text:?} at {origin:?}");
                let w = text.chars().count() as f32 * 6.0 * cell / 7.0;
                assert!(
                    origin[0] + w <= 1920.0 + cell && origin[1] + cell <= 1080.0 + 1.0,
                    "{text:?} at {origin:?} cell {cell} leaves the canvas"
                );
            }
            LcdItem::Rect { rect, .. } | LcdItem::Image { rect, .. } => {
                assert!(rect.iter().all(|v| v.is_finite()));
                assert!(rect[0] <= rect[2] && rect[1] <= rect[3]);
            }
        }
    }
    // Legible at 16:9 (visual checklist): the current player's score is at least 5 % of the
    // canvas height.
    let score_cell = list
        .items
        .iter()
        .filter_map(|i| match i {
            LcdItem::Text { text, cell, .. } if text.contains("98,000") => Some(*cell),
            _ => None,
        })
        .fold(0.0f32, f32::max);
    assert!(score_cell >= 54.0, "score glyph cell only {score_cell} px of 1080");

    // No game: credits.
    m.scores_mut().ball = 0;
    let all = lcd_texts(&m.compose(0)).join(" | ");
    assert!(all.contains("CREDITS 3"), "{all}");
}

#[test]
fn model_applies_host_commands_and_scene_timeout_returns_to_score_view() {
    let mut m = dmd_model(128, 32, DmdColor::Orange);
    let baseline = dmd(m.compose(0));
    assert_eq!(dmd(m.compose(999_999)), baseline, "the idle score view is static");

    let mut q = HostCommandQueue::default();
    q.set_score(0, 77_000).unwrap();
    m.apply_commands(&q, 10);
    assert_eq!(m.scores().scores[0], 77_000);
    let scored = dmd(m.compose(10));
    assert_ne!(scored, baseline, "a new score changes the frame");

    // A one-shot scene replaces the score view, then the display returns to it.
    let lib = SceneLibrary::placeholders();
    let (sid, scene) = PLACEHOLDER_SCENES
        .iter()
        .filter_map(|(id, _)| {
            let sid = lib.id_of(id)?;
            let s = lib.get(sid)?;
            (s.duration_ms > 0 && !s.repeat && *id != "scene_score").then_some((sid, s.clone()))
        })
        .next()
        .expect("at least one placeholder is a one-shot scene with a duration");
    q.clear();
    q.show_scene(sid).unwrap();
    m.apply_commands(&q, 1000);
    q.clear();
    let during = dmd(m.compose(1000 + scene.duration_ms / 2));
    assert!(
        (0..=scene.duration_ms)
            .step_by(50)
            .any(|t| dmd(m.compose(1000 + t)) != scored),
        "the scene never showed anything different from the score view"
    );
    let _ = during;
    let after = dmd(m.compose(1000 + scene.duration_ms + 1));
    assert_eq!(after, scored, "after the scene ends the score view is back");
    m.apply_commands(&q, 1000 + scene.duration_ms + 2);
    assert_eq!(dmd(m.compose(1000 + scene.duration_ms + 2)), scored);

    // scene_score shown explicitly is the built-in score view.
    let mut m2 = dmd_model(128, 32, DmdColor::Orange);
    m2.scores_mut().scores[0] = 77_000;
    let plain = dmd(m2.compose(0));
    m2.scenes_mut().show(lib.id_of("scene_score").unwrap(), 0);
    assert_eq!(dmd(m2.compose(0)), plain);
    // An unknown scene id shows the score view (nothing else to show).
    m2.scenes_mut().show(SceneId(999), 0);
    assert_eq!(dmd(m2.compose(5)), plain);
}

#[test]
fn every_placeholder_scene_composes_on_every_display_at_any_time() {
    let lib = SceneLibrary::placeholders();
    for (id, _) in PLACEHOLDER_SCENES {
        let sid = lib.id_of(id).unwrap();
        let times = [0u64, 1, 16, 499, 500, 501, 999, 1000, 2500, 60_000, u64::MAX / 2];
        for (w, h) in [(128u32, 32u32), (192, 64), (16, 8), (512, 256)] {
            for color in [DmdColor::Orange, DmdColor::Red, DmdColor::Rgb] {
                let mut m = dmd_model(w, h, color);
                m.scores_mut().scores = [i64::MAX, i64::MIN, -1, 0];
                m.scores_mut().players = 4;
                m.scores_mut().ball = 255;
                m.scores_mut().credits = u32::MAX;
                m.scenes_mut().show(sid, 0);
                let mut any = 0;
                for t in times {
                    let f = dmd(m.compose(t));
                    assert_eq!((f.width, f.height), (w, h));
                    any += f.lit_count();
                }
                if w >= 128 {
                    assert!(any > 0, "{id} at {w}x{h} never lights a dot");
                }
            }
        }
        let mut m = lcd_model();
        m.scores_mut().scores = [i64::MAX, i64::MIN, -1, 0];
        m.scores_mut().players = 4;
        m.scenes_mut().show(sid, 0);
        let mut any = 0;
        for t in times {
            match m.compose(t) {
                DisplayContent::Lcd(l) => any += l.items.len(),
                DisplayContent::Dmd(_) => panic!(),
            }
        }
        assert!(any > 0, "{id} never draws anything on the LCD");
    }
}

#[test]
fn powerball_mania_scene_announces_itself() {
    // D32 hand-off: `display_scenes` commands for the Powerball mode have content.
    let lib = SceneLibrary::placeholders();
    let sid = lib.id_of("scene_powerball_mania").unwrap();
    let mut m = lcd_model();
    m.scenes_mut().show(sid, 0);
    let mut texts = Vec::new();
    for t in (0..3000).step_by(100) {
        texts.extend(lcd_texts(&m.compose(t)));
    }
    let all = texts.join(" ").to_uppercase();
    assert!(
        all.contains("POWERBALL"),
        "scene_powerball_mania shows no POWERBALL text on the LCD: {all:?}"
    );
    // On the DMD it differs from the plain score view.
    let mut m = dmd_model(128, 32, DmdColor::Orange);
    let plain = dmd(m.compose(0));
    m.scenes_mut().show(sid, 0);
    assert!((0..3000).step_by(100).any(|t| dmd(m.compose(t)) != plain));
}

// ---------------------------------------------------------------------------------------
// Change detection (GPU-free tracker)
// ---------------------------------------------------------------------------------------

#[test]
fn change_tracker_reports_only_what_changed() {
    assert!((LAMP_REDRAW_EPSILON - 1.0 / 1024.0).abs() < 1e-9);
    let mut m = dmd_model(128, 32, DmdColor::Orange);
    let mut tracker = ChangeTracker::new(vec![LampId(3), LampId(200)]);
    let mut lamps = vec![0.0f32; 256];
    lamps[3] = 0.5;

    let c0 = m.compose(0);
    let d = tracker.observe(&lamps, &c0);
    assert!(d.backglass && d.score, "the first observation draws everything");
    // Nothing changed: nothing to redraw, however often we ask.
    for t in 1..200u64 {
        let d = tracker.observe(&lamps, &m.compose(t));
        assert!(!d.backglass && !d.score, "idle frame {t} asked for a redraw");
    }
    // Unregistered lamps: nothing.
    lamps[0] = 1.0;
    lamps[4] = 0.7;
    lamps[255] = 1.0;
    let d = tracker.observe(&lamps, &c0);
    assert!(!d.backglass && !d.score);
    // Registered lamp below epsilon: nothing. Above: backglass only.
    lamps[3] += 0.4 / 1024.0;
    assert!(!tracker.observe(&lamps, &c0).backglass);
    lamps[200] = 2.0 / 1024.0;
    let d = tracker.observe(&lamps, &c0);
    assert!(d.backglass && !d.score, "a lamp change redraws only the backglass");
    let d = tracker.observe(&lamps, &c0);
    assert!(!d.backglass && !d.score);
    // Score change: score only.
    m.scores_mut().scores[0] = 10;
    let c1 = m.compose(300);
    let d = tracker.observe(&lamps, &c1);
    assert!(!d.backglass && d.score, "a score change redraws only the score display");
    let d = tracker.observe(&lamps, &c1);
    assert!(!d.backglass && !d.score);
    // Both.
    lamps[3] = 1.0;
    let d = tracker.observe(&lamps, &c0);
    assert!(d.backglass && d.score);
    // A lamp slice too short for a registered lamp reads as 0 (no panic).
    let d = tracker.observe(&lamps[..4], &c0);
    assert!(d.backglass, "lamp 200 went from 2/1024 to missing (= 0)");
    let d = tracker.observe(&[], &c0);
    assert!(d.backglass, "lamp 3 went from 1 to missing (= 0)");
    let d = tracker.observe(&[], &c0);
    assert!(!d.backglass && !d.score);
    // LCD content is tracked the same way.
    let mut l = lcd_model();
    let mut tracker = ChangeTracker::new(vec![]);
    let a = l.compose(0);
    assert!(tracker.observe(&[], &a).score);
    assert!(!tracker.observe(&[], &l.compose(1)).score);
    l.scores_mut().scores[0] = 1;
    assert!(tracker.observe(&[], &l.compose(2)).score);
    // With no registered lamps the backglass never needs a second draw.
    assert!(!tracker.observe(&[1.0; 256], &l.compose(2)).backglass);
}

proptest! {
    #[test]
    fn prop_change_tracker_lamp_epsilon(
        a in 0.0f32..=1.0, b in 0.0f32..=1.0, other in 0.0f32..=1.0,
    ) {
        let content = DisplayContent::Dmd(DmdFrame::new(128, 32));
        let mut tracker = ChangeTracker::new(vec![LampId(1)]);
        let mut lamps = [0.0f32, a, 0.0];
        tracker.observe(&lamps, &content);
        lamps[1] = b;
        lamps[2] = other;
        lamps[0] = other;
        let d = tracker.observe(&lamps, &content);
        prop_assert!(!d.score);
        let delta = (a - b).abs();
        if delta >= LAMP_REDRAW_EPSILON * 1.01 {
            prop_assert!(d.backglass, "moved by {}", delta);
        } else if delta <= LAMP_REDRAW_EPSILON * 0.99 {
            prop_assert!(!d.backglass, "moved by only {}", delta);
        }
    }
}
