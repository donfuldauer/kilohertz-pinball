//! Subphase 5.5 fix (PR #20 CodeRabbit threads r4576739026 and r4576739031):
//! `BackboxRenderer::encode_scene` and `encode_composite` must be correct
//! when several encodes are recorded before one queue submit. Each pass has
//! to see the draw / composite uniforms that were current when its own
//! `encode_*` call ran, not the last ones written before the submit.
//! D30: analytic pixel assertions (region means where the camera projects
//! the mesh, and the clear colour where it must not be).
#![cfg(feature = "gpu-tests")]

use pin_render::backbox::*;
use pin_render::displays::RgbaImage;
use pin_render::gpu::{GpuContext, context_for_tests, wait_for_device};
use pin_render::pbr::MeshGeometry;
use pin_render::pbr::math::{IDENTITY, translation};
use pin_render::topology::Rect;
use std::sync::OnceLock;

/// One device per test process, shared by every test (lavapipe crashes
/// with a device per test thread; see displays_gpu.rs).
fn ctx(name: &str) -> Option<&'static GpuContext> {
    static CTX: OnceLock<Option<GpuContext>> = OnceLock::new();
    let c = CTX.get_or_init(|| context_for_tests(name)).as_ref();
    if c.is_none() {
        println!("[{name}] SKIP: no GPU adapter");
    }
    c
}

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// The scene's offscreen size (square, so 1 m at the plane `y = 0` spans
/// 32 px with the 90° camera 1 m away).
const SCENE: (u32, u32) = (64, 64);
const GREEN: [f32; 4] = [0.1, 0.85, 0.2, 1.0];
/// `GREEN` in `Rgba8Unorm`, unlit.
const GREEN_U8: [u8; 3] = [26, 217, 51];
/// Destination clear colour (opaque black).
const BLACK_U8: [u8; 3] = [0, 0, 0];
/// Half size of the green square, m (about 5 px on screen).
const HALF: f32 = 0.15;

/// One green square centred on the origin in the plane `y = 0`, facing the
/// camera at `(0, −1, 0)` (90°, unlit).
fn renderer(ctx: &GpuContext) -> BackboxRenderer {
    let scene = BackboxScene {
        meshes: vec![BackboxMesh {
            name: "square".into(),
            geometry: MeshGeometry {
                positions: vec![
                    [-HALF, 0.0, -HALF],
                    [HALF, 0.0, -HALF],
                    [HALF, 0.0, HALF],
                    [-HALF, 0.0, HALF],
                ],
                normals: vec![[0.0, -1.0, 0.0]; 4],
                indices: vec![0, 1, 2, 0, 2, 3],
            },
            color: GREEN,
            transform: IDENTITY,
        }],
    };
    let view = BackboxView {
        eye: [0.0, -1.0, 0.0],
        target: [0.0, 0.0, 0.0],
        fov_y: 90f32.to_radians(),
    };
    let mut bb = BackboxRenderer::new(ctx, FORMAT, &scene, view).unwrap();
    bb.set_lighting(BackboxLighting::UNLIT);
    bb
}

/// Where the square centred at world `c` lands in the scene image.
fn scene_px(bb: &BackboxRenderer, c: [f32; 3]) -> [f32; 2] {
    bb.view().project(SCENE.0, SCENE.1, c).unwrap()
}

fn dest(ctx: &GpuContext, size: (u32, u32)) -> (wgpu::Texture, wgpu::TextureView) {
    let tex = ctx.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("submit-order dest"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    (tex, view)
}

/// Records a clear of `view` to opaque black.
fn clear(enc: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) {
    let _pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("submit-order clear"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
}

/// A recorded texture → buffer copy, read after the submit.
struct Pending {
    buf: wgpu::Buffer,
    width: u32,
    height: u32,
    padded: u32,
}

fn record_readback(
    ctx: &GpuContext,
    enc: &mut wgpu::CommandEncoder,
    tex: &wgpu::Texture,
) -> Pending {
    let size = tex.size();
    let padded = (size.width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buf = ctx.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("submit-order readback"),
        size: u64::from(padded * size.height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(size.height),
            },
        },
        size,
    );
    Pending {
        buf,
        width: size.width,
        height: size.height,
        padded,
    }
}

/// Maps every pending readback (after the one submit) into images.
fn finish(ctx: &GpuContext, pending: Vec<Pending>) -> Vec<RgbaImage> {
    for p in &pending {
        p.buf.map_async(wgpu::MapMode::Read, .., |r| r.unwrap());
    }
    wait_for_device(ctx.device(), std::time::Duration::from_secs(60)).unwrap();
    pending
        .iter()
        .map(|p| {
            let mapped = p.buf.get_mapped_range(..).unwrap();
            let row = (p.width * 4) as usize;
            let mut px = Vec::with_capacity(row * p.height as usize);
            for y in 0..p.height as usize {
                let s = y * p.padded as usize;
                px.extend_from_slice(&mapped[s..s + row]);
            }
            RgbaImage::from_rgba(p.width, p.height, px).unwrap()
        })
        .collect()
}

/// Mean RGB (0..=255) of the `2r+1` square around `(x, y)` (clipped).
fn mean(img: &RgbaImage, [x, y]: [f32; 2], r: i32) -> [f32; 3] {
    let (cx, cy) = (x.floor() as i32, y.floor() as i32);
    let mut acc = [0.0f32; 3];
    let mut n = 0.0f32;
    for dy in -r..=r {
        for dx in -r..=r {
            let (px, py) = (cx + dx, cy + dy);
            if px < 0 || py < 0 || px >= img.width as i32 || py >= img.height as i32 {
                continue;
            }
            let p = img.pixel(px as u32, py as u32);
            for k in 0..3 {
                acc[k] += f32::from(p[k]);
            }
            n += 1.0;
        }
    }
    assert!(n > 0.0, "region around ({x}, {y}) is outside the image");
    acc.map(|v| v / n)
}

fn assert_near(img: &RgbaImage, at: [f32; 2], want: [u8; 3], what: &str) {
    let m = mean(img, at, 2);
    assert!(
        (0..3).all(|k| (m[k] - f32::from(want[k])).abs() <= 6.0),
        "{what}: mean {m:?} at ({:.1}, {:.1}), want {want:?}",
        at[0],
        at[1]
    );
}

fn offset(p: [f32; 2], rect: Rect) -> [f32; 2] {
    [p[0] + rect.x as f32, p[1] + rect.y as f32]
}

#[test]
fn scene_uniforms_per_encode() {
    let Some(ctx) = ctx("scene_uniforms_per_encode") else {
        return;
    };
    let mut bb = renderer(ctx);
    let (ca, cb) = ([-0.5f32, 0.0, 0.3], [0.5f32, 0.0, -0.3]);
    let (pa, pb) = (scene_px(&bb, ca), scene_px(&bb, cb));
    // The two positions are far apart on screen (≥ 25 px each axis).
    assert!((pa[0] - pb[0]).abs() > 25.0 && (pa[1] - pb[1]).abs() > 15.0);
    let full = Rect::new(0, 0, SCENE.0, SCENE.1);
    let (tx, vx) = dest(ctx, SCENE);
    let (ty, vy) = dest(ctx, SCENE);
    let mut enc = ctx.device().create_command_encoder(&Default::default());
    clear(&mut enc, &vx);
    clear(&mut enc, &vy);
    bb.set_dynamic(0, translation(ca));
    bb.encode_scene(&mut enc, SCENE).unwrap();
    bb.encode_composite(&mut enc, &vx, SCENE, full);
    bb.set_dynamic(0, translation(cb));
    bb.encode_scene(&mut enc, SCENE).unwrap();
    bb.encode_composite(&mut enc, &vy, SCENE, full);
    let pending = vec![
        record_readback(ctx, &mut enc, &tx),
        record_readback(ctx, &mut enc, &ty),
    ];
    ctx.queue().submit([enc.finish()]);
    let imgs = finish(ctx, pending);
    let (x, y) = (&imgs[0], &imgs[1]);
    assert_near(x, pa, GREEN_U8, "pass A shows the mesh at A");
    assert_near(x, pb, BLACK_U8, "pass A does not show the mesh at B");
    assert_near(y, pb, GREEN_U8, "pass B shows the mesh at B");
    assert_near(y, pa, BLACK_U8, "pass B does not show the mesh at A");

    // Visibility changes between encodes are honoured per pass as well.
    let (tx, vx) = dest(ctx, SCENE);
    let (ty, vy) = dest(ctx, SCENE);
    let mut enc = ctx.device().create_command_encoder(&Default::default());
    clear(&mut enc, &vx);
    clear(&mut enc, &vy);
    bb.set_dynamic(0, translation(ca));
    bb.encode_scene(&mut enc, SCENE).unwrap();
    bb.encode_composite(&mut enc, &vx, SCENE, full);
    bb.set_visible(0, false);
    bb.encode_scene(&mut enc, SCENE).unwrap();
    bb.encode_composite(&mut enc, &vy, SCENE, full);
    let pending = vec![
        record_readback(ctx, &mut enc, &tx),
        record_readback(ctx, &mut enc, &ty),
    ];
    ctx.queue().submit([enc.finish()]);
    let imgs = finish(ctx, pending);
    let (x, y) = (&imgs[0], &imgs[1]);
    assert_near(x, pa, GREEN_U8, "first pass shows the visible mesh");
    assert_near(y, pa, BLACK_U8, "second pass hides the mesh");

    // A camera change between encodes: the second pass looks from the
    // other side of the scene (mirrored in x), the first keeps its view.
    let (tx, vx) = dest(ctx, SCENE);
    let (ty, vy) = dest(ctx, SCENE);
    let mut enc = ctx.device().create_command_encoder(&Default::default());
    clear(&mut enc, &vx);
    clear(&mut enc, &vy);
    bb.set_visible(0, true);
    let front = bb.view();
    bb.encode_scene(&mut enc, SCENE).unwrap();
    bb.encode_composite(&mut enc, &vx, SCENE, full);
    let back = BackboxView {
        eye: [0.0, 1.0, 0.0],
        ..front
    };
    bb.set_view(back);
    let pm = back.project(SCENE.0, SCENE.1, ca).unwrap();
    assert!((pm[0] - pa[0]).abs() > 25.0, "mirrored view moves the mesh");
    bb.encode_scene(&mut enc, SCENE).unwrap();
    bb.encode_composite(&mut enc, &vy, SCENE, full);
    let pending = vec![
        record_readback(ctx, &mut enc, &tx),
        record_readback(ctx, &mut enc, &ty),
    ];
    ctx.queue().submit([enc.finish()]);
    let imgs = finish(ctx, pending);
    let (x, y) = (&imgs[0], &imgs[1]);
    assert_near(x, pa, GREEN_U8, "front view pass shows the mesh at A");
    assert_near(x, pm, BLACK_U8, "front view pass is not mirrored");
    let m = mean(y, pm, 2);
    assert!(
        m.iter().sum::<f32>() > 30.0,
        "back view pass shows the mesh mirrored: {m:?}"
    );
    assert_near(y, pa, BLACK_U8, "back view pass has nothing at A");
}

/// Two composites of one scene into `rects` of a `size` target, one submit;
/// each must show the mesh at its own origin + `c`.
fn composite_twice(ctx: &GpuContext, size: (u32, u32), rects: [Rect; 2], c: [f32; 3]) {
    let mut bb = renderer(ctx);
    bb.set_dynamic(0, translation(c));
    let p = scene_px(&bb, c);
    let (tex, view) = dest(ctx, size);
    let mut enc = ctx.device().create_command_encoder(&Default::default());
    clear(&mut enc, &view);
    bb.encode_scene(&mut enc, SCENE).unwrap();
    for r in rects {
        bb.encode_composite(&mut enc, &view, size, r);
    }
    let pending = vec![record_readback(ctx, &mut enc, &tex)];
    ctx.queue().submit([enc.finish()]);
    let img = finish(ctx, pending).remove(0);
    for (i, r) in rects.iter().enumerate() {
        assert_near(
            &img,
            offset(p, *r),
            GREEN_U8,
            &format!("composite {i} at its own origin ({}, {})", r.x, r.y),
        );
    }
    // Away from both meshes, inside each rect: clear colour (premultiplied
    // transparent scene background leaves the destination unchanged).
    for r in rects {
        let bg = offset([p[0], p[1] + 14.0], r);
        if bg[0] >= 0.0 && bg[1] >= 0.0 && bg[0] < size.0 as f32 && bg[1] < size.1 as f32 {
            assert_near(&img, bg, BLACK_U8, "scene background is transparent");
        }
    }
}

#[test]
fn composite_rect_per_encode() {
    let Some(ctx) = ctx("composite_rect_per_encode") else {
        return;
    };
    // Mesh centred near scene pixel (40, 24).
    let c = [0.25f32, 0.0, 0.25];
    // Both rects inside the target, non-overlapping.
    composite_twice(
        ctx,
        (160, 96),
        [Rect::new(0, 0, 64, 64), Rect::new(80, 16, 64, 64)],
        c,
    );
    // Clipped: the first rect sticks out on the left, the second past the
    // right and bottom edges; the mesh stays inside both clipped areas.
    composite_twice(
        ctx,
        (160, 96),
        [Rect::new(-24, 0, 64, 64), Rect::new(100, 40, 64, 64)],
        c,
    );
}

#[test]
fn single_encode_unchanged() {
    let Some(ctx) = ctx("single_encode_unchanged") else {
        return;
    };
    let mut bb = renderer(ctx);
    let c = [-0.25f32, 0.0, -0.25];
    bb.set_dynamic(0, translation(c));
    let p = scene_px(&bb, c);
    // The scene image alone.
    let img = bb.render_scene_to_image(ctx, SCENE.0, SCENE.1).unwrap();
    let m = mean(&img, p, 2);
    assert!(
        (0..3).all(|k| (m[k] - f32::from(GREEN_U8[k])).abs() <= 6.0),
        "scene image shows the mesh: {m:?}"
    );
    assert_eq!(
        img.pixel(0, 0),
        [0, 0, 0, 0],
        "scene background transparent"
    );
    // One scene + one composite per submit, twice in a row.
    for rect in [Rect::new(0, 0, 64, 64), Rect::new(10, 6, 64, 64)] {
        let (tex, view) = dest(ctx, (96, 80));
        let mut enc = ctx.device().create_command_encoder(&Default::default());
        clear(&mut enc, &view);
        bb.encode_scene(&mut enc, SCENE).unwrap();
        bb.encode_composite(&mut enc, &view, (96, 80), rect);
        let pending = vec![record_readback(ctx, &mut enc, &tex)];
        ctx.queue().submit([enc.finish()]);
        let img = finish(ctx, pending).remove(0);
        assert_near(&img, offset(p, rect), GREEN_U8, "single composite");
        assert_near(&img, [90.0, 76.0], BLACK_U8, "outside the rect");
    }
}
