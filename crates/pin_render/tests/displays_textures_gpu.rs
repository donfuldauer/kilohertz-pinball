//! KP-017 Independent Tester, GPU tests of the display textures
//! (REQ-VIEW-002, SPEC §7.1): the translite and the score display drawn into
//! sampleable `Rgba8Unorm` textures with a mip chain. D30: analytic and
//! oracle-based assertions only (no new snapshots); mip 0, mip 1 and the last
//! mip are written to `$CARGO_TARGET_TMPDIR/kp017_images/` for the verifier.
#![cfg(feature = "gpu-tests")]

use pin_core::LampId;
use pin_core::schema::backglass::DmdColor;
use pin_render::displays::*;
use pin_render::gpu::{GpuContext, GpuError, context_for_tests, wait_for_device};
use pin_render::topology::Rect;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// One device for the whole binary (CI hazard #209).
fn ctx(name: &str) -> Option<&'static GpuContext> {
    static CTX: OnceLock<Option<GpuContext>> = OnceLock::new();
    CTX.get_or_init(|| context_for_tests(name)).as_ref()
}

/// Serialises the tests so only one uses the device at a time.
fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Starts a test: the lock and the shared context, or `None` to skip.
macro_rules! setup {
    ($name:expr) => {{
        let guard = serial();
        match ctx($name) {
            Some(c) => (guard, c),
            None => return,
        }
    }};
}

// ---------- readback and image helpers ----------

#[derive(Clone)]
struct Img {
    w: u32,
    h: u32,
    px: Vec<u8>,
}

impl Img {
    fn at(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.w + x) * 4) as usize;
        [self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3]]
    }
}

fn from_rgba(i: &RgbaImage) -> Img {
    Img {
        w: i.width,
        h: i.height,
        px: i.pixels.clone(),
    }
}

fn level_size(w: u32, h: u32, k: u32) -> (u32, u32) {
    ((w >> k).max(1), (h >> k).max(1))
}

/// Copies mip `level` of `tex` into a buffer and reads it (bounded wait).
fn read_level(ctx: &GpuContext, tex: &wgpu::Texture, w: u32, h: u32, level: u32) -> Img {
    let (lw, lh) = level_size(w, h, level);
    let row = (lw * 4) as usize;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
    let padded = row.div_ceil(align) * align;
    let device = ctx.device();
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("kp017 readback"),
        size: (padded * lh as usize) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: level,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded as u32),
                rows_per_image: Some(lh),
            },
        },
        wgpu::Extent3d {
            width: lw,
            height: lh,
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
    let mut px = Vec::with_capacity(row * lh as usize);
    {
        let mapped = buf.get_mapped_range(..).expect("mapped range");
        for y in 0..lh as usize {
            px.extend_from_slice(&mapped[y * padded..y * padded + row]);
        }
    }
    buf.unmap();
    drop(buf);
    Img { w: lw, h: lh, px }
}

/// Mean per channel over `[x0,x1) x [y0,y1)`.
fn mean(img: &Img, x0: u32, y0: u32, x1: u32, y1: u32) -> [f64; 4] {
    let mut s = [0f64; 4];
    for y in y0..y1 {
        for x in x0..x1 {
            let p = img.at(x, y);
            for c in 0..4 {
                s[c] += f64::from(p[c]);
            }
        }
    }
    let n = f64::from((x1 - x0) * (y1 - y0));
    s.map(|v| v / n)
}

fn mean_all(img: &Img) -> [f64; 4] {
    mean(img, 0, 0, img.w, img.h)
}

fn max_diff4(a: [f64; 4], b: [f64; 4]) -> f64 {
    (0..4).map(|i| (a[i] - b[i]).abs()).fold(0.0, f64::max)
}

/// Whole image and the 2x2 quadrant grid, as fractions of the size.
fn regions(img: &Img) -> Vec<(u32, u32, u32, u32)> {
    let (w, h) = (img.w, img.h);
    vec![
        (0, 0, w, h),
        (0, 0, w / 2, h / 2),
        (w / 2, 0, w, h / 2),
        (0, h / 2, w / 2, h),
        (w / 2, h / 2, w, h),
    ]
}

/// Level 0 equals the oracle: within 1/255 per channel, at most 0.5 % of
/// channels may differ by up to 2.
fn assert_matches_oracle(what: &str, got: &Img, want: &RgbaImage) {
    assert_eq!((got.w, got.h), (want.width, want.height), "{what} size");
    let (mut off1, mut worst) = (0usize, 0u8);
    for (a, b) in got.px.iter().zip(&want.pixels) {
        let d = a.abs_diff(*b);
        worst = worst.max(d);
        off1 += usize::from(d > 1);
    }
    assert!(
        worst <= 2 && off1 as f64 <= 0.005 * got.px.len() as f64,
        "{what}: REQ-VIEW-002 level 0 vs existing renderer: worst {worst}, {off1} channels > 1"
    );
}

fn out_dir() -> std::path::PathBuf {
    let d = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("kp017_images");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn save(name: &str, img: &Img) {
    image::save_buffer(
        out_dir().join(format!("{name}.png")),
        &img.px,
        img.w,
        img.h,
        image::ColorType::Rgba8,
    )
    .unwrap();
}

/// Writes mip 0, mip 1 and the last mip of `tex` as PNGs.
fn save_mips(ctx: &GpuContext, name: &str, tex: &wgpu::Texture, w: u32, h: u32, levels: u32) {
    for (tag, k) in [("mip0", 0), ("mip1", 1), ("mipLast", levels - 1)] {
        save(&format!("{name}_{tag}"), &read_level(ctx, tex, w, h, k));
    }
}

/// Reads every level of `tex`.
fn read_all(ctx: &GpuContext, tex: &wgpu::Texture, w: u32, h: u32, levels: u32) -> Vec<Img> {
    (0..levels).map(|k| read_level(ctx, tex, w, h, k)).collect()
}

/// Level k+1 is the 2x2 box average of level k (round to nearest), within 1.
/// The check is per level: each level is rounded to nearest on its own, so
/// level k+1 is compared with level k as stored, never with level 0.
fn assert_box_chain(what: &str, levels: &[Img]) {
    for k in 1..levels.len() {
        let (src, dst) = (&levels[k - 1], &levels[k]);
        assert_eq!(
            (dst.w, dst.h),
            ((src.w / 2).max(1), (src.h / 2).max(1)),
            "{what} level {k} size"
        );
        // Only exact 2:1 reductions have a well-defined box.
        if src.w != dst.w * 2 || src.h != dst.h * 2 {
            continue;
        }
        let mut bad = 0usize;
        for y in 0..dst.h {
            for x in 0..dst.w {
                let q = [
                    src.at(2 * x, 2 * y),
                    src.at(2 * x + 1, 2 * y),
                    src.at(2 * x, 2 * y + 1),
                    src.at(2 * x + 1, 2 * y + 1),
                ];
                let p = dst.at(x, y);
                for c in 0..4 {
                    let sum: u32 = q.iter().map(|t| u32::from(t[c])).sum();
                    let want = (f64::from(sum) / 4.0).round();
                    if (f64::from(p[c]) - want).abs() > 1.0 {
                        bad += 1;
                    }
                }
            }
        }
        assert_eq!(
            bad,
            0,
            "{what}: REQ-VIEW-002 level {k} is not the box average of level {}",
            k - 1
        );
    }
}

/// Mean of each level over the whole image and each quadrant is within 2/255
/// of level 0's; the last level is 1x1 and within 3/255 of the global mean.
/// Each level is rounded to nearest independently, so the 1x1 level is the
/// area-weighted average computed level by level: it may differ from the
/// global mean rounded once, by at most half a byte per reduction (so at
/// most half a byte times the number of reductions). The 3/255 tolerance is
/// what these contents need, not that worst case; the exact per-level
/// checks are `assert_box_chain` and `assert_chain_is_oracle`.
fn assert_mean_chain(what: &str, levels: &[Img]) {
    let base = &levels[0];
    let last = levels.last().unwrap();
    assert_eq!((last.w, last.h), (1, 1), "{what} last level is 1x1");
    for (k, lv) in levels.iter().enumerate().skip(1) {
        for ((x0, y0, x1, y1), (lx0, ly0, lx1, ly1)) in regions(base).into_iter().zip(regions(lv)) {
            if lx1 <= lx0
                || ly1 <= ly0
                || (lv.w < 2 || lv.h < 2) && (lx0, ly0, lx1, ly1) != (0, 0, lv.w, lv.h)
            {
                continue; // the quadrant vanished at this level
            }
            if (lv.w < 2 || lv.h < 2) && (x0, y0, x1, y1) != (0, 0, base.w, base.h) {
                continue; // quadrants are only defined while both sides are at least 2
            }
            let a = mean(base, x0, y0, x1, y1);
            let b = mean(lv, lx0, ly0, lx1, ly1);
            assert!(
                max_diff4(a, b) <= 2.0,
                "{what}: REQ-VIEW-002 level {k} region mean {b:?} vs level 0 {a:?}"
            );
        }
    }
    assert!(
        max_diff4(mean_all(base), mean_all(last)) <= 3.0,
        "{what}: REQ-VIEW-002 last level {:?} vs global mean {:?}",
        mean_all(last),
        mean_all(base)
    );
}

// ---------- content builders ----------

fn dmd_frame(spec: &DmdSpec, lit: bool) -> DmdFrame {
    let mut f = DmdFrame::new(spec.width, spec.height);
    if lit {
        for y in 0..spec.height {
            for x in 0..spec.width {
                f.set(x, y, [255; 3]);
            }
        }
    }
    f
}

fn solid(w: u32, h: u32, c: [u8; 4]) -> RgbaImage {
    RgbaImage::from_rgba(w, h, c.repeat((w * h) as usize)).unwrap()
}

/// Opaque mask, white where `x < x1` (left part) and black elsewhere.
fn left_mask(w: u32, h: u32, x1: u32) -> RgbaImage {
    let data = (0..w * h)
        .map(|i| if i % w < x1 { 255 } else { 0 })
        .collect();
    RgbaImage::from_gray(w, h, data).unwrap()
}

const BASE: [u8; 4] = [40, 60, 80, 255];
const LIT: [u8; 4] = [220, 200, 40, 255];
const FLASH: [u8; 4] = [100, 80, 60, 255];

fn lamp_desc(blend: LayerBlend, color: [u8; 4]) -> BackglassDesc {
    BackglassDesc {
        layers: vec![
            BackglassLayer {
                image: solid(256, 256, BASE),
                lamp: None,
            },
            BackglassLayer {
                image: solid(256, 256, color),
                lamp: Some(LayerLamp {
                    lamp: LampId(1),
                    mask: left_mask(256, 256, 128),
                    blend,
                }),
            },
        ],
    }
}

fn lcd_content(score: i64) -> (LcdSpec, LcdDrawList) {
    let spec = LcdSpec::new(256, 144).unwrap();
    let mut model =
        ScoreDisplayModel::new(ScoreDisplayConfig::Lcd(spec), SceneLibrary::placeholders());
    model.scores_mut().scores[0] = score;
    model.scores_mut().ball = 2;
    let DisplayContent::Lcd(list) = model.compose(0) else {
        panic!("LCD config composes an LCD draw list");
    };
    (spec, list)
}

fn lcd_oracle(ctx: &GpuContext, spec: LcdSpec, list: &LcdDrawList, w: u32, h: u32) -> RgbaImage {
    render_lcd_to_image(ctx, spec, list, &ImageSet::new(), w, h).unwrap()
}

// ---------- 1. DMD: change only touches the texture ----------

/// REQ-VIEW-002 (acceptance 1): all lit vs all off changes only the texture
/// and its region means match the dot texture within 1/255.
#[test]
fn req_view_002_dmd_all_lit_vs_off_region_means() {
    let (_g, ctx) = setup!("req_view_002_dmd_all_lit_vs_off_region_means");
    let (w, h) = (512, 128);
    let color = DmdColor::Orange;
    let spec = DmdSpec::new(128, 32, color).unwrap();
    let mut t = ScoreTexture::new(ctx, ScoreDisplayConfig::Dmd(spec), w, h).unwrap();
    let id = t.texture().clone();
    assert_eq!(t.redraw_count(), 0);

    let off = dmd_frame(&spec, false);
    let lit = dmd_frame(&spec, true);
    assert!(t.update(ctx, &DisplayContent::Dmd(off.clone())));
    let (n_before, levels) = (t.redraw_count(), t.mip_level_count());
    let a = read_level(ctx, t.texture(), w, h, 0);
    assert!(t.update(ctx, &DisplayContent::Dmd(lit.clone())));
    assert_eq!(t.redraw_count(), n_before + 1);
    assert!(*t.texture() == id, "texture object is stable");
    assert_eq!(t.size(), (w, h));
    assert_eq!(t.mip_level_count(), levels);
    let b = read_level(ctx, t.texture(), w, h, 0);
    assert_ne!(a.px, b.px, "lit differs from off");

    // Region means against the dot texture (existing renderer).
    for (name, frame, got) in [("off", &off, &a), ("lit", &lit, &b)] {
        let want = from_rgba(&render_dmd_to_image(ctx, spec, frame, w, h).unwrap());
        for (x0, y0, x1, y1) in regions(got) {
            let m = mean(got, x0, y0, x1, y1);
            let o = mean(&want, x0, y0, x1, y1);
            assert!(
                max_diff4(m, o) <= 1.0,
                "REQ-VIEW-002 DMD {name} region ({x0},{y0},{x1},{y1}) mean {m:?} vs dot texture {o:?}"
            );
        }
    }
    assert!(
        max_diff4(mean_all(&a), mean_all(&b)) > 10.0,
        "all lit is clearly brighter than all off"
    );

    // Analytic points.
    let g = dmd_geometry(&spec, Rect::new(0, 0, w, h));
    let [cx, cy] = g.dot_center(0, 0);
    let (px, py) = (cx.floor() as u32, cy.floor() as u32);
    let want_lit = dmd_dot_color(color, [255; 3]);
    let want_off = dmd_dot_color(color, [0; 3]);
    let (pl, pu) = (b.at(px, py), a.at(px, py));
    for c in 0..3 {
        assert!(
            (f32::from(pl[c]) - want_lit[c] * 255.0).abs() <= 2.0,
            "REQ-VIEW-002 lit centre {pl:?} vs {want_lit:?}"
        );
        assert!(
            (f32::from(pu[c]) - want_off[c] * 255.0).abs() <= 2.0,
            "REQ-VIEW-002 unlit centre {pu:?} vs {want_off:?}"
        );
    }
    let luma =
        |p: [u8; 4]| 0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2]);
    assert!(luma(pl) - luma(pu) >= 40.0, "lit centre much brighter");
    assert!(
        luma(pu) > 0.0,
        "unlit dot is faintly visible (DMD_UNLIT_LEVEL > 0)"
    );
    // Gap pixel at the cell corner is the background.
    let gx = (cx - 0.5 * g.pitch + 0.5).floor() as u32;
    let gy = (cy - 0.5 * g.pitch + 0.5).floor() as u32;
    for img in [&a, &b] {
        let p = img.at(gx, gy);
        for c in 0..3 {
            assert!(
                (f32::from(p[c]) - DMD_BACKGROUND[c] * 255.0).abs() <= 2.0,
                "REQ-VIEW-002 gap pixel {p:?}"
            );
        }
    }
}

// ---------- 2. Redraw only on change ----------

/// REQ-VIEW-002: a DMD texture redraws only when the frame changes.
#[test]
fn req_view_002_dmd_redraw_only_on_change() {
    let (_g, ctx) = setup!("req_view_002_dmd_redraw_only_on_change");
    let spec = DmdSpec::new(128, 32, DmdColor::Orange).unwrap();
    let mut t = ScoreTexture::new(ctx, ScoreDisplayConfig::Dmd(spec), 256, 64).unwrap();
    let mut frame = dmd_frame(&spec, false);
    assert!(
        t.update(ctx, &DisplayContent::Dmd(frame.clone())),
        "first draw"
    );
    let n = t.redraw_count();
    assert_eq!(n, 1);
    for _ in 0..2 {
        assert!(
            !t.update(ctx, &DisplayContent::Dmd(frame.clone())),
            "equal frame"
        );
        assert_eq!(t.redraw_count(), n);
    }
    frame.set(5, 5, [255; 3]);
    assert!(
        t.update(ctx, &DisplayContent::Dmd(frame.clone())),
        "one dot changed"
    );
    assert_eq!(t.redraw_count(), n + 1);
    // The other mode's content is ignored.
    let (_, list) = lcd_content(1);
    assert!(!t.update(ctx, &DisplayContent::Lcd(list)));
    assert_eq!(t.redraw_count(), n + 1);
    // mark_dirty forces one redraw even with equal content.
    t.mark_dirty();
    assert!(t.update(ctx, &DisplayContent::Dmd(frame.clone())));
    assert_eq!(t.redraw_count(), n + 2);
    assert!(!t.update(ctx, &DisplayContent::Dmd(frame)));
    assert_eq!(t.redraw_count(), n + 2);
}

/// REQ-VIEW-002: an LCD texture redraws only when the draw list changes.
#[test]
fn req_view_002_lcd_redraw_only_on_change() {
    let (_g, ctx) = setup!("req_view_002_lcd_redraw_only_on_change");
    let (spec, list) = lcd_content(1_234_567);
    let mut t = ScoreTexture::new(ctx, ScoreDisplayConfig::Lcd(spec), 256, 128).unwrap();
    assert!(
        t.update(ctx, &DisplayContent::Lcd(list.clone())),
        "first draw"
    );
    let n = t.redraw_count();
    assert_eq!(n, 1);
    for _ in 0..2 {
        assert!(
            !t.update(ctx, &DisplayContent::Lcd(list.clone())),
            "equal list"
        );
        assert_eq!(t.redraw_count(), n);
    }
    let (_, other) = lcd_content(7_654_321);
    assert_ne!(list, other);
    assert!(
        t.update(ctx, &DisplayContent::Lcd(other.clone())),
        "changed list"
    );
    assert_eq!(t.redraw_count(), n + 1);
    // DMD content on an LCD texture is ignored.
    let dspec = DmdSpec::new(128, 32, DmdColor::Orange).unwrap();
    assert!(!t.update(ctx, &DisplayContent::Dmd(dmd_frame(&dspec, true))));
    assert_eq!(t.redraw_count(), n + 1);
    t.mark_dirty();
    assert!(t.update(ctx, &DisplayContent::Lcd(other.clone())));
    assert_eq!(t.redraw_count(), n + 2);
    assert!(!t.update(ctx, &DisplayContent::Lcd(other)));
}

// ---------- 3. Translite lamp toggle ----------

/// REQ-VIEW-002 (acceptance 2): a lamp toggle changes only its mask region.
#[test]
fn req_view_002_translite_lamp_toggle_only_mask_region() {
    let (_g, ctx) = setup!("req_view_002_translite_lamp_toggle_only_mask_region");
    let desc = lamp_desc(LayerBlend::Over, LIT);
    let mut t = BackglassTexture::new(ctx, &desc, 256, 256).unwrap();
    assert_eq!(t.aspect(), (256, 256));
    assert_eq!(t.redraw_count(), 0);
    let id = t.texture().clone();
    let mut lamps = [0.0f32; 3];
    assert!(t.update(ctx, &lamps));
    let a = read_level(ctx, t.texture(), 256, 256, 0);

    lamps[1] = 1.0;
    assert!(t.update(ctx, &lamps));
    assert_eq!(t.redraw_count(), 2);
    assert!(*t.texture() == id, "texture object is stable");
    let b = read_level(ctx, t.texture(), 256, 256, 0);
    let mut outside_diff = 0;
    for y in 0..256 {
        for x in 128..256 {
            outside_diff += usize::from(a.at(x, y) != b.at(x, y));
        }
    }
    assert_eq!(
        outside_diff, 0,
        "REQ-VIEW-002 texels outside the mask changed"
    );
    let inside_a = mean(&a, 0, 0, 128, 256);
    let inside_b = mean(&b, 0, 0, 128, 256);
    assert!(
        max_diff4(inside_a, inside_b) > 20.0,
        "inside the mask the lamp changes the pixels: {inside_a:?} -> {inside_b:?}"
    );
    // Analytic: lit inside, base outside.
    for c in 0..3 {
        assert!((f64::from(b.at(10, 10)[c]) - f64::from(LIT[c])).abs() <= 1.0);
        assert!((f64::from(b.at(200, 10)[c]) - f64::from(BASE[c])).abs() <= 1.0);
        assert!((f64::from(a.at(10, 10)[c]) - f64::from(BASE[c])).abs() <= 1.0);
    }

    // Toggle back.
    lamps[1] = 0.0;
    assert!(t.update(ctx, &lamps));
    let c = read_level(ctx, t.texture(), 256, 256, 0);
    let worst =
        a.px.iter()
            .zip(&c.px)
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap();
    assert!(
        worst <= 1,
        "toggled back equals the first image (worst {worst})"
    );

    // Same lamps: nothing drawn; a change below the epsilon: nothing drawn.
    let n = t.redraw_count();
    assert!(!t.update(ctx, &lamps));
    lamps[1] = LAMP_REDRAW_EPSILON * 0.25;
    assert!(!t.update(ctx, &lamps));
    assert_eq!(t.redraw_count(), n);
    t.mark_dirty();
    assert!(t.update(ctx, &lamps), "mark_dirty forces a redraw");
    assert_eq!(t.redraw_count(), n + 1);
}

/// REQ-VIEW-002: a flasher layer adds about half its colour at level 0.5
/// inside its mask and nothing outside.
#[test]
fn req_view_002_translite_flasher_additive() {
    let (_g, ctx) = setup!("req_view_002_translite_flasher_additive");
    let desc = lamp_desc(LayerBlend::Additive, FLASH);
    let mut t = BackglassTexture::new(ctx, &desc, 256, 256).unwrap();
    let lamps = [0.0, 0.5, 0.0];
    assert!(t.update(ctx, &lamps));
    let got = read_level(ctx, t.texture(), 256, 256, 0);
    let want = render_backglass_to_image(ctx, &desc, &lamps, 256, 256).unwrap();
    let worst = got
        .px
        .iter()
        .zip(&want.pixels)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap();
    assert!(worst <= 3, "REQ-VIEW-002 flasher vs oracle: worst {worst}");
    for c in 0..3 {
        let expect = f64::from(BASE[c]) + 0.5 * f64::from(FLASH[c]);
        assert!(
            (f64::from(got.at(10, 10)[c]) - expect).abs() <= 3.0,
            "inside the mask"
        );
        assert!(
            (f64::from(got.at(200, 10)[c]) - f64::from(BASE[c])).abs() <= 1.0,
            "outside the mask"
        );
    }
}

// ---------- 4. Level 0 equals the existing renderers ----------

/// REQ-VIEW-002: level 0 equals `render_backglass_to_image`.
#[test]
fn req_view_002_level0_equals_backglass_renderer() {
    let (_g, ctx) = setup!("req_view_002_level0_equals_backglass_renderer");
    let desc = lamp_desc(LayerBlend::Over, LIT);
    let mut t = BackglassTexture::new(ctx, &desc, 256, 256).unwrap();
    for lamps in [[0.0, 1.0, 0.0], [0.0, 0.37, 0.0]] {
        t.update(ctx, &lamps);
        let got = read_level(ctx, t.texture(), 256, 256, 0);
        let want = render_backglass_to_image(ctx, &desc, &lamps, 256, 256).unwrap();
        assert_matches_oracle("backglass", &got, &want);
    }
}

/// REQ-VIEW-002: level 0 equals `render_dmd_to_image` for a text frame.
#[test]
fn req_view_002_level0_equals_dmd_renderer() {
    let (_g, ctx) = setup!("req_view_002_level0_equals_dmd_renderer");
    let spec = DmdSpec::new(128, 32, DmdColor::Orange).unwrap();
    let mut model =
        ScoreDisplayModel::new(ScoreDisplayConfig::Dmd(spec), SceneLibrary::placeholders());
    model.scores_mut().scores[0] = 1_234_567;
    model.scores_mut().ball = 2;
    let DisplayContent::Dmd(frame) = model.compose(0) else {
        panic!("DMD config composes a DMD frame");
    };
    assert!(frame.lit_count() > 20);
    let mut t = ScoreTexture::new(ctx, ScoreDisplayConfig::Dmd(spec), 512, 128).unwrap();
    assert!(t.update(ctx, &DisplayContent::Dmd(frame.clone())));
    let got = read_level(ctx, t.texture(), 512, 128, 0);
    assert_matches_oracle(
        "dmd",
        &got,
        &render_dmd_to_image(ctx, spec, &frame, 512, 128).unwrap(),
    );
}

/// REQ-VIEW-002: level 0 equals `render_lcd_to_image` for the score view.
#[test]
fn req_view_002_level0_equals_lcd_renderer() {
    let (_g, ctx) = setup!("req_view_002_level0_equals_lcd_renderer");
    let (spec, list) = lcd_content(1_234_567);
    let mut t = ScoreTexture::new(ctx, ScoreDisplayConfig::Lcd(spec), 256, 128).unwrap();
    assert!(t.update(ctx, &DisplayContent::Lcd(list.clone())));
    let got = read_level(ctx, t.texture(), 256, 128, 0);
    assert_matches_oracle("lcd", &got, &lcd_oracle(ctx, spec, &list, 256, 128));
}

// ---------- 5. Mip chain ----------

/// REQ-VIEW-002: the DMD mip chain (count, sizes, box filter, means), and
/// every level is regenerated after a content change.
#[test]
fn req_view_002_dmd_mip_chain() {
    let (_g, ctx) = setup!("req_view_002_dmd_mip_chain");
    let (w, h) = (512, 128);
    let spec = DmdSpec::new(128, 32, DmdColor::Orange).unwrap();
    let mut t = ScoreTexture::new(ctx, ScoreDisplayConfig::Dmd(spec), w, h).unwrap();
    assert_eq!(t.mip_level_count(), mip_level_count(w, h));
    assert_eq!(t.texture().mip_level_count(), mip_level_count(w, h));
    let levels_n = t.mip_level_count();

    t.update(ctx, &DisplayContent::Dmd(dmd_frame(&spec, false)));
    let off = read_all(ctx, t.texture(), w, h, levels_n);
    t.update(ctx, &DisplayContent::Dmd(dmd_frame(&spec, true)));
    let lit = read_all(ctx, t.texture(), w, h, levels_n);
    for (k, lv) in lit.iter().enumerate() {
        assert_eq!((lv.w, lv.h), level_size(w, h, k as u32), "level {k} size");
    }
    assert_box_chain("dmd lit", &lit);
    assert_mean_chain("dmd lit", &lit);
    assert_box_chain("dmd off", &off);
    assert_mean_chain("dmd off", &off);
    // Regenerated after the change.
    let (l1, o1) = (&lit[1], &off[1]);
    assert!(
        max_diff4(mean_all(l1), mean_all(o1)) >= 10.0,
        "level 1 moved"
    );
    let last = levels_n as usize - 1;
    assert!(
        max_diff4(mean_all(&lit[last]), mean_all(&off[last])) >= 10.0,
        "REQ-VIEW-002 the 1x1 level moved: {:?} vs {:?}",
        lit[last].at(0, 0),
        off[last].at(0, 0)
    );
    save_mips(ctx, "dmd_lit", t.texture(), w, h, levels_n);
}

/// REQ-VIEW-002: the LCD mip chain.
#[test]
fn req_view_002_lcd_mip_chain() {
    let (_g, ctx) = setup!("req_view_002_lcd_mip_chain");
    let (w, h) = (256, 128);
    let (spec, list) = lcd_content(1_234_567);
    let mut t = ScoreTexture::new(ctx, ScoreDisplayConfig::Lcd(spec), w, h).unwrap();
    t.update(ctx, &DisplayContent::Lcd(list));
    assert_eq!(t.mip_level_count(), mip_level_count(w, h));
    let levels = read_all(ctx, t.texture(), w, h, t.mip_level_count());
    assert_box_chain("lcd", &levels);
    assert_mean_chain("lcd", &levels);
    save_mips(ctx, "lcd_score", t.texture(), w, h, t.mip_level_count());
}

/// REQ-VIEW-002: the translite mip chain, lamp off and on, regenerated.
#[test]
fn req_view_002_backglass_mip_chain() {
    let (_g, ctx) = setup!("req_view_002_backglass_mip_chain");
    let (w, h) = (256, 256);
    let desc = lamp_desc(LayerBlend::Over, LIT);
    let mut t = BackglassTexture::new(ctx, &desc, w, h).unwrap();
    assert_eq!(t.mip_level_count(), mip_level_count(w, h));
    assert_eq!(t.texture().mip_level_count(), mip_level_count(w, h));
    let n = t.mip_level_count();
    t.update(ctx, &[0.0, 0.0, 0.0]);
    let off = read_all(ctx, t.texture(), w, h, n);
    save_mips(ctx, "backglass_lamp_off", t.texture(), w, h, n);
    t.update(ctx, &[0.0, 1.0, 0.0]);
    let on = read_all(ctx, t.texture(), w, h, n);
    save_mips(ctx, "backglass_lamp_on", t.texture(), w, h, n);
    assert_box_chain("backglass off", &off);
    assert_mean_chain("backglass off", &off);
    assert_box_chain("backglass on", &on);
    assert_mean_chain("backglass on", &on);
    let last = n as usize - 1;
    assert!(
        max_diff4(mean_all(&on[1]), mean_all(&off[1])) >= 10.0,
        "REQ-VIEW-002 level 1 regenerated after the lamp change"
    );
    assert!(
        max_diff4(mean_all(&on[last]), mean_all(&off[last])) >= 10.0,
        "REQ-VIEW-002 last level regenerated after the lamp change"
    );
}

// ---------- 6. Texture facts ----------

/// REQ-VIEW-002: format, usages and mip count of the textures.
#[test]
fn req_view_002_texture_facts() {
    let (_g, ctx) = setup!("req_view_002_texture_facts");
    let desc = lamp_desc(LayerBlend::Over, LIT);
    let spec = DmdSpec::new(128, 32, DmdColor::Orange).unwrap();
    let b = BackglassTexture::new(ctx, &desc, 128, 128).unwrap();
    let s = ScoreTexture::new(ctx, ScoreDisplayConfig::Dmd(spec), 256, 64).unwrap();
    for (name, tex, size, levels) in [
        ("backglass", b.texture(), b.size(), b.mip_level_count()),
        ("score", s.texture(), s.size(), s.mip_level_count()),
    ] {
        assert_eq!(DISPLAY_TEXTURE_FORMAT, wgpu::TextureFormat::Rgba8Unorm);
        assert_eq!(tex.format(), DISPLAY_TEXTURE_FORMAT, "{name}");
        for u in [
            wgpu::TextureUsages::RENDER_ATTACHMENT,
            wgpu::TextureUsages::TEXTURE_BINDING,
            wgpu::TextureUsages::COPY_SRC,
        ] {
            assert!(tex.usage().contains(u), "REQ-VIEW-002 {name} usage {u:?}");
        }
        assert_eq!((tex.width(), tex.height()), size, "{name}");
        assert_eq!(tex.mip_level_count(), levels, "{name}");
        assert_eq!(levels, mip_level_count(size.0, size.1), "{name}");
    }
    // The view exists (all levels); creating a bind group sampling it is
    // validated by the device.
    let _ = (b.view(), s.view());
}

/// REQ-VIEW-002: a non-power-of-two size constructs, updates and reads back
/// with the documented level sizes.
#[test]
fn req_view_002_non_power_of_two() {
    let (_g, ctx) = setup!("req_view_002_non_power_of_two");
    let (w, h) = (200, 50);
    let desc = lamp_desc(LayerBlend::Over, LIT);
    let mut b = BackglassTexture::new(ctx, &desc, w, h).unwrap();
    assert!(b.update(ctx, &[0.0, 1.0, 0.0]));
    assert_eq!(b.mip_level_count(), 8);
    let spec = DmdSpec::new(128, 32, DmdColor::Orange).unwrap();
    let mut s = ScoreTexture::new(ctx, ScoreDisplayConfig::Dmd(spec), w, h).unwrap();
    assert!(s.update(ctx, &DisplayContent::Dmd(dmd_frame(&spec, true))));
    assert_eq!(s.mip_level_count(), 8);
    for (name, tex, n) in [("backglass", b.texture(), 8), ("score", s.texture(), 8)] {
        for k in 0..n {
            let lv = read_level(ctx, tex, w, h, k);
            assert_eq!(
                (lv.w, lv.h),
                level_size(w, h, k),
                "REQ-VIEW-002 {name} level {k}"
            );
        }
    }
}

// ---------- 7. Validation ----------

/// REQ-VIEW-002, R11: bad sizes and bad images are refused, never a panic.
#[test]
fn req_view_002_validation_refuses_bad_input() {
    let (_g, ctx) = setup!("req_view_002_validation_refuses_bad_input");
    let max = ctx.device().limits().max_texture_dimension_2d;
    let desc = lamp_desc(LayerBlend::Over, LIT);
    let spec = DmdSpec::new(128, 32, DmdColor::Orange).unwrap();
    let cfg = ScoreDisplayConfig::Dmd(spec);
    for (w, h) in [(0, 64), (64, 0), (0, 0), (max + 1, 8), (8, max + 1)] {
        assert!(
            matches!(
                BackglassTexture::new(ctx, &desc, w, h),
                Err(GpuError::InvalidArgument(_))
            ),
            "backglass {w}x{h}"
        );
        assert!(
            matches!(
                ScoreTexture::new(ctx, cfg, w, h),
                Err(GpuError::InvalidArgument(_))
            ),
            "score {w}x{h}"
        );
    }
    let mut bad = lamp_desc(LayerBlend::Over, LIT);
    bad.layers[1].image = RgbaImage {
        width: 4,
        height: 4,
        pixels: vec![0; 10],
    };
    assert!(
        BackglassTexture::new(ctx, &bad, 64, 64).is_err(),
        "bad layer image"
    );
    let mut bad_base = lamp_desc(LayerBlend::Over, LIT);
    bad_base.layers[0].image = RgbaImage {
        width: 4,
        height: 4,
        pixels: vec![0; 10],
    };
    assert!(
        BackglassTexture::new(ctx, &bad_base, 64, 64).is_err(),
        "bad base image"
    );
}

// ---------- 8. LCD images registered on the score texture ----------
// #163 gate review comment 4703749506: `ScoreTexture::set_image` registers
// the images of LCD scene image layers; an unregistered file is skipped.

/// LCD canvas of the image tests (16:9, the texture has the same aspect so
/// the canvas fills it 1:1).
const IMG_LCD: (u32, u32) = (256, 144);
/// Background of the image scene.
const IMG_BG: [u8; 3] = [0x14, 0x1e, 0x28];
/// Flat colours of the registered image (opaque).
const IMG_A: [u8; 4] = [200, 50, 100, 255];
const IMG_B: [u8; 4] = [30, 220, 60, 255];
/// Image rectangle on the canvas: x 0.25..0.75, y 0.25..0.75.
const IMG_RECT: (u32, u32, u32, u32) = (64, 36, 192, 108);

/// One LCD scene with a single image layer (file `art/flat.png`, white tint,
/// opaque) over `IMG_BG`, composed into its draw list.
fn image_scene_list() -> (LcdSpec, LcdDrawList) {
    let scene = Scene::from_toml_str(
        r##"
schema_version = 1
duration_ms = 0
background = "#141e28"

[[layers]]
kind = "image"
file = "art/flat.png"
x = 0.25
y = 0.25
w = 0.5
h = 0.5
align = "left"
valign = "top"
color = "#ffffff"
"##,
    )
    .expect("image scene parses");
    let spec = LcdSpec::new(IMG_LCD.0, IMG_LCD.1).unwrap();
    let list = LcdDrawList::compose(&spec, &scene, 0, &ScoreState::default(), &ImageSet::new());
    assert_eq!(list.background, IMG_BG);
    assert!(
        matches!(list.items.as_slice(), [LcdItem::Image { file, .. }] if file == "art/flat.png"),
        "the scene composes one image item: {:?}",
        list.items
    );
    (spec, list)
}

/// Mean of the image rectangle (inset by 2 px against edge filtering) and
/// of the background strip left of it, on mip `level` of `img`.
fn image_and_background_means(img: &Img, level: u32) -> ([f64; 4], [f64; 4]) {
    let (x0, y0, x1, y1) = IMG_RECT;
    let s = |v: u32| v >> level;
    let inside = mean(img, s(x0) + 2, s(y0) + 2, s(x1) - 2, s(y1) - 2);
    let outside = mean(img, 0, 0, s(x0) - 2, img.h);
    (inside, outside)
}

fn rgb_close(what: &str, got: [f64; 4], want: [u8; 4], tol: f64) {
    for c in 0..4 {
        assert!(
            (got[c] - f64::from(want[c])).abs() <= tol,
            "{what}: channel {c} mean {got:?} vs {want:?} (tolerance {tol})"
        );
    }
}

const BG_RGBA: [u8; 4] = [IMG_BG[0], IMG_BG[1], IMG_BG[2], 255];

/// #97 / SPEC §7 (F.4), REQ-VIEW-002, R12 (a): an image registered with
/// `ScoreTexture::set_image` renders in its layer's rectangle (region mean =
/// the image colour), the rest shows the background, and mip levels 0, 1
/// and 2 agree on both means.
#[test]
fn req_view_002_lcd_registered_image_renders() {
    let (_g, ctx) = setup!("req_view_002_lcd_registered_image_renders");
    let (spec, list) = image_scene_list();
    let (w, h) = IMG_LCD;
    let mut t = ScoreTexture::new(ctx, ScoreDisplayConfig::Lcd(spec), w, h).unwrap();
    t.set_image(ctx, "art/flat.png", &solid(8, 8, IMG_A))
        .expect("a valid image registers");
    assert!(t.update(ctx, &DisplayContent::Lcd(list.clone())));
    for level in 0..3 {
        let lv = read_level(ctx, t.texture(), w, h, level);
        let (inside, outside) = image_and_background_means(&lv, level);
        rgb_close(
            &format!("registered image, mip {level}"),
            inside,
            IMG_A,
            1.0,
        );
        rgb_close(&format!("background, mip {level}"), outside, BG_RGBA, 1.0);
        if level == 0 {
            save("lcd_image_registered_mip0", &lv);
        }
    }
    // The texture equals the offscreen LCD renderer given the same image.
    let mut images = ImageSet::new();
    images.insert("art/flat.png", solid(8, 8, IMG_A));
    let oracle = render_lcd_to_image(ctx, spec, &list, &images, w, h).unwrap();
    assert_matches_oracle(
        "lcd registered image",
        &read_level(ctx, t.texture(), w, h, 0),
        &oracle,
    );
}

/// #97, documented behaviour: an image layer whose file is not registered
/// is skipped (no error); its rectangle shows the background.
#[test]
fn req_view_002_lcd_unregistered_image_is_skipped() {
    let (_g, ctx) = setup!("req_view_002_lcd_unregistered_image_is_skipped");
    let (spec, list) = image_scene_list();
    let (w, h) = IMG_LCD;
    let mut t = ScoreTexture::new(ctx, ScoreDisplayConfig::Lcd(spec), w, h).unwrap();
    // Another file is registered; the scene's file is not.
    t.set_image(ctx, "art/other.png", &solid(4, 4, IMG_B))
        .unwrap();
    assert!(t.update(ctx, &DisplayContent::Lcd(list)));
    let lv = read_level(ctx, t.texture(), w, h, 0);
    let (inside, outside) = image_and_background_means(&lv, 0);
    rgb_close("unregistered image rectangle", inside, BG_RGBA, 1.0);
    rgb_close("background", outside, BG_RGBA, 1.0);
    save("lcd_image_unregistered_mip0", &lv);
}

/// #97: registering the image after the first draw redraws on the next
/// update with unchanged content, and setting the file again replaces the
/// image in place (the output changes to the new colour).
#[test]
fn req_view_002_lcd_image_set_later_and_replaced() {
    let (_g, ctx) = setup!("req_view_002_lcd_image_set_later_and_replaced");
    let (spec, list) = image_scene_list();
    let (w, h) = IMG_LCD;
    let content = DisplayContent::Lcd(list);
    let mut t = ScoreTexture::new(ctx, ScoreDisplayConfig::Lcd(spec), w, h).unwrap();
    assert!(t.update(ctx, &content));
    assert!(
        !t.update(ctx, &content),
        "unchanged content does not redraw"
    );
    let before = read_level(ctx, t.texture(), w, h, 0);
    rgb_close(
        "before registration",
        image_and_background_means(&before, 0).0,
        BG_RGBA,
        1.0,
    );

    t.set_image(ctx, "art/flat.png", &solid(8, 8, IMG_A))
        .unwrap();
    assert!(
        t.update(ctx, &content),
        "set_image marks the display changed"
    );
    let a = read_level(ctx, t.texture(), w, h, 0);
    rgb_close("image A", image_and_background_means(&a, 0).0, IMG_A, 1.0);
    assert!(!t.update(ctx, &content));

    t.set_image(ctx, "art/flat.png", &solid(16, 4, IMG_B))
        .unwrap();
    assert!(t.update(ctx, &content), "replacing the image redraws");
    let b = read_level(ctx, t.texture(), w, h, 0);
    let (inside, outside) = image_and_background_means(&b, 0);
    rgb_close("image B (replaced in place)", inside, IMG_B, 1.0);
    rgb_close("background after replace", outside, BG_RGBA, 1.0);
    save("lcd_image_replaced_mip0", &b);
}

/// #97, D25, R11: an invalid or oversized image is refused with
/// `InvalidArgument` (never a panic), does not mark the display changed,
/// and the previous image of that file stays. A DMD score texture refuses
/// every image (it has no image layers).
#[test]
fn req_view_002_lcd_bad_image_refused_previous_stays() {
    let (_g, ctx) = setup!("req_view_002_lcd_bad_image_refused_previous_stays");
    let max = ctx.device().limits().max_texture_dimension_2d;
    let (spec, list) = image_scene_list();
    let (w, h) = IMG_LCD;
    let content = DisplayContent::Lcd(list);
    let mut t = ScoreTexture::new(ctx, ScoreDisplayConfig::Lcd(spec), w, h).unwrap();
    t.set_image(ctx, "art/flat.png", &solid(8, 8, IMG_A))
        .unwrap();
    assert!(t.update(ctx, &content));
    let bad_images = [
        (
            "short pixel buffer",
            RgbaImage {
                width: 4,
                height: 4,
                pixels: vec![0; 10],
            },
        ),
        (
            "zero size",
            RgbaImage {
                width: 0,
                height: 0,
                pixels: Vec::new(),
            },
        ),
        ("wider than the device limit", solid(max + 1, 1, IMG_B)),
        ("taller than the device limit", solid(1, max + 1, IMG_B)),
    ];
    for (what, img) in &bad_images {
        assert!(
            matches!(
                t.set_image(ctx, "art/flat.png", img),
                Err(GpuError::InvalidArgument(_))
            ),
            "{what} is refused with InvalidArgument"
        );
        assert!(
            !t.update(ctx, &content),
            "{what}: a refused image does not mark the display changed"
        );
    }
    t.mark_dirty();
    assert!(t.update(ctx, &content));
    let lv = read_level(ctx, t.texture(), w, h, 0);
    rgb_close(
        "previous image stays after refusals",
        image_and_background_means(&lv, 0).0,
        IMG_A,
        1.0,
    );

    let dmd = DmdSpec::new(128, 32, DmdColor::Orange).unwrap();
    let mut d = ScoreTexture::new(ctx, ScoreDisplayConfig::Dmd(dmd), 256, 64).unwrap();
    match d.set_image(ctx, "art/flat.png", &solid(8, 8, IMG_A)) {
        Err(GpuError::InvalidArgument(msg)) => {
            assert!(msg.contains("DMD"), "message names the DMD: {msg}")
        }
        other => panic!("a DMD score texture refuses images: {other:?}"),
    }
}

// ---------- 9. Area-weighted mips of odd sizes ----------
// #163 gate round 11 (review 6070807835, texture.rs mip shader): every texel
// of an odd-sized level contributes to the next level, so the 1x1 mip is the
// whole-image average; exact 2:1 reductions keep the four-texel average.
// "Whole-image average" here means the area-weighted average computed level
// by level: each level is rounded to nearest on its own, so the 1x1 byte is
// within half a byte per reduction of the original mean and need not equal
// it rounded once (#163 round 12, finding 22: a 5x1 red `[1, 1, 0, 0, 0]`
// reduces to `[1, 0]` then `[1]`, while its mean 0.4 rounds to 0). The
// oracle checks below stay exact per level.

/// Per-axis weights `(w0, w1, w2, denominator)` of the source texels
/// `2d, 2d + 1, 2d + 2` for destination texel `d` of a source side `s`:
/// the overlap of each texel with the span `[d*s/t, (d+1)*s/t)`, `t =
/// max(1, s / 2)`, scaled to integers summing to the denominator.
fn axis_weights(s: u32, d: u32) -> [u64; 4] {
    if s == 1 {
        [1, 0, 0, 1]
    } else if s.is_multiple_of(2) {
        [1, 1, 0, 2]
    } else {
        let t = u64::from(s / 2);
        let d = u64::from(d);
        [t - d, t, d + 1, u64::from(s)]
    }
}

/// Independent oracle of one mip step: area-weighted integer average of the
/// bytes, rounded to nearest with halves up.
fn mip_oracle(src: &Img) -> Img {
    let (w2, h2) = ((src.w / 2).max(1), (src.h / 2).max(1));
    let mut px = Vec::with_capacity((w2 * h2 * 4) as usize);
    for y in 0..h2 {
        let wy = axis_weights(src.h, y);
        for x in 0..w2 {
            let wx = axis_weights(src.w, x);
            let den = wx[3] * wy[3];
            for c in 0..4 {
                let mut sum = 0u64;
                for j in 0..3u32 {
                    for i in 0..3u32 {
                        let wgt = wx[i as usize] * wy[j as usize];
                        if wgt > 0 {
                            sum += wgt * u64::from(src.at(2 * x + i, 2 * y + j)[c]);
                        }
                    }
                }
                px.push(((sum + den / 2) / den) as u8);
            }
        }
    }
    Img { w: w2, h: h2, px }
}

/// A backglass texture of exactly the image's size whose level 0 is the
/// image (one opaque layer drawn 1:1), updated once.
fn image_texture(ctx: &GpuContext, image: RgbaImage) -> BackglassTexture {
    let (w, h) = (image.width, image.height);
    let desc = BackglassDesc {
        layers: vec![BackglassLayer { image, lamp: None }],
    };
    let mut t = BackglassTexture::new(ctx, &desc, w, h).unwrap();
    assert!(t.update(ctx, &[]));
    t
}

/// Every level `k + 1` equals the oracle of level `k` byte for byte.
fn assert_chain_is_oracle(what: &str, levels: &[Img]) {
    for k in 1..levels.len() {
        let want = mip_oracle(&levels[k - 1]);
        let got = &levels[k];
        assert_eq!((got.w, got.h), (want.w, want.h), "{what} level {k} size");
        let bad = got.px.iter().zip(&want.px).filter(|(a, b)| a != b).count();
        assert_eq!(
            bad,
            0,
            "{what}: REQ-VIEW-002 level {k} differs from the area-weighted average of level {} in {bad} channels",
            k - 1
        );
    }
}

fn opaque(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> RgbaImage {
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let [r, g, b] = f(x, y);
            data.extend_from_slice(&[r, g, b, 255]);
        }
    }
    RgbaImage::from_rgba(w, h, data).unwrap()
}

/// REQ-VIEW-002 (review 6070807835): a 3x1 image `[0, 0, 255]` red has the
/// whole-image average 85 as its 1x1 mip (the old 2x2 taps gave 0).
#[test]
fn req_view_002_odd_mip_3x1_is_whole_average() {
    let (_g, ctx) = setup!("req_view_002_odd_mip_3x1");
    let img = opaque(3, 1, |x, _| [if x == 2 { 255 } else { 0 }, 0, 0]);
    let t = image_texture(ctx, img.clone());
    assert_eq!(t.mip_level_count(), 2);
    let levels = read_all(ctx, t.texture(), 3, 1, 2);
    assert_eq!(levels[0].px, img.pixels, "level 0 is the image");
    assert_eq!(levels[1].at(0, 0), [85, 0, 0, 255], "REQ-VIEW-002 1x1 mip");
    assert_chain_is_oracle("3x1", &levels);
}

/// REQ-VIEW-002 (review 6070807835): an edge-only 5x3 image (255 only in
/// the last column and the last row) keeps its edge in every level.
#[test]
fn req_view_002_odd_mip_edge_only_5x3() {
    let (_g, ctx) = setup!("req_view_002_odd_mip_edge_only_5x3");
    let img = opaque(5, 3, |x, y| {
        let v = if x == 4 || y == 2 { 255 } else { 0 };
        [v, v, v]
    });
    let t = image_texture(ctx, img.clone());
    let n = t.mip_level_count();
    assert_eq!(n, 3);
    let levels = read_all(ctx, t.texture(), 5, 3, n);
    assert_eq!(levels[0].px, img.pixels, "level 0 is the image");
    // Level 1 is 2x1: x weights (2, 2, 1)/5 and (1, 2, 2)/5, y (1, 1, 1)/3.
    // Left: only row 2 is lit, weight 5/15 -> 85. Right: x = 4 in rows 0
    // and 1 (2/15 each) plus row 2 (5/15) -> 9/15 of 255 = 153.
    assert_eq!(
        levels[1].at(0, 0),
        [85, 85, 85, 255],
        "REQ-VIEW-002 level 1 left"
    );
    assert_eq!(
        levels[1].at(1, 0),
        [153, 153, 153, 255],
        "REQ-VIEW-002 level 1 right"
    );
    assert_chain_is_oracle("5x3 edge", &levels);
    // Whole-image average: 7 lit of 15 texels -> 7 * 255 / 15 = 119.
    let last = levels[2].at(0, 0);
    assert_eq!(
        last,
        [119, 119, 119, 255],
        "REQ-VIEW-002 1x1 mip of the edge-only image"
    );
}

/// REQ-VIEW-002 (review 6070807835): a 255x129 odd chain (a gradient with a
/// bright last column and row, and a pseudo-random image) equals the
/// area-weighted oracle at every level; the 1x1 level is the whole-image
/// average computed level by level (each level rounded to nearest, so within
/// half a byte per reduction of the original mean). Every level of the
/// gradient is written for the visual check.
#[test]
fn req_view_002_odd_mip_chain_255x129() {
    let (_g, ctx) = setup!("req_view_002_odd_mip_chain_255x129");
    let (w, h) = (255, 129);
    let gradient = opaque(w, h, |x, y| {
        if x == w - 1 || y == h - 1 {
            [255, 255, 255]
        } else {
            [x as u8, (y * 255 / (h - 1)) as u8, ((x + y) / 2) as u8]
        }
    });
    let noise = opaque(w, h, |x, y| {
        let mut v = x.wrapping_mul(0x9e37_79b9) ^ y.wrapping_mul(0x85eb_ca6b);
        v ^= v >> 15;
        v = v.wrapping_mul(0x2c1b_3c6d);
        v ^= v >> 12;
        [v as u8, (v >> 8) as u8, (v >> 16) as u8]
    });
    for (name, img) in [("odd_gradient", gradient), ("odd_noise", noise)] {
        let t = image_texture(ctx, img.clone());
        let n = t.mip_level_count();
        assert_eq!(n, mip_level_count(w, h));
        let levels = read_all(ctx, t.texture(), w, h, n);
        for (a, b) in levels[0].px.iter().zip(&img.pixels) {
            assert!(a.abs_diff(*b) <= 1, "{name}: level 0 is the image");
        }
        assert_chain_is_oracle(name, &levels);
        let (avg, last) = (mean_all(&levels[0]), mean_all(&levels[n as usize - 1]));
        assert!(
            max_diff4(avg, last) <= 1.5,
            "{name}: REQ-VIEW-002 1x1 mip {last:?} vs whole-image average {avg:?}"
        );
        for (k, lv) in levels.iter().enumerate() {
            save(&format!("{name}_mip{k}"), lv);
        }
    }
}

/// REQ-VIEW-002 (review 6070807835): exact 2:1 reductions keep the
/// four-texel average `(sum + 2) / 4` byte for byte (a pseudo-random 64x64
/// image, so every level is an exact 2:1 reduction on both axes).
#[test]
fn req_view_002_even_mip_chain_is_four_texel_average() {
    let (_g, ctx) = setup!("req_view_002_even_mip_chain");
    let (w, h) = (64, 64);
    let img = opaque(w, h, |x, y| {
        let v = (x * 131 + y * 71 + x * y * 17) ^ (x << 3);
        [v as u8, (v >> 3) as u8, (v >> 5) as u8]
    });
    let t = image_texture(ctx, img);
    let n = t.mip_level_count();
    let levels = read_all(ctx, t.texture(), w, h, n);
    for k in 1..levels.len() {
        let (src, dst) = (&levels[k - 1], &levels[k]);
        for y in 0..dst.h {
            for x in 0..dst.w {
                for c in 0..4 {
                    let sum: u32 = [(0, 0), (1, 0), (0, 1), (1, 1)]
                        .iter()
                        .map(|(i, j)| u32::from(src.at(2 * x + i, 2 * y + j)[c]))
                        .sum();
                    assert_eq!(
                        u32::from(dst.at(x, y)[c]),
                        (sum + 2) / 4,
                        "REQ-VIEW-002 level {k} ({x},{y}) ch {c}"
                    );
                }
            }
        }
    }
}
