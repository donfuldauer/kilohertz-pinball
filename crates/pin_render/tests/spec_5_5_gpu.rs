//! Subphase 5.5 spec tests (Independent Tester), GPU: the backbox scene
//! composited between translite layers, under the image test policy D30
//! (analytic pixel assertions first; a regression snapshot is compared, or
//! stored, only after the analytic assertions of the same image pass).
//!
//! * Depth ordering: a flat red backbox quad between a blue base layer
//!   (depth 0) and a green opaque-masked logo layer (depth 1): blue where
//!   nothing covers, red where the backbox covers the base, green where the
//!   logo covers the backbox. With the backbox moved above the logo, the
//!   overlap turns red.
//! * A backglass without a backbox renders byte-identically to the 5.3
//!   `render_backglass_to_image` of the same backglass.
//! * Score reels: changing the score by one digit changes only that reel's
//!   region of the image.
//! * Goldens (`tests/fixtures/spec_5_5/<name>_<w>x<h>.rgba8`, raw RGBA8) of
//!   the composited translite + mini-playfield and of the reels; written
//!   when missing or under `SPEC_5_5_BLESS=1`, always after the analytic
//!   checks.
//!
//! One shared `&'static GpuContext` per test binary (lavapipe). Without an
//! adapter every test prints a SKIP note, unless `PIN_REQUIRE_GPU=1`.
//! The backbox API is reached only through `spec_5_5_api.rs`.
#![cfg(feature = "gpu-tests")]
#![allow(clippy::needless_range_loop)]

#[path = "spec_5_5_api.rs"]
mod api;

use api::gpu::{flat_scene, quad, reel_scene, render};
use pin_render::displays::{BackglassDesc, BackglassLayer, RgbaImage, render_backglass_to_image};
use pin_render::gpu::{GpuContext, context_for_tests};

fn shared_context(name: &str) -> Option<&'static GpuContext> {
    static CTX: std::sync::OnceLock<Option<GpuContext>> = std::sync::OnceLock::new();
    CTX.get_or_init(|| context_for_tests(name)).as_ref()
}

/// Output size (4:3, the translite's aspect: no letterbox).
const W: u32 = 320;
const H: u32 = 240;
/// Translite image size (4:3).
const TW: u32 = 160;
const TH: u32 = 120;

const BLUE: [u8; 4] = [0, 0, 255, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const RED: [u8; 4] = [255, 0, 0, 255];

fn image_fn(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> RgbaImage {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            v.extend_from_slice(&f(x, y));
        }
    }
    RgbaImage::from_rgba(w, h, v).expect("valid image")
}

/// Logo rectangle, fractions of the translite: x 0..0.5, y 0.4..0.6.
const LOGO: [f32; 4] = [0.0, 0.4, 0.5, 0.6];

/// Blue opaque base (depth 0) and a green logo, opaque inside [`LOGO`] and
/// fully transparent elsewhere (depth 1).
fn translite() -> BackglassDesc {
    let base = image_fn(TW, TH, |_, _| BLUE);
    let logo = image_fn(TW, TH, |x, y| {
        let (fx, fy) = ((x as f32 + 0.5) / TW as f32, (y as f32 + 0.5) / TH as f32);
        if (LOGO[0]..LOGO[2]).contains(&fx) && (LOGO[1]..LOGO[3]).contains(&fy) {
            GREEN
        } else {
            [0, 0, 0, 0]
        }
    });
    BackglassDesc {
        layers: vec![
            BackglassLayer {
                image: base,
                lamp: None,
            },
            BackglassLayer {
                image: logo,
                lamp: None,
            },
        ],
    }
}

/// The red quad: x ±333.3 mm, y ±250 mm at z = 0, which the spec camera
/// (1000 mm tall view at 4:3, so 1333.3 mm wide) shows over the central
/// half of the image in both directions: x 25 %..75 %, y 25 %..75 %.
fn red_quad() -> api::gpu::Mesh {
    quad([-1000.0 / 3.0, -250.0, 1000.0 / 3.0, 250.0], RED)
}

fn mean(img: &RgbaImage, fx0: f32, fy0: f32, fx1: f32, fy1: f32) -> [f32; 3] {
    let (x0, x1) = (
        (fx0 * img.width as f32) as u32,
        (fx1 * img.width as f32) as u32,
    );
    let (y0, y1) = (
        (fy0 * img.height as f32) as u32,
        (fy1 * img.height as f32) as u32,
    );
    let mut s = [0f64; 3];
    let mut n = 0f64;
    for y in y0..y1 {
        for x in x0..x1 {
            let p = img.pixel(x, y);
            for k in 0..3 {
                s[k] += f64::from(p[k]);
            }
            n += 1.0;
        }
    }
    assert!(n > 0.0);
    [(s[0] / n) as f32, (s[1] / n) as f32, (s[2] / n) as f32]
}

/// The region's mean is dominated by channel `k`: that channel ≥ 160 and
/// each other channel ≤ 15 % of it.
fn assert_dominant(img: &RgbaImage, r: [f32; 4], k: usize, what: &str) {
    let m = mean(img, r[0], r[1], r[2], r[3]);
    let ok = m[k] >= 160.0 && (0..3).all(|j| j == k || m[j] <= 0.15 * m[k]);
    assert!(
        ok,
        "{what}: region {r:?} mean {m:?}, want channel {k} dominant"
    );
}

// Regions (fractions of the image): away from every edge by ≥ 5 %.
const R_BLUE_CORNER: [f32; 4] = [0.03, 0.04, 0.17, 0.17];
const R_BLUE_LOWER_RIGHT: [f32; 4] = [0.82, 0.82, 0.96, 0.96];
const R_RED_ONLY: [f32; 4] = [0.55, 0.29, 0.70, 0.36];
const R_LOGO_OVER_BACKBOX: [f32; 4] = [0.30, 0.45, 0.45, 0.55];
const R_LOGO_OVER_BASE: [f32; 4] = [0.04, 0.45, 0.19, 0.55];

fn fixture_path(name: &str, w: u32, h: u32) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/spec_5_5")
        .join(format!("{name}_{w}x{h}.rgba8"))
}

/// Secondary evidence (D30): compare with the stored snapshot; it is
/// written only under `SPEC_5_5_BLESS=1` (a missing one fails). Call only after the analytic
/// assertions on `img` passed. Tolerance as in 5.3 (rasterisers differ on
/// edges): at most 1 % of channels off by more than 6 levels, none by more
/// than 96.
fn snapshot(name: &str, img: &RgbaImage) {
    let path = fixture_path(name, img.width, img.height);
    let bless = std::env::var("SPEC_5_5_BLESS").is_ok_and(|v| v == "1");
    if !bless && !path.exists() {
        panic!(
            "[{name}] snapshot {} is missing; bless it on lavapipe with SPEC_5_5_BLESS=1",
            path.display()
        );
    }
    if bless {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &img.pixels).unwrap();
        println!(
            "[{name}] snapshot written to {} (analytic checks passed first)",
            path.display()
        );
        return;
    }
    let want = std::fs::read(&path).unwrap();
    assert_eq!(want.len(), img.pixels.len(), "[{name}] snapshot size");
    let mut off = 0usize;
    let mut worst = 0u8;
    for (a, b) in want.iter().zip(&img.pixels) {
        let d = a.abs_diff(*b);
        worst = worst.max(d);
        if d > 6 {
            off += 1;
        }
    }
    let frac = off as f64 / want.len() as f64;
    println!("[{name}] snapshot: {off} channels off by > 6 ({frac:.5}), worst {worst}");
    assert!(
        frac <= 0.01 && worst <= 96,
        "[{name}] differs from the snapshot: {frac:.4} of channels off, worst {worst}"
    );
}

#[test]
fn backbox_composites_between_translite_layers_by_depth() {
    let Some(ctx) = shared_context("backbox_depth_order") else {
        return;
    };
    let desc = translite();
    // Backbox at depth 0: above the base, below the logo.
    let between = flat_scene(vec![red_quad()], 0);
    let img = render(ctx, &desc, Some(&between), &[], W, H);
    assert_dominant(&img, R_BLUE_CORNER, 2, "nothing over the base");
    assert_dominant(&img, R_BLUE_LOWER_RIGHT, 2, "nothing over the base");
    assert_dominant(&img, R_RED_ONLY, 0, "backbox over the base");
    assert_dominant(&img, R_LOGO_OVER_BACKBOX, 1, "logo over the backbox");
    assert_dominant(&img, R_LOGO_OVER_BASE, 1, "logo over the base");
    // Left/right symmetry of the red quad's coverage (its centre row,
    // below the logo): the red span is centred.
    let row = (0.32 * H as f32) as u32;
    let reds: Vec<u32> = (0..W)
        .filter(|&x| img.pixel(x, row)[0] > 128 && img.pixel(x, row)[2] < 128)
        .collect();
    let (first, last) = (*reds.first().expect("red row"), *reds.last().unwrap());
    let centre = 0.5 * (first + last) as f32;
    assert!(
        (centre - 0.5 * W as f32).abs() <= 2.0,
        "red span {first}..{last} not centred"
    );
    assert!(
        ((last - first + 1) as f32 - 0.5 * W as f32).abs() <= 4.0,
        "red span {first}..{last}, want half the width"
    );

    // Backbox above every layer (depth 1): the overlap turns red, the
    // logo outside the quad stays green, the uncovered base stays blue.
    let above = flat_scene(vec![red_quad()], 1);
    let img2 = render(ctx, &desc, Some(&above), &[], W, H);
    assert_dominant(&img2, R_LOGO_OVER_BACKBOX, 0, "backbox above the logo");
    assert_dominant(&img2, R_LOGO_OVER_BASE, 1, "logo outside the backbox");
    assert_dominant(&img2, R_BLUE_CORNER, 2, "nothing over the base");

    snapshot("backbox_between_layers", &img);
}

#[test]
fn backglass_without_backbox_is_byte_identical_to_5_3() {
    let Some(ctx) = shared_context("backbox_none_identical") else {
        return;
    };
    let desc = translite();
    let baseline = render_backglass_to_image(ctx, &desc, &[], W, H).expect("5.3 render");
    let with_none = render(ctx, &desc, None, &[], W, H);
    assert_eq!(baseline.width, with_none.width);
    assert_eq!(baseline.height, with_none.height);
    let differing = baseline
        .pixels
        .iter()
        .zip(&with_none.pixels)
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(differing, 0, "{differing} bytes differ from the 5.3 render");
}

/// Bounding box `(x0, y0, x1, y1)` (inclusive) of pixels that differ by
/// more than 8 levels in any channel; `None` when nothing differs.
fn diff_bbox(a: &RgbaImage, b: &RgbaImage) -> Option<(u32, u32, u32, u32)> {
    let mut bb: Option<(u32, u32, u32, u32)> = None;
    for y in 0..a.height {
        for x in 0..a.width {
            let (p, q) = (a.pixel(x, y), b.pixel(x, y));
            if (0..3).any(|k| p[k].abs_diff(q[k]) > 8) {
                bb = Some(match bb {
                    None => (x, y, x, y),
                    Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
                });
            }
        }
    }
    bb
}

/// Image x of a scene x (mm, z = 0) under the spec camera at 4:3.
fn image_x(x_mm: f32) -> f32 {
    (0.5 + x_mm / (4.0 / 3.0 * 1000.0)) * W as f32
}

fn image_y(y_mm: f32) -> f32 {
    (0.5 - y_mm / 1000.0) * H as f32
}

#[test]
fn reels_change_only_their_own_region() {
    let Some(ctx) = shared_context("backbox_reels") else {
        return;
    };
    let desc = translite();
    let scene = reel_scene(4, 0);
    let a = render(ctx, &desc, Some(&scene), &[1, 2, 3, 4], W, H);
    // The reels are drawn: the region of the reel bank is not the bare
    // translite.
    let bare = render(ctx, &desc, None, &[], W, H);
    assert!(diff_bbox(&a, &bare).is_some(), "reels not drawn");
    let b = render(ctx, &desc, Some(&scene), &[1, 2, 3, 5], W, H);
    let c = render(ctx, &desc, Some(&scene), &[1, 2, 4, 4], W, H);
    let units = diff_bbox(&a, &b).expect("units digit change left the image unchanged");
    let tens = diff_bbox(&a, &c).expect("tens digit change left the image unchanged");
    // Reel i spans x ∈ centre ± 20 mm (width 40), y ∈ ±25 mm (radius);
    // allow 3 px for edge rasterisation.
    let reel_box = |i: usize| {
        let cx = -90.0 + 60.0 * i as f32;
        (
            image_x(cx - 20.0) - 3.0,
            image_y(25.0) - 3.0,
            image_x(cx + 20.0) + 3.0,
            image_y(-25.0) + 3.0,
        )
    };
    for (what, bb, reel) in [("units", units, 3usize), ("tens", tens, 2)] {
        let r = reel_box(reel);
        assert!(
            bb.0 as f32 >= r.0 && bb.2 as f32 <= r.2 && bb.1 as f32 >= r.1 && bb.3 as f32 <= r.3,
            "{what} change touched {bb:?}, outside reel {reel}'s region {r:?}"
        );
    }
    // Disjoint, units to the right of tens.
    assert!(
        tens.2 < units.0,
        "tens {tens:?} and units {units:?} overlap"
    );
    // Same digits, same image (reels settle deterministically).
    let a2 = render(ctx, &desc, Some(&scene), &[1, 2, 3, 4], W, H);
    assert!(a.pixels == a2.pixels, "same score rendered differently");

    snapshot("backbox_reels_1234", &a);
}
