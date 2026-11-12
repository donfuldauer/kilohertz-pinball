//! Debug line renderer (PLAN §4 Subphase 1.4).
//!
//! * [`DebugLines`]: a fixed-capacity line list (no allocation after
//!   construction) filled by [`snapshot_lines`], [`box_lines`],
//!   [`polyline_lines`] and [`ball_axes_lines`] — or by any caller with
//!   collider wireframes as generic line segments.
//! * [`Camera`]: a column-major view-projection matrix;
//!   [`Camera::ortho_top_down`] maps a world XY rectangle (X right, Y up) onto
//!   the full viewport.
//! * [`DebugRenderer`]: draws a [`DebugLines`] list as 1-px lines.
//! * [`render_offscreen`]: renders a list into an RGBA8 image (tests, tools).

mod renderer;

pub use renderer::{DebugRenderer, render_offscreen};

use pin_core::ProfileId;
use pin_core::snapshot::{BallState, FrameStateSnapshot};

/// One line segment from `a` to `b` (world space, metres) in linear RGBA
/// `color` (`0..=1`).
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DebugLine {
    /// Start point.
    pub a: [f32; 3],
    /// End point.
    pub b: [f32; 3],
    /// Colour `[r, g, b, a]`.
    pub color: [f32; 4],
}

impl DebugLine {
    /// A line from `a` to `b` in `color`.
    pub const fn new(a: [f32; 3], b: [f32; 3], color: [f32; 4]) -> Self {
        Self { a, b, color }
    }
}

/// Colour of ball circles drawn by [`snapshot_lines`] (white).
pub const BALL_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// Number of segments of a ball circle.
pub const CIRCLE_SEGMENTS: usize = 16;

/// A fixed-capacity list of [`DebugLine`]s.
///
/// The storage is allocated once in [`DebugLines::new`]; [`push`](Self::push)
/// beyond the capacity drops the line and counts it.
#[derive(Clone, Debug)]
pub struct DebugLines {
    lines: Vec<DebugLine>,
    capacity: usize,
    dropped: u64,
}

impl DebugLines {
    /// An empty list holding at most `capacity` lines.
    pub fn new(capacity: usize) -> Self {
        Self {
            lines: Vec::with_capacity(capacity),
            capacity,
            dropped: 0,
        }
    }

    /// Appends `line`; returns `false` (and counts a drop) when full.
    #[inline]
    pub fn push(&mut self, line: DebugLine) -> bool {
        if self.lines.len() >= self.capacity {
            self.dropped = self.dropped.saturating_add(1);
            return false;
        }
        self.lines.push(line);
        true
    }

    /// Lines dropped because the list was full, since construction or the
    /// last [`clear`](Self::clear).
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Removes every line and resets the drop counter (keeps the storage).
    pub fn clear(&mut self) {
        self.lines.clear();
        self.dropped = 0;
    }

    /// The lines.
    pub fn as_slice(&self) -> &[DebugLine] {
        &self.lines
    }

    /// Number of lines.
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// `true` when there are no lines.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Maximum number of lines.
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

/// Unit circle points for [`CIRCLE_SEGMENTS`] segments (`cos`, `sin`).
fn unit_circle() -> [[f32; 2]; CIRCLE_SEGMENTS] {
    let mut pts = [[0.0f32; 2]; CIRCLE_SEGMENTS];
    for (k, p) in pts.iter_mut().enumerate() {
        let t = core::f64::consts::TAU * k as f64 / CIRCLE_SEGMENTS as f64;
        *p = [t.cos() as f32, t.sin() as f32];
    }
    pts
}

/// Appends a circle of [`CIRCLE_SEGMENTS`] segments in the world XY plane
/// (at height `center[2]`). Consecutive segments share their end points
/// exactly, so the outline has no gaps.
pub fn circle_lines(center: [f32; 3], radius: f32, color: [f32; 4], out: &mut DebugLines) {
    let pts = unit_circle();
    let p = |k: usize| {
        let [c, s] = pts[k % CIRCLE_SEGMENTS];
        [center[0] + radius * c, center[1] + radius * s, center[2]]
    };
    for k in 0..CIRCLE_SEGMENTS {
        out.push(DebugLine::new(p(k), p(k + 1), color));
    }
}

/// Appends the debug lines of a snapshot.
///
/// * Each valid ball: a circle of 16 segments in the world XY plane at the
///   ball's position, with radius `radii_m(ball.profile)` and colour
///   [`BALL_COLOR`]. Balls whose radius is not finite and positive are skipped.
/// * Flippers: skipped. The snapshot carries only the bat angle relative to
///   its rest angle, not the pivot position or length (those come from the
///   table's flipper configuration), so nothing positional can be drawn from
///   the snapshot alone. Callers with the table geometry draw bats with
///   [`polyline_lines`].
pub fn snapshot_lines(
    s: &FrameStateSnapshot,
    radii_m: &dyn Fn(ProfileId) -> f32,
    out: &mut DebugLines,
) {
    for ball in s.balls() {
        let r = radii_m(ball.profile);
        if r.is_finite() && r > 0.0 {
            circle_lines(ball.position_m, r, BALL_COLOR, out);
        }
    }
}

/// Appends an orientation cross for `ball`: its local X (red), Y (green) and
/// Z (blue) axes rotated by the ball's orientation, each `radius` long from
/// the centre (shows spin in the debug view).
pub fn ball_axes_lines(ball: &BallState, radius: f32, out: &mut DebugLines) {
    let [x, y, z, w] = ball.orientation;
    let rotate = |v: [f32; 3]| -> [f32; 3] {
        // v' = v + 2w(q×v) + 2 q×(q×v)
        let q = [x, y, z];
        let cross = |a: [f32; 3], b: [f32; 3]| {
            [
                a[1] * b[2] - a[2] * b[1],
                a[2] * b[0] - a[0] * b[2],
                a[0] * b[1] - a[1] * b[0],
            ]
        };
        let t = cross(q, v).map(|c| 2.0 * c);
        let u = cross(q, t);
        [
            v[0] + w * t[0] + u[0],
            v[1] + w * t[1] + u[1],
            v[2] + w * t[2] + u[2],
        ]
    };
    let c = ball.position_m;
    let axes = [
        ([radius, 0.0, 0.0], [1.0, 0.0, 0.0, 1.0]),
        ([0.0, radius, 0.0], [0.0, 1.0, 0.0, 1.0]),
        ([0.0, 0.0, radius], [0.0, 0.0, 1.0, 1.0]),
    ];
    for (axis, color) in axes {
        let d = rotate(axis);
        out.push(DebugLine::new(
            c,
            [c[0] + d[0], c[1] + d[1], c[2] + d[2]],
            color,
        ));
    }
}

/// Appends the 12 edges of the axis-aligned box `min`..`max`.
pub fn box_lines(min: [f32; 3], max: [f32; 3], color: [f32; 4], out: &mut DebugLines) {
    let corner = |i: usize| {
        [
            if i & 1 == 0 { min[0] } else { max[0] },
            if i & 2 == 0 { min[1] } else { max[1] },
            if i & 4 == 0 { min[2] } else { max[2] },
        ]
    };
    // Corner pairs that differ in exactly one bit.
    const EDGES: [(usize, usize); 12] = [
        (0, 1),
        (2, 3),
        (4, 5),
        (6, 7),
        (0, 2),
        (1, 3),
        (4, 6),
        (5, 7),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ];
    for (i, j) in EDGES {
        out.push(DebugLine::new(corner(i), corner(j), color));
    }
}

/// Appends `points.len() - 1` segments joining consecutive points (nothing
/// for fewer than two points).
pub fn polyline_lines(points: &[[f32; 3]], color: [f32; 4], out: &mut DebugLines) {
    for w in points.windows(2) {
        out.push(DebugLine::new(w[0], w[1], color));
    }
}

/// A view-projection matrix, column-major (`view_proj[c][r]`, as WGSL
/// `mat4x4<f32>`): `clip = view_proj · [x, y, z, 1]`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Camera {
    /// Column-major view-projection matrix.
    pub view_proj: [[f32; 4]; 4],
}

impl Default for Camera {
    /// The identity (world = clip space).
    fn default() -> Self {
        Self {
            view_proj: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        }
    }
}

impl Camera {
    /// Orthographic top-down camera: world X in `min_xy[0]..max_xy[0]` maps to
    /// the viewport's left..right, world Y in `min_xy[1]..max_xy[1]` to
    /// bottom..top (Y up). World Z is ignored (depth 0.5).
    ///
    /// A degenerate range (equal bounds) is widened to 1 m to keep the matrix
    /// finite.
    pub fn ortho_top_down(min_xy: [f32; 2], max_xy: [f32; 2]) -> Camera {
        let span = |lo: f32, hi: f32| {
            let d = hi - lo;
            if d == 0.0 || !d.is_finite() { 1.0 } else { d }
        };
        let w = span(min_xy[0], max_xy[0]);
        let h = span(min_xy[1], max_xy[1]);
        let sx = 2.0 / w;
        let sy = 2.0 / h;
        let tx = -1.0 - min_xy[0] * sx;
        let ty = -1.0 - min_xy[1] * sy;
        Camera {
            view_proj: [
                [sx, 0.0, 0.0, 0.0],
                [0.0, sy, 0.0, 0.0],
                [0.0, 0.0, 0.0, 0.0],
                [tx, ty, 0.5, 1.0],
            ],
        }
    }

    /// `view_proj · [p, 1]` (clip coordinates).
    pub fn project(&self, p: [f32; 3]) -> [f32; 4] {
        let m = &self.view_proj;
        let mut out = [0.0f32; 4];
        for (r, o) in out.iter_mut().enumerate() {
            *o = m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r];
        }
        out
    }
}

#[cfg(test)]
mod tests;
