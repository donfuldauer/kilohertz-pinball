//! GPU reflection sources: octahedral probes and the probe-less fallback.

use super::{
    CUBE_FACE_RESOLUTION, MAX_PROBES, PROBE_BUDGET, PROBE_RESOLUTION, ProbeScheduler, ProbeStats,
    ReflectionSource, ReflectiveBall, face_basis, face_camera,
};
use crate::gpu::{GpuContext, GpuError};
use crate::pbr::{
    DEPTH_FORMAT, ExternalTarget, FrameDesc, HDR_FORMAT, HdrImage, NO_PROBE, PbrRenderer,
    ReflectionBinding,
};

/// The remap module: the octahedral helpers of the PBR reflection include,
/// then the remap entry points.
pub(crate) const REMAP_SOURCE: &str = concat!(
    include_str!("../pbr/shaders/reflection.wgsl"),
    "\n",
    include_str!("shaders/remap.wgsl")
);

/// Probe capture settings.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ProbeConfig {
    /// Captures per frame (clamped to `1..=PROBE_BUDGET`).
    pub budget: u32,
    /// Capture near plane = `near_factor` · ball radius (the ball's own
    /// surface is culled from inside; nearby objects start at one radius).
    pub near_factor: f32,
    /// Capture far plane (m).
    pub far: f32,
}

impl Default for ProbeConfig {
    fn default() -> Self {
        ProbeConfig {
            budget: PROBE_BUDGET,
            near_factor: 0.5,
            far: 20.0,
        }
    }
}

fn sampler(device: &wgpu::Device, label: &str) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some(label),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    })
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct FaceBasisUniform {
    right: [[f32; 4]; 6],
    up: [[f32; 4]; 6],
    forward: [[f32; 4]; 6],
    params: [f32; 4],
    env_sky: [f32; 4],
    env_horizon: [f32; 4],
    env_ground: [f32; 4],
}

/// Octahedral reflection probes, one per ball, captured under the
/// [`ProbeScheduler`] budget (see the [module docs](super)).
///
/// Memory: a `PROBE_RESOLUTION`² × [`MAX_PROBES`] `Rgba16Float` atlas
/// (2 MiB) plus one capture cube (six `CUBE_FACE_RESOLUTION`² faces and a
/// depth buffer). Each capture renders six faces with the PBR renderer, one
/// submitted command buffer per face (the renderer's uploads apply at the
/// next submit), the last one followed by the octahedral remap.
pub struct OctahedralProbes {
    config: ProbeConfig,
    scheduler: ProbeScheduler,
    atlas: wgpu::Texture,
    layer_views: Vec<wgpu::TextureView>,
    atlas_bind: wgpu::BindGroup,
    face_views: Vec<wgpu::TextureView>,
    face_depth: wgpu::TextureView,
    remap_pipeline: wgpu::RenderPipeline,
    remap_bind: wgpu::BindGroup,
    basis: FaceBasisUniform,
    basis_buffer: wgpu::Buffer,
    draw_layers: Vec<u32>,
    positions: Vec<[f32; 3]>,
}

impl OctahedralProbes {
    /// Probes for `pbr` (whose [`reflection_layout`](PbrRenderer::reflection_layout)
    /// the atlas bind group uses).
    pub fn new(
        ctx: &GpuContext,
        pbr: &PbrRenderer,
        config: ProbeConfig,
    ) -> Result<OctahedralProbes, GpuError> {
        let device = ctx.device();
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("probe atlas"),
            size: wgpu::Extent3d {
                width: PROBE_RESOLUTION,
                height: PROBE_RESOLUTION,
                depth_or_array_layers: MAX_PROBES as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let layer_view = |tex: &wgpu::Texture, layer: u32| {
            tex.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: layer,
                array_layer_count: Some(1),
                ..Default::default()
            })
        };
        let layer_views = (0..MAX_PROBES as u32)
            .map(|l| layer_view(&atlas, l))
            .collect();
        let atlas_view = atlas.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let atlas_sampler = sampler(device, "probe atlas");
        let atlas_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("probe atlas"),
            layout: pbr.reflection_layout(),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&atlas_sampler),
                },
            ],
        });

        let face_size = wgpu::Extent3d {
            width: CUBE_FACE_RESOLUTION,
            height: CUBE_FACE_RESOLUTION,
            depth_or_array_layers: 6,
        };
        let faces = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("probe faces"),
            size: face_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let face_views = (0..6).map(|l| layer_view(&faces, l)).collect();
        let faces_view = faces.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let face_depth = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("probe face depth"),
                size: wgpu::Extent3d {
                    depth_or_array_layers: 1,
                    ..face_size
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut basis = FaceBasisUniform {
            right: [[0.0; 4]; 6],
            up: [[0.0; 4]; 6],
            forward: [[0.0; 4]; 6],
            params: [PROBE_RESOLUTION as f32, 0.0, 0.0, 0.0],
            env_sky: [0.0; 4],
            env_horizon: [0.0; 4],
            env_ground: [0.0; 4],
        };
        for k in 0..6 {
            let (r, u, f) = face_basis(k);
            basis.right[k] = [r[0], r[1], r[2], 0.0];
            basis.up[k] = [u[0], u[1], u[2], 0.0];
            basis.forward[k] = [f[0], f[1], f[2], 0.0];
        }
        let basis_buffer = {
            use wgpu::util::DeviceExt;
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("probe face basis"),
                contents: bytemuck::bytes_of(&basis),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            })
        };
        let remap_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("probe remap"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            core::mem::size_of::<FaceBasisUniform>() as u64,
                        ),
                    },
                    count: None,
                },
            ],
        });
        let face_sampler = sampler(device, "probe faces");
        let remap_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("probe remap"),
            layout: &remap_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&faces_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&face_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: basis_buffer.as_entire_binding(),
                },
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("probe_remap"),
            source: wgpu::ShaderSource::Wgsl(REMAP_SOURCE.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("probe remap"),
            bind_group_layouts: &[Some(&remap_layout)],
            immediate_size: 0,
        });
        let remap_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("probe remap"),
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
                entry_point: Some("fs_remap"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Ok(OctahedralProbes {
            config,
            scheduler: ProbeScheduler::new(config.budget),
            atlas,
            layer_views,
            atlas_bind,
            face_views,
            face_depth,
            remap_pipeline,
            remap_bind,
            basis,
            basis_buffer,
            draw_layers: Vec::new(),
            positions: Vec::with_capacity(MAX_PROBES),
        })
    }

    /// The capture settings.
    pub fn config(&self) -> ProbeConfig {
        self.config
    }

    /// The scheduler (budget, ages, which probe each ball samples).
    pub fn scheduler(&self) -> &ProbeScheduler {
        &self.scheduler
    }

    /// The atlas layer each draw samples after the last
    /// [`prepare`](ReflectionSource::prepare) ([`NO_PROBE`] = environment).
    pub fn draw_layers(&self) -> &[u32] {
        &self.draw_layers
    }

    /// Reads probe `slot` back as a `PROBE_RESOLUTION`² linear image (pixel
    /// `(x, y)` holds direction `oct_decode([(x + ½)/R, (y + ½)/R])`).
    pub fn read_probe(&self, ctx: &GpuContext, slot: usize) -> Result<HdrImage, GpuError> {
        if slot >= MAX_PROBES {
            return Err(GpuError::InvalidArgument(format!(
                "probe slot {slot} (max {})",
                MAX_PROBES - 1
            )));
        }
        crate::post::readback::read_hdr_layer(ctx, &self.atlas, slot as u32)
    }

    /// Captures the probe of `ball` into atlas layer `slot`.
    fn capture(
        &self,
        ctx: &GpuContext,
        pbr: &mut PbrRenderer,
        frame: &FrameDesc<'_>,
        ball: &ReflectiveBall,
        slot: usize,
    ) {
        let radius = if ball.radius.is_finite() {
            ball.radius.abs()
        } else {
            0.0
        };
        let near = (self.config.near_factor * radius).max(1e-4);
        let far = self.config.far.max(near * 2.0);
        for face in 0..6 {
            let cam = face_camera(ball.position, face, near, far);
            // Transparent black: the remap fills uncovered texels with the
            // environment.
            let face_frame = FrameDesc {
                camera: &cam,
                clear: [0.0; 4],
                ..*frame
            };
            let mut encoder = ctx
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("probe capture"),
                });
            pbr.encode_to_target(
                &mut encoder,
                &face_frame,
                &ExternalTarget {
                    color: &self.face_views[face],
                    depth: &self.face_depth,
                    width: CUBE_FACE_RESOLUTION,
                    height: CUBE_FACE_RESOLUTION,
                },
            );
            if face == 5 {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("probe remap"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.layer_views[slot],
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
                pass.set_pipeline(&self.remap_pipeline);
                pass.set_bind_group(0, &self.remap_bind, &[]);
                pass.draw(0..3, 0..1);
            }
            ctx.queue().submit([encoder.finish()]);
        }
    }
}

impl ReflectionSource for OctahedralProbes {
    fn name(&self) -> &'static str {
        "octahedral-probes"
    }

    fn prepare(
        &mut self,
        ctx: &GpuContext,
        pbr: &mut PbrRenderer,
        frame: &FrameDesc<'_>,
        balls: &[ReflectiveBall],
    ) -> Result<(), GpuError> {
        self.positions.clear();
        self.positions.extend(balls.iter().map(|b| b.position));
        let mut updates = [usize::MAX; PROBE_BUDGET as usize];
        let scheduled = self.scheduler.schedule(&self.positions);
        for (u, &s) in updates.iter_mut().zip(scheduled) {
            *u = s;
        }
        if updates[0] != usize::MAX {
            let env = |c: [f32; 3]| {
                let c = c.map(|v| if v.is_finite() { v.max(0.0) } else { 0.0 });
                [c[0], c[1], c[2], 0.0]
            };
            self.basis.env_sky = env(frame.environment.sky);
            self.basis.env_horizon = env(frame.environment.horizon);
            self.basis.env_ground = env(frame.environment.ground);
            ctx.queue()
                .write_buffer(&self.basis_buffer, 0, bytemuck::bytes_of(&self.basis));
        }
        for &i in updates.iter().filter(|&&i| i != usize::MAX) {
            self.capture(ctx, pbr, frame, &balls[i], i);
        }
        self.draw_layers.clear();
        self.draw_layers.resize(frame.draws.len(), NO_PROBE);
        for (i, b) in balls.iter().enumerate() {
            if let (Some(slot), Some(layer)) = (
                self.scheduler.probe_for_ball(i),
                self.draw_layers.get_mut(b.draw),
            ) {
                *layer = slot as u32;
            }
        }
        Ok(())
    }

    fn binding(&self) -> ReflectionBinding<'_> {
        ReflectionBinding {
            bind_group: &self.atlas_bind,
            draw_layers: &self.draw_layers,
        }
    }

    fn stats(&self) -> ProbeStats {
        self.scheduler.stats()
    }
}

/// No probes: every draw uses the placeholder environment (the 5.1 look).
/// Counts frames; never captures.
pub struct EnvironmentReflections {
    bind: wgpu::BindGroup,
    stats: ProbeStats,
}

impl EnvironmentReflections {
    /// The fallback source for `pbr`.
    pub fn new(ctx: &GpuContext, pbr: &PbrRenderer) -> EnvironmentReflections {
        let device = ctx.device();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("no probes"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("no probes"),
            layout: pbr.reflection_layout(),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler(device, "no probes")),
                },
            ],
        });
        EnvironmentReflections {
            bind,
            stats: ProbeStats::default(),
        }
    }
}

impl ReflectionSource for EnvironmentReflections {
    fn name(&self) -> &'static str {
        "environment"
    }

    fn prepare(
        &mut self,
        _ctx: &GpuContext,
        _pbr: &mut PbrRenderer,
        _frame: &FrameDesc<'_>,
        balls: &[ReflectiveBall],
    ) -> Result<(), GpuError> {
        self.stats.frames += 1;
        self.stats.balls = balls.len() as u32;
        self.stats.last_frame_updates = 0;
        Ok(())
    }

    fn binding(&self) -> ReflectionBinding<'_> {
        ReflectionBinding {
            bind_group: &self.bind,
            draw_layers: &[],
        }
    }

    fn stats(&self) -> ProbeStats {
        self.stats
    }
}
