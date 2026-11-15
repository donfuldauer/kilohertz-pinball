//! The GPU side: acceleration structures, the trace dispatch and the
//! [`ReflectionSource`] implementation.

use super::plan::{
    GeometryPool, InstanceRecord, LayerPlan, PlanDraw, PoolRange, RAY_ATLAS_LAYERS, RayJobKind,
    instance_record, mesh_top, plan_layers, tlas_transform, validate_mesh,
};
use super::select::FallbackReason;
use super::{RAY_QUERY_FEATURE, RayQueryConfig, ReflectionQuality, TRACE_SOURCE, validate_config};
use crate::gpu::{GpuContext, GpuError};
use crate::pbr::{
    FrameDesc, HDR_FORMAT, HdrImage, MAX_LIGHTS, MeshGeometry, MeshId, PbrRenderer,
    ReflectionBinding,
};
use crate::reflections::{
    MAX_PROBES, OctahedralProbes, ProbeStats, ReflectionSource, ReflectiveBall,
};
use core::fmt;
use std::collections::HashMap;

/// Jobs per dispatch (one per atlas layer); equals `TRACE_MAX_JOBS` in the
/// WGSL.
pub(crate) const MAX_JOBS: usize = RAY_ATLAS_LAYERS as usize;

/// Initial TLAS capacity (instances); grows by powers of two.
const INITIAL_TLAS_CAPACITY: u32 = 64;

/// Ray-query and acceleration-structure counters (5.4 budget: no per-frame
/// BLAS builds for static meshes).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct RayStats {
    /// Frames prepared on the ray-query path (not counting frames after a
    /// permanent fallback).
    pub frames: u64,
    /// Bottom-level structures built (one per static mesh, once).
    pub blas_builds: u64,
    /// Top-level structure builds / refits (one per ray-query frame).
    pub tlas_refits: u64,
    /// Top-level structures (re)allocated because the draw count grew.
    pub tlas_allocations: u64,
    /// Static meshes registered.
    pub static_meshes: u32,
    /// TLAS instances in the last frame.
    pub last_instances: u32,
    /// Draws of the last frame left out of the TLAS (mesh not registered).
    pub last_skipped_draws: u32,
    /// Atlas layers traced in the last frame.
    pub last_traced_layers: u32,
    /// Atlas layers traced over all frames.
    pub total_traced_layers: u64,
    /// Frames that used the probe result for every surface (nothing at or
    /// below the roughness cutoff).
    pub probe_frames: u64,
    /// `true` after the permanent fallback to probes.
    pub fallen_back: bool,
}

/// Why [`RayQueryReflections::new`] declined.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RayQueryError {
    /// The device was created without [`RAY_QUERY_FEATURE`] (the adapter
    /// lacks it, or the context did not request it).
    FeatureAbsent {
        /// The adapter's name.
        adapter: String,
    },
    /// [`ReflectionQuality::Off`]: nothing to trace.
    QualityOff,
    /// A static mesh cannot become an acceleration structure.
    InvalidMesh(String),
    /// The [`RayQueryConfig`] cannot be used on this device
    /// ([`validate_config`](super::validate_config) gives the reason).
    InvalidConfig(String),
    /// Creating the GPU objects failed.
    Gpu(GpuError),
}

impl RayQueryError {
    /// The [`FallbackReason`] the caller logs.
    pub fn reason(&self) -> FallbackReason {
        match self {
            RayQueryError::FeatureAbsent { .. } => FallbackReason::FeatureAbsent,
            RayQueryError::QualityOff => FallbackReason::QualityOff,
            RayQueryError::InvalidMesh(_)
            | RayQueryError::InvalidConfig(_)
            | RayQueryError::Gpu(_) => FallbackReason::RuntimeError,
        }
    }
}

impl fmt::Display for RayQueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RayQueryError::FeatureAbsent { adapter } => write!(
                f,
                "adapter \"{adapter}\": device lacks Features::EXPERIMENTAL_RAY_QUERY \
                 (create the context with GpuContext::new_headless_with_features)"
            ),
            RayQueryError::QualityOff => f.write_str("reflection quality is off"),
            RayQueryError::InvalidMesh(e) => write!(f, "static mesh rejected: {e}"),
            RayQueryError::InvalidConfig(e) => write!(f, "ray-query config rejected: {e}"),
            RayQueryError::Gpu(e) => write!(f, "ray-query setup failed: {e}"),
        }
    }
}

impl std::error::Error for RayQueryError {}

#[repr(C)]
#[derive(Copy, Clone, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct JobGpu {
    a: [f32; 4],
    b: [f32; 4],
    mode: [u32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct LightGpu {
    position_range: [f32; 4],
    color_intensity: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct TraceParams {
    eye: [f32; 4],
    env_sky: [f32; 4],
    env_horizon: [f32; 4],
    env_ground: [f32; 4],
    gi: [f32; 4],
    counts: [u32; 4],
    tune: [f32; 4],
    jobs: [JobGpu; MAX_JOBS],
    lights: [LightGpu; MAX_LIGHTS],
}

/// The `tune` vector of the trace uniform: x ray `t_max` (m), y ball
/// origin offset (fraction of the radius), z plane origin offset (m), w the
/// ray roughness cutoff (a hit at or below it gets a second traced bounce in
/// the shader; passed through unchanged, so a NaN cutoff never traces).
pub(crate) fn trace_tune(config: &RayQueryConfig) -> [f32; 4] {
    [
        config.max_distance.max(1e-3),
        config.origin_offset.max(0.0),
        config.plane_offset.max(0.0),
        config.ray_roughness_cutoff,
    ]
}

/// The `counts` vector of the trace uniform: x light count, y map side
/// (texels, [`RayQueryConfig::map_resolution`]), z job count, w rays per
/// ray cell ([`RayQueryConfig::supersample`]).
pub(crate) fn trace_counts(config: &RayQueryConfig, lights: u32, jobs: u32) -> [u32; 4] {
    [lights, config.map_resolution, jobs, config.supersample]
}

/// Ray cell side (texels; `mode.z` of a trace job): the clear-coat block
/// for a job planned at reduced resolution, 1 otherwise.
pub(crate) fn cell_block(config: &RayQueryConfig, half_res: bool) -> u32 {
    if half_res { config.clearcoat_block } else { 1 }
}

/// Byte size of the trace uniform (checked against the WGSL layout).
pub(crate) const TRACE_PARAMS_SIZE: usize = core::mem::size_of::<TraceParams>();

const _: () = assert!(TRACE_PARAMS_SIZE == 7 * 16 + MAX_JOBS * 48 + MAX_LIGHTS * 32);

struct StaticMesh {
    range: PoolRange,
    top: Option<f32>,
    blas: wgpu::Blas,
}

/// Hardware ray-query reflections (see the [module docs](super)).
///
/// Each frame ([`prepare`](ReflectionSource::prepare)) refits one TLAS
/// over every draw whose mesh was registered (balls, flippers, other
/// moving meshes by their transforms), then one compute dispatch traces the
/// octahedral reflection map of every ball and, with
/// [`ReflectionQuality::BallsAndClearCoat`], of the playfield clear coat
/// into its own atlas ([`RayQueryConfig::map_resolution`]² texels per
/// layer, [`RayQueryConfig::supersample`] rays per ray cell, one cell per
/// texel for balls and [`RayQueryConfig::clearcoat_block`]² texels for the
/// clear coat), which the PBR pass samples through the unchanged
/// [`ReflectionBinding`]. Surfaces rougher than
/// [`RayQueryConfig::ray_roughness_cutoff`] use the probe result: when no
/// surface of a frame is at or below the cutoff the frame runs the inner
/// [`OctahedralProbes`] and binds them (output identical to the probe
/// path); otherwise rough balls get a radially traced, probe-equivalent
/// map.
///
/// Any GPU error during a frame (validation, out of memory, internal; a
/// lost device shows up this way) switches the source permanently to its
/// inner probes, which keep rendering the same frame.
pub struct RayQueryReflections {
    config: RayQueryConfig,
    quality: ReflectionQuality,
    probes: OctahedralProbes,
    atlas: wgpu::Texture,
    atlas_bind: wgpu::BindGroup,
    storage_view: wgpu::TextureView,
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    params: Box<TraceParams>,
    params_buffer: wgpu::Buffer,
    pool: GeometryPool,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    pool_dirty: bool,
    meshes: Vec<StaticMesh>,
    mesh_index: HashMap<MeshId, usize>,
    tlas: wgpu::Tlas,
    tlas_capacity: u32,
    tlas_used: usize,
    records: Vec<InstanceRecord>,
    instance_buffer: wgpu::Buffer,
    instance_capacity: u64,
    trace_bind: Option<wgpu::BindGroup>,
    plan: LayerPlan,
    plan_draws: Vec<PlanDraw>,
    probes_bound: bool,
    failure: Option<String>,
    stats: ProbeStats,
    ray_stats: RayStats,
}

fn storage_buffer(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(16).next_multiple_of(16),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn create_tlas(device: &wgpu::Device, capacity: u32) -> wgpu::Tlas {
    device.create_tlas(&wgpu::CreateTlasDescriptor {
        label: Some("raytrace tlas"),
        max_instances: capacity,
        flags: wgpu::AccelerationStructureFlags::PREFER_FAST_BUILD
            | wgpu::AccelerationStructureFlags::ALLOW_UPDATE,
        update_mode: wgpu::AccelerationStructureUpdateMode::PreferUpdate,
    })
}

/// Pops the three error scopes of a guarded section (reverse order).
struct Scopes(Option<[wgpu::ErrorScopeGuard; 3]>);

impl Scopes {
    fn push(device: &wgpu::Device) -> Scopes {
        Scopes(Some([
            device.push_error_scope(wgpu::ErrorFilter::Validation),
            device.push_error_scope(wgpu::ErrorFilter::OutOfMemory),
            device.push_error_scope(wgpu::ErrorFilter::Internal),
        ]))
    }

    fn pop(mut self) -> Result<(), String> {
        let [v, o, i] = self.0.take().expect("scopes popped once");
        let mut first = None;
        for g in [i, o, v] {
            if let Some(e) = pollster::block_on(g.pop()) {
                first.get_or_insert_with(|| e.to_string());
            }
        }
        first.map_or(Ok(()), Err)
    }
}

impl RayQueryReflections {
    /// A ray-query source for `pbr` at `quality` over the static `meshes`
    /// (one BLAS each, built here; more with [`add_mesh`](Self::add_mesh)).
    ///
    /// `Err` with the reason when the device lacks [`RAY_QUERY_FEATURE`]
    /// (create the context with
    /// [`GpuContext::new_headless_with_features`]), for
    /// [`ReflectionQuality::Off`], for an unusable mesh, for a config that
    /// [`validate_config`](super::validate_config) rejects on this device,
    /// or when a GPU object cannot be created; the caller then uses the probe path
    /// ([`select_reflection_source`](super::select_reflection_source) does
    /// that).
    pub fn new(
        ctx: &GpuContext,
        pbr: &PbrRenderer,
        quality: ReflectionQuality,
        meshes: &[(MeshId, &MeshGeometry)],
        config: &RayQueryConfig,
    ) -> Result<RayQueryReflections, RayQueryError> {
        let device = ctx.device();
        if !device.features().contains(RAY_QUERY_FEATURE) {
            return Err(RayQueryError::FeatureAbsent {
                adapter: ctx.adapter_info().name,
            });
        }
        if quality == ReflectionQuality::Off {
            return Err(RayQueryError::QualityOff);
        }
        validate_config(config, device.limits().max_texture_dimension_2d)
            .map_err(RayQueryError::InvalidConfig)?;
        for (_, m) in meshes {
            validate_mesh(m).map_err(RayQueryError::InvalidMesh)?;
        }
        let probes = OctahedralProbes::new(ctx, pbr, config.probes).map_err(RayQueryError::Gpu)?;
        let scopes = Scopes::push(device);
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("raytrace atlas"),
            size: wgpu::Extent3d {
                width: config.map_resolution,
                height: config.map_resolution,
                depth_or_array_layers: RAY_ATLAS_LAYERS,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let array_view = || {
            atlas.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            })
        };
        let sample_view = array_view();
        let storage_view = array_view();
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("raytrace atlas"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let atlas_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("raytrace atlas"),
            layout: pbr.reflection_layout(),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&sample_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let storage = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("raytrace"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::AccelerationStructure {
                        vertex_return: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: HDR_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            core::mem::size_of::<TraceParams>() as u64,
                        ),
                    },
                    count: None,
                },
                storage(3),
                storage(4),
                storage(5),
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("raytrace_trace"),
            source: wgpu::ShaderSource::Wgsl(TRACE_SOURCE.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("raytrace"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("raytrace"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("cs_trace"),
            compilation_options: Default::default(),
            cache: None,
        });
        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("raytrace params"),
            size: core::mem::size_of::<TraceParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let instance_capacity = u64::from(INITIAL_TLAS_CAPACITY);
        let instance_buffer = storage_buffer(
            device,
            "raytrace instances",
            instance_capacity * core::mem::size_of::<InstanceRecord>() as u64,
        );
        let tlas = create_tlas(device, INITIAL_TLAS_CAPACITY);
        scopes
            .pop()
            .map_err(|e| RayQueryError::Gpu(GpuError::InvalidArgument(e)))?;
        let mut rq = RayQueryReflections {
            config: *config,
            quality,
            probes,
            atlas,
            atlas_bind,
            storage_view,
            pipeline,
            layout,
            params: Box::new(bytemuck::Zeroable::zeroed()),
            params_buffer,
            pool: GeometryPool::default(),
            vertex_buffer: storage_buffer(device, "raytrace vertices", 16),
            index_buffer: storage_buffer(device, "raytrace indices", 16),
            pool_dirty: true,
            meshes: Vec::with_capacity(meshes.len()),
            mesh_index: HashMap::with_capacity(meshes.len()),
            tlas,
            tlas_capacity: INITIAL_TLAS_CAPACITY,
            tlas_used: 0,
            records: Vec::with_capacity(INITIAL_TLAS_CAPACITY as usize),
            instance_buffer,
            instance_capacity,
            trace_bind: None,
            plan: LayerPlan::default(),
            plan_draws: Vec::new(),
            probes_bound: true,
            failure: None,
            stats: ProbeStats {
                budget: RAY_ATLAS_LAYERS,
                ..ProbeStats::default()
            },
            ray_stats: RayStats::default(),
        };
        for (id, m) in meshes {
            rq.add_mesh(ctx, *id, m).map_err(|e| match e {
                GpuError::InvalidArgument(s) => RayQueryError::InvalidMesh(s),
                e => RayQueryError::Gpu(e),
            })?;
        }
        Ok(rq)
    }

    /// Registers static mesh `id` (the same id the PBR renderer uses) and
    /// builds its bottom-level structure (once: registering an id again is
    /// a no-op). Draws of unregistered meshes are invisible to rays.
    pub fn add_mesh(
        &mut self,
        ctx: &GpuContext,
        id: MeshId,
        mesh: &MeshGeometry,
    ) -> Result<(), GpuError> {
        if self.mesh_index.contains_key(&id) {
            return Ok(());
        }
        let range = self
            .pool
            .add(mesh)
            .map_err(|e| GpuError::InvalidArgument(format!("ray-query mesh: {e}")))?;
        let device = ctx.device();
        let scopes = Scopes::push(device);
        let size = wgpu::BlasTriangleGeometrySizeDescriptor {
            vertex_format: wgpu::VertexFormat::Float32x3,
            vertex_count: mesh.positions.len() as u32,
            index_format: Some(wgpu::IndexFormat::Uint32),
            index_count: Some(mesh.indices.len() as u32),
            flags: wgpu::AccelerationStructureGeometryFlags::OPAQUE,
        };
        let blas = device.create_blas(
            &wgpu::CreateBlasDescriptor {
                label: Some("raytrace blas"),
                flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
                update_mode: wgpu::AccelerationStructureUpdateMode::Build,
            },
            wgpu::BlasGeometrySizeDescriptors::Triangles {
                descriptors: vec![size.clone()],
            },
        );
        let input = |label: &str, bytes: &[u8]| {
            use wgpu::util::DeviceExt;
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytes,
                usage: wgpu::BufferUsages::BLAS_INPUT,
            })
        };
        let vertices = input(
            "raytrace blas vertices",
            bytemuck::cast_slice(&mesh.positions),
        );
        let indices = input("raytrace blas indices", bytemuck::cast_slice(&mesh.indices));
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("raytrace blas"),
        });
        encoder.build_acceleration_structures(
            [&wgpu::BlasBuildEntry {
                blas: &blas,
                geometry: wgpu::BlasGeometries::TriangleGeometries(vec![
                    wgpu::BlasTriangleGeometry {
                        size: &size,
                        vertex_buffer: &vertices,
                        first_vertex: 0,
                        vertex_stride: 12,
                        index_buffer: Some(&indices),
                        first_index: Some(0),
                        transform_buffer: None,
                        transform_buffer_offset: None,
                    },
                ]),
            }],
            core::iter::empty::<&wgpu::Tlas>(),
        );
        ctx.queue().submit([encoder.finish()]);
        scopes
            .pop()
            .map_err(|e| GpuError::InvalidArgument(format!("ray-query BLAS build failed: {e}")))?;
        self.mesh_index.insert(id, self.meshes.len());
        self.meshes.push(StaticMesh {
            range,
            top: mesh_top(mesh),
            blas,
        });
        self.pool_dirty = true;
        self.ray_stats.blas_builds += 1;
        self.ray_stats.static_meshes = self.meshes.len() as u32;
        Ok(())
    }

    /// The settings.
    pub fn config(&self) -> &RayQueryConfig {
        &self.config
    }

    /// The quality setting.
    pub fn quality(&self) -> ReflectionQuality {
        self.quality
    }

    /// Changes what is traced from the next frame on;
    /// [`ReflectionQuality::Off`] makes every frame use the inner probes.
    pub fn set_quality(&mut self, quality: ReflectionQuality) {
        self.quality = quality;
    }

    /// Ray-query counters.
    pub fn ray_stats(&self) -> RayStats {
        self.ray_stats
    }

    /// The last frame's layer plan.
    pub fn plan(&self) -> &LayerPlan {
        &self.plan
    }

    /// `true` when the last frame bound the inner probes (nothing traced,
    /// or after the permanent fallback).
    pub fn probes_bound(&self) -> bool {
        self.probes_bound || self.failure.is_some()
    }

    /// Why the source fell back to probes for good, if it did.
    pub fn fallback_reason(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// Switches to the inner probes for good (e.g. when the application
    /// learns that the device was lost). Idempotent; logged once per
    /// process.
    pub fn fall_back(&mut self, reason: &str) {
        if self.failure.is_none() {
            super::select::log_fallback_once(&format!(
                "{}: {reason}",
                FallbackReason::RuntimeError
            ));
            self.failure = Some(reason.to_owned());
            self.ray_stats.fallen_back = true;
        }
    }

    /// Reads atlas layer `layer` back
    /// ([`RayQueryConfig::map_resolution`]², linear; the
    /// octahedral layout of [`oct_decode`](crate::reflections::oct_decode)).
    pub fn read_layer(&self, ctx: &GpuContext, layer: u32) -> Result<HdrImage, GpuError> {
        if layer >= RAY_ATLAS_LAYERS {
            return Err(GpuError::InvalidArgument(format!(
                "ray atlas layer {layer} (max {})",
                RAY_ATLAS_LAYERS - 1
            )));
        }
        crate::post::readback::read_hdr_layer(ctx, &self.atlas, layer)
    }

    fn upload_pool(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let vbytes: &[u8] = bytemuck::cast_slice(&self.pool.vertices);
        let ibytes: &[u8] = bytemuck::cast_slice(&self.pool.indices);
        self.vertex_buffer = storage_buffer(device, "raytrace vertices", vbytes.len() as u64);
        self.index_buffer = storage_buffer(device, "raytrace indices", ibytes.len() as u64);
        if !vbytes.is_empty() {
            queue.write_buffer(&self.vertex_buffer, 0, vbytes);
        }
        if !ibytes.is_empty() {
            queue.write_buffer(&self.index_buffer, 0, ibytes);
        }
        self.pool_dirty = false;
        self.trace_bind = None;
    }

    fn fill_params(&mut self, frame: &FrameDesc<'_>) {
        let p = &mut *self.params;
        let clean = |v: f32| if v.is_finite() { v } else { 0.0 };
        let v4 = |c: [f32; 3]| {
            let c = c.map(|v| clean(v).max(0.0));
            [c[0], c[1], c[2], 0.0]
        };
        let e = frame.camera.eye.map(clean);
        p.eye = [e[0], e[1], e[2], 1.0];
        p.env_sky = v4(frame.environment.sky);
        p.env_horizon = v4(frame.environment.horizon);
        p.env_ground = v4(frame.environment.ground);
        p.gi = frame.gi.map(|v| clean(v).max(0.0));
        let n_lights = frame.lights.len().min(MAX_LIGHTS);
        for (dst, l) in p.lights.iter_mut().zip(&frame.lights[..n_lights]) {
            *dst = LightGpu {
                position_range: [l.position[0], l.position[1], l.position[2], l.range],
                color_intensity: [l.color[0], l.color[1], l.color[2], l.intensity],
            };
        }
        let n_jobs = if self.plan.traced {
            self.plan.jobs.len().min(MAX_JOBS)
        } else {
            0
        };
        for (dst, job) in p.jobs.iter_mut().zip(&self.plan.jobs[..n_jobs]) {
            *dst = match job.kind {
                RayJobKind::BallView { centre, radius } => JobGpu {
                    a: [centre[0], centre[1], centre[2], radius],
                    b: [0.0; 4],
                    mode: [0, job.layer, 1, 0],
                },
                RayJobKind::BallCentre { centre, radius } => JobGpu {
                    a: [centre[0], centre[1], centre[2], radius],
                    b: [0.0; 4],
                    mode: [1, job.layer, 1, 0],
                },
                RayJobKind::Plane { point, normal } => JobGpu {
                    a: [point[0], point[1], point[2], 0.0],
                    b: [normal[0], normal[1], normal[2], 0.0],
                    mode: [2, job.layer, cell_block(&self.config, job.half_res), 0],
                },
            };
        }
        p.counts = trace_counts(&self.config, n_lights as u32, n_jobs as u32);
        p.tune = trace_tune(&self.config);
    }

    /// TLAS refit and (when anything is traced) the trace dispatch, all in
    /// one submission. `Err` with the first GPU error.
    fn trace(&mut self, ctx: &GpuContext, frame: &FrameDesc<'_>) -> Result<u32, String> {
        let device = ctx.device();
        let queue = ctx.queue();
        let scopes = Scopes::push(device);
        if self.pool_dirty {
            self.upload_pool(device, queue);
        }
        // Instances: every draw with a registered mesh, in draw order.
        self.records.clear();
        let mut skipped = 0u32;
        for d in frame.draws {
            match self.mesh_index.get(&d.mesh) {
                Some(&m) => self.records.push(instance_record(
                    &d.material,
                    d.emission,
                    self.meshes[m].range,
                )),
                None => skipped += 1,
            }
        }
        let n = self.records.len();
        if n as u64 > u64::from(self.tlas_capacity) {
            let cap = (n as u32).next_power_of_two();
            self.tlas = create_tlas(device, cap);
            self.tlas_capacity = cap;
            self.tlas_used = 0;
            self.trace_bind = None;
            self.ray_stats.tlas_allocations += 1;
        }
        if n as u64 > self.instance_capacity {
            self.instance_capacity = (n as u64).next_power_of_two();
            self.instance_buffer = storage_buffer(
                device,
                "raytrace instances",
                self.instance_capacity * core::mem::size_of::<InstanceRecord>() as u64,
            );
            self.trace_bind = None;
        }
        let mut k = 0usize;
        for d in frame.draws {
            if let Some(&m) = self.mesh_index.get(&d.mesh) {
                let mask = if d.transparent { 2 } else { 1 };
                self.tlas[k] = Some(wgpu::TlasInstance::new(
                    &self.meshes[m].blas,
                    tlas_transform(&d.model),
                    k as u32,
                    mask,
                ));
                k += 1;
            }
        }
        for i in n..self.tlas_used {
            self.tlas[i] = None;
        }
        self.tlas_used = n;
        if n > 0 {
            queue.write_buffer(
                &self.instance_buffer,
                0,
                bytemuck::cast_slice(&self.records),
            );
        }
        self.fill_params(frame);
        queue.write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(&*self.params));
        if self.trace_bind.is_none() {
            self.trace_bind = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("raytrace"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.tlas.as_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&self.storage_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.params_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: self.instance_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: self.vertex_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: self.index_buffer.as_entire_binding(),
                    },
                ],
            }));
        }
        let jobs = self.params.counts[2];
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("raytrace"),
        });
        encoder.build_acceleration_structures(core::iter::empty(), [&self.tlas]);
        if jobs > 0 {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("raytrace"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, self.trace_bind.as_ref(), &[]);
            let groups = self.config.map_resolution.div_ceil(8);
            pass.dispatch_workgroups(groups, groups, jobs);
        }
        queue.submit([encoder.finish()]);
        scopes.pop()?;
        self.ray_stats.tlas_refits += 1;
        self.ray_stats.last_instances = n as u32;
        self.ray_stats.last_skipped_draws = skipped;
        Ok(jobs)
    }
}

impl ReflectionSource for RayQueryReflections {
    /// `"ray_query"`; after the permanent fallback, the probes' name.
    fn name(&self) -> &'static str {
        if self.failure.is_some() {
            self.probes.name()
        } else {
            "ray_query"
        }
    }

    fn prepare(
        &mut self,
        ctx: &GpuContext,
        pbr: &mut PbrRenderer,
        frame: &FrameDesc<'_>,
        balls: &[ReflectiveBall],
    ) -> Result<(), GpuError> {
        if self.failure.is_some() {
            self.probes_bound = true;
            return self.probes.prepare(ctx, pbr, frame, balls);
        }
        self.plan_draws.clear();
        for d in frame.draws {
            let top = self
                .mesh_index
                .get(&d.mesh)
                .and_then(|&m| self.meshes[m].top);
            self.plan_draws.push(PlanDraw::of(d, top));
        }
        plan_layers(
            self.quality,
            self.config.ray_roughness_cutoff,
            &self.plan_draws,
            balls,
            &mut self.plan,
        );
        let traced = match self.trace(ctx, frame) {
            Ok(jobs) => jobs,
            Err(e) => {
                self.fall_back(&e);
                self.probes_bound = true;
                return self.probes.prepare(ctx, pbr, frame, balls);
            }
        };
        self.ray_stats.frames += 1;
        self.ray_stats.last_traced_layers = traced;
        self.ray_stats.total_traced_layers += u64::from(traced);
        self.stats.frames += 1;
        self.stats.balls = balls.len() as u32;
        let updates = if self.plan.traced {
            self.probes_bound = false;
            self.stats.max_age = 0;
            traced
        } else {
            self.probes_bound = true;
            self.ray_stats.probe_frames += 1;
            self.probes.prepare(ctx, pbr, frame, balls)?;
            let p = self.probes.stats();
            self.stats.max_age = p.max_age;
            p.last_frame_updates
        };
        self.stats.last_frame_updates = updates;
        self.stats.total_updates += u64::from(updates);
        self.stats.max_frame_updates = self.stats.max_frame_updates.max(updates);
        self.stats.max_age_ever = self.stats.max_age_ever.max(self.stats.max_age);
        Ok(())
    }

    fn binding(&self) -> ReflectionBinding<'_> {
        if self.probes_bound() {
            self.probes.binding()
        } else {
            ReflectionBinding {
                bind_group: &self.atlas_bind,
                draw_layers: &self.plan.draw_layers,
            }
        }
    }

    /// The probes' counters after the permanent fallback; before it,
    /// updates count traced layers (or probe captures on probe-only
    /// frames) and the budget is the number of atlas layers.
    fn stats(&self) -> ProbeStats {
        if self.failure.is_some() {
            self.probes.stats()
        } else {
            self.stats
        }
    }
}

const _: () = assert!(MAX_JOBS == MAX_PROBES + 1);
