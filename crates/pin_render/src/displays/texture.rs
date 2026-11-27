//! Render-to-texture displays (KP-017, REQ-VIEW-002): the translite and the
//! score display drawn into sampleable textures for the 3D backbox.
//!
//! Each display owns one [`DISPLAY_TEXTURE_FORMAT`] texture with a full mip
//! chain. Mip level 0 is drawn by the existing renderer
//! ([`BackglassRenderer`] / [`ScoreRenderer`]) over the whole texture, exactly
//! as `render_*_to_image` draws an offscreen image of the same size; levels
//! 1.. are an area-weighted average of the level above (the 2 × 2 box
//! average for an exact 2:1 reduction; on an odd side every texel
//! contributes by its overlap), computed on the stored (display-encoded)
//! bytes in integers with round-to-nearest.
//!
//! Each level is rounded to nearest on its own, from the rounded bytes of
//! the level above. The 1 × 1 level is therefore the area-weighted average
//! computed level by level: every texel of level 0 contributes to it, but it
//! need not equal the original mean rounded once. The area weighting keeps
//! the mean of a level equal to the mean of the level above before rounding,
//! so each reduction moves the mean by at most half a byte, and the 1 × 1
//! byte is within half a byte times the number of reductions of the
//! original mean. Example: a 5 × 1 image with red bytes `[1, 1, 0, 0, 0]`
//! reduces to `[1, 0]` and then `[1]`, while its mean 0.4 rounds to 0.
//!
//! A redraw happens only when the content changed (DMD
//! frame, LCD draw list, lamp levels per
//! [`BackglassRenderer::needs_redraw`]) or after `mark_dirty`; it redraws
//! level 0 and regenerates every mip in one submit. The GPU texture object is
//! created once and kept for the life of the display, so a material bound to
//! [`BackglassTexture::view`] / [`ScoreTexture::view`] never needs rebuilding.
//!
//! LCD images: the owner of an LCD [`ScoreTexture`] registers every image the
//! scenes' image layers use with [`ScoreTexture::set_image`] (keyed by the
//! layer's `file`). An image item whose file is not registered is skipped
//! when drawing: a missing bundle image never aborts rendering (untrusted
//! bundles, D25), and registering the file is the way to draw it.

use super::{
    BackglassDesc, BackglassRenderer, DisplayContent, DmdSpec, RgbaImage, ScoreDisplayConfig,
    ScoreRenderer,
};
use crate::gpu::{GpuContext, GpuError};
use crate::topology::Rect;

/// Format of the display textures: `Rgba8Unorm`, a non-sRGB format, so the
/// stored bytes are the display-encoded values the renderers write (the
/// colour convention of the displays module; no extra encode step). A
/// sampler reading it returns display-encoded values; the consumer decodes
/// to linear if it lights in linear space.
pub const DISPLAY_TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Number of mip levels of a full chain for a `width × height` texture:
/// `1 + floor(log2(max(width, height)))`, or 0 when either side is 0 (no
/// texture of that size exists).
pub fn mip_level_count(width: u32, height: u32) -> u32 {
    if width == 0 || height == 0 {
        return 0;
    }
    u32::BITS - width.max(height).leading_zeros()
}

/// Refuses a side of 0 or above `max` (the device's
/// `max_texture_dimension_2d`).
pub(super) fn check_texture_size(width: u32, height: u32, max: u32) -> Result<(), GpuError> {
    if width == 0 || height == 0 || width > max || height > max {
        return Err(GpuError::InvalidArgument(format!(
            "display texture size {width}x{height} (each side must be 1..={max})"
        )));
    }
    Ok(())
}

/// Refuses a score display `config` its renderer cannot build without a
/// wgpu panic: a DMD spec outside [`DmdSpec::new`]'s bounds, or whose dot
/// texture has a side above `max` (the device's `max_texture_dimension_2d`).
/// An LCD config creates no canvas-sized texture and is accepted.
fn check_score_config(config: &ScoreDisplayConfig, max: u32) -> Result<(), GpuError> {
    match *config {
        ScoreDisplayConfig::Dmd(spec) => {
            DmdSpec::new(spec.width, spec.height, spec.color)
                .map_err(|e| GpuError::InvalidArgument(format!("score display: {e}")))?;
            if spec.width > max || spec.height > max {
                return Err(GpuError::InvalidArgument(format!(
                    "score display: DMD dot texture {}x{} (each side must be 1..={max})",
                    spec.width, spec.height
                )));
            }
            Ok(())
        }
        ScoreDisplayConfig::Lcd(_) => Ok(()),
    }
}

/// Area-weighted mip step: one fullscreen triangle per level, reading the
/// level above with `textureLoad` and averaging its bytes with integer
/// weights, rounded to nearest (halves round up).
///
/// Per axis, a source side `s` maps to `t = max(1, s / 2)` destination
/// texels; destination texel `d` covers the source span
/// `[d * s / t, (d + 1) * s / t)` and each source texel is weighted by its
/// overlap with that span:
///
/// * `s` even (an exact 2:1 axis): texels `2d, 2d + 1`, weights `1, 1` of 2;
/// * `s` odd and above 1: texels `2d, 2d + 1, 2d + 2`, weights
///   `t - d, t, d + 1` of `s` (every texel of the axis contributes; with
///   `s = 3` the one destination texel covers the whole axis);
/// * `s == 1`: texel 0, weight 1 of 1.
///
/// The two axes combine as the product of their weights over the divisor
/// `D = Dx * Dy`; the result is `floor((S + floor(D / 2)) / D)` of the
/// weighted byte sum `S`, so an exact 2 × 2 reduction is
/// `(sum + 2) / 4`, the plain four-texel average. `S` can exceed 32 bits on
/// large odd sides, so it is carried as quotient and remainder per row
/// (every intermediate stays below `2.5 * D`, within `u32` for sides up to
/// 32 768). All arithmetic on bytes is integer, so the result is the same
/// on every adapter.
const MIP_WGSL: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let x = f32((vi << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(vi & 2u) * 2.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

fn load_bytes(p: vec2<u32>, last: vec2<u32>) -> vec4<u32> {
    let c = textureLoad(src, vec2<i32>(min(p, last)), 0);
    return vec4<u32>(round(clamp(c, vec4<f32>(0.0), vec4<f32>(1.0)) * 255.0));
}

// Weights of the source texels 2d, 2d + 1, 2d + 2 for destination texel d
// of a source side s, and their divisor (w).
fn axis_weights(s: u32, d: u32) -> vec4<u32> {
    if (s == 1u) {
        return vec4<u32>(1u, 0u, 0u, 1u);
    }
    if ((s & 1u) == 0u) {
        return vec4<u32>(1u, 1u, 0u, 2u);
    }
    let t = s / 2u;
    return vec4<u32>(t - d, t, d + 1u, s);
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let size = textureDimensions(src);
    let last = size - vec2<u32>(1u, 1u);
    let d = vec2<u32>(floor(pos.xy));
    let base = d * 2u;
    let wx = axis_weights(size.x, d.x);
    let wy = axis_weights(size.y, d.y);
    // S = dx * a + b with a = sum wy * (row / dx), b = sum wy * (row % dx).
    var a = vec4<u32>(0u);
    var b = vec4<u32>(0u);
    for (var j = 0u; j < 3u; j = j + 1u) {
        let wyj = wy[j];
        if (wyj == 0u) {
            continue;
        }
        var row = vec4<u32>(0u);
        for (var i = 0u; i < 3u; i = i + 1u) {
            let wxi = wx[i];
            if (wxi == 0u) {
                continue;
            }
            row = row + wxi * load_bytes(base + vec2<u32>(i, j), last);
        }
        a = a + wyj * (row / wx.w);
        b = b + wyj * (row % wx.w);
    }
    let den = wx.w * wy.w;
    let avg = a / wy.w + (wx.w * (a % wy.w) + b + den / 2u) / den;
    return vec4<f32>(avg) / 255.0;
}
"#;

/// WGSL of the mip generator (for naga validation in tests).
#[cfg(test)]
pub(super) fn mip_shader_source() -> &'static str {
    MIP_WGSL
}

/// Per-axis weights `[w0, w1, w2, divisor]` of the source texels `2d`,
/// `2d + 1`, `2d + 2` for destination texel `d` of a source side `s`, as in
/// `axis_weights` of [`MIP_WGSL`].
#[cfg(test)]
pub(super) fn mip_axis_weights(s: u32, d: u32) -> [u32; 4] {
    if s == 1 {
        [1, 0, 0, 1]
    } else if s.is_multiple_of(2) {
        [1, 1, 0, 2]
    } else {
        let t = s / 2;
        [t - d, t, d + 1, s]
    }
}

/// CPU reference of one step of [`MIP_WGSL`]: the next level of the
/// `w × h` level `src` (row-major texels), with its size
/// `(max(1, w / 2), max(1, h / 2))`. Each byte is the area-weighted sum
/// `S` over the divisor `D` of [`mip_axis_weights`], computed exactly in
/// 64 bits and rounded to nearest with halves up: `(S + D / 2) / D`.
#[cfg(test)]
pub(super) fn mip_reference(src: &[[u8; 4]], w: u32, h: u32) -> (Vec<[u8; 4]>, u32, u32) {
    assert_eq!(src.len(), (w as usize) * (h as usize), "mip_reference size");
    let (w2, h2) = ((w / 2).max(1), (h / 2).max(1));
    let at = |x: u32, y: u32| src[(y.min(h - 1) * w + x.min(w - 1)) as usize];
    let mut out = Vec::with_capacity((w2 * h2) as usize);
    for y in 0..h2 {
        let wy = mip_axis_weights(h, y);
        for x in 0..w2 {
            let wx = mip_axis_weights(w, x);
            let den = u64::from(wx[3]) * u64::from(wy[3]);
            let mut texel = [0u8; 4];
            for (c, out_c) in texel.iter_mut().enumerate() {
                let mut sum = 0u64;
                for j in 0..3 {
                    for i in 0..3 {
                        let weight = u64::from(wx[i as usize]) * u64::from(wy[j as usize]);
                        sum += weight * u64::from(at(2 * x + i, 2 * y + j)[c]);
                    }
                }
                *out_c = u8::try_from((sum + den / 2) / den).expect("weighted mean of bytes");
            }
            out.push(texel);
        }
    }
    (out, w2, h2)
}

/// The final division of [`MIP_WGSL`] in its 32-bit form: given the weighted
/// row sums `rows[j] = (wy_j, sum_i wx_i * byte_ij)` and the divisors
/// `dx`, `dy`, returns `(S + D / 2) / D` for `S = sum_j wy_j * row_j`,
/// `D = dx * dy`, or `None` if any intermediate overflows `u32`.
#[cfg(test)]
pub(super) fn mip_split_divide(rows: &[(u32, u32)], dx: u32, dy: u32) -> Option<u32> {
    let (mut a, mut b) = (0u32, 0u32);
    for &(wy, row) in rows {
        a = a.checked_add(wy.checked_mul(row / dx)?)?;
        b = b.checked_add(wy.checked_mul(row % dx)?)?;
    }
    let den = dx.checked_mul(dy)?;
    let rest = dx
        .checked_mul(a % dy)?
        .checked_add(b)?
        .checked_add(den / 2)?;
    Some(a / dy + rest / den)
}

/// The texture, its views and the mip generator shared by both displays.
/// The redraw encoder is created on, and submitted to, the device and queue
/// of the context the texture was created with (kept here), never a
/// caller's (#163 round 12, finding 21).
struct MipTexture {
    device: wgpu::Device,
    queue: wgpu::Queue,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// One single-level view per mip level (render targets).
    level_views: Vec<wgpu::TextureView>,
    /// `mip_groups[k]` binds level `k` as the source of level `k + 1`.
    mip_groups: Vec<wgpu::BindGroup>,
    mip_pipeline: wgpu::RenderPipeline,
    size: (u32, u32),
    redraws: u64,
}

impl MipTexture {
    fn new(ctx: &GpuContext, label: &str, width: u32, height: u32) -> Result<MipTexture, GpuError> {
        let device = ctx.device();
        check_texture_size(width, height, device.limits().max_texture_dimension_2d)?;
        let levels = mip_level_count(width, height);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DISPLAY_TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some(label),
            dimension: Some(wgpu::TextureViewDimension::D2),
            ..Default::default()
        });
        let level_views: Vec<wgpu::TextureView> = (0..levels)
            .map(|k| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("display texture level"),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_mip_level: k,
                    mip_level_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("display mips"),
            source: wgpu::ShaderSource::Wgsl(MIP_WGSL.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("display mips"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("display mips"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let mip_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("display mips"),
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
                targets: &[Some(wgpu::ColorTargetState {
                    format: DISPLAY_TEXTURE_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let mip_groups = level_views
            .iter()
            .take(level_views.len().saturating_sub(1))
            .map(|v| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("display mips"),
                    layout: &layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(v),
                    }],
                })
            })
            .collect();
        let t = MipTexture {
            device: device.clone(),
            queue: ctx.queue().clone(),
            texture,
            view,
            level_views,
            mip_groups,
            mip_pipeline,
            size: (width, height),
            redraws: 0,
        };
        // Every level starts transparent black.
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("display texture clear"),
        });
        for v in &t.level_views {
            clear_pass(&mut encoder, v);
        }
        t.queue.submit([encoder.finish()]);
        Ok(t)
    }

    fn levels(&self) -> u32 {
        self.level_views.len() as u32
    }

    /// Clears level 0, records `draw` into it over the whole texture,
    /// regenerates every mip and submits it all at once, on the owning
    /// device and queue.
    fn redraw(
        &mut self,
        draw: impl FnOnce(&mut wgpu::CommandEncoder, &wgpu::TextureView, (u32, u32), Rect),
    ) {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("display texture redraw"),
            });
        let (w, h) = self.size;
        // Clearing first makes level 0 match `render_*_to_image`, which draws
        // into a freshly cleared texture.
        clear_pass(&mut encoder, &self.level_views[0]);
        draw(
            &mut encoder,
            &self.level_views[0],
            self.size,
            Rect::new(0, 0, w, h),
        );
        for (dst, group) in self.level_views.iter().skip(1).zip(&self.mip_groups) {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("display mip"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: dst,
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
            pass.set_pipeline(&self.mip_pipeline);
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        self.redraws += 1;
    }
}

/// A pass that clears `view` to transparent black.
fn clear_pass(encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) {
    let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("display texture clear"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
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

/// The translite (layers, lamp masks, flashers) rendered into a sampleable
/// mip-mapped texture for the 3D backbox (REQ-VIEW-002).
pub struct BackglassTexture {
    renderer: BackglassRenderer,
    target: MipTexture,
}

impl BackglassTexture {
    /// Builds the renderer for [`DISPLAY_TEXTURE_FORMAT`] and a
    /// `width × height` texture with [`mip_level_count`] levels, every level
    /// transparent black. A side of 0 or above the device's
    /// `max_texture_dimension_2d`, or an image refused by
    /// [`BackglassRenderer::new`], is a [`GpuError::InvalidArgument`].
    pub fn new(
        ctx: &GpuContext,
        desc: &BackglassDesc,
        width: u32,
        height: u32,
    ) -> Result<BackglassTexture, GpuError> {
        check_texture_size(
            width,
            height,
            ctx.device().limits().max_texture_dimension_2d,
        )?;
        let renderer = BackglassRenderer::new(ctx, DISPLAY_TEXTURE_FORMAT, desc)?;
        let target = MipTexture::new(ctx, "backglass texture", width, height)?;
        Ok(BackglassTexture { renderer, target })
    }

    /// Redraws level 0 with `lamps` (indexed by `LampId`) and regenerates
    /// every mip, in one submit, only when
    /// [`BackglassRenderer::needs_redraw`] says so (first call, after
    /// [`mark_dirty`](Self::mark_dirty), or a referenced lamp moved by at
    /// least [`LAMP_REDRAW_EPSILON`](super::LAMP_REDRAW_EPSILON)). Returns
    /// whether it redrew; otherwise no GPU work is recorded and nothing is
    /// allocated.
    ///
    /// `ctx` must be the context `self` was created with. The redraw is
    /// recorded on that creating context's device and submitted to its
    /// queue (both kept by `self`), never `ctx`'s (#163 round 12, finding
    /// 21).
    pub fn update(&mut self, ctx: &GpuContext, lamps: &[f32]) -> bool {
        let _ = ctx; // the owning device and queue are used (see above)
        if !self.renderer.needs_redraw(lamps) {
            return false;
        }
        let renderer = &mut self.renderer;
        self.target.redraw(|encoder, view, size, viewport| {
            renderer.encode(encoder, view, size, viewport, lamps);
        });
        true
    }

    /// Makes the next [`update`](Self::update) redraw.
    pub fn mark_dirty(&mut self) {
        self.renderer.mark_dirty();
    }

    /// The texture; the same object for the life of `self`.
    pub fn texture(&self) -> &wgpu::Texture {
        &self.target.texture
    }

    /// A 2D view of every mip level, for sampling.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.target.view
    }

    /// Size of mip level 0, `(width, height)`.
    pub fn size(&self) -> (u32, u32) {
        self.target.size
    }

    /// Number of mip levels ([`mip_level_count`] of [`size`](Self::size)).
    pub fn mip_level_count(&self) -> u32 {
        self.target.levels()
    }

    /// Number of redraws since [`new`](Self::new) (0 before the first
    /// [`update`](Self::update)).
    pub fn redraw_count(&self) -> u64 {
        self.target.redraws
    }

    /// Aspect of the translite ([`BackglassDesc::aspect`]); the art is
    /// letterboxed in black when the texture has another aspect.
    pub fn aspect(&self) -> (u32, u32) {
        self.renderer.aspect()
    }
}

/// The score display (DMD or LCD) rendered into a sampleable mip-mapped
/// texture for the 3D backbox (REQ-VIEW-002).
pub struct ScoreTexture {
    renderer: ScoreRenderer,
    target: MipTexture,
}

impl ScoreTexture {
    /// Builds the renderer of `config` for [`DISPLAY_TEXTURE_FORMAT`] and a
    /// `width × height` texture with [`mip_level_count`] levels, every level
    /// transparent black.
    ///
    /// Errors, all [`GpuError::InvalidArgument`] and never a wgpu panic
    /// (D25), checked before any GPU resource is created:
    ///
    /// * a `width` or `height` of 0 or above the device's
    ///   `max_texture_dimension_2d`;
    /// * a DMD `config` whose [`DmdSpec`] is outside the bounds of
    ///   [`DmdSpec::new`] (16..=512 × 8..=256 dots; the fields are public, so
    ///   a spec can bypass that constructor) or whose dot texture
    ///   (`spec.width × spec.height`) exceeds `max_texture_dimension_2d`
    ///   (#163 round 13, finding 16).
    ///
    /// An LCD `config` is not checked here: its renderer creates no
    /// canvas-sized texture (the canvas only scales the draw list), so no
    /// canvas size reaches a wgpu panic. Valid configurations are unchanged.
    pub fn new(
        ctx: &GpuContext,
        config: ScoreDisplayConfig,
        width: u32,
        height: u32,
    ) -> Result<ScoreTexture, GpuError> {
        let max = ctx.device().limits().max_texture_dimension_2d;
        check_texture_size(width, height, max)?;
        check_score_config(&config, max)?;
        let renderer = ScoreRenderer::new(ctx, DISPLAY_TEXTURE_FORMAT, config);
        let target = MipTexture::new(ctx, "score texture", width, height)?;
        Ok(ScoreTexture { renderer, target })
    }

    /// Registers the image that LCD scene image layers with `file = <file>`
    /// draw (forwards to [`ScoreRenderer::set_image`]). Call it for every
    /// image the scenes use (the bundle's [`ImageSet`](super::ImageSet));
    /// setting the same file again replaces its texture in place. A
    /// successful call makes the next `update` with LCD content redraw, so
    /// the image appears without a content change.
    ///
    /// How much a caller must do depends on the layer's height (#163 round
    /// 13, finding 17):
    ///
    /// * **Explicit height** (`h > 0`): the composed draw list already holds
    ///   the [`LcdItem::Image`](super::LcdItem::Image) with its rectangle, so
    ///   calling `set_image` before or after the first
    ///   [`update`](Self::update) is enough (redraw only); replacing the
    ///   image redraws the same rectangle with the new pixels.
    /// * **Auto height** (`h = 0`): [`LcdDrawList::compose`](super::LcdDrawList::compose)
    ///   takes the height from the image's aspect ratio, so the image must
    ///   *also* be in the `ImageSet` passed to `compose`. When it is missing
    ///   (or invalid) there, `compose` leaves the layer out of the draw list,
    ///   and `set_image` cannot add it to an existing list. After
    ///   registering such an image late, or replacing it with one of a
    ///   different aspect ratio, add it to that `ImageSet`, recompose the
    ///   draw list and pass the new list to [`update`](Self::update).
    ///
    /// An [`LcdItem::Image`](super::LcdItem::Image) whose file was never
    /// registered is skipped when drawing (no error, no log): a missing
    /// bundle image never aborts rendering, and the rest of the scene still
    /// draws. Registering the file is the way to draw it.
    ///
    /// Errors, all [`GpuError::InvalidArgument`] and never a panic
    /// (untrusted bundles, D25): an invalid image, or one with a side above
    /// the device's `max_texture_dimension_2d` (a previous image of that
    /// file stays and the display is not marked changed), and any image on a
    /// DMD score display, which has no image layers.
    ///
    /// `ctx` must be the context `self` was created with. The upload always
    /// uses the queue of that creating context (kept by the renderer), so
    /// another context can neither panic wgpu nor replace the previous image
    /// with a texture that lacks its pixels (#163 round 12, finding 21).
    pub fn set_image(
        &mut self,
        ctx: &GpuContext,
        file: &str,
        image: &RgbaImage,
    ) -> Result<(), GpuError> {
        self.renderer.set_image(ctx, file, image)
    }

    /// Takes `content` and, only when [`ScoreRenderer::update`] reports a
    /// change (a different DMD frame or LCD draw list, the first call, or
    /// after [`mark_dirty`](Self::mark_dirty) or a successful
    /// [`set_image`](Self::set_image)), redraws level 0 and regenerates every
    /// mip in one submit. Returns whether it redrew; content of the other
    /// display mode is ignored (`false`). LCD image items draw the image
    /// registered for their file with [`set_image`](Self::set_image); an
    /// image item whose file is not registered is skipped (the rest of the
    /// draw list still draws).
    ///
    /// `ctx` must be the context `self` was created with. Frame uploads and
    /// the redraw use that creating context's device and queue (kept by
    /// `self`), never `ctx`'s (#163 round 12, finding 21).
    pub fn update(&mut self, ctx: &GpuContext, content: &DisplayContent) -> bool {
        let _ = ctx; // the owning device and queue are used (see above)
        if !self.renderer.update(&self.target.queue, content) {
            return false;
        }
        let renderer = &mut self.renderer;
        self.target.redraw(|encoder, view, size, viewport| {
            renderer.encode(encoder, view, size, viewport);
        });
        true
    }

    /// Makes the next [`update`](Self::update) with content of this
    /// display's mode redraw.
    pub fn mark_dirty(&mut self) {
        self.renderer.mark_dirty();
    }

    /// The texture; the same object for the life of `self`.
    pub fn texture(&self) -> &wgpu::Texture {
        &self.target.texture
    }

    /// A 2D view of every mip level, for sampling.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.target.view
    }

    /// Size of mip level 0, `(width, height)`.
    pub fn size(&self) -> (u32, u32) {
        self.target.size
    }

    /// Number of mip levels ([`mip_level_count`] of [`size`](Self::size)).
    pub fn mip_level_count(&self) -> u32 {
        self.target.levels()
    }

    /// Number of redraws since [`new`](Self::new) (0 before the first
    /// [`update`](Self::update)).
    pub fn redraw_count(&self) -> u64 {
        self.target.redraws
    }
}

/// GPU smoke tests (one shared lib-test context, tiny textures). The
/// Independent Tester's `tests/displays_textures_gpu.rs` is the spec
/// evidence; these check the plumbing of this module.
#[cfg(all(test, feature = "gpu-tests"))]
mod gpu_tests {
    use super::*;
    use crate::displays::{
        BackglassLayer, DMD_128X32, DmdFrame, LayerBlend, LayerLamp, RgbaImage,
        render_backglass_to_image, render_dmd_to_image,
    };
    use pin_core::LampId;

    /// Reads mip `level` of an `Rgba8Unorm` texture (bounded wait).
    fn read_level(ctx: &GpuContext, texture: &wgpu::Texture, level: u32) -> RgbaImage {
        let device = ctx.device();
        let size = texture
            .size()
            .mip_level_size(level, wgpu::TextureDimension::D2);
        let (width, height) = (size.width, size.height);
        let row = width as usize * 4;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
        let padded = row.div_ceil(align) * align;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("texture test readback"),
            size: crate::gpu::readback_buffer_size(device, padded, height).unwrap(),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
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
        buf.map_async(wgpu::MapMode::Read, .., move |r| {
            let _ = tx.send(r);
        });
        crate::gpu::finish_map(device, &rx, crate::gpu::READBACK_WAIT_TIMEOUT).unwrap();
        let mut pixels = Vec::with_capacity(row * height as usize);
        {
            let mapped = buf.get_mapped_range(..).unwrap();
            for y in 0..height as usize {
                pixels.extend_from_slice(&mapped[y * padded..y * padded + row]);
            }
        }
        buf.unmap();
        RgbaImage {
            width,
            height,
            pixels,
        }
    }

    /// Mean per channel of `img` over `x0..x1 × y0..y1`.
    fn mean(img: &RgbaImage, x0: u32, y0: u32, x1: u32, y1: u32) -> [f64; 4] {
        let mut s = [0.0; 4];
        for y in y0..y1 {
            for x in x0..x1 {
                let p = img.pixel(x, y);
                for c in 0..4 {
                    s[c] += f64::from(p[c]);
                }
            }
        }
        let n = f64::from((x1 - x0) * (y1 - y0));
        s.map(|v| v / n)
    }

    /// Optional dumps for the visual checklist (`PIN_TEXTURE_DUMP_DIR`).
    fn dump(name: &str, img: &RgbaImage) {
        if let Ok(dir) = std::env::var("PIN_TEXTURE_DUMP_DIR") {
            let dir = std::path::Path::new(&dir);
            std::fs::create_dir_all(dir).unwrap();
            image::save_buffer(
                dir.join(format!("{name}.png")),
                &img.pixels,
                img.width,
                img.height,
                image::ColorType::Rgba8,
            )
            .unwrap();
        }
    }

    /// Every level's aligned-quadrant means are within 2/255 of level 0's
    /// (power-of-two sizes).
    fn assert_mips_preserve_means(ctx: &GpuContext, texture: &wgpu::Texture, levels: u32) {
        let l0 = read_level(ctx, texture, 0);
        for k in 1..levels {
            let lk = read_level(ctx, texture, k);
            assert_eq!(lk.width, (l0.width >> k).max(1));
            assert_eq!(lk.height, (l0.height >> k).max(1));
            let (qx, qy) = ((lk.width / 2).max(1), (lk.height / 2).max(1));
            for (ax, ay) in [(0, 0), (qx, 0), (0, qy), (qx, qy)] {
                let (bx, by) = ((ax + qx).min(lk.width), (ay + qy).min(lk.height));
                if bx <= ax || by <= ay {
                    continue;
                }
                let mk = mean(&lk, ax, ay, bx, by);
                // Level-0 texels per level-k texel along each axis (a side
                // clamped at 1 covers the whole level-0 side).
                let (sx, sy) = (l0.width / lk.width, l0.height / lk.height);
                let m0 = mean(&l0, ax * sx, ay * sy, bx * sx, by * sy);
                for c in 0..4 {
                    assert!(
                        (mk[c] - m0[c]).abs() <= 2.0,
                        "REQ-VIEW-002 level {k} region ({ax},{ay}) ch {c}: {} vs {}",
                        mk[c],
                        m0[c]
                    );
                }
            }
        }
    }

    /// REQ-VIEW-002: DMD texture redraws only on change, keeps its texture,
    /// level 0 matches `render_dmd_to_image`, mips preserve region means.
    #[test]
    fn req_view_002_dmd_score_texture_smoke() {
        let Some(ctx) = crate::gpu::shared_context_for_tests("req_view_002_dmd_score_texture")
        else {
            return;
        };
        let (w, h) = (128, 32);
        let mut t = ScoreTexture::new(ctx, ScoreDisplayConfig::Dmd(DMD_128X32), w, h).unwrap();
        assert_eq!(t.size(), (w, h));
        assert_eq!(t.mip_level_count(), mip_level_count(w, h));
        assert_eq!(t.redraw_count(), 0);
        let blank = read_level(ctx, t.texture(), 0);
        assert!(blank.pixels.iter().all(|&b| b == 0), "starts transparent");
        let original = t.texture().clone();

        let off = DmdFrame::new(DMD_128X32.width, DMD_128X32.height);
        let mut lit = off.clone();
        for y in 0..DMD_128X32.height {
            for x in 0..DMD_128X32.width {
                lit.set(x, y, [255, 255, 255]);
            }
        }
        assert!(t.update(ctx, &DisplayContent::Dmd(off.clone())));
        assert_eq!(t.redraw_count(), 1);
        let off0 = read_level(ctx, t.texture(), 0);
        assert!(!t.update(ctx, &DisplayContent::Dmd(off.clone())));
        assert_eq!(t.redraw_count(), 1);
        assert!(t.update(ctx, &DisplayContent::Dmd(lit.clone())));
        assert_eq!(t.redraw_count(), 2);
        assert!(*t.texture() == original, "same GPU texture object");
        assert_eq!(t.size(), (w, h));
        let lit0 = read_level(ctx, t.texture(), 0);
        assert_ne!(lit0.pixels, off0.pixels);

        let reference = render_dmd_to_image(ctx, DMD_128X32, &lit, w, h).unwrap();
        for (a, b) in lit0.pixels.iter().zip(&reference.pixels) {
            assert!(a.abs_diff(*b) <= 1, "REQ-VIEW-002 level 0 vs reference");
        }
        assert_mips_preserve_means(ctx, t.texture(), t.mip_level_count());
        let last = t.mip_level_count() - 1;
        dump("dmd_lit_mip0", &lit0);
        dump("dmd_lit_mip1", &read_level(ctx, t.texture(), 1));
        dump("dmd_lit_mip_last", &read_level(ctx, t.texture(), last));

        // Mode mismatch is ignored; mark_dirty forces one redraw.
        let lcd = DisplayContent::Lcd(crate::displays::LcdDrawList {
            width: 16,
            height: 9,
            background: [0, 0, 0],
            items: Vec::new(),
        });
        assert!(!t.update(ctx, &lcd));
        t.mark_dirty();
        assert!(t.update(ctx, &DisplayContent::Dmd(lit)));
        assert_eq!(t.redraw_count(), 3);
    }

    /// REQ-VIEW-002: lamp toggle changes level 0 only inside the mask; the
    /// result matches `render_backglass_to_image`; refused sizes error.
    #[test]
    fn req_view_002_backglass_texture_smoke() {
        let Some(ctx) = crate::gpu::shared_context_for_tests("req_view_002_backglass_texture")
        else {
            return;
        };
        let (w, h) = (32, 16);
        let base = RgbaImage::from_rgb(w, h, vec![40; (w * h * 3) as usize]).unwrap();
        let lit = RgbaImage::from_rgb(w, h, [250, 200, 20].repeat((w * h) as usize)).unwrap();
        let mut mask = RgbaImage::new(w, h);
        let (mx0, my0, mx1, my1) = (8, 4, 20, 12);
        for y in my0..my1 {
            for x in mx0..mx1 {
                let o = ((y * w + x) * 4) as usize;
                mask.pixels[o..o + 4].copy_from_slice(&[255; 4]);
            }
        }
        let desc = BackglassDesc {
            layers: vec![
                BackglassLayer {
                    image: base,
                    lamp: None,
                },
                BackglassLayer {
                    image: lit,
                    lamp: Some(LayerLamp {
                        lamp: LampId(0),
                        mask,
                        blend: LayerBlend::Over,
                    }),
                },
            ],
        };
        assert!(matches!(
            BackglassTexture::new(ctx, &desc, 0, h),
            Err(GpuError::InvalidArgument(_))
        ));
        let max = ctx.device().limits().max_texture_dimension_2d;
        assert!(matches!(
            BackglassTexture::new(ctx, &desc, max + 1, h),
            Err(GpuError::InvalidArgument(_))
        ));
        let mut t = BackglassTexture::new(ctx, &desc, w, h).unwrap();
        assert_eq!(t.aspect(), desc.aspect());
        assert_eq!(t.mip_level_count(), 6);
        assert!(t.update(ctx, &[0.0]));
        let before = read_level(ctx, t.texture(), 0);
        assert!(!t.update(ctx, &[0.0]));
        assert!(!t.update(ctx, &[LAMP_EPS_HALF]));
        assert!(t.update(ctx, &[1.0]));
        assert_eq!(t.redraw_count(), 2);
        let after = read_level(ctx, t.texture(), 0);
        let mut changed_inside = false;
        for y in 0..h {
            for x in 0..w {
                let inside = (mx0..mx1).contains(&x) && (my0..my1).contains(&y);
                if inside {
                    changed_inside |= before.pixel(x, y) != after.pixel(x, y);
                } else {
                    assert_eq!(before.pixel(x, y), after.pixel(x, y), "outside ({x},{y})");
                }
            }
        }
        assert!(changed_inside);
        let reference = render_backglass_to_image(ctx, &desc, &[1.0], w, h).unwrap();
        for (a, b) in after.pixels.iter().zip(&reference.pixels) {
            assert!(a.abs_diff(*b) <= 1);
        }
        assert_mips_preserve_means(ctx, t.texture(), t.mip_level_count());
        let _ = t.view();
        assert_eq!(t.size(), (w, h));
    }

    const LAMP_EPS_HALF: f32 = crate::displays::LAMP_REDRAW_EPSILON / 2.0;

    /// REQ-VIEW-002: a non-power-of-two LCD texture builds a full chain and
    /// redraws without validation errors; the last level is 1×1 and opaque
    /// (the LCD letterbox is opaque black).
    #[test]
    fn req_view_002_npot_lcd_score_texture_smoke() {
        let Some(ctx) = crate::gpu::shared_context_for_tests("req_view_002_npot_lcd_texture")
        else {
            return;
        };
        let spec = crate::displays::LcdSpec::new(32, 18).unwrap();
        let mut t = ScoreTexture::new(ctx, ScoreDisplayConfig::Lcd(spec), 27, 13).unwrap();
        assert_eq!(t.mip_level_count(), 5);
        let list = crate::displays::LcdDrawList {
            width: 32,
            height: 18,
            background: [0, 0, 200],
            items: Vec::new(),
        };
        assert!(t.update(ctx, &DisplayContent::Lcd(list.clone())));
        assert!(!t.update(ctx, &DisplayContent::Lcd(list)));
        let last = read_level(ctx, t.texture(), t.mip_level_count() - 1);
        assert_eq!((last.width, last.height), (1, 1));
        assert_eq!(last.pixels[3], 255);
        assert!(
            last.pixels[2] > 100,
            "blue survives the chain: {:?}",
            last.pixels
        );
        let _ = t.view();
        assert!(matches!(
            ScoreTexture::new(ctx, ScoreDisplayConfig::Lcd(spec), 5, 0),
            Err(GpuError::InvalidArgument(_))
        ));
    }
}
