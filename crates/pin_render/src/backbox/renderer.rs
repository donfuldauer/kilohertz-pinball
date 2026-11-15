//! The GPU backbox pass: the scene drawn with its own camera into an
//! offscreen colour + depth target, then composited into the backglass
//! between translite layers.

use super::scene::{BackboxScene, BackboxView, compose};
use super::toys::ToyRig;
use crate::displays::{BackglassDesc, BackglassRenderer, RgbaImage, fit_aspect};
use crate::gpu::{GpuContext, GpuError};
use crate::pbr::MeshGeometry;
use crate::pbr::math::{IDENTITY, Mat4, mul, normal_matrix, scale_translation};
use crate::topology::Rect;
use wgpu::util::DeviceExt;

/// Colour format of the offscreen backbox target (display-encoded values,
/// premultiplied alpha: 1 where the scene drew, 0 elsewhere).
pub const BACKBOX_COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Depth format of the offscreen backbox target.
pub const BACKBOX_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Most balls drawn on the backbox at once.
pub const MAX_BACKBOX_BALLS: usize = 16;
/// Radius of a drawn ball, m (the standard 27 mm ball).
pub const BALL_RADIUS_M: f32 = 0.0135;
/// Colour of a drawn ball (display-encoded).
pub const BALL_COLOR: [f32; 4] = [0.80, 0.80, 0.84, 1.0];
/// Byte stride of one draw's uniforms (the largest
/// `min_uniform_buffer_offset_alignment` wgpu allows).
const DRAW_STRIDE: u64 = 256;

/// Per-draw uniforms (`Draw` in the WGSL), padded to [`DRAW_STRIDE`].
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct DrawUniform {
    mvp: Mat4,
    normal: Mat4,
    color: [f32; 4],
    light: [f32; 4],
    _pad: [f32; 24],
}

const SCENE_WGSL: &str = r#"
struct Draw {
    mvp: mat4x4<f32>,
    normal: mat4x4<f32>,
    color: vec4<f32>,
    light: vec4<f32>,
};

@group(0) @binding(0) var<uniform> draw: Draw;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) normal: vec3<f32>,
};

@vertex
fn vs_scene(@location(0) p: vec3<f32>, @location(1) n: vec3<f32>) -> VsOut {
    var out: VsOut;
    out.pos = draw.mvp * vec4<f32>(p, 1.0);
    out.normal = (draw.normal * vec4<f32>(n, 0.0)).xyz;
    return out;
}

// Two-sided Lambert with an ambient floor: k = a + (1 − a)·max(n·l, 0).
// The colour is display-encoded; the result is opaque (alpha 1).
@fragment
fn fs_scene(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    var k = draw.light.w;
    let len = length(in.normal);
    if (len > 1.0e-6) {
        var n = in.normal / len;
        if (!front) {
            n = -n;
        }
        k = draw.light.w + (1.0 - draw.light.w) * max(dot(n, draw.light.xyz), 0.0);
    }
    return vec4<f32>(clamp(draw.color.rgb * k, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
"#;

const COMPOSITE_WGSL: &str = r#"
struct Comp {
    origin: vec2<f32>,
    srgb: f32,
    _pad: f32,
};

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var<uniform> comp: Comp;

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    return vec4<f32>(p, 0.0, 1.0);
}

// Premultiplied "over" of the offscreen backbox image placed at `origin`
// (target pixels), 1:1. Display-encoded → linear for *Srgb targets, as the
// display quads do.
@fragment
fn fs_composite(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let dims = vec2<i32>(textureDimensions(src));
    let t = vec2<i32>(floor(pos.xy - comp.origin));
    if (any(t < vec2<i32>(0, 0)) || any(t >= dims)) {
        discard;
    }
    let c = textureLoad(src, t, 0);
    if (comp.srgb > 0.5) {
        return vec4<f32>(srgb_to_linear(clamp(c.rgb, vec3<f32>(0.0), vec3<f32>(1.0))), c.a);
    }
    return c;
}
"#;

/// `(name, WGSL source)` of the backbox shaders (for naga validation).
pub fn shader_sources() -> [(&'static str, &'static str); 2] {
    [
        ("backbox_scene", SCENE_WGSL),
        ("backbox_composite", COMPOSITE_WGSL),
    ]
}

/// Lighting of the backbox scene: `k = ambient + (1 − ambient)·max(n·l, 0)`
/// times the mesh colour.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BackboxLighting {
    /// Ambient floor, 0..=1 (1 = unlit flat colours).
    pub ambient: f32,
    /// Direction towards the key light, scene frame (normalised on use).
    pub key_dir: [f32; 3],
}

impl Default for BackboxLighting {
    /// A key light from the front, upper left (towards a camera on the `−y`
    /// side), ambient 0.45.
    fn default() -> Self {
        BackboxLighting {
            ambient: 0.45,
            key_dir: [-0.3, -0.8, 0.5],
        }
    }
}

impl BackboxLighting {
    /// Flat colours (ambient 1).
    pub const UNLIT: BackboxLighting = BackboxLighting {
        ambient: 1.0,
        key_dir: [0.0, -1.0, 0.0],
    };

    /// `[l̂, ambient]` as the shader reads it (`l̂` unit or 0, ambient
    /// clamped to 0..=1, NaN → 1).
    pub(super) fn uniform(&self) -> [f32; 4] {
        let a = if self.ambient.is_finite() {
            self.ambient.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let d = self.key_dir;
        let len = libm::sqrtf(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]);
        if len.is_finite() && len > 1e-6 {
            [d[0] / len, d[1] / len, d[2] / len, a]
        } else {
            [0.0, 0.0, 0.0, a]
        }
    }
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    count: u32,
}

fn upload(device: &wgpu::Device, name: &str, g: &MeshGeometry) -> Result<GpuMesh, GpuError> {
    let n = g.positions.len();
    if n == 0
        || g.normals.len() != n
        || g.indices.is_empty()
        || !g.indices.len().is_multiple_of(3)
        || g.indices.iter().any(|&i| i as usize >= n)
        || u32::try_from(g.indices.len()).is_err()
    {
        return Err(GpuError::InvalidArgument(format!(
            "backbox mesh {name:?}: {n} positions, {} normals, {} indices (need a non-empty \
             triangle list in range with one normal per position)",
            g.normals.len(),
            g.indices.len()
        )));
    }
    let mut vertices = Vec::with_capacity(n * 6);
    for (p, nm) in g.positions.iter().zip(&g.normals) {
        vertices.extend_from_slice(p);
        vertices.extend_from_slice(nm);
    }
    Ok(GpuMesh {
        vertices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("backbox mesh vertices"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        }),
        indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("backbox mesh indices"),
            contents: bytemuck::cast_slice(&g.indices),
            usage: wgpu::BufferUsages::INDEX,
        }),
        count: g.indices.len() as u32,
    })
}

struct Offscreen {
    size: (u32, u32),
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    composite: wgpu::BindGroup,
}

/// Draws a [`BackboxScene`] with its own camera into an offscreen target
/// and composites it into a backglass.
pub struct BackboxRenderer {
    device: wgpu::Device,
    scene_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    composite_layout: wgpu::BindGroupLayout,
    composite_uniform: wgpu::Buffer,
    srgb: bool,
    draw_buffer: wgpu::Buffer,
    draw_bind: wgpu::BindGroup,
    draw_capacity: usize,
    meshes: Vec<GpuMesh>,
    names: Vec<String>,
    colors: Vec<[f32; 4]>,
    base: Vec<Mat4>,
    dynamic: Vec<Mat4>,
    visible: Vec<bool>,
    ball: GpuMesh,
    balls: Vec<[f32; 3]>,
    view: BackboxView,
    lighting: BackboxLighting,
    target: Option<Offscreen>,
    draws: Vec<DrawUniform>,
}

impl BackboxRenderer {
    /// Uploads `scene` for compositing into targets of `format`. A mesh
    /// without a valid triangle list (or with a normal count different from
    /// its position count) is refused with [`GpuError::InvalidArgument`].
    pub fn new(
        ctx: &GpuContext,
        format: wgpu::TextureFormat,
        scene: &BackboxScene,
        view: BackboxView,
    ) -> Result<BackboxRenderer, GpuError> {
        let device = ctx.device();
        let mut meshes = Vec::with_capacity(scene.meshes.len());
        for m in &scene.meshes {
            meshes.push(upload(device, &m.name, &m.geometry)?);
        }
        let ball = upload(device, "ball", &MeshGeometry::uv_sphere(1.0, 24, 16))?;

        let scene_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("backbox scene"),
            source: wgpu::ShaderSource::Wgsl(SCENE_WGSL.into()),
        });
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("backbox draw"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(160),
                },
                count: None,
            }],
        });
        let scene_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("backbox scene"),
            bind_group_layouts: &[Some(&draw_layout)],
            immediate_size: 0,
        });
        const ATTRIBUTES: [wgpu::VertexAttribute; 2] =
            wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3];
        let scene_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("backbox scene"),
            layout: Some(&scene_layout),
            vertex: wgpu::VertexState {
                module: &scene_module,
                entry_point: Some("vs_scene"),
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
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: BACKBOX_DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &scene_module,
                entry_point: Some("fs_scene"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: BACKBOX_COLOR_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let composite_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("backbox composite"),
            source: wgpu::ShaderSource::Wgsl(COMPOSITE_WGSL.into()),
        });
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("backbox composite"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(16),
                    },
                    count: None,
                },
            ],
        });
        let composite_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("backbox composite"),
                bind_group_layouts: &[Some(&composite_layout)],
                immediate_size: 0,
            });
        let composite_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("backbox composite"),
            layout: Some(&composite_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &composite_module,
                entry_point: Some("vs_full"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &composite_module,
                entry_point: Some("fs_composite"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let composite_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("backbox composite"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let draw_capacity = scene.meshes.len() + MAX_BACKBOX_BALLS;
        let draw_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("backbox draws"),
            size: DRAW_STRIDE * draw_capacity as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let draw_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("backbox draws"),
            layout: &draw_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &draw_buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(160),
                }),
            }],
        });
        Ok(BackboxRenderer {
            device: device.clone(),
            scene_pipeline,
            composite_pipeline,
            composite_layout,
            composite_uniform,
            srgb: format.is_srgb(),
            draw_buffer,
            draw_bind,
            draw_capacity,
            meshes,
            names: scene.meshes.iter().map(|m| m.name.clone()).collect(),
            colors: scene.meshes.iter().map(|m| m.color).collect(),
            base: scene.meshes.iter().map(|m| m.transform).collect(),
            dynamic: vec![IDENTITY; scene.meshes.len()],
            visible: vec![true; scene.meshes.len()],
            ball,
            balls: Vec::with_capacity(MAX_BACKBOX_BALLS),
            view,
            lighting: BackboxLighting::default(),
            target: None,
            draws: Vec::with_capacity(draw_capacity),
        })
    }

    /// Number of scene meshes.
    pub fn mesh_count(&self) -> usize {
        self.meshes.len()
    }

    /// Name of mesh `i`.
    pub fn mesh_name(&self, i: usize) -> Option<&str> {
        self.names.get(i).map(String::as_str)
    }

    /// The camera.
    pub fn view(&self) -> BackboxView {
        self.view
    }

    /// Replaces the camera.
    pub fn set_view(&mut self, view: BackboxView) {
        self.view = view;
    }

    /// Replaces the lighting.
    pub fn set_lighting(&mut self, lighting: BackboxLighting) {
        self.lighting = lighting;
    }

    /// Sets mesh `i`'s dynamic transform (applied on top of its own:
    /// `dynamic · base`). Unknown indices are ignored.
    pub fn set_dynamic(&mut self, i: usize, m: Mat4) {
        if let Some(d) = self.dynamic.get_mut(i) {
            *d = m;
        }
    }

    /// Sets the dynamic transform of every mesh named `name`; returns how
    /// many meshes it set.
    pub fn set_dynamic_by_name(&mut self, name: &str, m: Mat4) -> usize {
        let mut n = 0;
        for (d, nm) in self.dynamic.iter_mut().zip(&self.names) {
            if nm == name {
                *d = m;
                n += 1;
            }
        }
        n
    }

    /// Applies every toy transform of `rig` (by mesh name).
    pub fn apply_toys(&mut self, rig: &ToyRig) {
        rig.for_each_transform(|name, m| {
            self.set_dynamic_by_name(name, *m);
        });
    }

    /// Shows or hides mesh `i`.
    pub fn set_visible(&mut self, i: usize, visible: bool) {
        if let Some(v) = self.visible.get_mut(i) {
            *v = visible;
        }
    }

    /// The balls to draw this frame (scene frame centres, m; at most
    /// [`MAX_BACKBOX_BALLS`], non-finite ones skipped).
    pub fn set_balls(&mut self, centres: &[[f32; 3]]) {
        self.balls.clear();
        for c in centres {
            if self.balls.len() < MAX_BACKBOX_BALLS && c.iter().all(|v| v.is_finite()) {
                self.balls.push(*c);
            }
        }
    }

    fn ensure_target(&mut self, size: (u32, u32)) -> Result<(), GpuError> {
        if self.target.as_ref().is_some_and(|t| t.size == size) {
            return Ok(());
        }
        let max = self.device.limits().max_texture_dimension_2d;
        if size.0 == 0 || size.1 == 0 || size.0 > max || size.1 > max {
            return Err(GpuError::InvalidArgument(format!(
                "backbox target {}x{} (each side must be 1..={max})",
                size.0, size.1
            )));
        }
        let extent = wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        };
        let color = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("backbox colour"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: BACKBOX_COLOR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let depth = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("backbox depth"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: BACKBOX_DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
        let composite = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("backbox composite"),
            layout: &self.composite_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&color_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.composite_uniform.as_entire_binding(),
                },
            ],
        });
        self.target = Some(Offscreen {
            size,
            color,
            color_view,
            depth_view: depth.create_view(&wgpu::TextureViewDescriptor::default()),
            composite,
        });
        Ok(())
    }

    /// Records the scene into the offscreen target of `size` (created or
    /// resized as needed): cleared to transparent, depth-tested, with the
    /// backbox camera at the target's aspect.
    ///
    /// The draw uniforms are captured from the renderer's state at the time
    /// of this call and uploaded by a copy recorded in `encoder` just before
    /// the scene pass, so it is safe to record several encodes (with state
    /// changes in between) before one submit: each pass sees its own data.
    pub fn encode_scene(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        size: (u32, u32),
    ) -> Result<(), GpuError> {
        self.ensure_target(size)?;
        let cam = self.view.camera(size.0 as f32 / size.1 as f32);
        let vp = cam.view_proj();
        let light = self.lighting.uniform();
        self.draws.clear();
        let mut order: Vec<(usize, Option<usize>)> = Vec::with_capacity(self.draw_capacity);
        for i in 0..self.meshes.len() {
            if !self.visible[i] {
                continue;
            }
            let model = compose(&self.dynamic[i], &self.base[i]);
            self.draws.push(DrawUniform {
                mvp: mul(&vp, &model),
                normal: normal_matrix(&model),
                color: self.colors[i],
                light,
                _pad: [0.0; 24],
            });
            order.push((i, None));
        }
        for (k, c) in self.balls.iter().enumerate() {
            let model = scale_translation(BALL_RADIUS_M, *c);
            self.draws.push(DrawUniform {
                mvp: mul(&vp, &model),
                normal: normal_matrix(&model),
                color: BALL_COLOR,
                light,
                _pad: [0.0; 24],
            });
            order.push((usize::MAX, Some(k)));
        }
        let Some(target) = &self.target else {
            return Ok(());
        };
        if !self.draws.is_empty() {
            let bytes: &[u8] = bytemuck::cast_slice(&self.draws);
            encode_upload(
                &self.device,
                encoder,
                "backbox draws",
                bytes,
                &self.draw_buffer,
            );
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("backbox scene"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.color_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &target.depth_view,
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
        pass.set_pipeline(&self.scene_pipeline);
        for (slot, (mesh, ball)) in order.iter().enumerate() {
            let m = match ball {
                Some(_) => &self.ball,
                None => &self.meshes[*mesh],
            };
            pass.set_bind_group(0, &self.draw_bind, &[(slot as u64 * DRAW_STRIDE) as u32]);
            pass.set_vertex_buffer(0, m.vertices.slice(..));
            pass.set_index_buffer(m.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..m.count, 0, 0..1);
        }
        Ok(())
    }

    /// Composites the last [`encode_scene`](Self::encode_scene) image into
    /// `view` (a texture of `target_size`) with its top-left at `rect`'s
    /// (1:1 pixels, premultiplied "over", clipped to `rect` and the target).
    /// Does nothing before the first scene encode.
    ///
    /// The composite uniform (`rect` origin, sRGB flag) is uploaded by a copy
    /// recorded in `encoder` just before the composite pass, so it is safe to
    /// record several composites (into different rectangles or views) before
    /// one submit: each pass uses its own `rect`.
    pub fn encode_composite(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        target_size: (u32, u32),
        rect: Rect,
    ) {
        let Some(target) = &self.target else {
            return;
        };
        let x0 = i64::from(rect.x).clamp(0, i64::from(target_size.0));
        let y0 = i64::from(rect.y).clamp(0, i64::from(target_size.1));
        let x1 = rect.right().clamp(0, i64::from(target_size.0));
        let y1 = rect.bottom().clamp(0, i64::from(target_size.1));
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let uniform = [
            rect.x as f32,
            rect.y as f32,
            if self.srgb { 1.0 } else { 0.0 },
            0.0f32,
        ];
        encode_upload(
            &self.device,
            encoder,
            "backbox composite",
            bytemuck::cast_slice(&uniform),
            &self.composite_uniform,
        );
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("backbox composite"),
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
        });
        pass.set_scissor_rect(x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32);
        pass.set_pipeline(&self.composite_pipeline);
        pass.set_bind_group(0, &target.composite, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Renders the scene alone into a `width × height` image (transparent
    /// where nothing was drawn) and reads it back.
    pub fn render_scene_to_image(
        &mut self,
        ctx: &GpuContext,
        width: u32,
        height: u32,
    ) -> Result<RgbaImage, GpuError> {
        let mut encoder = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("backbox scene image"),
            });
        self.encode_scene(&mut encoder, (width, height))?;
        let Some(target) = &self.target else {
            return Err(GpuError::InvalidArgument("no backbox target".into()));
        };
        let pixels = read_rgba8(ctx, encoder, &target.color)?;
        Ok(RgbaImage {
            width,
            height,
            pixels,
        })
    }
}

/// Records an upload of `bytes` to the start of `dst` into `encoder`, ordered
/// against the passes recorded around it (unlike `Queue::write_buffer`, which
/// lands before every command of the next submit). The staging buffer is
/// released by wgpu once the submit that uses it completes.
fn encode_upload(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    bytes: &[u8],
    dst: &wgpu::Buffer,
) {
    let staging = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytes,
        usage: wgpu::BufferUsages::COPY_SRC,
    });
    encoder.copy_buffer_to_buffer(&staging, 0, dst, 0, bytes.len() as u64);
}

/// Records a backglass with its backbox into `viewport` of `view` (a texture
/// of `target_size`): letterbox and the first `behind` translite layers, the
/// backbox scene (rendered at the translite's on-screen size with its own
/// camera) composited over them, then the remaining layers over it.
/// `behind` comes from [`super::manifest_layers_behind`].
#[allow(clippy::too_many_arguments)] // the backglass encode arguments plus the backbox
pub fn encode_backglass_with_backbox(
    encoder: &mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    target_size: (u32, u32),
    viewport: Rect,
    lamps: &[f32],
    backglass: &mut BackglassRenderer,
    behind: usize,
    backbox: &mut BackboxRenderer,
) -> Result<(), GpuError> {
    let n = backglass.layer_count();
    let behind = behind.min(n);
    backglass.encode_layers(encoder, view, target_size, viewport, lamps, 0..behind, true);
    let (aw, ah) = backglass.aspect();
    let art = fit_aspect(viewport, aw, ah);
    if art.w > 0 && art.h > 0 {
        backbox.encode_scene(encoder, (art.w, art.h))?;
        backbox.encode_composite(encoder, view, target_size, art);
    }
    backglass.encode_layers(
        encoder,
        view,
        target_size,
        viewport,
        lamps,
        behind..n,
        false,
    );
    Ok(())
}

/// Renders `desc` with its backbox (see [`encode_backglass_with_backbox`])
/// filling a `width × height` `Rgba8Unorm` image and reads it back.
pub fn render_backglass_with_backbox_to_image(
    ctx: &GpuContext,
    desc: &BackglassDesc,
    behind: usize,
    backbox: &mut BackboxRenderer,
    lamps: &[f32],
    width: u32,
    height: u32,
) -> Result<RgbaImage, GpuError> {
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let max = ctx.device().limits().max_texture_dimension_2d;
    if width == 0 || height == 0 || width > max || height > max {
        return Err(GpuError::InvalidArgument(format!(
            "image size {width}x{height} (each side must be 1..={max})"
        )));
    }
    let mut backglass = BackglassRenderer::new(ctx, format, desc)?;
    let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("backbox composite image"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = ctx
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("backbox composite image"),
        });
    {
        let _clear = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("backbox composite clear"),
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
    encode_backglass_with_backbox(
        &mut encoder,
        &view,
        (width, height),
        Rect::new(0, 0, width, height),
        lamps,
        &mut backglass,
        behind,
        backbox,
    )?;
    let pixels = read_rgba8(ctx, encoder, &texture)?;
    Ok(RgbaImage {
        width,
        height,
        pixels,
    })
}

/// Submits `encoder` after copying the RGBA8 `texture` into a buffer and
/// returns its tightly packed rows (bounded waits).
fn read_rgba8(
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
        label: Some("backbox readback"),
        size: crate::gpu::readback_buffer_size(device, padded, height)?,
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
