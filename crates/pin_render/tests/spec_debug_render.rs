//! Independent spec tests for the GPU side of `pin_render::debug` and for
//! `pin_render::gpu::GpuContext` (Subphase 1.4 verification criterion 2, D30: analytic
//! pixel assertions).
//!
//! Every test here needs an adapter, so the whole file is behind the `gpu-tests` feature.
//! Without an adapter a test prints a note and passes, unless `PIN_REQUIRE_GPU=1`.
//!
//! Conventions asserted (from the brief): RGBA8 unorm (not sRGB), tightly packed rows,
//! row 0 = top of the image; `Camera::ortho_top_down(min, max)` maps world X to the right
//! and world Y up onto the full viewport. So world (x, y) in a camera over [0,1]² lands at
//! the continuous image position (x·W, (1−y)·H), and pixel (col, row) covers
//! [col, col+1) × [row, row+1).
//!
//! Tolerances: colour channels ±2/255. A line that runs exactly along a pixel boundary may
//! light either adjacent row/column (rasteriser tie-break); a line through pixel centres
//! must light exactly that row/column. Lines are 1 px wide.
#![cfg(feature = "gpu-tests")]

use pin_core::snapshot::BallState;
use pin_core::{FrameStateSnapshot, ProfileId};
use pin_render::debug::{
    Camera, DebugLine, DebugLines, box_lines, polyline_lines, render_offscreen, snapshot_lines,
};
use pin_render::gpu::{GpuContext, GpuOptions, require_gpu};

const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// Returns a context, or `None` (after printing a note) when no adapter exists and
/// `PIN_REQUIRE_GPU` is not set.
fn gpu() -> Option<GpuContext> {
    match GpuContext::new_headless(&GpuOptions::from_env()) {
        Ok(ctx) => Some(ctx),
        Err(e) => {
            if require_gpu() {
                panic!("PIN_REQUIRE_GPU=1 but no GPU context: {e}");
            }
            println!("SKIP (no GPU adapter, PIN_REQUIRE_GPU not set): {e}");
            None
        }
    }
}

fn unit_camera() -> Camera {
    Camera::ortho_top_down([0.0, 0.0], [1.0, 1.0])
}

fn seg(a: [f32; 2], b: [f32; 2], color: [f32; 4]) -> DebugLine {
    DebugLine {
        a: [a[0], a[1], 0.0],
        b: [b[0], b[1], 0.0],
        color,
    }
}

fn to_u8(c: [f32; 4]) -> [u8; 4] {
    c.map(|v| (v * 255.0).round() as u8)
}

fn close(a: [u8; 4], b: [u8; 4]) -> bool {
    (0..4).all(|i| a[i].abs_diff(b[i]) <= 2)
}

struct Image {
    w: u32,
    h: u32,
    px: Vec<u8>,
    clear: [u8; 4],
}

impl Image {
    fn at(&self, col: u32, row: u32) -> [u8; 4] {
        let i = ((row * self.w + col) * 4) as usize;
        [self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3]]
    }
    /// A pixel is lit when it differs from the clear colour by more than the tolerance.
    fn lit(&self, col: u32, row: u32) -> bool {
        !close(self.at(col, row), self.clear)
    }
    fn lit_pixels(&self) -> Vec<(u32, u32)> {
        let mut v = Vec::new();
        for row in 0..self.h {
            for col in 0..self.w {
                if self.lit(col, row) {
                    v.push((col, row));
                }
            }
        }
        v
    }
    fn lit_rows_in_col(&self, col: u32) -> Vec<u32> {
        (0..self.h).filter(|&r| self.lit(col, r)).collect()
    }
    fn lit_cols_in_row(&self, row: u32) -> Vec<u32> {
        (0..self.w).filter(|&c| self.lit(c, row)).collect()
    }
    /// Every pixel is either the untouched clear colour or `color` (no blending, no AA).
    fn assert_only(&self, color: [f32; 4]) {
        let want = to_u8(color);
        for row in 0..self.h {
            for col in 0..self.w {
                let p = self.at(col, row);
                assert!(
                    close(p, self.clear) || close(p, want),
                    "pixel ({col},{row}) = {p:?} is neither clear {:?} nor line {want:?}",
                    self.clear
                );
            }
        }
    }
}

fn render(
    ctx: &GpuContext,
    w: u32,
    h: u32,
    clear: [f32; 4],
    camera: &Camera,
    lines: &[DebugLine],
) -> Image {
    let mut list = DebugLines::new(lines.len().max(1));
    for l in lines {
        assert!(list.push(*l));
    }
    render_list(ctx, w, h, clear, camera, &list)
}

fn render_list(
    ctx: &GpuContext,
    w: u32,
    h: u32,
    clear: [f32; 4],
    camera: &Camera,
    list: &DebugLines,
) -> Image {
    let px = render_offscreen(ctx, w, h, clear, camera, list)
        .unwrap_or_else(|e| panic!("render_offscreen failed: {e}"));
    assert_eq!(
        px.len(),
        (w * h * 4) as usize,
        "RGBA8, tightly packed rows (no row padding) for {w}×{h}"
    );
    Image {
        w,
        h,
        px,
        clear: to_u8(clear),
    }
}

// ---------------------------------------------------------------------------------------
// context
// ---------------------------------------------------------------------------------------

#[test]
fn headless_context_reports_an_adapter() {
    let Some(ctx) = gpu() else { return };
    let info = ctx.adapter_info();
    println!("adapter: {} ({:?}, {:?})", info.name, info.backend, info.device_type);
    assert!(!info.name.is_empty());
    // Device and queue are usable handles.
    let _device: &wgpu::Device = ctx.device();
    let _queue: &wgpu::Queue = ctx.queue();
}

#[test]
fn two_contexts_can_coexist() {
    let Some(a) = gpu() else { return };
    let Some(b) = gpu() else { return };
    let ia = render(&a, 16, 16, BLUE, &unit_camera(), &[]);
    let ib = render(&b, 16, 16, RED, &unit_camera(), &[]);
    assert_eq!(ia.at(3, 3), [0, 0, 255, 255]);
    assert_eq!(ib.at(3, 3), [255, 0, 0, 255]);
}

// ---------------------------------------------------------------------------------------
// clear, format and packing
// ---------------------------------------------------------------------------------------

#[test]
fn empty_frame_is_the_clear_colour_everywhere() {
    let Some(ctx) = gpu() else { return };
    // Sizes whose row byte length is not a multiple of wgpu's 256-byte copy alignment are
    // included on purpose: the result must still be tightly packed.
    for (w, h) in [(256u32, 256u32), (200, 100), (64, 64), (1, 1), (300, 7), (3, 130)] {
        for clear in [BLACK, [0.25, 0.5, 0.75, 1.0], WHITE, [1.0, 0.0, 0.0, 0.0]] {
            let img = render(&ctx, w, h, clear, &unit_camera(), &[]);
            let want = to_u8(clear);
            for row in 0..h {
                for col in 0..w {
                    assert!(
                        close(img.at(col, row), want),
                        "{w}×{h} clear {clear:?}: pixel ({col},{row}) = {:?}, want {want:?}",
                        img.at(col, row)
                    );
                }
            }
        }
    }
}

/// Unorm, not sRGB: 0.5 must read back as 127/128, not 188.
#[test]
fn target_is_linear_unorm_not_srgb() {
    let Some(ctx) = gpu() else { return };
    let img = render(
        &ctx,
        64,
        64,
        [0.5, 0.5, 0.5, 1.0],
        &unit_camera(),
        &[seg([0.0, 0.5078125], [1.0, 0.5078125], [0.2, 0.6, 1.0, 1.0])],
    );
    assert!(close(img.at(5, 5), [128, 128, 128, 255]), "clear: {:?}", img.at(5, 5));
    // y = 0.5078125 → image y = 31.5 → the centre of row 31.
    assert!(
        close(img.at(32, 31), [51, 153, 255, 255]),
        "line colour 0.2/0.6/1.0 must read back as 51/153/255, got {:?}",
        img.at(32, 31)
    );
}

// ---------------------------------------------------------------------------------------
// the brief's analytic examples
// ---------------------------------------------------------------------------------------

/// "on a black 256×256 clear with an ortho camera over [0,1]², a red horizontal line at
/// y=0.5 lights pixels in rows 127–128 only and is red".
#[test]
fn red_horizontal_line_at_half_height() {
    let Some(ctx) = gpu() else { return };
    let img = render(
        &ctx,
        256,
        256,
        BLACK,
        &unit_camera(),
        &[seg([0.0, 0.5], [1.0, 0.5], RED)],
    );
    img.assert_only(RED);
    let lit = img.lit_pixels();
    assert!(!lit.is_empty(), "the line is not visible at all");
    for &(col, row) in &lit {
        assert!(
            row == 127 || row == 128,
            "lit pixel ({col},{row}) outside rows 127–128"
        );
        assert!(close(img.at(col, row), [255, 0, 0, 255]), "not red: {:?}", img.at(col, row));
    }
    // The line spans the viewport: every interior column is covered, 1 px wide (the line
    // sits exactly on the 127/128 boundary, so either row may be chosen per column).
    for col in 1..255 {
        let rows = img.lit_rows_in_col(col);
        assert!(!rows.is_empty(), "gap in the line at column {col}");
    }
    let double = (1..255).filter(|&c| img.lit_rows_in_col(c).len() > 1).count();
    assert_eq!(double, 0, "{double} columns are 2 px thick: lines must be 1 px wide");
}

/// "a green vertical line at x=0.25 lights column ~64".
#[test]
fn green_vertical_line_at_quarter_width() {
    let Some(ctx) = gpu() else { return };
    let img = render(
        &ctx,
        256,
        256,
        BLACK,
        &unit_camera(),
        &[seg([0.25, 0.0], [0.25, 1.0], GREEN)],
    );
    img.assert_only(GREEN);
    let lit = img.lit_pixels();
    assert!(!lit.is_empty(), "the line is not visible at all");
    for &(col, row) in &lit {
        assert!(col == 63 || col == 64, "lit pixel ({col},{row}) outside columns 63–64");
        assert!(close(img.at(col, row), [0, 255, 0, 255]));
    }
    for row in 1..255 {
        let cols = img.lit_cols_in_row(row);
        assert_eq!(cols.len(), 1, "row {row}: lit columns {cols:?}, want exactly one");
    }
}

/// "a box outline leaves its interior black".
#[test]
fn box_outline_leaves_interior_and_exterior_black() {
    let Some(ctx) = gpu() else { return };
    let mut list = DebugLines::new(32);
    box_lines([0.25, 0.25, 0.0], [0.75, 0.75, 0.02], WHITE, &mut list);
    assert_eq!(list.as_slice().len(), 12);
    let img = render_list(&ctx, 256, 256, BLACK, &unit_camera(), &list);
    img.assert_only(WHITE);

    // The outline is the square with image corners at 64 and 192 (boundary positions, so
    // 63|64 and 191|192 are both acceptable).
    let on_edge = |v: u32| (63..=64).contains(&v) || (191..=192).contains(&v);
    let lit = img.lit_pixels();
    assert!(!lit.is_empty());
    for &(col, row) in &lit {
        let inside_span = |v: u32| (63..=192).contains(&v);
        assert!(
            (on_edge(col) && inside_span(row)) || (on_edge(row) && inside_span(col)),
            "lit pixel ({col},{row}) is not on the box outline"
        );
    }
    // Interior untouched.
    for row in 66..=189 {
        for col in 66..=189 {
            assert_eq!(img.at(col, row), [0, 0, 0, 255], "interior pixel ({col},{row})");
        }
    }
    // All four sides are present along their whole length.
    for t in 66..=189 {
        let cols = img.lit_cols_in_row(t);
        assert!(
            cols.iter().any(|c| (63..=64).contains(c)) && cols.iter().any(|c| (191..=192).contains(c)),
            "row {t}: left/right sides missing, lit columns {cols:?}"
        );
        assert_eq!(cols.len(), 2, "row {t}: sides must be 1 px wide, lit columns {cols:?}");
        let rows = img.lit_rows_in_col(t);
        assert!(
            rows.iter().any(|r| (63..=64).contains(r)) && rows.iter().any(|r| (191..=192).contains(r)),
            "column {t}: top/bottom sides missing, lit rows {rows:?}"
        );
        assert_eq!(rows.len(), 2, "column {t}: sides must be 1 px wide, lit rows {rows:?}");
    }
}

/// "colour channels match the line colour within ±2/255".
#[test]
fn line_colours_are_exact_within_two_levels() {
    let Some(ctx) = gpu() else { return };
    let colours: [[f32; 4]; 8] = [
        RED,
        GREEN,
        BLUE,
        WHITE,
        [0.2, 0.6, 1.0, 1.0],
        [0.5, 0.5, 0.5, 1.0],
        [1.0, 0.5, 0.0, 1.0],
        [0.1, 0.9, 0.3, 1.0],
    ];
    // One horizontal line per colour through the centre of rows 20, 40, …, 160.
    let rows: Vec<u32> = (1..=8).map(|i| i * 20).collect();
    let lines: Vec<DebugLine> = colours
        .iter()
        .zip(&rows)
        .map(|(c, &row)| {
            let y = 1.0 - (row as f32 + 0.5) / 256.0;
            seg([0.0, y], [1.0, y], *c)
        })
        .collect();
    let img = render(&ctx, 256, 256, BLACK, &unit_camera(), &lines);
    for (c, &row) in colours.iter().zip(&rows) {
        let want = to_u8(*c);
        for col in 2..254 {
            let got = img.at(col, row);
            assert!(
                close(got, want),
                "colour {c:?} at ({col},{row}): got {got:?}, want {want:?} ±2"
            );
        }
        // Neighbouring rows are untouched.
        for col in 2..254 {
            assert_eq!(img.at(col, row - 1), [0, 0, 0, 255]);
            assert_eq!(img.at(col, row + 1), [0, 0, 0, 255]);
        }
    }
    assert_eq!(img.lit_pixels().iter().filter(|(_, r)| !rows.contains(r)).count(), 0);
}

// ---------------------------------------------------------------------------------------
// position, orientation, width
// ---------------------------------------------------------------------------------------

/// Lines through pixel centres have no tie to break: exactly that row / column.
#[test]
fn lines_through_pixel_centres_light_exactly_one_row_or_column() {
    let Some(ctx) = gpu() else { return };
    for row in [0u32, 1, 37, 100, 128, 200, 254, 255] {
        let y = 1.0 - (row as f32 + 0.5) / 256.0;
        let img = render(&ctx, 256, 256, BLACK, &unit_camera(), &[seg([0.0, y], [1.0, y], RED)]);
        img.assert_only(RED);
        let lit = img.lit_pixels();
        assert!(lit.iter().all(|&(_, r)| r == row), "world y {y}: expected row {row} only");
        for col in 1..255 {
            assert!(img.lit(col, row), "row {row}: gap at column {col}");
        }
    }
    for col in [0u32, 1, 40, 127, 128, 201, 254, 255] {
        let x = (col as f32 + 0.5) / 256.0;
        let img = render(&ctx, 256, 256, BLACK, &unit_camera(), &[seg([x, 0.0], [x, 1.0], GREEN)]);
        img.assert_only(GREEN);
        let lit = img.lit_pixels();
        assert!(lit.iter().all(|&(c, _)| c == col), "world x {x}: expected column {col} only");
        for row in 1..255 {
            assert!(img.lit(col, row), "column {col}: gap at row {row}");
        }
    }
}

/// World Y is up and row 0 is the top: a mark near world (0.1, 0.9) is in the top-left
/// corner of the image, near pixel (25.6, 25.6); world (0.9, 0.1) is bottom-right.
#[test]
fn y_is_up_and_x_is_right() {
    let Some(ctx) = gpu() else { return };
    let cross = |c: [f32; 2], color: [f32; 4]| {
        [
            seg([c[0] - 0.02, c[1]], [c[0] + 0.02, c[1]], color),
            seg([c[0], c[1] - 0.02], [c[0], c[1] + 0.02], color),
        ]
    };
    let mut lines = Vec::new();
    lines.extend(cross([0.1, 0.9], RED));
    lines.extend(cross([0.9, 0.1], GREEN));
    lines.extend(cross([0.9, 0.8], BLUE));
    let img = render(&ctx, 256, 256, BLACK, &unit_camera(), &lines);

    let centroid = |want: [u8; 4]| {
        let pts: Vec<(u32, u32)> = img
            .lit_pixels()
            .into_iter()
            .filter(|&(c, r)| close(img.at(c, r), want))
            .collect();
        assert!(pts.len() >= 15, "mark {want:?} not drawn ({} pixels)", pts.len());
        let n = pts.len() as f32;
        (
            pts.iter().map(|p| p.0 as f32 + 0.5).sum::<f32>() / n,
            pts.iter().map(|p| p.1 as f32 + 0.5).sum::<f32>() / n,
        )
    };
    let near = |got: (f32, f32), want: (f32, f32), what: &str| {
        assert!(
            (got.0 - want.0).abs() <= 1.5 && (got.1 - want.1).abs() <= 1.5,
            "{what}: centroid {got:?}, expected about {want:?}"
        );
    };
    near(centroid([255, 0, 0, 255]), (25.6, 25.6), "world (0.1,0.9) → top-left");
    near(centroid([0, 255, 0, 255]), (230.4, 230.4), "world (0.9,0.1) → bottom-right");
    near(centroid([0, 0, 255, 255]), (230.4, 51.2), "world (0.9,0.8) → upper right");
}

/// The diagonal from world (0,0) to (1,1) runs from the bottom-left to the top-right pixel.
#[test]
fn diagonal_runs_bottom_left_to_top_right_and_is_thin() {
    let Some(ctx) = gpu() else { return };
    let img = render(&ctx, 256, 256, BLACK, &unit_camera(), &[seg([0.0, 0.0], [1.0, 1.0], WHITE)]);
    img.assert_only(WHITE);
    let lit = img.lit_pixels();
    assert!(lit.len() >= 250, "diagonal has only {} pixels", lit.len());
    for &(col, row) in &lit {
        let expect_row = 255 - col as i64;
        assert!(
            (row as i64 - expect_row).abs() <= 1,
            "lit pixel ({col},{row}) is off the bottom-left → top-right diagonal"
        );
    }
    for col in 1..255 {
        let n = img.lit_rows_in_col(col).len();
        assert!((1..=2).contains(&n), "column {col}: {n} lit pixels on a 45° 1 px line");
    }
    assert!(lit.len() <= 2 * 256);
}

/// A shallow line is straight: its row is a monotone, 1-px-per-column staircase that stays
/// within one pixel of the ideal line.
#[test]
fn shallow_line_is_straight_and_one_pixel_wide() {
    let Some(ctx) = gpu() else { return };
    let (a, b) = ([0.05f32, 0.2f32], [0.95f32, 0.43f32]);
    let img = render(&ctx, 256, 256, BLACK, &unit_camera(), &[seg(a, b, RED)]);
    img.assert_only(RED);
    let (x0, x1) = (a[0] * 256.0, b[0] * 256.0);
    for col in 0..256u32 {
        let rows = img.lit_rows_in_col(col);
        let xc = col as f32 + 0.5;
        if xc > x0 + 1.5 && xc < x1 - 1.5 {
            assert_eq!(rows.len(), 1, "column {col}: lit rows {rows:?}, want exactly one");
            let t = (xc / 256.0 - a[0]) / (b[0] - a[0]);
            let ideal = (1.0 - (a[1] + t * (b[1] - a[1]))) * 256.0;
            assert!(
                (rows[0] as f32 + 0.5 - ideal).abs() <= 1.0,
                "column {col}: row {} but the ideal line is at {ideal}",
                rows[0]
            );
        } else if xc < x0 - 1.5 || xc > x1 + 1.5 {
            assert!(rows.is_empty(), "column {col} is beyond the segment ends");
        }
    }
}

/// A segment that extends beyond the viewport is clipped, not dropped or wrapped.
#[test]
fn lines_are_clipped_to_the_viewport() {
    let Some(ctx) = gpu() else { return };
    let y = 1.0 - 90.5 / 256.0;
    let img = render(
        &ctx,
        256,
        256,
        BLACK,
        &unit_camera(),
        &[
            seg([-3.0, y], [4.0, y], RED),
            // Entirely outside: must leave no trace.
            seg([1.5, 0.0], [1.5, 1.0], GREEN),
            seg([0.0, -0.5], [1.0, -0.5], GREEN),
        ],
    );
    img.assert_only(RED);
    for col in 0..256 {
        assert_eq!(img.lit_rows_in_col(col), vec![90], "column {col}");
    }
}

/// Background stays exactly the clear colour, and line pixels are not blended with it.
#[test]
fn background_is_untouched_and_lines_are_opaque() {
    let Some(ctx) = gpu() else { return };
    let clear = [0.1, 0.2, 0.3, 1.0];
    let y = 1.0 - 50.5 / 128.0;
    let img = render(&ctx, 128, 128, clear, &unit_camera(), &[seg([0.0, y], [1.0, y], [1.0, 1.0, 0.0, 1.0])]);
    let bg = img.at(0, 0);
    assert!(close(bg, to_u8(clear)));
    for row in 0..128 {
        for col in 0..128 {
            if row == 50 {
                if (1..127).contains(&col) {
                    assert!(close(img.at(col, row), [255, 255, 0, 255]), "({col},{row})");
                }
            } else {
                assert_eq!(img.at(col, row), bg, "background pixel ({col},{row}) changed");
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// camera regions and non-square targets
// ---------------------------------------------------------------------------------------

#[test]
fn camera_region_maps_onto_the_full_viewport() {
    let Some(ctx) = gpu() else { return };
    // A playfield-like region: 0.5 m × 1.0 m, offset from the origin.
    let cam = Camera::ortho_top_down([0.2, 1.0], [0.7, 2.0]);
    let (w, h) = (200u32, 100u32);
    // Centre of column 150 → x = 0.2 + 0.5·150.5/200; centre of row 20 → y = 2.0 − 20.5/100.
    let x = 0.2 + 0.5 * 150.5 / 200.0;
    let y = 2.0 - 20.5 / 100.0;
    let img = render(
        &ctx,
        w,
        h,
        BLACK,
        &cam,
        &[
            DebugLine { a: [x, 1.0, 0.0], b: [x, 2.0, 0.0], color: GREEN },
            DebugLine { a: [0.2, y, 0.0], b: [0.7, y, 0.0], color: RED },
        ],
    );
    for &(col, row) in &img.lit_pixels() {
        assert!(col == 150 || row == 20, "lit pixel ({col},{row}) is on neither line");
    }
    for row in 1..99 {
        if row != 20 {
            assert_eq!(img.lit_cols_in_row(row), vec![150], "row {row}");
            assert!(close(img.at(150, row), [0, 255, 0, 255]));
        }
    }
    for col in 1..199 {
        if col != 150 {
            assert_eq!(img.lit_rows_in_col(col), vec![20], "column {col}");
            assert!(close(img.at(col, 20), [255, 0, 0, 255]));
        }
    }
}

// ---------------------------------------------------------------------------------------
// snapshot circles and polylines on the GPU
// ---------------------------------------------------------------------------------------

/// A ball circle renders as a closed 1-px ring: all lit pixels lie between the 16-gon's
/// inscribed and circumscribed radius (±1 px), no angular gaps (segment joints included),
/// and the inside stays black.
#[test]
fn ball_circle_is_a_closed_ring() {
    let Some(ctx) = gpu() else { return };
    let mut s = FrameStateSnapshot::default();
    s.push_ball(BallState {
        position_m: [0.5, 0.5, 0.0135],
        profile: ProfileId(2),
        ..BallState::default()
    });
    let mut list = DebugLines::new(64);
    snapshot_lines(&s, &|p| if p == ProfileId(2) { 0.25 } else { 0.01 }, &mut list);
    assert_eq!(list.as_slice().len(), 16);
    let img = render_list(&ctx, 256, 256, BLACK, &unit_camera(), &list);

    let lit = img.lit_pixels();
    assert!(lit.len() > 300, "ring has only {} pixels", lit.len());
    let r_out = 0.25 * 256.0;
    let r_in = r_out * (std::f32::consts::PI / 16.0).cos();
    let mut buckets = [false; 120];
    for &(col, row) in &lit {
        let dx = col as f32 + 0.5 - 128.0;
        let dy = row as f32 + 0.5 - 128.0;
        let d = (dx * dx + dy * dy).sqrt();
        assert!(
            d >= r_in - 1.0 && d <= r_out + 1.0,
            "lit pixel ({col},{row}) at radius {d} px, ring is {r_in}..{r_out}"
        );
        let a = dy.atan2(dx).rem_euclid(std::f32::consts::TAU);
        buckets[((a / std::f32::consts::TAU * 120.0) as usize).min(119)] = true;
    }
    let missing: Vec<usize> = (0..120).filter(|&i| !buckets[i]).collect();
    assert!(missing.is_empty(), "gaps in the ring at 3° buckets {missing:?}");
    // Thin: about one pixel per unit of circumference (allow staircase overhead).
    let circumference = std::f32::consts::TAU * r_out;
    assert!(
        (lit.len() as f32) < circumference * 1.3,
        "{} lit pixels for a circumference of {circumference} px: ring is too thick",
        lit.len()
    );
    // Inside of the ring is untouched.
    for row in 0..256u32 {
        for col in 0..256u32 {
            let dx = col as f32 + 0.5 - 128.0;
            let dy = row as f32 + 0.5 - 128.0;
            if (dx * dx + dy * dy).sqrt() < r_in - 2.0 {
                assert_eq!(img.at(col, row), [0, 0, 0, 255]);
            }
        }
    }
}

/// A closed polyline square: joints at the corners leave no hole larger than one pixel.
#[test]
fn polyline_square_is_closed() {
    let Some(ctx) = gpu() else { return };
    let c = |p: u32| (p as f32 + 0.5) / 256.0; // world coordinate of a pixel centre
    let (lo, hi) = (c(40), c(200));
    let pts = [
        [lo, lo, 0.0],
        [hi, lo, 0.0],
        [hi, hi, 0.0],
        [lo, hi, 0.0],
        [lo, lo, 0.0],
    ];
    let mut list = DebugLines::new(8);
    polyline_lines(&pts, BLUE, &mut list);
    let img = render_list(&ctx, 256, 256, BLACK, &unit_camera(), &list);
    img.assert_only(BLUE);
    // World y of pixel-centre p is image row 255 − p.
    let (r_lo, r_hi) = (255 - 200, 255 - 40);
    let mut missing = 0;
    for t in 40..=200u32 {
        for (col, row) in [(t, r_lo), (t, r_hi)] {
            if !img.lit(col, row) {
                missing += 1;
            }
        }
    }
    for t in r_lo..=r_hi {
        for (col, row) in [(40, t), (200, t)] {
            if !img.lit(col, row) {
                missing += 1;
            }
        }
    }
    // Each of the four corners may lose at most its end pixel to the line end rule.
    assert!(missing <= 4, "{missing} outline pixels missing: gaps at the joints");
    for &(col, row) in &img.lit_pixels() {
        assert!(
            ((col == 40 || col == 200) && (r_lo..=r_hi).contains(&row))
                || ((row == r_lo || row == r_hi) && (40..=200).contains(&col)),
            "lit pixel ({col},{row}) is off the square"
        );
    }
}

// ---------------------------------------------------------------------------------------
// repeatability and capacity
// ---------------------------------------------------------------------------------------

#[test]
fn rendering_is_repeatable_on_one_context() {
    let Some(ctx) = gpu() else { return };
    let mut list = DebugLines::new(128);
    box_lines([0.1, 0.1, 0.0], [0.9, 0.6, 0.01], WHITE, &mut list);
    polyline_lines(
        &[[0.1, 0.9, 0.0], [0.3, 0.7, 0.0], [0.6, 0.95, 0.0], [0.9, 0.7, 0.0]],
        [1.0, 0.5, 0.0, 1.0],
        &mut list,
    );
    let a = render_list(&ctx, 256, 256, BLACK, &unit_camera(), &list);
    let b = render_list(&ctx, 256, 256, BLACK, &unit_camera(), &list);
    // A different frame in between must not leak into the next one.
    let other = render(&ctx, 256, 256, RED, &unit_camera(), &[seg([0.0, 0.0], [1.0, 1.0], GREEN)]);
    let c = render_list(&ctx, 256, 256, BLACK, &unit_camera(), &list);
    assert!(a.px == b.px, "two renders of the same frame differ");
    assert!(a.px == c.px, "state leaked from an unrelated frame");
    assert!(a.px != other.px);
}

#[test]
fn many_lines_render_in_one_frame() {
    let Some(ctx) = gpu() else { return };
    // 256 horizontal lines, one through the centre of every row, alternating colours.
    let mut list = DebugLines::new(256);
    for row in 0..256u32 {
        let y = 1.0 - (row as f32 + 0.5) / 256.0;
        let color = if row % 2 == 0 { RED } else { BLUE };
        assert!(list.push(seg([0.0, y], [1.0, y], color)));
    }
    assert!(!list.push(seg([0.0, 0.0], [1.0, 1.0], GREEN)), "over capacity: dropped");
    let img = render_list(&ctx, 256, 256, BLACK, &unit_camera(), &list);
    for row in 0..256u32 {
        let want = if row % 2 == 0 { [255, 0, 0, 255] } else { [0, 0, 255, 255] };
        for col in 2..254 {
            assert!(close(img.at(col, row), want), "({col},{row}) = {:?}", img.at(col, row));
        }
    }
}
