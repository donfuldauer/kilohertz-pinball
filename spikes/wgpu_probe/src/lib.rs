//! Spike 0.1/Q3, Q4, Q5: wgpu adapter probes.
//!
//! * Q3 — which adapters report `Features::EXPERIMENTAL_RAY_QUERY` (report-only).
//! * Q4 — which present modes each adapter reports for a real surface (report-only).
//! * Q5 — offscreen render + readback on the CI software/OS adapter
//!   (lavapipe on Linux, WARP on Windows, Metal on macOS); asserted in CI.

use std::fmt;
use std::sync::mpsc;

use wgpu::{Adapter, Backends, Instance};

/// Environment variable that turns the Q5 "no adapter" outcome into a failure.
/// CI sets it; local runs without a GPU/ICD then degrade to a report.
pub const REQUIRE_ENV: &str = "SPIKE_REQUIRE_GPU";

/// Instance over `backends`, still honouring `WGPU_*` env overrides.
pub fn instance(backends: Backends) -> Instance {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = backends;
    Instance::new(desc.with_env())
}

/// One line of the Q3 report.
pub struct AdapterReport {
    pub name: String,
    pub backend: wgpu::Backend,
    pub device_type: wgpu::DeviceType,
    pub driver: String,
    pub ray_query: bool,
}

impl AdapterReport {
    pub fn of(adapter: &Adapter) -> Self {
        let info = adapter.get_info();
        Self {
            name: info.name,
            backend: info.backend,
            device_type: info.device_type,
            driver: format!("{} {}", info.driver, info.driver_info)
                .trim()
                .to_owned(),
            ray_query: adapter
                .features()
                .contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY),
        }
    }
}

impl fmt::Display for AdapterReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?} | {:?} | {} | driver: {} | EXPERIMENTAL_RAY_QUERY: {}",
            self.backend,
            self.device_type,
            self.name,
            if self.driver.is_empty() {
                "?"
            } else {
                &self.driver
            },
            if self.ray_query { "yes" } else { "no" }
        )
    }
}

/// Every adapter on every compiled-in backend.
pub fn all_adapters() -> Vec<Adapter> {
    pollster::block_on(instance(Backends::all()).enumerate_adapters(Backends::all()))
}

/// The adapter Q5 targets on this OS, and a substring its name must contain in CI.
pub struct CiTarget {
    pub backends: Backends,
    pub force_fallback: bool,
    pub expected_name: Option<&'static str>,
    pub label: &'static str,
}

pub fn ci_target() -> CiTarget {
    if cfg!(target_os = "windows") {
        CiTarget {
            backends: Backends::DX12,
            force_fallback: true,
            expected_name: Some("Microsoft Basic Render Driver"),
            label: "WARP (DX12)",
        }
    } else if cfg!(target_os = "macos") {
        CiTarget {
            backends: Backends::METAL,
            force_fallback: false,
            expected_name: None,
            label: "Metal",
        }
    } else {
        CiTarget {
            backends: Backends::VULKAN,
            force_fallback: true,
            expected_name: Some("llvmpipe"),
            label: "lavapipe (Vulkan)",
        }
    }
}

/// Request the Q5 adapter for this OS.
pub fn request_ci_adapter(target: &CiTarget) -> Result<Adapter, wgpu::RequestAdapterError> {
    let instance = instance(target.backends);
    pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::None,
        force_fallback_adapter: target.force_fallback,
        compatible_surface: None,
        apply_limit_buckets: false,
    }))
}

pub const SIZE: u32 = 64;
pub const CLEAR: wgpu::Color = wgpu::Color {
    r: 0.0,
    g: 0.0,
    b: 1.0,
    a: 1.0,
};
pub const CLEAR_RGBA8: [u8; 4] = [0, 0, 255, 255];
pub const DRAW_RGBA8: [u8; 4] = [0, 255, 0, 255];

const SHADER: &str = r#"
@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    // Full-screen triangle.
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    return vec4<f32>(p, 0.0, 1.0);
}

@fragment
fn fs() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 1.0, 0.0, 1.0);
}
"#;

/// Offscreen render: clear a `SIZE`² RGBA8 texture to [`CLEAR`], draw a
/// full-screen triangle restricted by viewport to the left half, read back.
/// Returns the tightly packed RGBA8 pixels.
pub fn render_offscreen(adapter: &Adapter) -> Result<Vec<u8>, String> {
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("spike"),
        required_limits: wgpu::Limits::downlevel_defaults(),
        ..Default::default()
    }))
    .map_err(|e| format!("request_device: {e}"))?;

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("target"),
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
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
    let row = SIZE * 4; // 256 bytes: already COPY_BYTES_PER_ROW_ALIGNMENT
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(row * SIZE),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("fullscreen"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("fullscreen"),
        layout: None,
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

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("spike"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("clear+draw"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(CLEAR),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_viewport(0.0, 0.0, (SIZE / 2) as f32, SIZE as f32, 0.0, 1.0);
        pass.draw(0..3, 0..1);
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(SIZE),
            },
        },
        wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);

    let (tx, rx) = mpsc::channel();
    readback.map_async(wgpu::MapMode::Read, .., move |r| {
        let _ = tx.send(r);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| format!("poll: {e}"))?;
    rx.recv()
        .map_err(|e| format!("map channel: {e}"))?
        .map_err(|e| format!("map_async: {e}"))?;
    let data = readback
        .get_mapped_range(..)
        .map_err(|e| format!("get_mapped_range: {e:?}"))?
        .to_vec();
    readback.unmap();
    Ok(data)
}

/// RGBA8 pixel at (x, y) of a tightly packed `SIZE`-wide image.
pub fn pixel(data: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE + x) * 4) as usize;
    [data[i], data[i + 1], data[i + 2], data[i + 3]]
}
