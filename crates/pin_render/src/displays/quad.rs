//! Shared GPU pieces of the displays: an instanced 2D quad compositor
//! (solid, image, lamp-masked image and vector-font glyph quads; "over" and
//! additive blending), texture upload and offscreen readback.

use super::RgbaImage;
use crate::gpu::{GpuContext, GpuError};
use crate::topology::Rect;
use pin_assetgen::fonts::{FIRST_CHAR, FONT_5X7, LAST_CHAR};
use std::collections::BTreeMap;
use wgpu::util::DeviceExt;

/// Glyph slots in the font atlas: ASCII 32–126 then the fallback box.
pub(super) const GLYPH_SLOTS: u32 = LAST_CHAR - FIRST_CHAR + 2;

/// Atlas slot of `c`.
pub(super) fn glyph_slot(c: char) -> u32 {
    let code = c as u32;
    if (FIRST_CHAR..=LAST_CHAR).contains(&code) {
        code - FIRST_CHAR
    } else {
        GLYPH_SLOTS - 1
    }
}

/// WGSL shared by the display shaders: display-encoded → linear for `*Srgb`
/// targets.
pub(super) const OUTPUT_WGSL: &str = r#"
fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

fn encode_output(c: vec4<f32>, srgb: f32) -> vec4<f32> {
    if (srgb > 0.5) {
        return vec4<f32>(srgb_to_linear(clamp(c.rgb, vec3<f32>(0.0), vec3<f32>(1.0))), c.a);
    }
    return c;
}
"#;

const QUAD_WGSL: &str = r#"
struct Globals {
    size: vec2<f32>,
    srgb: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var font: texture_2d<f32>;
@group(1) @binding(0) var image: texture_2d<f32>;
@group(1) @binding(1) var mask: texture_2d<f32>;
@group(1) @binding(2) var samp: sampler;

struct Inst {
    @location(0) rect: vec4<f32>,
    @location(1) uv: vec4<f32>,
    @location(2) color: vec4<f32>,
    @location(3) params: vec4<f32>,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) params: vec4<f32>,
};

@vertex
fn vs(@builtin(vertex_index) vi: u32, inst: Inst) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let k = corners[vi];
    let p = mix(inst.rect.xy, inst.rect.zw, k);
    let ndc = vec2<f32>(p.x / globals.size.x * 2.0 - 1.0, 1.0 - p.y / globals.size.y * 2.0);
    var out: VsOut;
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = mix(inst.uv.xy, inst.uv.zw, k);
    out.color = inst.color;
    out.params = inst.params;
    return out;
}

fn lit(g: i32, q: vec2<i32>) -> bool {
    if (q.x < 0 || q.y < 0 || q.x >= 5 || q.y >= 7) {
        return false;
    }
    return textureLoad(font, vec2<i32>(g * 5 + q.x, q.y), 0).r > 0.5;
}

fn seg_dist(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let ab = b - a;
    let t = clamp(dot(p - a, ab) / dot(ab, ab), 0.0, 1.0);
    return length(p - (a + ab * t));
}

// Distance (glyph units) from `p` to the strokes of glyph `g`: lit cells are
// dots joined to lit neighbours; diagonals only where both orthogonal
// neighbours are unlit.
fn glyph_dist(g: i32, p: vec2<f32>) -> f32 {
    let c = vec2<i32>(floor(p));
    var d = 1.0e9;
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let q = c + vec2<i32>(dx, dy);
            if (!lit(g, q)) {
                continue;
            }
            let qc = vec2<f32>(q) + vec2<f32>(0.5);
            d = min(d, length(p - qc));
            for (var ey = -1; ey <= 1; ey = ey + 1) {
                for (var ex = -1; ex <= 1; ex = ex + 1) {
                    if (ex == 0 && ey == 0) {
                        continue;
                    }
                    let n = q + vec2<i32>(ex, ey);
                    if (!lit(g, n)) {
                        continue;
                    }
                    if (ex != 0 && ey != 0 && (lit(g, vec2<i32>(n.x, q.y)) || lit(g, vec2<i32>(q.x, n.y)))) {
                        continue;
                    }
                    d = min(d, seg_dist(p, qc, qc + vec2<f32>(f32(ex), f32(ey))));
                }
            }
        }
    }
    return d;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    // Texture sampling stays in uniform control flow for every mode.
    let img = textureSample(image, samp, in.uv);
    let m = textureSample(mask, samp, in.uv).r;
    let mode = i32(in.params.x + 0.5);
    var c = in.color;
    if (mode == 1) {
        c = img * in.color;
    } else if (mode == 2) {
        c = img * in.color * m;
    } else if (mode == 3) {
        let d = glyph_dist(i32(in.params.y + 0.5), in.uv);
        let cov = clamp(0.5 - (d - STROKE_HALF_WIDTH) * in.params.z, 0.0, 1.0);
        c = in.color * cov;
    }
    return encode_output(c, globals.srgb);
}
"#;

/// One quad instance (64 bytes).
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct QuadInstance {
    /// `x0, y0, x1, y1` in target pixels.
    pub rect: [f32; 4],
    /// `u0, v0, u1, v1` (texture uv, or glyph units for glyphs).
    pub uv: [f32; 4],
    /// Premultiplied colour multiplier.
    pub color: [f32; 4],
    /// `mode` (0 solid, 1 image, 2 masked image, 3 glyph), glyph slot,
    /// target pixels per glyph unit, unused.
    pub params: [f32; 4],
}

/// How a quad combines with what is below it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Blend {
    /// Premultiplied alpha-over.
    Over,
    /// Colour added, destination alpha kept.
    Add,
}

/// One quad and its texture binding (`(image, mask)` texture indices).
#[derive(Copy, Clone, Debug)]
pub(super) struct QuadDraw {
    pub inst: QuadInstance,
    pub tex: (usize, usize),
    pub blend: Blend,
}

impl QuadDraw {
    /// A solid premultiplied-colour rectangle.
    pub fn solid(rect: [f32; 4], premul: [f32; 4]) -> QuadDraw {
        QuadDraw {
            inst: QuadInstance {
                rect,
                uv: [0.0; 4],
                color: premul,
                params: [0.0; 4],
            },
            tex: (WHITE, WHITE),
            blend: Blend::Over,
        }
    }
}

/// Texture index of the built-in 1×1 white texture.
pub(super) const WHITE: usize = 0;

/// Stroke half-width of the vector font, glyph units.
pub(super) const STROKE_HALF_WIDTH: f32 = 0.42;

pub(super) struct QuadRenderer {
    device: wgpu::Device,
    over: wgpu::RenderPipeline,
    add: wgpu::RenderPipeline,
    globals_layout: wgpu::BindGroupLayout,
    tex_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    font_view: wgpu::TextureView,
    textures: Vec<wgpu::TextureView>,
    groups: BTreeMap<(usize, usize), wgpu::BindGroup>,
    srgb: bool,
}

fn texture_entry(binding: u32, filterable: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

/// Uploads `data` (`w·h·4` bytes) as an `Rgba8Unorm` texture.
pub(super) fn upload_rgba8(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    w: u32,
    h: u32,
    data: &[u8],
) -> wgpu::Texture {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    write_rgba8(queue, &texture, data);
    texture
}

/// Rewrites the whole of an `Rgba8Unorm` `texture` with `data`.
pub(super) fn write_rgba8(queue: &wgpu::Queue, texture: &wgpu::Texture, data: &[u8]) {
    let size = texture.size();
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size.width * 4),
            rows_per_image: Some(size.height),
        },
        size,
    );
}

/// `viewport` clipped to a `target` of the given size, as a scissor
/// rectangle `(x, y, w, h)`; `None` when nothing is left.
pub(super) fn scissor(viewport: Rect, target: (u32, u32)) -> Option<(u32, u32, u32, u32)> {
    let x0 = i64::from(viewport.x).clamp(0, i64::from(target.0));
    let y0 = i64::from(viewport.y).clamp(0, i64::from(target.1));
    let x1 = viewport.right().clamp(0, i64::from(target.0));
    let y1 = viewport.bottom().clamp(0, i64::from(target.1));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some((x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32))
}

/// A render pass that loads `view` and keeps the result.
pub(super) fn load_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    label: &str,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

/// Renders into a fresh `width × height` `Rgba8Unorm` texture (cleared to
/// transparent black) with `draw` and reads it back.
pub(super) fn render_offscreen(
    ctx: &GpuContext,
    width: u32,
    height: u32,
    draw: impl FnOnce(&mut wgpu::CommandEncoder, &wgpu::TextureView),
) -> Result<RgbaImage, GpuError> {
    let max = ctx.device().limits().max_texture_dimension_2d;
    if width == 0 || height == 0 || width > max || height > max {
        return Err(GpuError::InvalidArgument(format!(
            "image size {width}x{height} (each side must be 1..={max})"
        )));
    }
    let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("display offscreen"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = ctx
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("display offscreen"),
        });
    {
        let _clear = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("display offscreen clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    draw(&mut encoder, &view);
    let pixels = read_texture(ctx, encoder, &texture)?;
    Ok(RgbaImage {
        width,
        height,
        pixels,
    })
}

/// Submits `encoder` after copying the RGBA8 `texture` into a buffer and
/// returns its tightly packed rows (bounded waits, as 1.4's debug readback).
fn read_texture(
    ctx: &GpuContext,
    mut encoder: wgpu::CommandEncoder,
    texture: &wgpu::Texture,
) -> Result<Vec<u8>, GpuError> {
    let device = ctx.device();
    let size = texture.size();
    let (width, height) = (size.width, size.height);
    let row = width as usize * 4;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
    let padded = row.div_ceil(align) * align;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("display readback"),
        size: (padded * height as usize) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded as u32),
                rows_per_image: Some(height),
            },
        },
        size,
    );
    ctx.queue().submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    readback.map_async(wgpu::MapMode::Read, .., move |r| {
        let _ = tx.send(r);
    });
    crate::gpu::finish_map(device, &rx, crate::gpu::READBACK_WAIT_TIMEOUT)?;
    let mut out = Vec::with_capacity(row * height as usize);
    {
        let mapped = readback
            .get_mapped_range(..)
            .map_err(|e| GpuError::Readback(format!("get_mapped_range: {e:?}")))?;
        for y in 0..height as usize {
            out.extend_from_slice(&mapped[y * padded..y * padded + row]);
        }
    }
    readback.unmap();
    Ok(out)
}

impl QuadRenderer {
    pub(super) fn new(ctx: &GpuContext, format: wgpu::TextureFormat) -> QuadRenderer {
        let device = ctx.device();
        let source = format!(
            "const STROKE_HALF_WIDTH: f32 = {STROKE_HALF_WIDTH:?};\n{OUTPUT_WGSL}\n{QUAD_WGSL}"
        );
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("display quads"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("display quad globals"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(16),
                    },
                    count: None,
                },
                texture_entry(1, false),
            ],
        });
        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("display quad textures"),
            entries: &[
                texture_entry(0, true),
                texture_entry(1, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("display quads"),
            bind_group_layouts: &[Some(&globals_layout), Some(&tex_layout)],
            immediate_size: 0,
        });
        const ATTRIBUTES: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
            0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4
        ];
        let make = |label: &str, blend: wgpu::BlendState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: core::mem::size_of::<QuadInstance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &ATTRIBUTES,
                    })],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let over = make(
            "display quads over",
            wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
        );
        let add = make(
            "display quads add",
            wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::Zero,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
            },
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("display linear clamp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        // Font atlas: the 5×7 glyph bits, one 5-column slot per glyph.
        let (aw, ah) = (GLYPH_SLOTS * 5, 7);
        let mut atlas = vec![0u8; (aw * ah * 4) as usize];
        for slot in 0..GLYPH_SLOTS {
            let g = if slot == GLYPH_SLOTS - 1 {
                FONT_5X7.fallback()
            } else {
                FONT_5X7.glyph_or_fallback(char::from_u32(FIRST_CHAR + slot).unwrap_or(' '))
            };
            for y in 0..7 {
                for x in 0..5 {
                    if g.pixel(x, y) {
                        let o = ((y * aw + slot * 5 + x) * 4) as usize;
                        atlas[o..o + 4].copy_from_slice(&[255; 4]);
                    }
                }
            }
        }
        let font = upload_rgba8(device, ctx.queue(), "display font atlas", aw, ah, &atlas);
        let white = upload_rgba8(device, ctx.queue(), "display white", 1, 1, &[255; 4]);
        QuadRenderer {
            device: device.clone(),
            over,
            add,
            globals_layout,
            tex_layout,
            sampler,
            font_view: font.create_view(&Default::default()),
            textures: vec![white.create_view(&Default::default())],
            groups: BTreeMap::new(),
            srgb: format.is_srgb(),
        }
    }

    /// Uploads `image` (premultiplied) and returns its texture index.
    pub(super) fn add_image(&mut self, queue: &wgpu::Queue, image: &RgbaImage) -> usize {
        let tex = upload_rgba8(
            &self.device,
            queue,
            "display image",
            image.width.max(1),
            image.height.max(1),
            &image.premultiplied(),
        );
        self.textures.push(tex.create_view(&Default::default()));
        self.textures.len() - 1
    }

    fn group(&mut self, key: (usize, usize)) -> &wgpu::BindGroup {
        let (device, layout, sampler, textures) = (
            &self.device,
            &self.tex_layout,
            &self.sampler,
            &self.textures,
        );
        self.groups.entry(key).or_insert_with(|| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("display quad textures"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&textures[key.0]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&textures[key.1]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                ],
            })
        })
    }

    /// Records `draws` in order into `view` (a `target`-sized texture),
    /// clipped to `viewport`.
    pub(super) fn encode(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        target: (u32, u32),
        viewport: Rect,
        draws: &[QuadDraw],
    ) {
        let Some((sx, sy, sw, sh)) = scissor(viewport, target) else {
            return;
        };
        if draws.is_empty() {
            return;
        }
        for d in draws {
            self.group(d.tex);
        }
        let globals = [
            target.0 as f32,
            target.1 as f32,
            if self.srgb { 1.0 } else { 0.0 },
            0.0,
        ];
        let globals_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("display quad globals"),
                contents: bytemuck::cast_slice(&globals),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let globals_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("display quad globals"),
            layout: &self.globals_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: globals_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.font_view),
                },
            ],
        });
        let instances: Vec<QuadInstance> = draws.iter().map(|d| d.inst).collect();
        let inst_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("display quads"),
                contents: bytemuck::cast_slice(&instances),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let mut pass = load_pass(encoder, view, "display quads");
        pass.set_scissor_rect(sx, sy, sw, sh);
        pass.set_bind_group(0, &globals_group, &[]);
        pass.set_vertex_buffer(0, inst_buf.slice(..));
        let mut start = 0;
        while start < draws.len() {
            let (tex, blend) = (draws[start].tex, draws[start].blend);
            let mut end = start + 1;
            while end < draws.len() && draws[end].tex == tex && draws[end].blend == blend {
                end += 1;
            }
            pass.set_pipeline(match blend {
                Blend::Over => &self.over,
                Blend::Add => &self.add,
            });
            pass.set_bind_group(1, &self.groups[&tex], &[]);
            pass.draw(0..6, start as u32..end as u32);
            start = end;
        }
    }
}

/// WGSL sources of the display shaders (for naga validation in tests).
pub(super) fn quad_shader_source() -> String {
    format!("const STROKE_HALF_WIDTH: f32 = {STROKE_HALF_WIDTH:?};\n{OUTPUT_WGSL}\n{QUAD_WGSL}")
}
