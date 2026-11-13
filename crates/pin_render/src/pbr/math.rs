//! Small column-major matrix helpers and the PBR camera.
//!
//! Matrices are `[[f32; 4]; 4]` indexed `m[column][row]`, the memory layout
//! of WGSL `mat4x4<f32>`. Frames (see the module docs of [`super`]): world is
//! the table frame (z up, metres); view space is right-handed with the camera
//! looking down −Z, +Y up and +X right; clip space is wgpu's (depth 0..1).

/// A column-major 4×4 matrix (`m[column][row]`).
pub type Mat4 = [[f32; 4]; 4];

/// The identity matrix.
pub const IDENTITY: Mat4 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(v: [f32; 3]) -> Option<[f32; 3]> {
    let l = dot(v, v).sqrt();
    (l.is_finite() && l > 1e-12).then(|| [v[0] / l, v[1] / l, v[2] / l])
}

/// `a · b`.
pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [[0.0f32; 4]; 4];
    for (c, col) in out.iter_mut().enumerate() {
        for (r, o) in col.iter_mut().enumerate() {
            *o = a[0][r] * b[c][0] + a[1][r] * b[c][1] + a[2][r] * b[c][2] + a[3][r] * b[c][3];
        }
    }
    out
}

/// `m · [p, 1]`, without the perspective divide (`w` dropped; exact for
/// affine `m`).
pub fn transform_point(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    for (r, o) in out.iter_mut().enumerate() {
        *o = m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r];
    }
    out
}

/// A translation by `t`.
pub fn translation(t: [f32; 3]) -> Mat4 {
    let mut m = IDENTITY;
    m[3] = [t[0], t[1], t[2], 1.0];
    m
}

/// A uniform scale by `s` followed by a translation by `t`.
pub fn scale_translation(s: f32, t: [f32; 3]) -> Mat4 {
    [
        [s, 0.0, 0.0, 0.0],
        [0.0, s, 0.0, 0.0],
        [0.0, 0.0, s, 0.0],
        [t[0], t[1], t[2], 1.0],
    ]
}

/// The normal matrix of an affine `model`: the inverse transpose of its
/// upper 3×3 block (in a 4×4 with zero translation). A singular block gives
/// the block itself (normals are renormalised in the shader).
pub fn normal_matrix(model: &Mat4) -> Mat4 {
    let c0 = [model[0][0], model[0][1], model[0][2]];
    let c1 = [model[1][0], model[1][1], model[1][2]];
    let c2 = [model[2][0], model[2][1], model[2][2]];
    // inverse(M)^T has columns (c1×c2, c2×c0, c0×c1) / det.
    let r0 = cross(c1, c2);
    let r1 = cross(c2, c0);
    let r2 = cross(c0, c1);
    let det = dot(c0, r0);
    if !det.is_finite() || det.abs() < 1e-20 {
        let mut m = IDENTITY;
        m[0][..3].copy_from_slice(&c0);
        m[1][..3].copy_from_slice(&c1);
        m[2][..3].copy_from_slice(&c2);
        return m;
    }
    let inv = 1.0 / det;
    [
        [r0[0] * inv, r0[1] * inv, r0[2] * inv, 0.0],
        [r1[0] * inv, r1[1] * inv, r1[2] * inv, 0.0],
        [r2[0] * inv, r2[1] * inv, r2[2] * inv, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// A right-handed view matrix: the camera at `eye` looks at `target`, with
/// `up` as the approximate up direction. Degenerate inputs (eye on target,
/// `up` parallel to the view direction) fall back to a valid basis.
pub fn look_at(eye: [f32; 3], target: [f32; 3], up: [f32; 3]) -> Mat4 {
    let f = normalize(sub(target, eye)).unwrap_or([0.0, 0.0, -1.0]);
    let s = normalize(cross(f, up))
        .or_else(|| normalize(cross(f, [0.0, 1.0, 0.0])))
        .or_else(|| normalize(cross(f, [1.0, 0.0, 0.0])))
        .unwrap_or([1.0, 0.0, 0.0]);
    let u = cross(s, f);
    [
        [s[0], u[0], -f[0], 0.0],
        [s[1], u[1], -f[1], 0.0],
        [s[2], u[2], -f[2], 0.0],
        [-dot(s, eye), -dot(u, eye), dot(f, eye), 1.0],
    ]
}

/// A right-handed perspective projection onto wgpu clip space (depth 0 at
/// `near`, 1 at `far`). `fov_y` is the full vertical field of view in
/// radians; `aspect` = width / height.
pub fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1.0 / (0.5 * fov_y).tan();
    let range = far / (near - far);
    [
        [f / aspect, 0.0, 0.0, 0.0],
        [0.0, f, 0.0, 0.0],
        [0.0, 0.0, range, -1.0],
        [0.0, 0.0, near * range, 0.0],
    ]
}

/// A perspective camera for the PBR renderer.
///
/// Construct with [`PbrCamera::look_at`]; the parameters are sanitised
/// (`near` > 0, `far` > `near`, `fov_y` in `(0, π)`, `aspect` > 0) so the
/// matrices and the cluster grid built from them are always finite.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PbrCamera {
    /// Camera position (world).
    pub eye: [f32; 3],
    /// World → view.
    pub view: Mat4,
    /// View → clip.
    pub proj: Mat4,
    /// Full vertical field of view (radians).
    pub fov_y: f32,
    /// Width / height.
    pub aspect: f32,
    /// Near plane distance (m).
    pub near: f32,
    /// Far plane distance (m).
    pub far: f32,
}

impl PbrCamera {
    /// A camera at `eye` looking at `target` (`up` approximately up; world
    /// z-up scenes pass `[0, 0, 1]`).
    pub fn look_at(
        eye: [f32; 3],
        target: [f32; 3],
        up: [f32; 3],
        fov_y: f32,
        aspect: f32,
        near: f32,
        far: f32,
    ) -> PbrCamera {
        let (fov_y, aspect, near, far) = sanitize(fov_y, aspect, near, far);
        PbrCamera {
            eye,
            view: look_at(eye, target, up),
            proj: perspective(fov_y, aspect, near, far),
            fov_y,
            aspect,
            near,
            far,
        }
    }

    /// `proj · view`.
    pub fn view_proj(&self) -> Mat4 {
        mul(&self.proj, &self.view)
    }
}

/// Clamps camera parameters into the ranges the projection and the cluster
/// grid need.
pub(crate) fn sanitize(fov_y: f32, aspect: f32, near: f32, far: f32) -> (f32, f32, f32, f32) {
    let fov_y = if fov_y.is_finite() {
        fov_y.clamp(1e-3, core::f32::consts::PI - 1e-3)
    } else {
        core::f32::consts::FRAC_PI_4
    };
    let aspect = if aspect.is_finite() && aspect > 1e-6 {
        aspect
    } else {
        1.0
    };
    let near = if near.is_finite() && near > 1e-6 {
        near
    } else {
        0.01
    };
    let far = if far.is_finite() && far > near * 1.001 {
        far
    } else {
        near * 1000.0
    };
    (fov_y, aspect, near, far)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-5)
    }

    #[test]
    fn look_at_puts_target_on_negative_z() {
        let v = look_at([1.0, 2.0, 3.0], [1.0, 2.0, 0.0], [0.0, 1.0, 0.0]);
        assert!(close(transform_point(&v, [1.0, 2.0, 0.0]), [0.0, 0.0, -3.0]));
        assert!(close(transform_point(&v, [2.0, 2.0, 3.0]), [1.0, 0.0, 0.0]));
        assert!(close(transform_point(&v, [1.0, 3.0, 3.0]), [0.0, 1.0, 0.0]));
        // Degenerate up vector and eye == target stay finite.
        let d = look_at([0.0; 3], [0.0, 0.0, -1.0], [0.0, 0.0, 1.0]);
        assert!(d.iter().flatten().all(|x| x.is_finite()));
        let e = look_at([0.0; 3], [0.0; 3], [0.0, 0.0, 0.0]);
        assert!(e.iter().flatten().all(|x| x.is_finite()));
    }

    #[test]
    fn perspective_maps_near_far_to_0_1() {
        let p = perspective(1.0, 2.0, 0.5, 10.0);
        let clip = |z: f32| {
            let c: [f32; 4] = core::array::from_fn(|r| p[2][r] * z + p[3][r]);
            c[2] / c[3]
        };
        assert!((clip(-0.5) - 0.0).abs() < 1e-6);
        assert!((clip(-10.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn normal_matrix_of_scale_and_singular() {
        let m = scale_translation(2.0, [1.0, 1.0, 1.0]);
        let n = normal_matrix(&m);
        assert!((n[0][0] - 0.5).abs() < 1e-6 && n[3][0] == 0.0);
        let mut s = IDENTITY;
        s[2] = [0.0; 4];
        let n = normal_matrix(&s);
        assert!(n.iter().flatten().all(|x| x.is_finite()));
        assert_eq!(mul(&IDENTITY, &m), m);
        assert_eq!(translation([1.0, 2.0, 3.0])[3], [1.0, 2.0, 3.0, 1.0]);
    }

    #[test]
    fn camera_sanitizes() {
        let c = PbrCamera::look_at(
            [0.0, 0.0, 1.0],
            [0.0; 3],
            [0.0, 1.0, 0.0],
            f32::NAN,
            -1.0,
            0.0,
            f32::INFINITY,
        );
        assert!(c.near > 0.0 && c.far > c.near && c.aspect == 1.0);
        assert!(c.view_proj().iter().flatten().all(|x| x.is_finite()));
    }
}
