//! GPU side: the clustered forward renderer drawing into a linear HDR target.

use super::cluster::{ClusterAssignment, ClusterGrid, DEFAULT_CLUSTER_DIMS, assign_lights};
use super::light::{LightList, PointLight};
use super::material::{MaterialKind, MaterialParams};
use super::math::{Mat4, PbrCamera, mul, normal_matrix, transform_point};
use super::mesh::MeshGeometry;
use super::{FORWARD_SOURCE, MAX_LIGHTS};
use crate::gpu::{GpuContext, GpuError, READBACK_WAIT_TIMEOUT, wait_for_device};
use wgpu::util::DeviceExt;

/// Format of the HDR colour target (linear radiance; 5.2 post-processes it).
pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Format of the depth target.
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Bytes per draw in the dynamic-offset uniform buffer.
const DRAW_STRIDE: u64 = 256;

/// The placeholder environment (ambient and mirror term) until 5.2's
/// probes: three linear RGB radiances, by world direction (z up).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Environment {
    /// Radiance from straight up.
    pub sky: [f32; 3],
    /// Radiance from the horizon band.
    pub horizon: [f32; 3],
    /// Radiance from straight down.
    pub ground: [f32; 3],
}

impl Environment {
    /// No ambient light at all (analytic tests).
    pub const BLACK: Environment = Environment {
        sky: [0.0; 3],
        horizon: [0.0; 3],
        ground: [0.0; 3],
    };

    /// A neutral studio: dim sky, bright horizon band, dark floor.
    pub const STUDIO: Environment = Environment {
        sky: [0.12, 0.12, 0.13],
        horizon: [0.9, 0.9, 0.9],
        ground: [0.02, 0.02, 0.02],
    };
}

impl Default for Environment {
    fn default() -> Self {
        Self::STUDIO
    }
}

/// A mesh uploaded with [`PbrRenderer::upload_mesh`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct MeshId(u32);

/// One object to draw.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct DrawItem {
    /// The mesh.
    pub mesh: MeshId,
    /// Object → world (affine).
    pub model: Mat4,
    /// Surface parameters.
    pub material: MaterialParams,
    /// Drawn in the blended pass (after all opaque draws, back to front,
    /// premultiplied colour, no depth writes).
    pub transparent: bool,
    /// Warmed-up lamp level `0..=1` driving `lamp_emissive` (light inserts;
    /// from [`LampBank`](super::LampBank)).
    pub emission: f32,
}

impl DrawItem {
    /// `mesh` at `model` with the default parameters of `kind`.
    pub fn new(mesh: MeshId, model: Mat4, kind: MaterialKind) -> DrawItem {
        DrawItem {
            mesh,
            model,
            material: kind.params(),
            transparent: kind.is_transparent(),
            emission: 0.0,
        }
    }
}

/// Everything one frame needs.
#[derive(Copy, Clone, Debug)]
pub struct FrameDesc<'a> {
    /// The camera.
    pub camera: &'a PbrCamera,
    /// Dynamic lights; only the first [`MAX_LIGHTS`] are used, the rest are
    /// counted in [`PbrStats::lights_rejected`].
    pub lights: &'a [PointLight],
    /// Objects.
    pub draws: &'a [DrawItem],
    /// Ambient / mirror environment.
    pub environment: Environment,
    /// GI tint `[r, g, b, intensity]` (the snapshot's `gi`).
    pub gi: [f32; 4],
    /// Background (linear RGBA).
    pub clear: [f32; 4],
}

impl<'a> FrameDesc<'a> {
    /// A frame with the [`Environment::STUDIO`] environment, full white GI
    /// and a black background.
    pub fn new(
        camera: &'a PbrCamera,
        lights: &'a [PointLight],
        draws: &'a [DrawItem],
    ) -> FrameDesc<'a> {
        FrameDesc {
            camera,
            lights,
            draws,
            environment: Environment::STUDIO,
            gi: [1.0, 1.0, 1.0, 1.0],
            clear: [0.0, 0.0, 0.0, 1.0],
        }
    }
}

/// Renderer counters.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct PbrStats {
    /// Lights past [`MAX_LIGHTS`] dropped, over all frames.
    pub lights_rejected: u64,
    /// Lights past [`MAX_LIGHTS`] dropped in the last frame.
    pub frame_lights_rejected: u64,
    /// Lights used in the last frame.
    pub frame_lights: u32,
    /// Longest cluster light list in the last frame.
    pub max_cluster_lights: u32,
    /// Draws with an unknown mesh id skipped, over all frames.
    pub draws_skipped: u64,
    /// Frames encoded.
    pub frames: u64,
}

/// A linear HDR image read back from the GPU: `pixels[y · width + x]`,
/// row 0 at the top.
#[derive(Clone, Debug, PartialEq)]
pub struct HdrImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Linear RGBA.
    pub pixels: Vec<[f32; 4]>,
}

/// sRGB transfer function of a linear value clamped to `0..=1`.
fn srgb_encode(x: f32) -> u8 {
    let x = if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) };
    let s = if x <= 0.003_130_8 {
        12.92 * x
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round() as u8
}

impl HdrImage {
    /// Pixel at column `x`, row `y` (row 0 = top).
    pub fn at(&self, x: u32, y: u32) -> [f32; 4] {
        self.pixels[(y * self.width + x) as usize]
    }

    /// Mean RGBA over columns `x0..x1` and rows `y0..y1` (clamped to the
    /// image; zero for an empty region).
    pub fn region_mean(&self, x0: u32, y0: u32, x1: u32, y1: u32) -> [f32; 4] {
        let (x1, y1) = (x1.min(self.width), y1.min(self.height));
        let mut sum = [0.0f64; 4];
        let mut n = 0u64;
        for y in y0..y1 {
            for x in x0..x1 {
                for (s, v) in sum.iter_mut().zip(self.at(x, y)) {
                    *s += f64::from(v);
                }
                n += 1;
            }
        }
        if n == 0 {
            return [0.0; 4];
        }
        sum.map(|s| (s / n as f64) as f32)
    }

    /// Tightly packed RGBA8 with the sRGB transfer function (values clamped
    /// to `0..=1` first, alpha 255), for viewing and regression snapshots.
    pub fn to_srgb8(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pixels.len() * 4);
        for p in &self.pixels {
            out.extend_from_slice(&[srgb_encode(p[0]), srgb_encode(p[1]), srgb_encode(p[2]), 255]);
        }
        out
    }
}

/// IEEE half → single.
pub(crate) fn f16_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0f32 } else { 1.0 };
    let exp = u32::from((h >> 10) & 0x1f);
    let frac = u32::from(h & 0x3ff);
    match exp {
        0 => sign * frac as f32 * (1.0 / 16_777_216.0),
        31 if frac == 0 => sign * f32::INFINITY,
        31 => f32::NAN,
        _ => f32::from_bits((u32::from(h & 0x8000) << 16) | ((exp + 112) << 23) | (frac << 13)),
    }
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct FrameUniform {
    view: Mat4,
    proj: Mat4,
    view_proj: Mat4,
    camera_pos: [f32; 4],
    env_sky: [f32; 4],
    env_horizon: [f32; 4],
    env_ground: [f32; 4],
    gi: [f32; 4],
    cluster_dims: [u32; 4],
    cluster_params: [f32; 4],
    screen: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct DrawUniform {
    model: Mat4,
    normal_matrix: Mat4,
    material: MaterialParams,
    emission: [f32; 4],
    _pad: [[f32; 4]; 3],
}

const _: () = assert!(core::mem::size_of::<DrawUniform>() as u64 == DRAW_STRIDE);
const _: () = assert!(core::mem::size_of::<FrameUniform>() == 320);

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
}

struct Targets {
    width: u32,
    height: u32,
    hdr: wgpu::Texture,
    hdr_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
}

fn make_targets(device: &wgpu::Device, width: u32, height: u32) -> Targets {
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let hdr = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("pbr hdr"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: HDR_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("pbr depth"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    Targets {
        width,
        height,
        hdr_view: hdr.create_view(&wgpu::TextureViewDescriptor::default()),
        hdr,
        depth_view: depth.create_view(&wgpu::TextureViewDescriptor::default()),
    }
}

fn storage_buffer(device: &wgpu::Device, label: &str, bytes: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: bytes.max(16),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// The clustered forward PBR renderer.
///
/// Owns an HDR colour target ([`HDR_FORMAT`]) and a depth target of the
/// size given at construction ([`resize`](Self::resize) changes it). Each
/// frame: [`encode`](Self::encode) into a command encoder and submit it, or
/// [`render_to_image`](Self::render_to_image) to render and read back
/// (tests, tools). Uploads use `queue.write_buffer`, which takes effect at
/// the next submit: encode at most one frame per submitted command buffer.
pub struct PbrRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    targets: Targets,
    opaque: wgpu::RenderPipeline,
    blended: wgpu::RenderPipeline,
    frame_layout: wgpu::BindGroupLayout,
    draw_layout: wgpu::BindGroupLayout,
    frame_buffer: wgpu::Buffer,
    light_buffer: wgpu::Buffer,
    ranges_buffer: wgpu::Buffer,
    indices_buffer: wgpu::Buffer,
    frame_bind: wgpu::BindGroup,
    draw_buffer: wgpu::Buffer,
    draw_capacity: u64,
    draw_bind: wgpu::BindGroup,
    meshes: Vec<GpuMesh>,
    grid: Option<ClusterGrid>,
    assignment: ClusterAssignment,
    light_list: LightList,
    draw_data: Vec<DrawUniform>,
    blend_order: Vec<(f32, u32)>,
    stats: PbrStats,
}

fn check_size(device: &wgpu::Device, width: u32, height: u32) -> Result<(), GpuError> {
    let max = device.limits().max_texture_dimension_2d;
    if width == 0 || height == 0 || width > max || height > max {
        return Err(GpuError::InvalidArgument(format!(
            "image size {width}x{height} (each side must be 1..={max})"
        )));
    }
    Ok(())
}

impl PbrRenderer {
    /// A renderer with `width`×`height` HDR and depth targets.
    pub fn new(ctx: &GpuContext, width: u32, height: u32) -> Result<PbrRenderer, GpuError> {
        let device = ctx.device();
        check_size(device, width, height)?;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("pbr_forward"),
            source: wgpu::ShaderSource::Wgsl(FORWARD_SOURCE.into()),
        });
        let uniform = |binding, bytes| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(bytes),
            },
            count: None,
        };
        let storage = |binding, bytes| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(bytes),
            },
            count: None,
        };
        let light_bytes = (MAX_LIGHTS * core::mem::size_of::<PointLight>()) as u64;
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pbr frame"),
            entries: &[
                uniform(0, core::mem::size_of::<FrameUniform>() as u64),
                uniform(1, light_bytes),
                storage(2, 8),
                storage(3, 4),
            ],
        });
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pbr draw"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(DRAW_STRIDE),
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pbr"),
            bind_group_layouts: &[Some(&frame_layout), Some(&draw_layout)],
            immediate_size: 0,
        });
        const ATTRIBUTES: [wgpu::VertexAttribute; 2] =
            wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3];
        let make = |label: &str, entry: &str, blended: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: 24,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &ATTRIBUTES,
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: if blended {
                        None
                    } else {
                        Some(wgpu::Face::Back)
                    },
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(!blended),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: HDR_FORMAT,
                        blend: blended.then_some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let opaque = make("pbr opaque", "fs_opaque", false);
        let blended = make("pbr blended", "fs_blended", true);
        let frame_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pbr frame"),
            size: core::mem::size_of::<FrameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let light_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pbr lights"),
            size: light_bytes,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let clusters = DEFAULT_CLUSTER_DIMS.iter().product::<u32>() as u64;
        let ranges_buffer = storage_buffer(device, "pbr cluster ranges", clusters * 8);
        let indices_buffer = storage_buffer(device, "pbr cluster indices", clusters * 4 * 8);
        let frame_bind = Self::frame_bind_group(
            device,
            &frame_layout,
            &frame_buffer,
            &light_buffer,
            &ranges_buffer,
            &indices_buffer,
        );
        let draw_capacity = 16;
        let draw_buffer = Self::draw_buffer(device, draw_capacity);
        let draw_bind = Self::draw_bind_group(device, &draw_layout, &draw_buffer);
        Ok(PbrRenderer {
            device: device.clone(),
            queue: ctx.queue().clone(),
            targets: make_targets(device, width, height),
            opaque,
            blended,
            frame_layout,
            draw_layout,
            frame_buffer,
            light_buffer,
            ranges_buffer,
            indices_buffer,
            frame_bind,
            draw_buffer,
            draw_capacity,
            draw_bind,
            meshes: Vec::new(),
            grid: None,
            assignment: ClusterAssignment::new(),
            light_list: LightList::new(),
            draw_data: Vec::new(),
            blend_order: Vec::new(),
            stats: PbrStats::default(),
        })
    }

    fn frame_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        frame: &wgpu::Buffer,
        lights: &wgpu::Buffer,
        ranges: &wgpu::Buffer,
        indices: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pbr frame"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: frame.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: lights.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: ranges.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: indices.as_entire_binding(),
                },
            ],
        })
    }

    fn draw_buffer(device: &wgpu::Device, capacity: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pbr draws"),
            size: capacity * DRAW_STRIDE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn draw_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        buffer: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pbr draws"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(DRAW_STRIDE),
                }),
            }],
        })
    }

    /// Target size `(width, height)`.
    pub fn size(&self) -> (u32, u32) {
        (self.targets.width, self.targets.height)
    }

    /// Recreates the targets at `width`×`height` (no-op when unchanged).
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), GpuError> {
        check_size(&self.device, width, height)?;
        if (width, height) != self.size() {
            self.targets = make_targets(&self.device, width, height);
        }
        Ok(())
    }

    /// The HDR colour target (linear radiance, [`HDR_FORMAT`]; usages
    /// `RENDER_ATTACHMENT | TEXTURE_BINDING | COPY_SRC`).
    pub fn hdr_texture(&self) -> &wgpu::Texture {
        &self.targets.hdr
    }

    /// A view of [`hdr_texture`](Self::hdr_texture).
    pub fn hdr_view(&self) -> &wgpu::TextureView {
        &self.targets.hdr_view
    }

    /// Counters.
    pub fn stats(&self) -> PbrStats {
        self.stats
    }

    /// The cluster lists of the last frame.
    pub fn cluster_assignment(&self) -> &ClusterAssignment {
        &self.assignment
    }

    /// Uploads `mesh`.
    pub fn upload_mesh(&mut self, mesh: &MeshGeometry) -> Result<MeshId, GpuError> {
        self.upload_mesh_data(&mesh.positions, &mesh.normals, &mesh.indices)
    }

    /// Uploads a triangle mesh (`normals.len() == positions.len()`, indices
    /// a non-empty multiple of 3, each `< positions.len()`), e.g. from a
    /// `pin_assets` `MeshData`.
    pub fn upload_mesh_data(
        &mut self,
        positions: &[[f32; 3]],
        normals: &[[f32; 3]],
        indices: &[u32],
    ) -> Result<MeshId, GpuError> {
        if positions.is_empty() || normals.len() != positions.len() {
            return Err(GpuError::InvalidArgument(format!(
                "mesh: {} positions, {} normals",
                positions.len(),
                normals.len()
            )));
        }
        if indices.is_empty()
            || !indices.len().is_multiple_of(3)
            || indices.iter().any(|&i| i as usize >= positions.len())
            || u32::try_from(indices.len()).is_err()
        {
            return Err(GpuError::InvalidArgument(format!(
                "mesh: {} indices over {} vertices (need a non-empty triangle list in range)",
                indices.len(),
                positions.len()
            )));
        }
        let mut vertices = Vec::with_capacity(positions.len() * 6);
        for (p, n) in positions.iter().zip(normals) {
            vertices.extend_from_slice(p);
            vertices.extend_from_slice(n);
        }
        let vertices = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("pbr mesh vertices"),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let index_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("pbr mesh indices"),
                contents: bytemuck::cast_slice(indices),
                usage: wgpu::BufferUsages::INDEX,
            });
        let id = MeshId(self.meshes.len() as u32);
        self.meshes.push(GpuMesh {
            vertices,
            indices: index_buffer,
            index_count: indices.len() as u32,
        });
        Ok(id)
    }

    /// Records `frame` into the HDR target (cleared to `frame.clear`).
    pub fn encode(&mut self, encoder: &mut wgpu::CommandEncoder, frame: &FrameDesc<'_>) {
        let camera = frame.camera;
        // Lights: at most MAX_LIGHTS, the rest counted.
        self.light_list.clear();
        for l in frame.lights {
            self.light_list.push(*l);
        }
        let rejected = self.light_list.rejected();
        self.stats.lights_rejected = self.stats.lights_rejected.saturating_add(rejected);
        self.stats.frame_lights_rejected = rejected;
        self.stats.frame_lights = self.light_list.len() as u32;
        self.stats.frames += 1;

        // Clusters.
        if !self
            .grid
            .as_ref()
            .is_some_and(|g| g.matches(DEFAULT_CLUSTER_DIMS, camera))
        {
            self.grid = Some(ClusterGrid::for_camera(camera));
        }
        let grid = self.grid.as_ref().expect("grid");
        assign_lights(
            grid,
            &camera.view,
            self.light_list.as_slice(),
            &mut self.assignment,
        );
        let dims = grid.dims();
        let cluster_params = [grid.near(), grid.far(), grid.slice_scale(), 0.0];
        self.stats.max_cluster_lights = self.assignment.max_count();
        self.upload_clusters();

        let env = frame.environment;
        let v4 = |c: [f32; 3]| [c[0], c[1], c[2], 0.0];
        let (w, h) = self.size();
        let uniform = FrameUniform {
            view: camera.view,
            proj: camera.proj,
            view_proj: camera.view_proj(),
            camera_pos: [camera.eye[0], camera.eye[1], camera.eye[2], 1.0],
            env_sky: v4(env.sky),
            env_horizon: v4(env.horizon),
            env_ground: v4(env.ground),
            gi: frame.gi,
            cluster_dims: [dims[0], dims[1], dims[2], self.light_list.len() as u32],
            cluster_params,
            screen: [w as f32, h as f32, 1.0 / w as f32, 1.0 / h as f32],
        };
        self.queue
            .write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(&uniform));
        if !self.light_list.is_empty() {
            self.queue.write_buffer(
                &self.light_buffer,
                0,
                bytemuck::cast_slice(self.light_list.as_slice()),
            );
        }

        // Per-draw uniforms.
        self.draw_data.clear();
        for d in frame.draws {
            self.draw_data.push(DrawUniform {
                model: d.model,
                normal_matrix: normal_matrix(&d.model),
                material: d.material,
                emission: [d.emission, 0.0, 0.0, 0.0],
                _pad: [[0.0; 4]; 3],
            });
        }
        let needed = self.draw_data.len() as u64;
        if needed > self.draw_capacity {
            self.draw_capacity = needed.next_power_of_two();
            self.draw_buffer = Self::draw_buffer(&self.device, self.draw_capacity);
            self.draw_bind =
                Self::draw_bind_group(&self.device, &self.draw_layout, &self.draw_buffer);
        }
        if !self.draw_data.is_empty() {
            self.queue
                .write_buffer(&self.draw_buffer, 0, bytemuck::cast_slice(&self.draw_data));
        }

        // Blended draws back to front (by the view depth of the origin).
        self.blend_order.clear();
        for (i, d) in frame.draws.iter().enumerate() {
            if d.transparent {
                let p = transform_point(&camera.view, [d.model[3][0], d.model[3][1], d.model[3][2]]);
                self.blend_order.push((-p[2], i as u32));
            }
        }
        self.blend_order
            .sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));

        let [r, g, b, a] = frame.clear.map(f64::from);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("pbr forward"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.targets.hdr_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.targets.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &self.frame_bind, &[]);
        let mut skipped = 0u64;
        let mut draw = |pass: &mut wgpu::RenderPass<'_>, i: usize, d: &DrawItem| {
            let Some(mesh) = self.meshes.get(d.mesh.0 as usize) else {
                skipped += 1;
                return;
            };
            pass.set_bind_group(1, &self.draw_bind, &[(i as u64 * DRAW_STRIDE) as u32]);
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, 0..1);
        };
        pass.set_pipeline(&self.opaque);
        for (i, d) in frame.draws.iter().enumerate() {
            if !d.transparent {
                draw(&mut pass, i, d);
            }
        }
        pass.set_pipeline(&self.blended);
        for &(_, i) in &self.blend_order {
            draw(&mut pass, i as usize, &frame.draws[i as usize]);
        }
        drop(pass);
        self.stats.draws_skipped = self.stats.draws_skipped.saturating_add(skipped);
    }

    /// Uploads the cluster lists, growing the storage buffers as needed.
    fn upload_clusters(&mut self) {
        let ranges = self.assignment.ranges();
        let indices = self.assignment.indices();
        let ranges_bytes = core::mem::size_of_val(ranges) as u64;
        let indices_bytes = (core::mem::size_of_val(indices) as u64).max(4);
        let mut rebind = false;
        if ranges_bytes > self.ranges_buffer.size() {
            self.ranges_buffer = storage_buffer(
                &self.device,
                "pbr cluster ranges",
                ranges_bytes.next_power_of_two(),
            );
            rebind = true;
        }
        if indices_bytes > self.indices_buffer.size() {
            self.indices_buffer = storage_buffer(
                &self.device,
                "pbr cluster indices",
                indices_bytes.next_power_of_two(),
            );
            rebind = true;
        }
        if rebind {
            self.frame_bind = Self::frame_bind_group(
                &self.device,
                &self.frame_layout,
                &self.frame_buffer,
                &self.light_buffer,
                &self.ranges_buffer,
                &self.indices_buffer,
            );
        }
        self.queue
            .write_buffer(&self.ranges_buffer, 0, bytemuck::cast_slice(ranges));
        if !indices.is_empty() {
            self.queue
                .write_buffer(&self.indices_buffer, 0, bytemuck::cast_slice(indices));
        }
    }

    /// Renders `frame`, submits, waits (bounded) and reads the HDR target
    /// back as linear RGBA `f32` (row 0 = top). For tests and tools.
    pub fn render_to_image(
        &mut self,
        ctx: &GpuContext,
        frame: &FrameDesc<'_>,
    ) -> Result<HdrImage, GpuError> {
        let device = ctx.device();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("pbr offscreen"),
        });
        self.encode(&mut encoder, frame);
        let (width, height) = self.size();
        let row = width as usize * 8;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
        let padded = row.div_ceil(align) * align;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pbr readback"),
            size: (padded * height as usize) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.targets.hdr,
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
            self.targets.hdr.size(),
        );
        ctx.queue().submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        readback.map_async(wgpu::MapMode::Read, .., move |r| {
            let _ = tx.send(r);
        });
        wait_for_device(device, READBACK_WAIT_TIMEOUT)
            .map_err(|e| GpuError::Readback(format!("poll: {e}")))?;
        rx.recv()
            .map_err(|e| GpuError::Readback(format!("map channel: {e}")))?
            .map_err(|e| GpuError::Readback(format!("map_async: {e}")))?;
        let mut pixels = Vec::with_capacity(width as usize * height as usize);
        {
            let mapped = readback
                .get_mapped_range(..)
                .map_err(|e| GpuError::Readback(format!("get_mapped_range: {e:?}")))?;
            for y in 0..height as usize {
                let line = &mapped[y * padded..y * padded + row];
                for px in line.chunks_exact(8) {
                    let h = |k: usize| f16_to_f32(u16::from_le_bytes([px[2 * k], px[2 * k + 1]]));
                    pixels.push([h(0), h(1), h(2), h(3)]);
                }
            }
        }
        readback.unmap();
        Ok(HdrImage {
            width,
            height,
            pixels,
        })
    }
}

/// `camera.proj · camera.view · model`, for callers projecting points.
pub fn model_view_proj(camera: &PbrCamera, model: &Mat4) -> Mat4 {
    mul(&camera.view_proj(), model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats_decode() {
        assert_eq!(f16_to_f32(0x0000), 0.0);
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        assert_eq!(f16_to_f32(0xc000), -2.0);
        assert_eq!(f16_to_f32(0x7bff), 65504.0);
        assert_eq!(f16_to_f32(0x0001), 1.0 / 16_777_216.0);
        assert_eq!(f16_to_f32(0x7c00), f32::INFINITY);
        assert_eq!(f16_to_f32(0xfc00), f32::NEG_INFINITY);
        assert!(f16_to_f32(0x7e00).is_nan());
        assert_eq!(f16_to_f32(0x3555), 0.333_251_95);
    }

    #[test]
    fn hdr_image_helpers() {
        let img = HdrImage {
            width: 2,
            height: 1,
            pixels: vec![[0.0, 0.5, 2.0, 1.0], [1.0, f32::NAN, 0.001, 1.0]],
        };
        assert_eq!(img.at(1, 0)[0], 1.0);
        assert_eq!(img.region_mean(0, 0, 2, 1)[2], 1.0005);
        assert_eq!(img.region_mean(5, 5, 6, 6), [0.0; 4]);
        let b = img.to_srgb8();
        assert_eq!(&b[..4], &[0, 188, 255, 255]);
        assert_eq!(b[5], 0);
        assert_eq!(b[6], 3);
        assert_eq!(Environment::default(), Environment::STUDIO);
    }
}
