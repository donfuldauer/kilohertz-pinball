//! #163 gate round 12 (review 6076344934, findings 21 and 25), GPU tests of
//! the display renderers (REQ-VIEW-002, SPEC §7.1; D25 untrusted bundles):
//!
//! * finding 21: a display renderer writes and submits with the queue of the
//!   device that owns its textures, so a caller that passes another
//!   [`GpuContext`] to `set_image` / `update` neither panics wgpu nor loses
//!   the upload;
//! * finding 25: [`BackglassRenderer::needs_redraw`] keeps its decisions for
//!   repeated lamp ids and lamp-less layers, and allocates nothing on the
//!   unchanged-frame path.
//!
//! D30: analytic pixel assertions only (no snapshots). This binary holds two
//! contexts for its whole life, created one after the other on one thread
//! and never dropped (a lavapipe device created or destroyed while another
//! renders crashed the driver, DECISIONS "Wave 5 gate fix"); the tests run
//! one at a time under a lock.
#![cfg(feature = "gpu-tests")]

use pin_core::LampId;
use pin_render::displays::*;
use pin_render::gpu::{GpuContext, context_for_tests, wait_for_device};
use pin_render::topology::Rect;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::{Mutex, MutexGuard, OnceLock};

// ---------- allocation counting (this thread only) ----------

/// Counts allocations of the current thread while counting is on.
struct Counting;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
}

fn note_alloc() {
    // `try_with`: the thread-locals may already be gone at thread exit.
    let _ = COUNTING.try_with(|on| {
        if on.get() {
            let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
        }
    });
}

// SAFETY: forwards every call to `System` unchanged; the counter only reads
// and writes const-initialised thread-locals, which never allocate.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_alloc();
        // SAFETY: same contract as the caller's.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_alloc();
        // SAFETY: same contract as the caller's.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: same contract as the caller's.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_alloc();
        // SAFETY: same contract as the caller's.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Runs `f` and returns its result and the heap allocations it made on this
/// thread.
fn count_allocs<T>(f: impl FnOnce() -> T) -> (T, u64) {
    ALLOCS.with(|n| n.set(0));
    COUNTING.with(|on| on.set(true));
    let out = f();
    COUNTING.with(|on| on.set(false));
    (out, ALLOCS.with(Cell::get))
}

// ---------- contexts ----------

/// Two independent contexts for the whole binary, or `None` (skip, through
/// [`context_for_tests`]: a panic instead under `PIN_REQUIRE_GPU=1`).
fn contexts() -> Option<&'static (GpuContext, GpuContext)> {
    static CTX: OnceLock<Option<(GpuContext, GpuContext)>> = OnceLock::new();
    CTX.get_or_init(|| {
        let a = context_for_tests("displays_device_r12 owner")?;
        let b = context_for_tests("displays_device_r12 foreign")?;
        Some((a, b))
    })
    .as_ref()
}

/// Serialises the tests so only one uses the devices at a time.
fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Starts a test: the lock and both contexts, or `None` to skip.
macro_rules! setup {
    () => {{
        let guard = serial();
        match contexts() {
            Some((a, b)) => (guard, a, b),
            None => return,
        }
    }};
}

/// Pushes a validation scope on both devices, runs `f`, pops both and fails
/// on any captured validation error.
fn no_validation_errors<T>(what: &str, devices: [&wgpu::Device; 2], f: impl FnOnce() -> T) -> T {
    let scopes = devices.map(|d| d.push_error_scope(wgpu::ErrorFilter::Validation));
    let out = f();
    for (k, scope) in scopes.into_iter().enumerate().rev() {
        if let Some(e) = pollster::block_on(scope.pop()) {
            panic!("{what}: wgpu validation error on device {k}: {e}");
        }
    }
    out
}

// ---------- readback ----------

/// Copies mip 0 of the `w × h` RGBA8 `tex` (owned by `ctx`) into a buffer and
/// returns its tightly packed rows (bounded wait).
fn read_rgba8(ctx: &GpuContext, tex: &wgpu::Texture, w: u32, h: u32) -> Vec<u8> {
    let row = (w * 4) as usize;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
    let padded = row.div_ceil(align) * align;
    let device = ctx.device();
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("r12 readback"),
        size: (padded * h as usize) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
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
                bytes_per_row: Some(padded as u32),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    ctx.queue().submit([enc.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    buf.map_async(wgpu::MapMode::Read, .., move |r| {
        let _ = tx.send(r);
    });
    wait_for_device(device, std::time::Duration::from_secs(60)).expect("device poll");
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .expect("map callback")
        .expect("map ok");
    let mut px = Vec::with_capacity(row * h as usize);
    {
        let mapped = buf.get_mapped_range(..).expect("mapped range");
        for y in 0..h as usize {
            px.extend_from_slice(&mapped[y * padded..y * padded + row]);
        }
    }
    buf.unmap();
    px
}

fn texel(px: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [px[i], px[i + 1], px[i + 2], px[i + 3]]
}

fn solid(w: u32, h: u32, c: [u8; 4]) -> RgbaImage {
    RgbaImage::from_rgba(w, h, c.repeat((w * h) as usize)).unwrap()
}

/// An LCD draw list of one opaque image item `file` over the whole canvas.
fn image_list(spec: LcdSpec, file: &str) -> LcdDrawList {
    LcdDrawList {
        width: spec.width,
        height: spec.height,
        background: [0, 0, 0],
        items: vec![LcdItem::Image {
            file: file.to_owned(),
            rect: [0.0, 0.0, spec.width as f32, spec.height as f32],
            color: [255, 255, 255, 255],
        }],
    }
}

/// Opaque colours with no channel at 0 or 255, so a missing upload
/// (transparent black, or another image) cannot pass by accident.
const FIRST: [u8; 4] = [200, 40, 120, 255];
const SECOND: [u8; 4] = [30, 170, 90, 255];

// ---------- finding 21: owning queue ----------

/// REQ-VIEW-002 / D25 (#163 round 12, finding 21): a `ScoreRenderer::Lcd`
/// built on context A takes `set_image` and `update` with context B's
/// arguments: no panic, no validation error on either device, `set_image`
/// is `Ok` and marks the display changed, the image is drawn with exactly
/// its uploaded colour, and replacing it keeps the texture count (in place).
#[test]
fn req_view_002_lcd_image_upload_uses_owning_queue() {
    let (_g, a, b) = setup!();
    let devices = [a.device(), b.device()];
    let spec = LcdSpec::new(32, 18).unwrap();
    let (w, h) = (32, 18);
    let target = a.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("r12 lcd target"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut r = ScoreRenderer::new(
        a,
        wgpu::TextureFormat::Rgba8Unorm,
        ScoreDisplayConfig::Lcd(spec),
    );
    let list = DisplayContent::Lcd(image_list(spec, "img.png"));
    let draw = |r: &mut ScoreRenderer| {
        let mut enc = a.device().create_command_encoder(&Default::default());
        r.encode(&mut enc, &view, (w, h), Rect::new(0, 0, w, h));
        a.queue().submit([enc.finish()]);
        read_rgba8(a, &target, w, h)
    };
    assert!(r.update(a.queue(), &list), "first content is a change");
    assert!(!r.update(a.queue(), &list), "same content, no change");

    for (step, colour) in [("set", FIRST), ("replace", SECOND)] {
        no_validation_errors(step, devices, || {
            r.set_image(b, "img.png", &solid(4, 4, colour))
                .expect("a valid image through the foreign context is accepted");
            assert!(
                r.update(b.queue(), &list),
                "{step}: a successful set_image marks the display changed"
            );
            assert!(!r.update(b.queue(), &list), "{step}: then unchanged");
        });
        let px = no_validation_errors(step, devices, || draw(&mut r));
        for (x, y) in [(0, 0), (w / 2, h / 2), (w - 1, h - 1)] {
            assert_eq!(
                texel(&px, w, x, y),
                colour,
                "{step}: REQ-VIEW-002 texel ({x},{y}) is the uploaded image colour"
            );
        }
        if let ScoreRenderer::Lcd(l) = &r {
            assert_eq!(l.texture_count(), 2, "{step}: white + one image, in place");
        }
    }

    // A refused image through the foreign context leaves the previous one
    // and the dirty flag alone (D25).
    no_validation_errors("refused", devices, || {
        let bad = RgbaImage {
            width: 4,
            height: 4,
            pixels: vec![0; 3],
        };
        assert!(matches!(
            r.set_image(b, "img.png", &bad),
            Err(pin_render::gpu::GpuError::InvalidArgument(_))
        ));
        assert!(!r.update(b.queue(), &list), "a refused image is no change");
    });
    let px = draw(&mut r);
    assert_eq!(texel(&px, w, w / 2, h / 2), SECOND, "previous image stays");
}

/// REQ-VIEW-002 (#163 round 12, finding 21): a DMD `ScoreRenderer` built on
/// context A takes frames through context B's queue: no panic, no
/// validation error, and the lit dot centre shows the frame.
#[test]
fn req_view_002_dmd_frame_upload_uses_owning_queue() {
    let (_g, a, b) = setup!();
    let devices = [a.device(), b.device()];
    let spec = DmdSpec::new(16, 8, pin_core::schema::backglass::DmdColor::Rgb).unwrap();
    let pitch = 4;
    let (w, h) = (spec.width * pitch, spec.height * pitch);
    let mut r = ScoreRenderer::new(
        a,
        wgpu::TextureFormat::Rgba8Unorm,
        ScoreDisplayConfig::Dmd(spec),
    );
    let mut frame = DmdFrame::new(spec.width, spec.height);
    frame.set(1, 0, [255, 255, 255]);
    no_validation_errors("dmd frame", devices, || {
        assert!(r.update(b.queue(), &DisplayContent::Dmd(frame.clone())));
    });
    let target = a.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("r12 dmd target"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut enc = a.device().create_command_encoder(&Default::default());
    r.encode(&mut enc, &view, (w, h), Rect::new(0, 0, w, h));
    a.queue().submit([enc.finish()]);
    let px = read_rgba8(a, &target, w, h);
    let g = dmd_geometry(&spec, Rect::new(0, 0, w, h));
    let lit = g.dot_center(1, 0);
    let off = g.dot_center(2, 1);
    let want = dmd_dot_color(spec.color, [255, 255, 255]);
    let got = texel(&px, w, lit[0] as u32, lit[1] as u32);
    for c in 0..3 {
        assert!(
            (f32::from(got[c]) - want[c] * 255.0).abs() <= 2.0,
            "REQ-VIEW-002 lit dot centre {got:?} vs {want:?}"
        );
    }
    let dark = texel(&px, w, off[0] as u32, off[1] as u32);
    assert!(
        (0..3).all(|c| dark[c] < got[c] / 2),
        "unlit dot {dark:?} is dark against the lit one {got:?}"
    );
}

/// REQ-VIEW-002 (#163 round 12, finding 21): a `ScoreTexture` (LCD) and a
/// `BackglassTexture` built on context A take `set_image` / `update` with
/// context B: no panic, no validation error, and level 0 holds the uploaded
/// image colour (the redraw is recorded and submitted on A).
#[test]
fn req_view_002_display_textures_use_owning_queue() {
    let (_g, a, b) = setup!();
    let devices = [a.device(), b.device()];
    let spec = LcdSpec::new(32, 18).unwrap();
    let (w, h) = (32, 18);
    let mut t = ScoreTexture::new(a, ScoreDisplayConfig::Lcd(spec), w, h).unwrap();
    let list = DisplayContent::Lcd(image_list(spec, "img.png"));
    no_validation_errors("score texture", devices, || {
        t.set_image(b, "img.png", &solid(4, 4, FIRST)).unwrap();
        assert!(t.update(b, &list), "first update redraws");
        assert!(!t.update(b, &list), "unchanged content does not");
        t.set_image(b, "img.png", &solid(4, 4, SECOND)).unwrap();
        assert!(t.update(b, &list), "a new image redraws");
    });
    let px = read_rgba8(a, t.texture(), w, h);
    assert_eq!(texel(&px, w, w / 2, h / 2), SECOND, "REQ-VIEW-002 level 0");
    assert_eq!(t.redraw_count(), 2);

    let desc = BackglassDesc {
        layers: vec![BackglassLayer {
            image: solid(4, 3, FIRST),
            lamp: None,
        }],
    };
    let mut bt = BackglassTexture::new(a, &desc, 8, 6).unwrap();
    no_validation_errors("backglass texture", devices, || {
        assert!(bt.update(b, &[]), "first update redraws");
        assert!(!bt.update(b, &[]), "unchanged lamps do not");
    });
    let px = read_rgba8(a, bt.texture(), 8, 6);
    assert_eq!(texel(&px, 8, 4, 3), FIRST, "REQ-VIEW-002 translite level 0");
}

// ---------- finding 25: needs_redraw without allocation ----------

/// A `w × h` render target on `ctx`.
fn target(ctx: &GpuContext, w: u32, h: u32) -> (wgpu::Texture, wgpu::TextureView) {
    let tex = ctx.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("r12 backglass target"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    (tex, view)
}

/// REQ-VIEW-002 (#163 round 12, finding 25): layers `[plain, lamp 2,
/// lamp 0, plain, lamp 2]` depend on lamps `[2, 0, 2]` in layer order with
/// the repeat kept; after a draw, `needs_redraw` compares each referenced
/// lamp with the level it was drawn at (an order or de-duplication slip
/// would compare lamp 0 with lamp 2's level and redraw), ignores
/// unreferenced lamps, and allocates nothing on the unchanged path.
#[test]
fn req_view_002_needs_redraw_repeated_lamps_no_alloc() {
    let (_g, a, _b) = setup!();
    let plain = |c| BackglassLayer {
        image: solid(4, 3, c),
        lamp: None,
    };
    let lit = |id: u16| BackglassLayer {
        image: solid(4, 3, [255, 255, 255, 255]),
        lamp: Some(LayerLamp {
            lamp: LampId(id),
            mask: solid(4, 3, [255, 255, 255, 255]),
            blend: LayerBlend::Over,
        }),
    };
    let desc = BackglassDesc {
        layers: vec![
            plain([10, 10, 10, 255]),
            lit(2),
            lit(0),
            plain([0, 0, 0, 0]),
            lit(2),
        ],
    };
    let mut r = BackglassRenderer::new(a, wgpu::TextureFormat::Rgba8Unorm, &desc).unwrap();
    assert_eq!(r.lamps(), vec![LampId(2), LampId(0), LampId(2)]);
    let lamps = [0.5, 0.0, 0.25];
    assert!(r.needs_redraw(&lamps), "never drawn");
    let (_tex, view) = target(a, 8, 6);
    let mut enc = a.device().create_command_encoder(&Default::default());
    r.encode(&mut enc, &view, (8, 6), Rect::new(0, 0, 8, 6), &lamps);
    a.queue().submit([enc.finish()]);

    let (same, allocs) = count_allocs(|| {
        let mut any = false;
        for _ in 0..100 {
            any |= r.needs_redraw(std::hint::black_box(&lamps));
        }
        any
    });
    assert!(!same, "unchanged lamps: no redraw");
    assert_eq!(allocs, 0, "needs_redraw allocated on the unchanged path");

    assert!(
        !r.needs_redraw(&[0.5, 1.0, 0.25]),
        "lamp 1 is not referenced"
    );
    assert!(
        !r.needs_redraw(&[0.5, 0.0, 0.25, 1.0]),
        "lamp 3 is not referenced"
    );
    assert!(
        !r.needs_redraw(&[0.5 + 1.0 / 4096.0, 0.0, 0.25]),
        "sub-epsilon move of lamp 0"
    );
    assert!(r.needs_redraw(&[0.6, 0.0, 0.25]), "lamp 0 moved");
    assert!(r.needs_redraw(&[0.5, 0.0, 0.5]), "lamp 2 moved");
    assert!(r.needs_redraw(&[0.5, 0.0, 0.0]), "lamp 2 reached 0");
    assert!(r.needs_redraw(&[0.5]), "missing lamp 2 reads as 0");
    let ((), allocs) = count_allocs(|| {
        let _ = r.needs_redraw(std::hint::black_box(&[0.6, 0.0, 0.25]));
    });
    assert_eq!(allocs, 0, "needs_redraw allocated on the changed path");
    r.mark_dirty();
    assert!(r.needs_redraw(&lamps), "dirty after mark_dirty");
}
