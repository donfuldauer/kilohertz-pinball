//! Subphase 5.5 spec tests (Independent Tester): the one place where the
//! render spec tests touch the `pin_render::backbox` API, included with
//! `#[path]` (an empty test crate on its own).
//!
//! BOUND TO THE REAL API (5.5-FIX1, DECISIONS "5.5 — spec adapter bound to
//! the real API"): every function below translates the assumed calls
//! described next onto `pin_render::backbox`'s real types. The original
//! header follows.
//!
//! The 5.5 brief fixes the names `pin_render::backbox::BackboxRenderer` and
//! `pin_render::backbox::ToyRig` and their behaviour (score reels: digit `d`
//! at `d·36°` modulo 360, shortest-path settle, finite bounded transforms;
//! the backbox scene composited between translite layers by layer `depth`;
//! a backglass without `[backbox]` renders byte-identically to 5.3), not
//! the signatures. The signatures below are the tester's assumption,
//! modelled on the merged 5.3 displays API (`BackglassDesc`,
//! `render_backglass_to_image`); see DECISIONS "5.5-T — Assumed backbox API
//! of the spec tests". If the implementation differs, only this file needs
//! to change.
//!
//! Assumed:
//!
//! * `ToyRig::new()`, `ToyRig::add_score_reel(&mut self) -> usize`,
//!   `set_reel_digit(&mut self, reel: usize, digit: u8)`,
//!   `step(&mut self, dt_s: f32)`, `reel_angle_deg(&self, reel) -> f32`
//!   (current angle, `[0, 360)`), `transforms(&self) -> &[[[f32; 4]; 4]]`
//!   (one column-major model matrix per toy).
//! * `BackboxDesc { camera: BackboxCamera, depth: u8, meshes:
//!   Vec<BackboxMesh>, reels: Vec<ReelDesc> }`; `BackboxMesh { positions_mm:
//!   Vec<[f32; 3]>, indices: Vec<u32>, color: [u8; 4] }` (unlit flat
//!   colour); `ReelDesc { center_mm: [f32; 3], radius_mm: f32, width_mm:
//!   f32 }` (axis along x). Translite layers with `depth <=` the backbox
//!   `depth` are behind the backbox scene, the others in front.
//! * `render_backglass_with_backbox_to_image(ctx, &BackglassDesc,
//!   Option<&BackboxDesc>, &[u8] /* reel digits */, lamps, w, h) ->
//!   Result<RgbaImage, GpuError>`, rendering settled reels.
#![allow(dead_code)]

use pin_render::backbox::{ScoreReel, ToyRig as RealRig};

/// The tester's rig: the real [`pin_render::backbox::ToyRig`] plus the digit
/// each reel is commanded to (the real rig is driven by player scores:
/// reel `i` shows the units digit of player `i`'s score).
pub struct ToyRig {
    rig: RealRig,
    scores: Vec<u64>,
}

/// Settle speed of the spec reels, degrees per second (the longest move,
/// five digits = 180 degrees, takes 0.25 s).
const SETTLE_DEG_PER_S: f32 = 720.0;

/// A rig with `n` score reels (all showing 0).
pub fn reel_rig(n: usize) -> ToyRig {
    let mut rig = RealRig::new();
    for i in 0..n {
        let idx = rig.add_reel(ScoreReel {
            meshes: Vec::new(),
            player: i,
            place: 0,
            axis: [1.0, 0.0, 0.0],
            pivot: [0.0, 0.0, 0.0],
            settle_deg_per_s: SETTLE_DEG_PER_S,
        });
        assert_eq!(idx, i);
    }
    ToyRig {
        rig,
        scores: vec![0; n],
    }
}

/// Commands reel `reel` to show `digit` (0..=9).
pub fn set_digit(r: &mut ToyRig, reel: usize, digit: u8) {
    r.scores[reel] = u64::from(digit);
    r.rig.set_scores(&r.scores);
}

/// Advances the rig by `dt` seconds.
pub fn step(r: &mut ToyRig, dt: f32) {
    r.rig.advance(dt);
}

/// Current angle of reel `reel`, degrees.
pub fn angle(r: &ToyRig, reel: usize) -> f32 {
    r.rig.reel_angle(reel)
}

/// Model matrices of every toy (column-major): the reels' transforms.
pub fn transforms(r: &ToyRig) -> Vec<[[f32; 4]; 4]> {
    (0..r.rig.reel_count())
        .map(|i| r.rig.reel_transform(i))
        .collect()
}

#[cfg(feature = "gpu-tests")]
pub mod gpu {
    use super::{RealRig, ScoreReel};
    use pin_core::schema::backglass::BackboxCamera;
    pub use pin_render::backbox::BackboxRenderer;
    use pin_render::backbox::{
        BackboxLighting, BackboxMesh as RealMesh, BackboxScene, BackboxView, ReelSpec,
        layers_behind, reel_scene as real_reel_scene, render_backglass_with_backbox_to_image,
    };
    use pin_render::displays::{BackglassDesc, RgbaImage};
    use pin_render::gpu::GpuContext;
    use pin_render::pbr::MeshGeometry;

    /// One flat-coloured mesh of a backbox scene, in the tester's frame
    /// (x right, y up, z towards the viewer, mm).
    #[derive(Clone, Debug)]
    pub struct Mesh {
        /// Positions, mm.
        pub positions_mm: Vec<[f32; 3]>,
        /// Triangle list.
        pub indices: Vec<u32>,
        /// Unlit colour.
        pub color: [u8; 4],
    }

    /// One score reel of a tester scene.
    #[derive(Clone, Copy, Debug)]
    pub struct ReelDesc {
        /// Centre, mm (tester frame).
        pub center_mm: [f32; 3],
        /// Circumradius, mm.
        pub radius_mm: f32,
        /// Width along the axis (x), mm.
        pub width_mm: f32,
    }

    /// The tester's backbox scene.
    #[derive(Clone, Debug)]
    pub struct BackboxDesc {
        /// Camera, in the real scene frame (z up).
        pub camera: BackboxCamera,
        /// Translite layers with `depth <= this` are behind the backbox.
        pub depth: u8,
        /// Flat meshes.
        pub meshes: Vec<Mesh>,
        /// Score reels.
        pub reels: Vec<ReelDesc>,
    }

    /// Tester frame (x right, y up, z to the viewer) to the real scene
    /// frame (x right, y away from the viewer, z up): `(x, y, z) ->
    /// (x, -z, y)`, a proper rotation. Millimetres to metres.
    fn to_scene_m(p: [f32; 3]) -> [f32; 3] {
        [p[0] * 1e-3, -p[2] * 1e-3, p[1] * 1e-3]
    }

    /// The camera every spec scene uses: 1 m in front of the origin (on
    /// the viewer side), looking at it, with a vertical field of view of
    /// `2·atan(0.5)` ≈ 53.13°: the plane through the origin shows 1000 mm
    /// vertically, centred on the origin. In the real scene frame the
    /// viewer is on `−y` and `z` is up.
    pub fn camera() -> BackboxCamera {
        BackboxCamera {
            position: [0.0, -1000.0, 0.0],
            look_at: [0.0, 0.0, 0.0],
            fov_deg: 2.0 * libm::atanf(0.5).to_degrees(),
        }
    }

    /// A flat quad `[x0, y0, x1, y1]` (mm, z = 0) of one unlit colour.
    pub fn quad(rect: [f32; 4], color: [u8; 4]) -> Mesh {
        let [x0, y0, x1, y1] = rect;
        Mesh {
            positions_mm: vec![[x0, y0, 0.0], [x1, y0, 0.0], [x1, y1, 0.0], [x0, y1, 0.0]],
            indices: vec![0, 1, 2, 0, 2, 3],
            color,
        }
    }

    /// A backbox scene of flat quads at `depth`.
    pub fn flat_scene(quads: Vec<Mesh>, depth: u8) -> BackboxDesc {
        BackboxDesc {
            camera: camera(),
            depth,
            meshes: quads,
            reels: Vec::new(),
        }
    }

    /// A backbox scene of `n` score reels side by side (60 mm pitch,
    /// radius 25 mm, 40 mm wide, centred on the origin) at `depth`.
    pub fn reel_scene(n: usize, depth: u8) -> BackboxDesc {
        let x0 = -30.0 * (n as f32 - 1.0);
        BackboxDesc {
            camera: camera(),
            depth,
            meshes: Vec::new(),
            reels: (0..n)
                .map(|i| ReelDesc {
                    center_mm: [x0 + 60.0 * i as f32, 0.0, 0.0],
                    radius_mm: 25.0,
                    width_mm: 40.0,
                })
                .collect(),
        }
    }

    fn real_mesh(i: usize, m: &Mesh) -> RealMesh {
        let positions: Vec<[f32; 3]> = m.positions_mm.iter().map(|&p| to_scene_m(p)).collect();
        // Flat normal of the first triangle (the colour is unlit anyway).
        let normal = match m.indices.get(0..3) {
            Some(&[a, b, c]) => {
                let (a, b, c) = (
                    positions[a as usize],
                    positions[b as usize],
                    positions[c as usize],
                );
                let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                let n = [
                    u[1] * v[2] - u[2] * v[1],
                    u[2] * v[0] - u[0] * v[2],
                    u[0] * v[1] - u[1] * v[0],
                ];
                let len = libm::sqrtf(n[0] * n[0] + n[1] * n[1] + n[2] * n[2]);
                if len > 0.0 {
                    [n[0] / len, n[1] / len, n[2] / len]
                } else {
                    [0.0, -1.0, 0.0]
                }
            }
            _ => [0.0, -1.0, 0.0],
        };
        RealMesh {
            name: format!("flat{i}"),
            geometry: MeshGeometry {
                normals: vec![normal; positions.len()],
                positions,
                indices: m.indices.clone(),
            },
            color: m.color.map(|c| f32::from(c) / 255.0),
            transform: pin_render::pbr::math::IDENTITY,
        }
    }

    /// Renders `desc` with the optional backbox scene (reels settled on
    /// `digits`) into a `w × h` `Rgba8Unorm` image.
    ///
    /// The translite layers of `desc` are taken to have depths 0, 1, 2, ...
    /// (their order); the backbox plane sits at `depth + 0.5`, so the
    /// layers with `depth <= backbox depth` are behind it (the real rule
    /// is `layers_behind`: layers with `depth < plane`). Without a
    /// backbox the real entry point is given an empty scene (which the
    /// real renderer composites to nothing).
    pub fn render(
        ctx: &GpuContext,
        desc: &BackglassDesc,
        backbox: Option<&BackboxDesc>,
        digits: &[u8],
        w: u32,
        h: u32,
    ) -> RgbaImage {
        let mut scene = BackboxScene::default();
        let mut rig = RealRig::new();
        let (camera, plane) = match backbox {
            Some(b) => {
                for (i, m) in b.meshes.iter().enumerate() {
                    scene.meshes.push(real_mesh(i, m));
                }
                for (i, r) in b.reels.iter().enumerate() {
                    let name = format!("reel{i}");
                    let center = to_scene_m(r.center_mm);
                    // Circumradius -> apothem -> face height of the decagon.
                    let apothem = r.radius_mm * 1e-3 * libm::cosf(core::f32::consts::PI / 10.0);
                    let spec = ReelSpec {
                        center,
                        face_height: 2.0 * apothem * libm::tanf(core::f32::consts::PI / 10.0),
                        width: r.width_mm * 1e-3,
                    };
                    scene.extend(real_reel_scene(&name, &spec));
                    rig.add_reel(ScoreReel {
                        meshes: vec![name.clone(), format!("{name}.digits")],
                        player: i,
                        place: 0,
                        axis: [1.0, 0.0, 0.0],
                        pivot: center,
                        settle_deg_per_s: 720.0,
                    });
                }
                (b.camera, f32::from(b.depth) + 0.5)
            }
            None => (camera(), 0.5),
        };
        let scores: Vec<u64> = digits.iter().map(|&d| u64::from(d)).collect();
        rig.set_scores(&scores);
        rig.snap_reels();
        let mut renderer =
            BackboxRenderer::new(ctx, wgpu_rgba8(), &scene, BackboxView::from_camera(&camera))
                .expect("backbox renderer");
        renderer.set_lighting(BackboxLighting::UNLIT);
        renderer.apply_toys(&rig);
        let depths: Vec<u8> = (0..desc.layers.len() as u8).collect();
        let behind = layers_behind(&depths, plane);
        render_backglass_with_backbox_to_image(ctx, desc, behind, &mut renderer, &[], w, h)
            .expect("render")
    }

    fn wgpu_rgba8() -> wgpu::TextureFormat {
        pin_render::backbox::BACKBOX_COLOR_FORMAT
    }
}
