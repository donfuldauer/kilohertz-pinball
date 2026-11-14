//! GPU side of the post chain.

use super::{LdrImage, PostConfig, PostFrame, ToneMap, screen_motion};
use crate::gpu::{GpuContext, GpuError};
use crate::pbr::{HDR_FORMAT, HdrImage};

/// Most balls motion-blurred per frame (further balls are drawn sharp).
pub const MAX_BLUR_BALLS: usize = 16;

/// The post shader module.
const POST_SOURCE: &str = include_str!("shaders/post.wgsl");

/// `(name, WGSL source)` of every 5.2 shader module, each complete on its
/// own (for naga validation): the post chain and the probe remap.
pub fn shader_sources() -> &'static [(&'static str, &'static str)] {
    static SOURCES: [(&str, &str); 2] = [
        ("post_chain", POST_SOURCE),
        ("probe_remap", crate::reflections::REMAP_SOURCE),
    ];
    &SOURCES
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct BlurBall {
    center_radius: [f32; 4],
    disp: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct PostUniform {
    params: [f32; 4],
    bloom: [f32; 4],
    flags: [f32; 4],
    balls: [BlurBall; MAX_BLUR_BALLS],
}

const _: () = assert!(core::mem::size_of::<PostUniform>() == 48 + 32 * MAX_BLUR_BALLS);

/// Post chain counters.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct PostStats {
    /// Frames encoded.
    pub frames: u64,
    /// Bloom mip levels (0 when bloom is off) in the last frame.
    pub bloom_levels: u32,
    /// Balls motion-blurred in the last frame.
    pub blurred_balls: u32,
    /// Longest streak (pixels, after the length clamp) in the last frame.
    pub max_blur_px: f32,
}

/// An HDR image uploaded as an [`HDR_FORMAT`] texture (feed a CPU-made image
/// into the chain without the PBR pass).
pub struct HdrInput {
    /// The texture (`TEXTURE_BINDING | COPY_DST | COPY_SRC`).
    pub texture: wgpu::Texture,
    /// A view of it.
    pub view: wgpu::TextureView,
}

/// f32 → IEEE half (round to nearest even; NaN stays NaN, overflow → inf).
fn f32_to_f16(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let mant = bits & 0x007f_ffff;
    if exp == 0xff {
        return sign | 0x7c00 | if mant != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = mant | 0x0080_0000;
        let shift = (14 - e) as u32;
        let half = 1u32 << (shift - 1);
        let rest = m & ((1u32 << shift) - 1);
        let mut h = m >> shift;
        if rest > half || (rest == half && h & 1 == 1) {
            h += 1;
        }
        return sign | h as u16;
    }
    let mut h = ((e as u32) << 10) | (mant >> 13);
    let rest = mant & 0x1fff;
    if rest > 0x1000 || (rest == 0x1000 && h & 1 == 1) {
        h += 1;
    }
    sign | h as u16
}

impl HdrInput {
    /// Uploads `image` (linear RGBA; values are rounded to half floats).
    pub fn from_image(ctx: &GpuContext, image: &HdrImage) -> Result<HdrInput, GpuError> {
        let (w, h) = (image.width, image.height);
        if w == 0 || h == 0 || image.pixels.len() != (w as usize) * (h as usize) {
            return Err(GpuError::InvalidArgument(format!(
                "HDR image {w}x{h} with {} pixels",
                image.pixels.len()
            )));
        }
        let max = ctx.device().limits().max_texture_dimension_2d;
        if w > max || h > max {
            return Err(GpuError::InvalidArgument(format!(
                "HDR image {w}x{h} (max side {max})"
            )));
        }
        let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("post hdr input"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let mut bytes = Vec::with_capacity(image.pixels.len() * 8);
        for p in &image.pixels {
            for c in p {
                bytes.extend_from_slice(&f32_to_f16(*c).to_le_bytes());
            }
        }
        ctx.queue().write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 8),
                rows_per_image: Some(h),
            },
            texture.size(),
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(HdrInput { texture, view })
    }
}

struct Level {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

fn level(device: &wgpu::Device, label: &str, w: u32, h: u32, format: wgpu::TextureFormat) -> Level {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: w.max(1),
            height: h.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Level { texture, view }
}

/// Size-dependent textures.
struct Targets {
    width: u32,
    height: u32,
    /// Exposed, motion-blurred scene.
    scene: Level,
    /// Bloom source (prefiltered scene).
    bright: Level,
    /// Downsample chain: `down[i]` is `w`×`h` halved `i + 1` times,
    /// rounding up ([`level_size`]).
    down: Vec<Level>,
    /// Upsample chain: `up[i]` has the size of `down[i]`, `i < levels − 1`.
    up: Vec<Level>,
    ldr: Level,
    hdr: Level,
}

/// Size of the next bloom level: `ceil(n / 2)` (an odd last row or column
/// gets a half-covered texel, so no source texel is dropped).
fn half_up(n: u32) -> u32 {
    n.div_ceil(2).max(1)
}

/// Bloom levels for a `w`×`h` image: halvings (rounding up) while both
/// sides are ≥ 2, at most `max_levels`, at least 1.
fn bloom_levels(w: u32, h: u32, max_levels: u32) -> usize {
    let mut n = 0;
    let (mut a, mut b) = (w, h);
    while n < max_levels.clamp(1, 16) && a >= 2 && b >= 2 {
        a = half_up(a);
        b = half_up(b);
        n += 1;
    }
    n.max(1) as usize
}

/// Size of bloom level `i` (`0` = half the image) of a `w`×`h` image.
fn level_size(w: u32, h: u32, i: usize) -> (u32, u32) {
    let (mut a, mut b) = (half_up(w), half_up(h));
    for _ in 0..i {
        a = half_up(a);
        b = half_up(b);
    }
    (a, b)
}

fn make_targets(
    device: &wgpu::Device,
    w: u32,
    h: u32,
    max_levels: u32,
    output: wgpu::TextureFormat,
) -> Targets {
    let n = bloom_levels(w, h, max_levels);
    let size = |i: usize| level_size(w, h, i);
    let down = (0..n)
        .map(|i| {
            let (a, b) = size(i);
            level(device, "post bloom down", a, b, HDR_FORMAT)
        })
        .collect();
    let up = (0..n.saturating_sub(1))
        .map(|i| {
            let (a, b) = size(i);
            level(device, "post bloom up", a, b, HDR_FORMAT)
        })
        .collect();
    Targets {
        width: w,
        height: h,
        scene: level(device, "post scene", w, h, HDR_FORMAT),
        bright: level(device, "post bright", w, h, HDR_FORMAT),
        down,
        up,
        ldr: level(device, "post ldr", w, h, output),
        hdr: level(device, "post hdr out", w, h, HDR_FORMAT),
    }
}

/// The HDR post chain (see the [module docs](super)).
///
/// [`encode`](Self::encode) records the whole chain from an [`HDR_FORMAT`]
/// view of the chain's size into an output view of the chain's output
/// format. The frame uniform is uploaded with `queue.write_buffer`, so
/// encode at most one post frame per submitted command buffer.
pub struct PostChain {
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: PostConfig,
    output_format: wgpu::TextureFormat,
    encode_srgb: bool,
    uniform: wgpu::Buffer,
    uniform_bind: wgpu::BindGroup,
    tex_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    blur: wgpu::RenderPipeline,
    prefilter: wgpu::RenderPipeline,
    down: wgpu::RenderPipeline,
    up: wgpu::RenderPipeline,
    composite_ldr: wgpu::RenderPipeline,
    composite_hdr: wgpu::RenderPipeline,
    targets: Targets,
    stats: PostStats,
}

fn check_size(device: &wgpu::Device, w: u32, h: u32) -> Result<(), GpuError> {
    let max = device.limits().max_texture_dimension_2d;
    if w == 0 || h == 0 || w > max || h > max {
        return Err(GpuError::InvalidArgument(format!(
            "post size {w}x{h} (each side must be 1..={max})"
        )));
    }
    Ok(())
}

/// Whether `format` is an 8-bit RGBA / BGRA colour format the chain can
/// write and read back, and whether it is sRGB.
fn output_kind(format: wgpu::TextureFormat) -> Option<(bool, bool)> {
    use wgpu::TextureFormat as F;
    match format {
        F::Rgba8Unorm => Some((false, false)),
        F::Rgba8UnormSrgb => Some((true, false)),
        F::Bgra8Unorm => Some((false, true)),
        F::Bgra8UnormSrgb => Some((true, true)),
        _ => None,
    }
}

impl PostChain {
    /// A chain for `width`×`height` images writing `output_format` (8-bit
    /// RGBA or BGRA, sRGB or not; a non-sRGB format gets the sRGB curve in
    /// the shader). Starts with [`PostConfig::default`].
    pub fn new(
        ctx: &GpuContext,
        width: u32,
        height: u32,
        output_format: wgpu::TextureFormat,
    ) -> Result<PostChain, GpuError> {
        let device = ctx.device();
        // A shader or pipeline validation error becomes a `GpuError`
        // instead of reaching wgpu's default (panicking) handler; dropping
        // the guard on an early return pops the scope.
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        check_size(device, width, height)?;
        let Some((is_srgb, _)) = output_kind(output_format) else {
            return Err(GpuError::InvalidArgument(format!(
                "post output format {output_format:?} (need Rgba8/Bgra8, Unorm or UnormSrgb)"
            )));
        };
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("post uniform"),
            size: core::mem::size_of::<PostUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post uniform"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(
                        core::mem::size_of::<PostUniform>() as u64
                    ),
                },
                count: None,
            }],
        });
        let uniform_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("post uniform"),
            layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let tex = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post inputs"),
            entries: &[
                tex(0),
                tex(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("post linear clamp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("post_chain"),
            source: wgpu::ShaderSource::Wgsl(POST_SOURCE.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("post"),
            bind_group_layouts: &[Some(&uniform_layout), Some(&tex_layout)],
            immediate_size: 0,
        });
        let pipeline = |label: &str, entry: &str, format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_fullscreen"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let config = PostConfig::default();
        let chain = PostChain {
            device: device.clone(),
            queue: ctx.queue().clone(),
            output_format,
            encode_srgb: !is_srgb,
            blur: pipeline("post blur", "fs_blur", HDR_FORMAT),
            prefilter: pipeline("post prefilter", "fs_prefilter", HDR_FORMAT),
            down: pipeline("post down", "fs_down", HDR_FORMAT),
            up: pipeline("post up", "fs_up", HDR_FORMAT),
            composite_ldr: pipeline("post composite", "fs_composite", output_format),
            composite_hdr: pipeline("post composite hdr", "fs_composite", HDR_FORMAT),
            targets: make_targets(
                device,
                width,
                height,
                config.bloom.max_levels,
                output_format,
            ),
            config,
            uniform,
            uniform_bind,
            tex_layout,
            sampler,
            stats: PostStats::default(),
        };
        // Pipelines and targets are created above, inside the scope.
        if let Some(e) = pollster::block_on(scope.pop()) {
            return Err(GpuError::InvalidArgument(format!(
                "post chain shaders/pipelines: {e}"
            )));
        }
        Ok(chain)
    }

    /// The configuration.
    pub fn config(&self) -> &PostConfig {
        &self.config
    }

    /// Replaces the configuration (the bloom chain is rebuilt when
    /// `bloom.max_levels` changes).
    pub fn set_config(&mut self, config: PostConfig) {
        let rebuild = config.bloom.max_levels != self.config.bloom.max_levels;
        self.config = config;
        if rebuild {
            let (w, h) = self.size();
            self.targets = make_targets(
                &self.device,
                w,
                h,
                config.bloom.max_levels,
                self.output_format,
            );
        }
    }

    /// Image size `(width, height)`.
    pub fn size(&self) -> (u32, u32) {
        (self.targets.width, self.targets.height)
    }

    /// The output format.
    pub fn output_format(&self) -> wgpu::TextureFormat {
        self.output_format
    }

    /// Resizes the internal targets (no-op when unchanged).
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), GpuError> {
        check_size(&self.device, width, height)?;
        if (width, height) != self.size() {
            self.targets = make_targets(
                &self.device,
                width,
                height,
                self.config.bloom.max_levels,
                self.output_format,
            );
        }
        Ok(())
    }

    /// Counters.
    pub fn stats(&self) -> PostStats {
        self.stats
    }

    /// The chain's own output texture (output format; what
    /// [`render_to_ldr`](Self::render_to_ldr) reads back).
    pub fn output_texture(&self) -> &wgpu::Texture {
        &self.targets.ldr.texture
    }

    /// A view of [`output_texture`](Self::output_texture).
    pub fn output_view(&self) -> &wgpu::TextureView {
        &self.targets.ldr.view
    }

    fn bind(&self, a: &wgpu::TextureView, b: &wgpu::TextureView) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("post inputs"),
            layout: &self.tex_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(a),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(b),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    fn pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        label: &str,
        pipeline: &wgpu::RenderPipeline,
        inputs: &wgpu::BindGroup,
        target: &wgpu::TextureView,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
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
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &self.uniform_bind, &[]);
        pass.set_bind_group(1, inputs, &[]);
        pass.draw(0..3, 0..1);
    }

    /// The frame uniform for `frame` (`hdr_out`: stop before tone mapping).
    fn uniform_for(&mut self, frame: &PostFrame<'_>, hdr_out: bool) -> PostUniform {
        let cfg = &self.config;
        let (w, h) = self.size();
        let mb = &cfg.motion_blur;
        let mut balls = [BlurBall {
            center_radius: [0.0; 4],
            disp: [0.0; 4],
        }; MAX_BLUR_BALLS];
        let mut n = 0usize;
        let mut longest = 0.0f32;
        if mb.enabled && mb.shutter_s.is_finite() && mb.shutter_s > 0.0 {
            let max_len = if mb.max_length_px.is_finite() {
                mb.max_length_px.max(0.0)
            } else {
                0.0
            };
            for ball in frame.balls.iter().take(MAX_BLUR_BALLS) {
                let Some(m) = screen_motion(frame.camera, ball, mb.shutter_s, w, h) else {
                    continue;
                };
                let mut d = m.displacement_px;
                let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
                if len > max_len {
                    let s = max_len / len;
                    d = [d[0] * s, d[1] * s];
                }
                let len = len.min(max_len);
                if len < 1e-3 {
                    continue;
                }
                longest = longest.max(len);
                balls[n] = BlurBall {
                    center_radius: [m.center_px[0], m.center_px[1], m.radius_px, 0.0],
                    disp: [d[0], d[1], 0.0, 0.0],
                };
                n += 1;
            }
        }
        self.stats.blurred_balls = n as u32;
        self.stats.max_blur_px = longest;
        let exposure = if cfg.exposure.is_finite() {
            cfg.exposure.max(0.0)
        } else {
            0.0
        };
        let b = &cfg.bloom;
        let clean = |x: f32| if x.is_finite() { x } else { 0.0 };
        PostUniform {
            params: [
                exposure,
                n as f32,
                mb.samples.clamp(1, 64) as f32,
                if cfg.tone_map == ToneMap::Linear {
                    1.0
                } else {
                    0.0
                },
            ],
            bloom: [
                clean(b.threshold).max(0.0),
                clean(b.knee).max(0.0),
                clean(b.firefly_clamp),
                clean(b.intensity).clamp(0.0, 1.0),
            ],
            flags: [
                clean(b.radius).clamp(0.0, 1.0),
                if self.encode_srgb { 1.0 } else { 0.0 },
                if hdr_out { 1.0 } else { 0.0 },
                if b.enabled { 1.0 } else { 0.0 },
            ],
            balls,
        }
    }

    /// Records the chain: `input` (an [`HDR_FORMAT`] view of the chain's
    /// size, linear radiance) → `output` (a view of the output format).
    pub fn encode(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        input: &wgpu::TextureView,
        frame: &PostFrame<'_>,
        output: &wgpu::TextureView,
    ) {
        self.encode_impl(encoder, input, frame, Some(output), false);
    }

    fn encode_impl(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        input: &wgpu::TextureView,
        frame: &PostFrame<'_>,
        output: Option<&wgpu::TextureView>,
        hdr_out: bool,
    ) {
        let uniform = self.uniform_for(frame, hdr_out);
        self.queue
            .write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
        self.stats.frames += 1;
        let t = &self.targets;
        // 1. Exposure + motion blur.
        let g = self.bind(input, input);
        self.pass(encoder, "post blur", &self.blur, &g, &t.scene.view);
        // 2. Bloom.
        let bloom_on = self.config.bloom.enabled;
        if bloom_on {
            let g = self.bind(&t.scene.view, &t.scene.view);
            self.pass(
                encoder,
                "post prefilter",
                &self.prefilter,
                &g,
                &t.bright.view,
            );
            let mut src = &t.bright.view;
            for d in &t.down {
                let g = self.bind(src, src);
                self.pass(encoder, "post down", &self.down, &g, &d.view);
                src = &d.view;
            }
            let n = t.down.len();
            for i in (0..n.saturating_sub(1)).rev() {
                let coarse = if i + 1 == n - 1 {
                    &t.down[n - 1].view
                } else {
                    &t.up[i + 1].view
                };
                let g = self.bind(&t.down[i].view, coarse);
                self.pass(encoder, "post up", &self.up, &g, &t.up[i].view);
            }
        }
        self.stats.bloom_levels = if bloom_on { t.down.len() as u32 } else { 0 };
        // 3. Composite.
        let finest = if t.up.is_empty() {
            &t.down[0].view
        } else {
            &t.up[0].view
        };
        let g = self.bind(&t.scene.view, finest);
        let (pipeline, target) = if hdr_out {
            (&self.composite_hdr, &t.hdr.view)
        } else {
            (&self.composite_ldr, output.unwrap_or(&t.ldr.view))
        };
        self.pass(encoder, "post composite", pipeline, &g, target);
    }

    /// Runs the chain on `input` into the chain's own output and reads it
    /// back (display image; BGRA outputs are swizzled to RGBA).
    pub fn render_to_ldr(
        &mut self,
        ctx: &GpuContext,
        input: &wgpu::TextureView,
        frame: &PostFrame<'_>,
    ) -> Result<LdrImage, GpuError> {
        let mut encoder = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("post ldr"),
            });
        self.encode_impl(&mut encoder, input, frame, None, false);
        ctx.queue().submit([encoder.finish()]);
        self.read_ldr(ctx)
    }

    /// Reads the chain's own output texture back.
    pub(crate) fn read_ldr(&self, ctx: &GpuContext) -> Result<LdrImage, GpuError> {
        let bytes = super::readback::read_layer(ctx, &self.targets.ldr.texture, 0, 4)?;
        let bgra = output_kind(self.output_format).is_some_and(|(_, b)| b);
        let pixels = bytes
            .chunks_exact(4)
            .map(|p| {
                if bgra {
                    [p[2], p[1], p[0], p[3]]
                } else {
                    [p[0], p[1], p[2], p[3]]
                }
            })
            .collect();
        let (width, height) = self.size();
        Ok(LdrImage {
            width,
            height,
            pixels,
        })
    }

    /// Runs the chain on `input` up to (not including) tone mapping and
    /// reads the linear composite back: exposure · motion blur · bloom.
    pub fn render_to_hdr(
        &mut self,
        ctx: &GpuContext,
        input: &wgpu::TextureView,
        frame: &PostFrame<'_>,
    ) -> Result<HdrImage, GpuError> {
        let mut encoder = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("post hdr"),
            });
        self.encode_impl(&mut encoder, input, frame, None, true);
        ctx.queue().submit([encoder.finish()]);
        super::readback::read_hdr_layer(ctx, &self.targets.hdr.texture, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_float_encoding_round_trips() {
        use super::super::readback::f16_to_f32;
        for v in [
            0.0f32, 1.0, -2.0, 0.5, 65504.0, 1e-7, 6.1e-5, 3.0e-6, 0.333, 1234.5,
        ] {
            let h = f32_to_f16(v);
            let back = f16_to_f32(h);
            assert!(
                (back - v).abs() <= v.abs() * 1e-3 + 6e-8,
                "{v} -> {h:#x} -> {back}"
            );
        }
        assert_eq!(f32_to_f16(1e6), 0x7c00);
        assert_eq!(f32_to_f16(f32::INFINITY), 0x7c00);
        assert!(f16_to_f32(f32_to_f16(f32::NAN)).is_nan());
        assert_eq!(f32_to_f16(1e-9), 0);
        assert_eq!(f32_to_f16(-0.0), 0x8000);
        // Round to nearest even: 1 + 2^-11 is halfway between 1 and the next half.
        assert_eq!(f32_to_f16(1.0 + 1.0 / 2048.0), 0x3c00);
        assert_eq!(f32_to_f16(1.0 + 3.0 / 2048.0), 0x3c02);
    }

    #[test]
    fn bloom_levels_follow_the_size() {
        assert_eq!(bloom_levels(128, 128, 6), 6);
        assert_eq!(bloom_levels(128, 128, 0), 1);
        assert_eq!(bloom_levels(4, 64, 6), 2);
        assert_eq!(bloom_levels(1, 1, 6), 1);
        assert_eq!(bloom_levels(3, 3, 99), 2);
        assert_eq!(bloom_levels(2, 2, 99), 1);
        assert_eq!(level_size(127, 93, 0), (64, 47));
        assert_eq!(level_size(127, 93, 1), (32, 24));
        assert_eq!(level_size(1919, 1079, 5), (30, 17));
        assert!(output_kind(wgpu::TextureFormat::Rgba16Float).is_none());
        assert_eq!(
            output_kind(wgpu::TextureFormat::Bgra8UnormSrgb),
            Some((true, true))
        );
    }
}
