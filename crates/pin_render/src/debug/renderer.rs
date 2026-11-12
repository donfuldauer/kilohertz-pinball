//! GPU side of the debug renderer: one line-list pipeline, lines uploaded as
//! per-instance data (two vertices per instance pick `a` or `b`).

use super::{Camera, DebugLine, DebugLines};
use crate::gpu::{GpuContext, GpuError};

const SHADER: &str = r#"
struct Camera {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs(
    @builtin(vertex_index) vi: u32,
    @location(0) a: vec3<f32>,
    @location(1) b: vec3<f32>,
    @location(2) color: vec4<f32>,
) -> VsOut {
    var out: VsOut;
    let p = select(a, b, vi == 1u);
    out.pos = camera.view_proj * vec4<f32>(p, 1.0);
    out.color = color;
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
"#;

/// Initial line capacity of the GPU instance buffer (grows on demand).
const INITIAL_GPU_LINES: usize = 1024;

/// Draws [`DebugLines`] as 1-px lines into a colour target of one format.
pub struct DebugRenderer {
    device: wgpu::Device,
    pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    line_buffer: wgpu::Buffer,
    line_capacity: usize,
}

fn line_buffer(device: &wgpu::Device, lines: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("debug lines"),
        size: (lines * core::mem::size_of::<DebugLine>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

impl DebugRenderer {
    /// A renderer drawing into targets of `target_format`.
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("debug lines"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("debug camera"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(64),
                },
                count: None,
            }],
        });
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("debug camera"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("debug camera"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("debug lines"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        const ATTRIBUTES: [wgpu::VertexAttribute; 3] =
            wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("debug lines"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: core::mem::size_of::<DebugLine>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &ATTRIBUTES,
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Self {
            device: device.clone(),
            pipeline,
            camera_buffer,
            bind_group,
            line_buffer: line_buffer(device, INITIAL_GPU_LINES),
            line_capacity: INITIAL_GPU_LINES,
        }
    }

    /// Records drawing `lines` with `camera` on top of `view`'s contents.
    ///
    /// The camera and line data are uploaded with `queue.write_buffer`, which
    /// takes effect at the next submit: record at most one `draw` per
    /// renderer per submitted command buffer.
    pub fn draw(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        camera: &Camera,
        lines: &DebugLines,
    ) {
        self.draw_with(encoder, queue, view, camera, lines, None, None);
    }

    /// Like [`draw`](Self::draw), optionally clearing `view` to `clear` first
    /// and restricting drawing to `viewport` (`[x, y, w, h]` in pixels).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_with(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        camera: &Camera,
        lines: &DebugLines,
        clear: Option<[f32; 4]>,
        viewport: Option<[f32; 4]>,
    ) {
        let slice = lines.as_slice();
        if slice.len() > self.line_capacity {
            self.line_capacity = slice.len().next_power_of_two();
            self.line_buffer = line_buffer(&self.device, self.line_capacity);
        }
        queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::cast_slice(&camera.view_proj),
        );
        if !slice.is_empty() {
            queue.write_buffer(&self.line_buffer, 0, bytemuck::cast_slice(slice));
        }
        let load = match clear {
            Some([r, g, b, a]) => wgpu::LoadOp::Clear(wgpu::Color {
                r: f64::from(r),
                g: f64::from(g),
                b: f64::from(b),
                a: f64::from(a),
            }),
            None => wgpu::LoadOp::Load,
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("debug lines"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if slice.is_empty() {
            return;
        }
        if let Some([x, y, w, h]) = viewport {
            pass.set_viewport(x, y, w, h, 0.0, 1.0);
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        let bytes = core::mem::size_of_val(slice) as u64;
        pass.set_vertex_buffer(0, self.line_buffer.slice(..bytes));
        pass.draw(0..2, 0..slice.len() as u32);
    }
}

/// Renders `lines` with `camera` over a `clear` background into a
/// `width`×`height` RGBA8 (unorm, not sRGB) image and reads it back: tightly
/// packed rows of `width · 4` bytes, row 0 = top of the image.
pub fn render_offscreen(
    ctx: &GpuContext,
    width: u32,
    height: u32,
    clear: [f32; 4],
    camera: &Camera,
    lines: &DebugLines,
) -> Result<Vec<u8>, GpuError> {
    let max = ctx.device().limits().max_texture_dimension_2d;
    if width == 0 || height == 0 || width > max || height > max {
        return Err(GpuError::InvalidArgument(format!(
            "image size {width}x{height} (each side must be 1..={max})"
        )));
    }
    let device = ctx.device();
    let queue = ctx.queue();
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("debug offscreen"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let mut renderer = DebugRenderer::new(device, format);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("debug offscreen"),
    });
    renderer.draw_with(&mut encoder, queue, &view, camera, lines, Some(clear), None);
    read_texture(ctx, encoder, &texture)
}

/// Copies an RGBA8 `texture` (usage `COPY_SRC`) into a tightly packed byte
/// vector after submitting `encoder`.
pub(crate) fn read_texture(
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
        label: Some("debug readback"),
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
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| GpuError::Readback(format!("poll: {e}")))?;
    rx.recv()
        .map_err(|e| GpuError::Readback(format!("map channel: {e}")))?
        .map_err(|e| GpuError::Readback(format!("map_async: {e}")))?;
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
