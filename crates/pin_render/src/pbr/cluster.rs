//! CPU light clustering (view-space froxels), GPU-free.
//!
//! The view frustum is cut into `dims[0] × dims[1]` screen tiles and
//! `dims[2]` depth slices spaced exponentially between the near and far
//! planes (slice `k` covers view depths `near·(far/near)^(k/Z) ..
//! near·(far/near)^((k+1)/Z)`). Each cluster gets a list of the lights whose
//! range sphere touches its view-space bounding box, so the assignment is
//! conservative: every light that can reach a point is in the list of the
//! point's cluster. The fragment shader finds its cluster from the pixel
//! position and view depth with the same formulas ([`ClusterGrid::cluster_of`]).
//!
//! Cluster index: `x + dims[0] · (y + dims[1] · z)`, tile `x = 0` on the
//! left, `y = 0` at the bottom, slice `z = 0` nearest the camera.

use super::MAX_LIGHTS;
use super::light::PointLight;
use super::math::{Mat4, PbrCamera, sanitize, transform_point};
use arrayvec::ArrayVec;

/// Default grid: 16 × 9 tiles, 24 depth slices (3 456 clusters).
pub const DEFAULT_CLUSTER_DIMS: [u32; 3] = [16, 9, 24];

/// Largest number of tiles or slices along one axis.
pub const MAX_CLUSTER_DIM: u32 = 64;

/// Relative and absolute growth of a light's sphere in the cluster test, so
/// rounding differences between the CPU boxes and the shader's cluster
/// lookup can never drop a light.
const SPHERE_SLACK_REL: f32 = 1e-3;
const SPHERE_SLACK_ABS: f32 = 1e-4;

/// The cluster layout for one perspective projection.
#[derive(Clone, Debug, PartialEq)]
pub struct ClusterGrid {
    dims: [u32; 3],
    fov_y: f32,
    aspect: f32,
    near: f32,
    far: f32,
    tan_half_y: f32,
    /// `dims[2] / ln(far / near)`.
    slice_scale: f32,
    /// View-space boxes `[min x, min y, min z, max x, max y, max z]`.
    boxes: Vec<[f32; 6]>,
}

impl ClusterGrid {
    /// A grid of `dims` (each clamped to `1..=MAX_CLUSTER_DIM`) over the
    /// perspective frustum `fov_y` (full vertical, radians), `aspect`
    /// (width / height), `near`..`far` (sanitised as by
    /// [`PbrCamera::look_at`]).
    pub fn new(dims: [u32; 3], fov_y: f32, aspect: f32, near: f32, far: f32) -> ClusterGrid {
        let dims = dims.map(|d| d.clamp(1, MAX_CLUSTER_DIM));
        let (fov_y, aspect, near, far) = sanitize(fov_y, aspect, near, far);
        let tan_half_y = (0.5 * fov_y).tan();
        let slice_scale = dims[2] as f32 / (far / near).ln();
        let mut grid = ClusterGrid {
            dims,
            fov_y,
            aspect,
            near,
            far,
            tan_half_y,
            slice_scale,
            boxes: Vec::with_capacity((dims[0] * dims[1] * dims[2]) as usize),
        };
        for z in 0..dims[2] {
            let d0 = grid.slice_near(z);
            let d1 = grid.slice_near(z + 1);
            for y in 0..dims[1] {
                let y0 = -1.0 + 2.0 * y as f32 / dims[1] as f32;
                let y1 = -1.0 + 2.0 * (y + 1) as f32 / dims[1] as f32;
                for x in 0..dims[0] {
                    let x0 = -1.0 + 2.0 * x as f32 / dims[0] as f32;
                    let x1 = -1.0 + 2.0 * (x + 1) as f32 / dims[0] as f32;
                    let sx = tan_half_y * aspect;
                    let sy = tan_half_y;
                    let xs = [x0 * d0 * sx, x0 * d1 * sx, x1 * d0 * sx, x1 * d1 * sx];
                    let ys = [y0 * d0 * sy, y0 * d1 * sy, y1 * d0 * sy, y1 * d1 * sy];
                    let min = |v: [f32; 4]| v.into_iter().fold(f32::INFINITY, f32::min);
                    let max = |v: [f32; 4]| v.into_iter().fold(f32::NEG_INFINITY, f32::max);
                    grid.boxes
                        .push([min(xs), min(ys), -d1, max(xs), max(ys), -d0]);
                }
            }
        }
        grid
    }

    /// The default grid ([`DEFAULT_CLUSTER_DIMS`]) for `camera`.
    pub fn for_camera(camera: &PbrCamera) -> ClusterGrid {
        Self::new(
            DEFAULT_CLUSTER_DIMS,
            camera.fov_y,
            camera.aspect,
            camera.near,
            camera.far,
        )
    }

    /// `true` when this grid was built for exactly these parameters.
    pub fn matches(&self, dims: [u32; 3], camera: &PbrCamera) -> bool {
        self.dims == dims.map(|d| d.clamp(1, MAX_CLUSTER_DIM))
            && self.fov_y == camera.fov_y
            && self.aspect == camera.aspect
            && self.near == camera.near
            && self.far == camera.far
    }

    /// Tiles across, tiles up, depth slices.
    pub fn dims(&self) -> [u32; 3] {
        self.dims
    }

    /// Number of clusters.
    pub fn cluster_count(&self) -> usize {
        self.boxes.len()
    }

    /// Near plane distance.
    pub fn near(&self) -> f32 {
        self.near
    }

    /// Far plane distance.
    pub fn far(&self) -> f32 {
        self.far
    }

    /// `dims[2] / ln(far / near)` (the shader's slice scale).
    pub fn slice_scale(&self) -> f32 {
        self.slice_scale
    }

    /// View depth where slice `k` begins (`k = dims[2]` gives `far`).
    pub fn slice_near(&self, k: u32) -> f32 {
        if k >= self.dims[2] {
            return self.far;
        }
        let t = f64::from(k) / f64::from(self.dims[2]);
        (f64::from(self.near) * (f64::from(self.far) / f64::from(self.near)).powf(t)) as f32
    }

    /// The depth slice of view depth `depth` (= −z), clamped to the grid.
    pub fn slice_of_depth(&self, depth: f32) -> u32 {
        let s = (depth.max(self.near) / self.near).ln() * self.slice_scale;
        if s.is_nan() {
            return 0;
        }
        (s.max(0.0) as u32).min(self.dims[2] - 1)
    }

    /// Cluster index of tile `x`, `y` and slice `z`.
    pub fn index(&self, x: u32, y: u32, z: u32) -> usize {
        (x + self.dims[0] * (y + self.dims[1] * z)) as usize
    }

    /// The cluster holding the view-space point `p`, or `None` when `p` is
    /// outside the frustum (in front of `near`, beyond `far`, or off screen).
    pub fn cluster_of(&self, p: [f32; 3]) -> Option<usize> {
        let depth = -p[2];
        if !(depth >= self.near && depth <= self.far) {
            return None;
        }
        let ndc_x = p[0] / (depth * self.tan_half_y * self.aspect);
        let ndc_y = p[1] / (depth * self.tan_half_y);
        if !(ndc_x.abs() <= 1.0 && ndc_y.abs() <= 1.0) {
            return None;
        }
        let tile = |ndc: f32, n: u32| (((ndc + 1.0) * 0.5 * n as f32) as u32).min(n - 1);
        Some(self.index(
            tile(ndc_x, self.dims[0]),
            tile(ndc_y, self.dims[1]),
            self.slice_of_depth(depth),
        ))
    }

    /// [`cluster_of`](Self::cluster_of) for a world point seen through
    /// `view`.
    pub fn cluster_of_world(&self, view: &Mat4, p: [f32; 3]) -> Option<usize> {
        self.cluster_of(transform_point(view, p))
    }

    /// The view-space bounding box `[min x, min y, min z, max x, max y,
    /// max z]` of cluster `i`.
    pub fn cluster_box(&self, i: usize) -> Option<[f32; 6]> {
        self.boxes.get(i).copied()
    }
}

/// Per-cluster light lists, reused frame to frame (allocation only when
/// the grid size or the total list length grows).
#[derive(Clone, Debug, Default)]
pub struct ClusterAssignment {
    /// `[offset, count]` into `indices` per cluster.
    ranges: Vec<[u32; 2]>,
    /// Light indices (into the light slice given to [`assign_lights`]).
    indices: Vec<u32>,
    ignored: u64,
    max_count: u32,
}

impl ClusterAssignment {
    /// An empty assignment.
    pub fn new() -> Self {
        Self::default()
    }

    /// The light indices of cluster `i` (empty for an unknown cluster).
    pub fn lights_of(&self, i: usize) -> &[u32] {
        self.ranges.get(i).map_or(&[], |&[o, c]| {
            &self.indices[o as usize..o as usize + c as usize]
        })
    }

    /// `[offset, count]` per cluster (GPU layout).
    pub fn ranges(&self) -> &[[u32; 2]] {
        &self.ranges
    }

    /// All lists, back to back (GPU layout).
    pub fn indices(&self) -> &[u32] {
        &self.indices
    }

    /// Lights past [`MAX_LIGHTS`] in the last [`assign_lights`] call (not
    /// assigned anywhere).
    pub fn ignored(&self) -> u64 {
        self.ignored
    }

    /// The longest list (never above [`MAX_LIGHTS`]).
    pub fn max_count(&self) -> u32 {
        self.max_count
    }
}

fn sphere_touches_box(c: [f32; 3], r: f32, b: &[f32; 6]) -> bool {
    let mut d2 = 0.0f32;
    for k in 0..3 {
        let v = c[k];
        let e = if v < b[k] {
            b[k] - v
        } else if v > b[k + 3] {
            v - b[k + 3]
        } else {
            0.0
        };
        d2 += e * e;
    }
    d2 <= r * r
}

/// Assigns `lights` (world space) to the clusters of `grid` seen through
/// `view` (world → view), overwriting `out`.
///
/// Only the first [`MAX_LIGHTS`] lights are considered; the rest are
/// counted in [`ClusterAssignment::ignored`]. Inactive lights (see
/// [`PointLight::is_active`]) are in no list. A light is listed in every
/// cluster whose view-space box its range sphere (grown by 0.1 % + 0.1 mm)
/// touches, so the lists are conservative; no list exceeds [`MAX_LIGHTS`].
pub fn assign_lights(
    grid: &ClusterGrid,
    view: &Mat4,
    lights: &[PointLight],
    out: &mut ClusterAssignment,
) {
    let n = lights.len().min(MAX_LIGHTS);
    out.ignored = (lights.len() - n) as u64;
    out.ranges.clear();
    out.ranges.resize(grid.cluster_count(), [0, 0]);
    out.indices.clear();
    out.max_count = 0;
    // (view centre, radius, first slice, last slice, light index)
    let mut spheres: ArrayVec<([f32; 3], f32, u32, u32, u32), MAX_LIGHTS> = ArrayVec::new();
    for (i, l) in lights[..n].iter().enumerate() {
        if !l.is_active() {
            continue;
        }
        let c = transform_point(view, l.position);
        let r = l.range * (1.0 + SPHERE_SLACK_REL) + SPHERE_SLACK_ABS;
        let (d0, d1) = (-c[2] - r, -c[2] + r);
        if !(c.iter().all(|v| v.is_finite()) && d1 >= grid.near && d0 <= grid.far) {
            continue;
        }
        spheres.push((
            c,
            r,
            grid.slice_of_depth(d0),
            grid.slice_of_depth(d1),
            i as u32,
        ));
    }
    let [nx, ny, nz] = grid.dims;
    let mut in_slice: ArrayVec<([f32; 3], f32, u32), MAX_LIGHTS> = ArrayVec::new();
    for z in 0..nz {
        in_slice.clear();
        for &(c, r, k0, k1, i) in &spheres {
            if k0 <= z && z <= k1 {
                in_slice.push((c, r, i));
            }
        }
        for y in 0..ny {
            for x in 0..nx {
                let ci = grid.index(x, y, z);
                let offset = out.indices.len() as u32;
                let b = &grid.boxes[ci];
                for &(c, r, i) in &in_slice {
                    if sphere_touches_box(c, r, b) {
                        out.indices.push(i);
                    }
                }
                let count = out.indices.len() as u32 - offset;
                out.max_count = out.max_count.max(count);
                out.ranges[ci] = [offset, count];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pbr::math::look_at;

    #[test]
    fn grid_layout() {
        let g = ClusterGrid::new([4, 2, 3], 1.0, 2.0, 0.1, 10.0);
        assert_eq!(g.cluster_count(), 24);
        assert_eq!(g.dims(), [4, 2, 3]);
        assert!((g.slice_near(0) - 0.1).abs() < 1e-6);
        assert_eq!(g.slice_near(3), 10.0);
        assert_eq!(g.slice_of_depth(0.05), 0);
        assert_eq!(g.slice_of_depth(100.0), 2);
        assert_eq!(g.slice_of_depth(f32::NAN), 0);
        assert_eq!(g.cluster_of([0.0, 0.0, -0.05]), None);
        assert_eq!(g.cluster_of([0.0, 0.0, -11.0]), None);
        assert_eq!(g.cluster_of([100.0, 0.0, -1.0]), None);
        assert!(g.cluster_of([0.0, 0.0, -1.0]).is_some());
        assert!(g.cluster_box(23).is_some() && g.cluster_box(24).is_none());
        let clamped = ClusterGrid::new([0, 1000, 1], 1.0, 1.0, 0.1, 1.0);
        assert_eq!(clamped.dims(), [1, MAX_CLUSTER_DIM, 1]);
        assert!(g.slice_scale() > 0.0 && g.near() == 0.1 && g.far() == 10.0);
    }

    #[test]
    fn point_inside_box_of_its_cluster() {
        let g = ClusterGrid::new(DEFAULT_CLUSTER_DIMS, 0.9, 1.5, 0.05, 20.0);
        for &p in &[[0.1f32, 0.2, -1.0], [-0.5, -0.3, -2.5], [3.0, -1.0, -15.0]] {
            let c = g.cluster_of(p).unwrap();
            let b = g.cluster_box(c).unwrap();
            for k in 0..3 {
                assert!(
                    p[k] >= b[k] - 1e-4 && p[k] <= b[k + 3] + 1e-4,
                    "{p:?} {b:?}"
                );
            }
        }
    }

    #[test]
    fn assignment_lists_touching_lights() {
        let cam = PbrCamera::look_at(
            [0.0, 0.0, 2.0],
            [0.0; 3],
            [0.0, 1.0, 0.0],
            1.0,
            1.0,
            0.1,
            10.0,
        );
        let g = ClusterGrid::for_camera(&cam);
        assert!(g.matches(DEFAULT_CLUSTER_DIMS, &cam));
        let mut lights = vec![PointLight::new([0.0; 3], [1.0; 3], 1.0, 0.5); MAX_LIGHTS + 3];
        lights[1].range = 0.0; // inactive
        lights[2].position = [0.0, 0.0, 100.0]; // behind the camera
        let mut out = ClusterAssignment::new();
        assign_lights(&g, &cam.view, &lights, &mut out);
        assert_eq!(out.ignored(), 3);
        assert!(out.max_count() as usize <= MAX_LIGHTS);
        let c = g.cluster_of_world(
            &look_at([0.0, 0.0, 2.0], [0.0; 3], [0.0, 1.0, 0.0]),
            [0.0; 3],
        );
        let list = out.lights_of(c.unwrap());
        assert!(list.contains(&0) && !list.contains(&1) && !list.contains(&2));
        assert_eq!(out.ranges().len(), g.cluster_count());
        assert!(out.indices().len() >= list.len());
        assert!(out.lights_of(usize::MAX).is_empty());
    }
}
