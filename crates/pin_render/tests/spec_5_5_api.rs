//! Subphase 5.5 spec tests (Independent Tester): the one place where the
//! render spec tests touch the `pin_render::backbox` API, included with
//! `#[path]` (an empty test crate on its own).
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

pub use pin_render::backbox::ToyRig;

/// A rig with `n` score reels (all showing 0).
pub fn reel_rig(n: usize) -> ToyRig {
    let mut r = ToyRig::new();
    for i in 0..n {
        assert_eq!(r.add_score_reel(), i);
    }
    r
}

/// Commands reel `reel` to show `digit` (0..=9).
pub fn set_digit(r: &mut ToyRig, reel: usize, digit: u8) {
    r.set_reel_digit(reel, digit);
}

/// Advances the rig by `dt` seconds.
pub fn step(r: &mut ToyRig, dt: f32) {
    r.step(dt);
}

/// Current angle of reel `reel`, degrees.
pub fn angle(r: &ToyRig, reel: usize) -> f32 {
    r.reel_angle_deg(reel)
}

/// Model matrices of every toy (column-major).
pub fn transforms(r: &ToyRig) -> Vec<[[f32; 4]; 4]> {
    r.transforms().to_vec()
}

#[cfg(feature = "gpu-tests")]
pub mod gpu {
    use pin_core::schema::backglass::BackboxCamera;
    pub use pin_render::backbox::BackboxRenderer;
    use pin_render::backbox::{BackboxDesc, BackboxMesh, ReelDesc};
    use pin_render::displays::{BackglassDesc, RgbaImage};
    use pin_render::gpu::GpuContext;

    /// One flat-coloured mesh of a backbox scene.
    pub type Mesh = BackboxMesh;

    /// The camera every spec scene uses: 1 m in front of the origin, on
    /// +z, looking at it, with a vertical field of view of
    /// `2·atan(0.5)` ≈ 53.13°: the plane z = 0 shows 1000 mm vertically,
    /// centred on the origin (y up).
    pub fn camera() -> BackboxCamera {
        BackboxCamera {
            position: [0.0, 0.0, 1000.0],
            look_at: [0.0, 0.0, 0.0],
            fov_deg: 2.0 * libm::atanf(0.5).to_degrees(),
        }
    }

    /// A flat quad `[x0, y0, x1, y1]` (mm, z = 0) of one unlit colour.
    pub fn quad(rect: [f32; 4], color: [u8; 4]) -> BackboxMesh {
        let [x0, y0, x1, y1] = rect;
        BackboxMesh {
            positions_mm: vec![[x0, y0, 0.0], [x1, y0, 0.0], [x1, y1, 0.0], [x0, y1, 0.0]],
            indices: vec![0, 1, 2, 0, 2, 3],
            color,
        }
    }

    /// A backbox scene of flat quads at `depth`.
    pub fn flat_scene(quads: Vec<BackboxMesh>, depth: u8) -> BackboxDesc {
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

    /// Renders `desc` with the optional backbox scene (reels settled on
    /// `digits`) into a `w × h` `Rgba8Unorm` image.
    pub fn render(
        ctx: &GpuContext,
        desc: &BackglassDesc,
        backbox: Option<&BackboxDesc>,
        digits: &[u8],
        w: u32,
        h: u32,
    ) -> RgbaImage {
        pin_render::backbox::render_backglass_with_backbox_to_image(
            ctx,
            desc,
            backbox,
            digits,
            &[],
            w,
            h,
        )
        .expect("render")
    }
}
