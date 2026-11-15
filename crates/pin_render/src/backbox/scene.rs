//! GPU-free parts of the backbox pass: the scene description, its camera,
//! where it sits among the translite layers, and generated reel geometry.

use crate::pbr::MeshGeometry;
use crate::pbr::math::{IDENTITY, Mat4, PbrCamera, mul};
use pin_assetgen::fonts::FONT_5X7;
use pin_core::schema::backglass::{BackboxCamera, BackglassManifest};

/// The backbox plane's position among the translite layer depths when
/// `backglass.toml` does not say: between depth 0 (the background, drawn
/// behind the backbox) and depth 1 (frame and logo, drawn over it). See
/// DECISIONS.md "5.5 — Backbox depth compositing".
pub const DEFAULT_PLANE_DEPTH: f32 = 0.5;

/// Near clip plane of the backbox camera, m.
pub const BACKBOX_NEAR_M: f32 = 0.01;
/// Far clip plane of the backbox camera, m.
pub const BACKBOX_FAR_M: f32 = 20.0;

/// Number of translite layers (of `depths`, back to front, i.e. sorted
/// ascending) drawn **behind** a backbox at `plane_depth`: those with
/// `depth < plane_depth`. A NaN plane puts the backbox in front of every
/// layer.
pub fn layers_behind(depths: &[u8], plane_depth: f32) -> usize {
    if plane_depth.is_nan() {
        return depths.len();
    }
    depths
        .iter()
        .filter(|&&d| f32::from(d) < plane_depth)
        .count()
}

/// [`layers_behind`] for a manifest's translite (its layers in the
/// renderer's back-to-front order: stable-sorted by `depth`).
pub fn manifest_layers_behind(m: &BackglassManifest, plane_depth: f32) -> usize {
    let mut depths: Vec<u8> = m.translite.layers.iter().map(|l| l.depth).collect();
    depths.sort_unstable();
    layers_behind(&depths, plane_depth)
}

/// The backbox camera in SI: eye, target, vertical field of view. Its frame
/// is the backbox scene's (z up).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BackboxView {
    /// Eye, m.
    pub eye: [f32; 3],
    /// Target, m.
    pub target: [f32; 3],
    /// Vertical field of view, radians.
    pub fov_y: f32,
}

impl BackboxView {
    /// From `[backbox] camera` (mm, degrees).
    pub fn from_camera(c: &BackboxCamera) -> BackboxView {
        let m = |v: [f32; 3]| [v[0] * 1e-3, v[1] * 1e-3, v[2] * 1e-3];
        BackboxView {
            eye: m(c.position),
            target: m(c.look_at),
            fov_y: c.fov_deg.to_radians(),
        }
    }

    /// The camera for a target of `aspect` = width / height.
    pub fn camera(&self, aspect: f32) -> PbrCamera {
        PbrCamera::look_at(
            self.eye,
            self.target,
            [0.0, 0.0, 1.0],
            self.fov_y,
            aspect,
            BACKBOX_NEAR_M,
            BACKBOX_FAR_M,
        )
    }

    /// Where `p` (scene frame, m) lands in a target of `width × height`
    /// pixels: `(x, y)` from the top-left, or `None` behind the camera.
    pub fn project(&self, width: u32, height: u32, p: [f32; 3]) -> Option<[f32; 2]> {
        let cam = self.camera(width as f32 / height.max(1) as f32);
        let vp = cam.view_proj();
        let clip: [f32; 4] = core::array::from_fn(|r| {
            vp[0][r] * p[0] + vp[1][r] * p[1] + vp[2][r] * p[2] + vp[3][r]
        });
        // Behind the eye (or NaN): no screen position.
        if clip[3].is_nan() || clip[3] <= 1e-6 {
            return None;
        }
        let (x, y) = (clip[0] / clip[3], clip[1] / clip[3]);
        Some([
            (x * 0.5 + 0.5) * width as f32,
            (0.5 - y * 0.5) * height as f32,
        ])
    }
}

/// One mesh of the backbox scene.
#[derive(Clone, Debug, PartialEq)]
pub struct BackboxMesh {
    /// Node name (toys and dynamic transforms address meshes by name).
    pub name: String,
    /// Object-space geometry, metres.
    pub geometry: MeshGeometry,
    /// Display-encoded (sRGB) RGBA colour, 0..=1; alpha is ignored (the
    /// backbox scene is opaque).
    pub color: [f32; 4],
    /// Object → scene transform (column-major).
    pub transform: Mat4,
}

/// The backbox scene: meshes in the backbox frame (z up, metres).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BackboxScene {
    /// Meshes, drawn in order (depth-tested).
    pub meshes: Vec<BackboxMesh>,
}

/// Linear → sRGB-encoded channel.
pub fn linear_to_srgb(c: f32) -> f32 {
    let c = if c.is_finite() {
        c.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * libm::powf(c, 1.0 / 2.4) - 0.055
    }
}

impl BackboxScene {
    /// The scene of a loaded glTF (`[backbox] scene`): one mesh per
    /// primitive with its node's world transform and its material's base
    /// colour factor (linear in glTF, encoded to sRGB here); meshes without
    /// a material are light grey. Textures are not used.
    pub fn from_gltf(scene: &pin_assets::GltfScene) -> BackboxScene {
        let meshes = scene
            .meshes
            .iter()
            .map(|m| {
                let color = m.material.and_then(|i| scene.materials.get(i)).map_or(
                    [0.8, 0.8, 0.8, 1.0],
                    |mat| {
                        let c = mat.base_color;
                        [
                            linear_to_srgb(c[0]),
                            linear_to_srgb(c[1]),
                            linear_to_srgb(c[2]),
                            1.0,
                        ]
                    },
                );
                BackboxMesh {
                    name: m.name.clone(),
                    geometry: MeshGeometry {
                        positions: m.positions.clone(),
                        normals: m.normals.clone(),
                        indices: m.indices.clone(),
                    },
                    color,
                    transform: m.transform,
                }
            })
            .collect();
        BackboxScene { meshes }
    }

    /// Indices of the meshes named `name`.
    pub fn indices_of<'a>(&'a self, name: &'a str) -> impl Iterator<Item = usize> + 'a {
        self.meshes
            .iter()
            .enumerate()
            .filter(move |(_, m)| m.name == name)
            .map(|(i, _)| i)
    }

    /// Appends `other`'s meshes.
    pub fn extend(&mut self, other: BackboxScene) {
        self.meshes.extend(other.meshes);
    }
}

/// A score reel drum for [`reel_scene`]: a ten-sided prism along the scene
/// `+x` axis whose digit `d` faces the `−y` direction (towards a camera on
/// the `−y` side, `z` up) when the reel is turned by `d · 36°` about `+x`
/// (see [`super::toys::reel_angle_deg`]).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ReelSpec {
    /// Drum centre, m.
    pub center: [f32; 3],
    /// Height of one digit face, m.
    pub face_height: f32,
    /// Drum width along its axis, m.
    pub width: f32,
}

/// Drum colour (display-encoded).
pub const REEL_DRUM_COLOR: [f32; 4] = [0.93, 0.90, 0.80, 1.0];
/// Digit colour (display-encoded).
pub const REEL_DIGIT_COLOR: [f32; 4] = [0.08, 0.06, 0.05, 1.0];

impl ReelSpec {
    /// Distance from the axis to a face, m.
    pub fn apothem(&self) -> f32 {
        0.5 * self.face_height / libm::tanf(core::f32::consts::PI / 10.0)
    }

    /// Side of one glyph cell, m (a 5 × 7 glyph fills 80 % of the face
    /// height).
    pub fn cell(&self) -> f32 {
        0.8 * self.face_height / 7.0
    }

    /// Centre (scene frame, reel at angle 0) of glyph cell `(col, row)`
    /// (row 0 at the top) of digit `digit`'s face.
    pub fn cell_center(&self, digit: u32, col: u32, row: u32) -> [f32; 3] {
        let phi = -((digit % 10) as f32) * 36.0f32.to_radians();
        let cell = self.cell();
        let local = [
            (col as f32 - 2.0) * cell,
            -self.apothem() - self.glyph_lift(),
            (3.0 - row as f32) * cell,
        ];
        let p = rot_x(phi, local);
        [
            self.center[0] + p[0],
            self.center[1] + p[1],
            self.center[2] + p[2],
        ]
    }

    fn glyph_lift(&self) -> f32 {
        0.01 * self.face_height
    }
}

fn rot_x(phi: f32, p: [f32; 3]) -> [f32; 3] {
    let (s, c) = (libm::sinf(phi), libm::cosf(phi));
    [p[0], p[1] * c - p[2] * s, p[1] * s + p[2] * c]
}

fn push_quad(m: &mut MeshGeometry, q: [[f32; 3]; 4], n: [f32; 3]) {
    let base = m.positions.len() as u32;
    for p in q {
        m.positions.push(p);
        m.normals.push(n);
    }
    m.indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

/// The meshes of one reel (at angle 0): the drum (`<name>`) and its digits
/// (`<name>.digits`, the lit cells of the kit's 5 × 7 font, raised slightly
/// off each face). Both turn together; give both names to the
/// [`super::toys::ScoreReel`].
pub fn reel_scene(name: &str, spec: &ReelSpec) -> BackboxScene {
    let mut drum = MeshGeometry::default();
    let mut digits = MeshGeometry::default();
    let a = spec.apothem();
    let (hh, hw) = (0.5 * spec.face_height, 0.5 * spec.width);
    let at = |phi: f32, p: [f32; 3]| {
        let r = rot_x(phi, p);
        [
            spec.center[0] + r[0],
            spec.center[1] + r[1],
            spec.center[2] + r[2],
        ]
    };
    let cell = spec.cell();
    let pad = 0.06 * cell;
    for d in 0..10u32 {
        let phi = -(d as f32) * 36.0f32.to_radians();
        let n = rot_x(phi, [0.0, -1.0, 0.0]);
        // Face: counter-clockwise seen from −y (x right, z up).
        push_quad(
            &mut drum,
            [
                at(phi, [-hw, -a, -hh]),
                at(phi, [hw, -a, -hh]),
                at(phi, [hw, -a, hh]),
                at(phi, [-hw, -a, hh]),
            ],
            n,
        );
        let glyph = FONT_5X7.glyph_or_fallback(char::from(b'0' + d as u8));
        let y = -a - spec.glyph_lift();
        for row in 0..7u32 {
            for col in 0..5u32 {
                if !glyph.pixel(col, row) {
                    continue;
                }
                let (cx, cz) = ((col as f32 - 2.0) * cell, (3.0 - row as f32) * cell);
                let h = 0.5 * cell - pad;
                push_quad(
                    &mut digits,
                    [
                        at(phi, [cx - h, y, cz - h]),
                        at(phi, [cx + h, y, cz - h]),
                        at(phi, [cx + h, y, cz + h]),
                        at(phi, [cx - h, y, cz + h]),
                    ],
                    n,
                );
            }
        }
    }
    // End caps (decagons) so the drum reads as solid from an angle.
    let r = a / libm::cosf(core::f32::consts::PI / 10.0);
    for (x, nx) in [(-hw, -1.0f32), (hw, 1.0)] {
        let base = drum.positions.len() as u32;
        drum.positions.push(at(0.0, [x, 0.0, 0.0]));
        drum.normals.push([nx, 0.0, 0.0]);
        for k in 0..10 {
            // Corners between faces k and k+1 (at −(k + 0.5)·36°).
            let phi = -(k as f32 + 0.5) * 36.0f32.to_radians();
            drum.positions.push(at(phi, [x, -r, 0.0]));
            drum.normals.push([nx, 0.0, 0.0]);
        }
        for k in 0..10u32 {
            let (i, j) = (base + 1 + k, base + 1 + (k + 1) % 10);
            // The corners run clockwise seen from +x.
            if nx > 0.0 {
                drum.indices.extend_from_slice(&[base, j, i]);
            } else {
                drum.indices.extend_from_slice(&[base, i, j]);
            }
        }
    }
    BackboxScene {
        meshes: vec![
            BackboxMesh {
                name: name.to_owned(),
                geometry: drum,
                color: REEL_DRUM_COLOR,
                transform: IDENTITY,
            },
            BackboxMesh {
                name: format!("{name}.digits"),
                geometry: digits,
                color: REEL_DIGIT_COLOR,
                transform: IDENTITY,
            },
        ],
    }
}

/// `dynamic · base`: a toy's transform applied on top of the mesh's own.
pub fn compose(dynamic: &Mat4, base: &Mat4) -> Mat4 {
    mul(dynamic, base)
}
