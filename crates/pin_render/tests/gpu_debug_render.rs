//! Offscreen debug-line rendering (gpu-tests; D30: analytic pixel assertions
//! first, then a regression snapshot).
//!
//! Without an adapter every test prints a SKIP note and passes, unless
//! `PIN_REQUIRE_GPU=1` (CI), which turns "no adapter" into a failure.
//!
//! Regression snapshot: `tests/fixtures/debug_scene_256.txt` lists the lit
//! pixels (`x y r g b`) of [`scene`], committed after the analytic checks
//! passed on lavapipe. Tolerance (documented, backend independent): a pixel
//! counts as matching when a reference pixel within 1 px (Chebyshev) has the
//! same colour ±2/255; at most 2 % of the union of lit pixels may mismatch.
//! This absorbs the backends' different tie-breaking for lines lying exactly
//! on pixel boundaries while catching misplaced, missing or recoloured lines.
//! `PIN_UPDATE_SNAPSHOTS=1` rewrites the fixture.
#![cfg(feature = "gpu-tests")]

use pin_core::FrameStateSnapshot;
use pin_core::snapshot::BallState;
use pin_render::debug::{
    Camera, DebugLine, DebugLines, box_lines, render_offscreen, snapshot_lines,
};
use pin_render::gpu::{GpuContext, GpuError, context_for_tests};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::OnceLock;

const N: u32 = 256;
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

fn ctx() -> Option<&'static GpuContext> {
    static CTX: OnceLock<Option<GpuContext>> = OnceLock::new();
    CTX.get_or_init(|| context_for_tests("gpu_debug_render"))
        .as_ref()
}

fn unit_camera() -> Camera {
    Camera::ortho_top_down([0.0, 0.0], [1.0, 1.0])
}

struct Image {
    px: Vec<u8>,
    w: u32,
    h: u32,
}

impl Image {
    fn at(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.w + x) * 4) as usize;
        [self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3]]
    }
    fn lit(&self) -> Vec<(u32, u32, [u8; 4])> {
        let mut v = Vec::new();
        for y in 0..self.h {
            for x in 0..self.w {
                let p = self.at(x, y);
                if p[0] > 2 || p[1] > 2 || p[2] > 2 {
                    v.push((x, y, p));
                }
            }
        }
        v
    }
}

fn render(lines: &DebugLines) -> Option<Image> {
    let ctx = ctx()?;
    let px = render_offscreen(ctx, N, N, BLACK, &unit_camera(), lines).expect("render");
    assert_eq!(px.len(), (N * N * 4) as usize, "tightly packed RGBA8");
    Some(Image { px, w: N, h: N })
}

fn close(p: [u8; 4], c: [f32; 4]) -> bool {
    (0..3).all(|i| (i32::from(p[i]) - (c[i] * 255.0).round() as i32).abs() <= 2)
}

fn one(line: DebugLine) -> DebugLines {
    let mut l = DebugLines::new(4);
    l.push(line);
    l
}

#[test]
fn clear_only_is_uniform_and_opaque() {
    let Some(img) = render(&DebugLines::new(1)) else {
        return;
    };
    assert!(img.lit().is_empty());
    assert!(img.px.chunks(4).all(|p| p == [0, 0, 0, 255]));
}

#[test]
fn red_horizontal_line_at_half_height() {
    let Some(img) = render(&one(DebugLine::new([0.0, 0.5, 0.0], [1.0, 0.5, 0.0], RED))) else {
        return;
    };
    let lit = img.lit();
    assert!(
        lit.len() >= 250,
        "line spans the width: {} pixels",
        lit.len()
    );
    let rows: HashSet<u32> = lit.iter().map(|p| p.1).collect();
    assert!(rows.iter().all(|r| *r == 127 || *r == 128), "rows {rows:?}");
    assert_eq!(rows.len(), 1, "1 px wide: rows {rows:?}");
    for (x, y, p) in &lit {
        assert!(close(*p, RED), "({x},{y}) = {p:?}");
        assert_eq!(p[3], 255);
    }
}

#[test]
fn green_vertical_line_at_quarter_width() {
    let Some(img) = render(&one(DebugLine::new(
        [0.25, 0.0, 0.0],
        [0.25, 1.0, 0.0],
        GREEN,
    ))) else {
        return;
    };
    let lit = img.lit();
    assert!(lit.len() >= 250);
    let cols: HashSet<u32> = lit.iter().map(|p| p.0).collect();
    assert!(cols.iter().all(|c| *c == 63 || *c == 64), "cols {cols:?}");
    assert_eq!(cols.len(), 1);
    assert!(lit.iter().all(|(_, _, p)| close(*p, GREEN)));
}

#[test]
fn y_is_up_and_x_is_right() {
    // A short line near the top-right of world space lands in the top-right
    // of the image (row 0 = top).
    let Some(img) = render(&one(DebugLine::new(
        [0.80, 0.90, 0.0],
        [0.90, 0.90, 0.0],
        BLUE,
    ))) else {
        return;
    };
    let lit = img.lit();
    assert!(!lit.is_empty());
    for (x, y, p) in lit {
        assert!((25..=26).contains(&y), "row {y}");
        assert!((203..=231).contains(&x), "col {x}");
        assert!(close(p, BLUE));
    }
}

#[test]
fn box_outline_leaves_interior_black() {
    let mut l = DebugLines::new(16);
    box_lines([0.25, 0.25, 0.0], [0.75, 0.75, 0.0], BLUE, &mut l);
    let Some(img) = render(&l) else { return };
    let lit = img.lit();
    assert!(lit.len() >= 4 * 120, "{} lit", lit.len());
    for (x, y, p) in &lit {
        assert!(close(*p, BLUE));
        let on_edge = |v: u32| (63..=64).contains(&v) || (191..=192).contains(&v);
        assert!(on_edge(*x) || on_edge(*y), "({x},{y}) off the outline");
    }
    for y in 66..190 {
        for x in 66..190 {
            assert_eq!(img.at(x, y), [0, 0, 0, 255], "interior ({x},{y})");
        }
    }
    // Outside untouched.
    assert_eq!(img.at(10, 10), [0, 0, 0, 255]);
    assert_eq!(img.at(250, 250), [0, 0, 0, 255]);
}

#[test]
fn snapshot_ball_circle_is_closed_and_centred() {
    let mut s = FrameStateSnapshot::default();
    s.push_ball(BallState {
        position_m: [0.5, 0.5, 0.0],
        ..BallState::default()
    });
    let mut l = DebugLines::new(64);
    snapshot_lines(&s, &|_| 0.25, &mut l);
    assert_eq!(l.len(), 16);
    let Some(img) = render(&l) else { return };
    let lit = img.lit();
    // Every lit pixel lies near the circle (radius 64 px around (128,128)).
    for (x, y, p) in &lit {
        let d = ((*x as f32 + 0.5 - 128.0).powi(2) + (*y as f32 + 0.5 - 128.0).powi(2)).sqrt();
        assert!((58.0..=66.0).contains(&d), "({x},{y}) at distance {d}");
        assert!(close(*p, [1.0, 1.0, 1.0, 1.0]));
    }
    // No gaps at the joints: the outline is one 8-connected component.
    let set: HashSet<(u32, u32)> = lit.iter().map(|p| (p.0, p.1)).collect();
    let start = *set.iter().next().expect("lit");
    let mut seen = HashSet::from([start]);
    let mut q = VecDeque::from([start]);
    while let Some((x, y)) = q.pop_front() {
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                let n = ((x as i32 + dx) as u32, (y as i32 + dy) as u32);
                if set.contains(&n) && seen.insert(n) {
                    q.push_back(n);
                }
            }
        }
    }
    assert_eq!(seen.len(), set.len(), "circle outline has a gap");
    assert_eq!(img.at(128, 128), [0, 0, 0, 255]);
}

#[test]
fn invalid_sizes_are_errors() {
    let Some(ctx) = ctx() else { return };
    let l = DebugLines::new(1);
    for (w, h) in [(0, 4), (4, 0), (u32::MAX, 4)] {
        assert!(matches!(
            render_offscreen(ctx, w, h, BLACK, &unit_camera(), &l),
            Err(GpuError::InvalidArgument(_))
        ));
    }
    // Odd widths (row padding) read back tightly packed.
    let px = render_offscreen(ctx, 3, 2, [0.0, 0.0, 1.0, 1.0], &unit_camera(), &l).unwrap();
    assert_eq!(px.len(), 3 * 2 * 4);
    assert!(px.chunks(4).all(|p| p == [0, 0, 255, 255]));
}

#[test]
fn many_lines_grow_the_gpu_buffer() {
    // More lines than the renderer's initial GPU buffer (1024).
    let mut l = DebugLines::new(3000);
    for i in 0..3000 {
        let y = (i % 256) as f32 / 256.0 + 0.5 / 256.0;
        l.push(DebugLine::new([0.0, y, 0.0], [1.0, y, 0.0], GREEN));
    }
    let Some(img) = render(&l) else { return };
    let lit = img.lit().len();
    assert!(lit > 250 * 250, "{lit}");
}

/// The composite regression scene.
fn scene() -> DebugLines {
    let mut l = DebugLines::new(128);
    l.push(DebugLine::new([0.0, 0.5, 0.0], [1.0, 0.5, 0.0], RED));
    l.push(DebugLine::new([0.25, 0.0, 0.0], [0.25, 1.0, 0.0], GREEN));
    box_lines([0.6, 0.6, 0.0], [0.9, 0.9, 0.0], BLUE, &mut l);
    l.push(DebugLine::new(
        [0.05, 0.05, 0.0],
        [0.45, 0.35, 0.0],
        [1.0, 1.0, 0.0, 1.0],
    ));
    let mut s = FrameStateSnapshot::default();
    s.push_ball(BallState {
        position_m: [0.7, 0.25, 0.0],
        ..BallState::default()
    });
    snapshot_lines(&s, &|_| 0.15, &mut l);
    l
}

#[test]
fn regression_snapshot() {
    let Some(img) = render(&scene()) else { return };
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/debug_scene_256.txt"
    );
    let lit = img.lit();
    if std::env::var("PIN_UPDATE_SNAPSHOTS").is_ok_and(|v| v == "1") {
        let mut s = String::from("# lit pixels of gpu_debug_render::scene(), 256x256: x y r g b\n");
        for (x, y, p) in &lit {
            s.push_str(&format!("{x} {y} {} {} {}\n", p[0], p[1], p[2]));
        }
        std::fs::write(path, s).expect("write fixture");
        println!("[regression_snapshot] wrote {} pixels", lit.len());
        return;
    }
    let text = std::fs::read_to_string(path).expect("fixture");
    let reference: HashMap<(u32, u32), [u8; 3]> = text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let v: Vec<u32> = l.split_whitespace().map(|t| t.parse().unwrap()).collect();
            ((v[0], v[1]), [v[2] as u8, v[3] as u8, v[4] as u8])
        })
        .collect();
    let got: HashMap<(u32, u32), [u8; 3]> = lit
        .iter()
        .map(|(x, y, p)| ((*x, *y), [p[0], p[1], p[2]]))
        .collect();
    let near = |map: &HashMap<(u32, u32), [u8; 3]>, (x, y): (u32, u32), c: [u8; 3]| {
        (-1i32..=1).any(|dy| {
            (-1i32..=1).any(|dx| {
                map.get(&((x as i32 + dx) as u32, (y as i32 + dy) as u32))
                    .is_some_and(|r| (0..3).all(|i| (i32::from(r[i]) - i32::from(c[i])).abs() <= 2))
            })
        })
    };
    let mismatched = got
        .iter()
        .filter(|(k, c)| !near(&reference, **k, **c))
        .count()
        + reference
            .iter()
            .filter(|(k, c)| !near(&got, **k, **c))
            .count();
    let union = got.len().max(reference.len());
    println!(
        "[regression_snapshot] {} lit (reference {}), {mismatched} mismatched",
        got.len(),
        reference.len()
    );
    assert!(
        mismatched * 50 <= union,
        "{mismatched} of {union} pixels differ from the reference (tolerance 2 %)"
    );
}

/// N6: with a margin camera, a box exactly on the table bounds is visible on
/// all four sides.
#[test]
fn margin_camera_shows_lines_on_the_bounds() {
    let Some(ctx) = ctx() else { return };
    let mut l = DebugLines::new(16);
    box_lines([0.0, 0.0, 0.0], [1.0, 1.0, 0.0], RED, &mut l);
    let cam = Camera::ortho_top_down_with_margin([0.0, 0.0], [1.0, 1.0], 0.01);
    let px = render_offscreen(ctx, N, N, BLACK, &cam, &l).expect("render");
    let img = Image { px, w: N, h: N };
    let lit = img.lit();
    let (mut left, mut right, mut top, mut bottom) = (0, 0, 0, 0);
    for (x, y, _) in &lit {
        left += usize::from(*x < 5);
        right += usize::from(*x > N - 6);
        top += usize::from(*y < 5);
        bottom += usize::from(*y > N - 6);
    }
    assert!(
        left > 200 && right > 200 && top > 200 && bottom > 200,
        "{left} {right} {top} {bottom}"
    );
}
