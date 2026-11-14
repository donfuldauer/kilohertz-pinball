//! DMD mode: dot frames composed from scenes on the CPU, drawn as a
//! procedural dot grid on the GPU.

use super::quad::{OUTPUT_WGSL, load_pass, render_offscreen, scissor, upload_rgba8, write_rgba8};
use super::scene::{Align, FontChoice, LayerKind, Scene, VAlign};
use super::score::{ScoreState, expand_text, score_view_layout};
use super::{DisplayError, ImageSet, RgbaImage};
use crate::gpu::{GpuContext, GpuError};
use crate::topology::Rect;
use pin_assetgen::fonts::{BitmapFont, FONT_5X7, FONT_8X16, render_text};
use pin_core::schema::backglass::DmdColor;
use wgpu::util::DeviceExt;

/// DMD size and dot colour.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct DmdSpec {
    /// Dots across.
    pub width: u32,
    /// Dots down.
    pub height: u32,
    /// Dot colour mode.
    pub color: DmdColor,
}

/// The classic 128 × 32 orange DMD.
pub const DMD_128X32: DmdSpec = DmdSpec {
    width: 128,
    height: 32,
    color: DmdColor::Orange,
};

/// The large 192 × 64 orange DMD.
pub const DMD_192X64: DmdSpec = DmdSpec {
    width: 192,
    height: 64,
    color: DmdColor::Orange,
};

impl DmdSpec {
    /// A spec of `width × height` dots (16..=512 × 8..=256).
    pub fn new(width: u32, height: u32, color: DmdColor) -> Result<DmdSpec, DisplayError> {
        if !(16..=512).contains(&width) || !(8..=256).contains(&height) {
            return Err(DisplayError::BadSize(format!(
                "DMD {width}x{height} (16..=512 x 8..=256 dots)"
            )));
        }
        Ok(DmdSpec {
            width,
            height,
            color,
        })
    }
}

/// Fraction of the tint an unlit dot shows.
pub const DMD_UNLIT_LEVEL: f32 = 0.10;
/// Dot radius as a fraction of the dot pitch.
pub const DMD_DOT_RADIUS: f32 = 0.40;
/// Colour between and around the dots.
pub const DMD_BACKGROUND: [f32; 3] = [0.0, 0.0, 0.0];

/// The full-drive colour of a dot in mode `color` (`rgb`: white; the drive
/// colour then sets the hue).
pub fn dmd_tint(color: DmdColor) -> [f32; 3] {
    match color {
        DmdColor::Orange => [1.0, 0.42, 0.06],
        DmdColor::Red => [1.0, 0.12, 0.08],
        DmdColor::Rgb => [1.0, 1.0, 1.0],
    }
}

/// The colour at a dot's centre for drive level `drive` (CPU reference of
/// the shader): `tint · (UNLIT + (1 − UNLIT) · drive / 255)` per channel.
pub fn dmd_dot_color(color: DmdColor, drive: [u8; 3]) -> [f32; 3] {
    let t = dmd_tint(color);
    core::array::from_fn(|i| {
        t[i] * (DMD_UNLIT_LEVEL + (1.0 - DMD_UNLIT_LEVEL) * f32::from(drive[i]) / 255.0)
    })
}

/// Where the dots of a DMD fall in a viewport.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct DmdGeometry {
    /// Top-left corner of dot (0, 0)'s cell, target pixels.
    pub origin: [f32; 2],
    /// Distance between dot centres, pixels.
    pub pitch: f32,
    /// Dot radius, pixels.
    pub radius: f32,
}

impl DmdGeometry {
    /// Centre of dot `(x, y)` in target pixels.
    pub fn dot_center(&self, x: u32, y: u32) -> [f32; 2] {
        [
            self.origin[0] + (x as f32 + 0.5) * self.pitch,
            self.origin[1] + (y as f32 + 0.5) * self.pitch,
        ]
    }
}

/// The dot grid of `spec` in `viewport`: square cells of pitch
/// `min(vw / W, vh / H)`, the grid centred.
pub fn dmd_geometry(spec: &DmdSpec, viewport: Rect) -> DmdGeometry {
    let (w, h) = (spec.width as f32, spec.height as f32);
    let pitch = (viewport.w as f32 / w).min(viewport.h as f32 / h);
    DmdGeometry {
        origin: [
            viewport.x as f32 + (viewport.w as f32 - pitch * w) / 2.0,
            viewport.y as f32 + (viewport.h as f32 - pitch * h) / 2.0,
        ],
        pitch,
        radius: DMD_DOT_RADIUS * pitch,
    }
}

/// A DMD bitmap font.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DmdFont {
    /// `font_dmd_5x7`.
    Font5x7,
    /// `font_dmd_8x16`.
    Font8x16,
}

impl DmdFont {
    fn bitmap(self) -> &'static BitmapFont {
        match self {
            DmdFont::Font5x7 => FONT_5X7,
            DmdFont::Font8x16 => FONT_8X16,
        }
    }
}

/// Size in dots of `text` in `font` at integer `scale`.
pub fn dmd_text_dots(text: &str, font: DmdFont, scale: u32) -> (u32, u32) {
    let f = font.bitmap();
    (
        f.text_width(text).saturating_mul(scale),
        f.height().saturating_mul(scale),
    )
}

/// Font and scale for text `size` dots tall that should fit `max_w` dots.
fn pick_font(text: &str, size: f32, choice: FontChoice, max_w: u32) -> (DmdFont, u32) {
    let fits = |font, s| dmd_text_dots(text, font, s).0 <= max_w;
    if choice != FontChoice::Dmd5x7 {
        let mut s = (size / 16.0).floor().max(0.0) as u32;
        if choice == FontChoice::Dmd8x16 {
            s = s.max(1);
            while s > 1 && !fits(DmdFont::Font8x16, s) {
                s -= 1;
            }
            return (DmdFont::Font8x16, s);
        }
        while s >= 1 {
            if fits(DmdFont::Font8x16, s) {
                return (DmdFont::Font8x16, s);
            }
            s -= 1;
        }
    }
    let mut s = ((size / 7.0).floor() as u32).max(1);
    while s > 1 && !fits(DmdFont::Font5x7, s) {
        s -= 1;
    }
    (DmdFont::Font5x7, s)
}

/// One frame of DMD dot drive levels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DmdFrame {
    /// Dots across.
    pub width: u32,
    /// Dots down.
    pub height: u32,
    /// Row-major drive levels, row 0 at the top. Mono modes store the same
    /// value, quantised to 16 levels (`round(b·15)·17`), in all channels.
    pub dots: Vec<[u8; 3]>,
}

struct Canvas {
    w: u32,
    h: u32,
    acc: Vec<[f32; 3]>,
    /// Drawable dots `[x0, y0, x1, y1)` (a score layer's box).
    clip: [i64; 4],
}

impl Canvas {
    fn blend(&mut self, x: i64, y: i64, col: [f32; 3], a: f32) {
        let [cx0, cy0, cx1, cy1] = self.clip;
        if x < cx0.max(0)
            || y < cy0.max(0)
            || x >= cx1.min(i64::from(self.w))
            || y >= cy1.min(i64::from(self.h))
        {
            return;
        }
        let d = &mut self.acc[y as usize * self.w as usize + x as usize];
        for i in 0..3 {
            d[i] = d[i] * (1.0 - a) + col[i] * a;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn text(
        &mut self,
        text: &str,
        anchor: [f32; 2],
        size_dots: f32,
        align: Align,
        valign: VAlign,
        font: FontChoice,
        col: [f32; 3],
        a: f32,
        max_w: u32,
    ) {
        let (font, scale) = pick_font(text, size_dots, font, max_w);
        let mut bits = Vec::new();
        let (tw, th) = render_text(font.bitmap(), text, &mut bits);
        let (w, h) = (tw * scale, th * scale);
        let left = align.left_of(anchor[0], w as f32).round() as i64;
        let top = valign.top_of(anchor[1], h as f32).round() as i64;
        for y in 0..h {
            for x in 0..w {
                if bits[((y / scale) * tw + x / scale) as usize] != 0 {
                    self.blend(left + i64::from(x), top + i64::from(y), col, a);
                }
            }
        }
    }

    /// Dots whose centres lie in `[x0, x1) × [y0, y1)` (dot units).
    fn cells(&self, b: [f32; 4]) -> impl Iterator<Item = (i64, i64)> + use<> {
        let lo = |v: f32| (v - 0.5).ceil().max(0.0) as i64;
        let (x0, y0) = (lo(b[0]), lo(b[1]));
        let (x1, y1) = (
            ((b[2] - 0.5).ceil() as i64).min(i64::from(self.w)),
            ((b[3] - 0.5).ceil() as i64).min(i64::from(self.h)),
        );
        (y0..y1.max(y0)).flat_map(move |y| (x0..x1.max(x0)).map(move |x| (x, y)))
    }
}

fn rgb01(c: [u8; 3]) -> [f32; 3] {
    c.map(|v| f32::from(v) / 255.0)
}

impl DmdFrame {
    /// An all-off frame.
    pub fn new(width: u32, height: u32) -> DmdFrame {
        DmdFrame {
            width,
            height,
            dots: vec![[0; 3]; width as usize * height as usize],
        }
    }

    /// Drive of dot `(x, y)` (`[0; 3]` outside).
    pub fn get(&self, x: u32, y: u32) -> [u8; 3] {
        if x >= self.width || y >= self.height {
            return [0; 3];
        }
        self.dots[y as usize * self.width as usize + x as usize]
    }

    /// Sets dot `(x, y)` (ignored outside).
    pub fn set(&mut self, x: u32, y: u32, drive: [u8; 3]) {
        if x < self.width && y < self.height {
            self.dots[y as usize * self.width as usize + x as usize] = drive;
        }
    }

    /// Dots with any channel above 0.
    pub fn lit_count(&self) -> usize {
        self.dots
            .iter()
            .filter(|d| d.iter().any(|&c| c > 0))
            .count()
    }

    /// Composes `scene` at `elapsed_ms` for `spec`: background, then each
    /// visible layer back to front ("over" with the layer's opacity), then
    /// quantised (mono modes: brightest channel to 16 levels).
    pub fn compose(
        spec: &DmdSpec,
        scene: &Scene,
        elapsed_ms: u64,
        scores: &ScoreState,
        images: &ImageSet,
    ) -> DmdFrame {
        let (w, h) = (spec.width, spec.height);
        let (fw, fh) = (w as f32, h as f32);
        let mut c = Canvas {
            w,
            h,
            acc: vec![rgb01(scene.background); w as usize * h as usize],
            clip: [0, 0, i64::from(w), i64::from(h)],
        };
        for layer in &scene.layers {
            let Some(st) = layer.state_at(elapsed_ms, scene.duration_ms) else {
                continue;
            };
            let col = rgb01(layer.color);
            let a = st.opacity;
            let box_px = |bw: f32, bh: f32| {
                let x0 = layer.align.left_of(st.x * fw, bw);
                let y0 = layer.valign.top_of(st.y * fh, bh);
                [x0, y0, x0 + bw, y0 + bh]
            };
            match &layer.kind {
                LayerKind::Text(t) => {
                    let text = expand_text(t, scores);
                    c.text(
                        &text,
                        [st.x * fw, st.y * fh],
                        layer.size * fh,
                        layer.align,
                        layer.valign,
                        layer.font,
                        col,
                        a,
                        w,
                    );
                }
                LayerKind::Rect => {
                    let b = box_px(layer.w * fw, layer.h * fh);
                    for (x, y) in c.cells(b).collect::<Vec<_>>() {
                        c.blend(x, y, col, a);
                    }
                }
                LayerKind::Image(file) => {
                    let Some(img) = images.get(file) else {
                        continue;
                    };
                    let bw = layer.w * fw;
                    let bh = if layer.h > 0.0 {
                        layer.h * fh
                    } else {
                        bw * img.height as f32 / img.width.max(1) as f32
                    };
                    let b = box_px(bw, bh);
                    for (x, y) in c.cells(b).collect::<Vec<_>>() {
                        let u = ((x as f32 + 0.5 - b[0]) / bw * img.width as f32) as u32;
                        let v = ((y as f32 + 0.5 - b[1]) / bh * img.height as f32) as u32;
                        let p = img.pixel(u.min(img.width - 1), v.min(img.height - 1));
                        let pa = f32::from(p[3]) / 255.0;
                        let pc = [p[0], p[1], p[2]].map(|v| f32::from(v) / 255.0);
                        let tinted = core::array::from_fn(|i| pc[i] * col[i]);
                        c.blend(x, y, tinted, a * pa);
                    }
                }
                LayerKind::Score => {
                    let b = box_px(layer.w * fw, layer.h * fh);
                    let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
                    // Score text stays inside the box: dots whose centres lie in it.
                    let edge = |v: f32| (v - 0.5).ceil() as i64;
                    c.clip = [edge(b[0]), edge(b[1]), edge(b[2]), edge(b[3])];
                    for t in score_view_layout(scores) {
                        let tc = col.map(|v| v * t.emphasis);
                        c.text(
                            &t.text,
                            [b[0] + t.x * bw, b[1] + t.y * bh],
                            t.size * bh,
                            t.align,
                            t.valign,
                            layer.font,
                            tc,
                            a,
                            bw.max(0.0) as u32,
                        );
                    }
                    c.clip = [0, 0, i64::from(w), i64::from(h)];
                }
            }
        }
        let mono = spec.color != DmdColor::Rgb;
        let dots = c
            .acc
            .iter()
            .map(|p| {
                if mono {
                    let b = p[0].max(p[1]).max(p[2]).clamp(0.0, 1.0);
                    let v = ((b * 15.0).round() as u8) * 17;
                    [v; 3]
                } else {
                    p.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
                }
            })
            .collect();
        DmdFrame {
            width: w,
            height: h,
            dots,
        }
    }
}

const DMD_WGSL: &str = r#"
struct U {
    origin: vec2<f32>,
    dims: vec2<f32>,
    pitch: f32,
    radius: f32,
    unlit: f32,
    srgb: f32,
    tint: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var dots: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    return vec4<f32>(p, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let local = (pos.xy - u.origin) / u.pitch;
    let cell = floor(local);
    var col = BACKGROUND;
    if (all(cell >= vec2<f32>(0.0)) && all(cell < u.dims)) {
        let centre = u.origin + (cell + vec2<f32>(0.5)) * u.pitch;
        let d = distance(pos.xy, centre);
        let cov = clamp(u.radius - d + 0.5, 0.0, 1.0);
        let drive = textureLoad(dots, vec2<i32>(cell), 0).rgb;
        let dot_col = u.tint.rgb * (vec3<f32>(u.unlit) + (1.0 - u.unlit) * drive);
        col = mix(BACKGROUND, dot_col, cov);
    }
    return encode_output(vec4<f32>(col, 1.0), u.srgb);
}
"#;

pub(super) fn dmd_shader_source() -> String {
    let [r, g, b] = DMD_BACKGROUND;
    format!(
        "const BACKGROUND: vec3<f32> = vec3<f32>({r:?}, {g:?}, {b:?});\n{OUTPUT_WGSL}\n{DMD_WGSL}"
    )
}

/// Draws [`DmdFrame`]s as a procedural dot grid.
pub struct DmdRenderer {
    device: wgpu::Device,
    spec: DmdSpec,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    dots: wgpu::Texture,
    dots_view: wgpu::TextureView,
    frame: Option<DmdFrame>,
    srgb: bool,
}

impl DmdRenderer {
    /// A renderer of `spec` for targets of `format`.
    pub fn new(ctx: &GpuContext, format: wgpu::TextureFormat, spec: DmdSpec) -> DmdRenderer {
        let device = ctx.device();
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("dmd"),
            source: wgpu::ShaderSource::Wgsl(dmd_shader_source().into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("dmd"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(48),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("dmd"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("dmd"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let blank = vec![0u8; spec.width as usize * spec.height as usize * 4];
        let dots = upload_rgba8(
            device,
            ctx.queue(),
            "dmd dots",
            spec.width,
            spec.height,
            &blank,
        );
        let dots_view = dots.create_view(&Default::default());
        DmdRenderer {
            device: device.clone(),
            spec,
            pipeline,
            layout,
            dots,
            dots_view,
            frame: None,
            srgb: format.is_srgb(),
        }
    }

    /// The display spec.
    pub fn spec(&self) -> DmdSpec {
        self.spec
    }

    /// Uploads `frame` if it differs from the current one; returns whether it
    /// changed (the display then needs a redraw). A frame of another size is
    /// cropped / padded with off dots.
    pub fn set_frame(&mut self, queue: &wgpu::Queue, frame: &DmdFrame) -> bool {
        if self.frame.as_ref() == Some(frame) {
            return false;
        }
        let (w, h) = (self.spec.width, self.spec.height);
        let mut data = vec![0u8; w as usize * h as usize * 4];
        for y in 0..h {
            for x in 0..w {
                let d = frame.get(x, y);
                let o = (y as usize * w as usize + x as usize) * 4;
                data[o..o + 3].copy_from_slice(&d);
                data[o + 3] = 255;
            }
        }
        write_rgba8(queue, &self.dots, &data);
        self.frame = Some(frame.clone());
        true
    }

    /// Records the display into `viewport` of `view` (a texture of
    /// `target_size`): background, unlit and lit dots over the whole viewport.
    pub fn encode(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        target_size: (u32, u32),
        viewport: Rect,
    ) {
        let Some((sx, sy, sw, sh)) = scissor(viewport, target_size) else {
            return;
        };
        let g = dmd_geometry(&self.spec, viewport);
        let tint = dmd_tint(self.spec.color);
        let uniform: [f32; 12] = [
            g.origin[0],
            g.origin[1],
            self.spec.width as f32,
            self.spec.height as f32,
            g.pitch,
            g.radius,
            DMD_UNLIT_LEVEL,
            if self.srgb { 1.0 } else { 0.0 },
            tint[0],
            tint[1],
            tint[2],
            1.0,
        ];
        let buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("dmd uniform"),
                contents: bytemuck::cast_slice(&uniform),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("dmd"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.dots_view),
                },
            ],
        });
        let mut pass = load_pass(encoder, view, "dmd");
        pass.set_scissor_rect(sx, sy, sw, sh);
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// Renders `frame` on a `spec` DMD filling a `width × height` image
/// (`Rgba8Unorm`, viewport = whole image) and reads it back.
pub fn render_dmd_to_image(
    ctx: &GpuContext,
    spec: DmdSpec,
    frame: &DmdFrame,
    width: u32,
    height: u32,
) -> Result<RgbaImage, GpuError> {
    let mut r = DmdRenderer::new(ctx, wgpu::TextureFormat::Rgba8Unorm, spec);
    r.set_frame(ctx.queue(), frame);
    render_offscreen(ctx, width, height, |encoder, view| {
        r.encode(
            encoder,
            view,
            (width, height),
            Rect::new(0, 0, width, height),
        );
    })
}
