//! The presenter thread of a backglass / score surface (PR #21 round 4,
//! D19: "a separate presenter if only FIFO is available").
//!
//! The render thread draws the surface's table frame into an offscreen
//! texture ([`offscreen_frame_texture`], same format as the surface) and
//! posts it through the latest-frame [`Handoff`]. The presenter thread owns
//! the `wgpu::Surface`; it reconfigures on a size change, acquires the
//! swap-chain image (the only call that may block on vertical blank),
//! copies the frame into it ([`FrameCopy`]: `copy_texture_to_texture` when
//! the surface allows `COPY_DST`, otherwise a fullscreen-triangle blit) and
//! presents. Submissions and configurations on both threads are serialised
//! by one `gpu_lock`, taken only around `queue.submit` and
//! `Surface::configure`, never around an acquire or a present.

use super::handoff::{Handoff, Outcome};
use pin_render::gpu::wgpu;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// Locks the submit / configure lock (it guards no data, so a poisoned
/// lock is still usable).
pub(crate) fn lock_gpu(m: &Mutex<()>) -> MutexGuard<'_, ()> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Polls a future that wgpu resolves synchronously on native backends
/// (error scopes), yielding between polls; `None` if it never resolves.
fn resolve<F: core::future::Future>(f: F) -> Option<F::Output> {
    let mut f = core::pin::pin!(f);
    let mut cx = core::task::Context::from_waker(core::task::Waker::noop());
    for _ in 0..10_000 {
        if let core::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return Some(v);
        }
        std::thread::yield_now();
    }
    None
}

/// Configure attempts per [`configure_surface`] call.
const CONFIGURE_ATTEMPTS: u32 = 4;

/// Longest device wait between configure attempts.
const CONFIGURE_WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Configures `surface` under `gpu_lock`, capturing validation errors with
/// an error scope instead of letting wgpu's default handler panic.
/// `Surface::configure` reports an error when the queue is still busy after
/// its idle wait (possible with another thread submitting on the device),
/// so a failure is retried a few times after a bounded device wait.
/// `false` when every attempt failed.
pub(crate) fn configure_surface(
    device: &wgpu::Device,
    surface: &wgpu::Surface<'_>,
    config: &wgpu::SurfaceConfiguration,
    gpu_lock: &Mutex<()>,
) -> bool {
    let _g = lock_gpu(gpu_lock);
    for attempt in 0..CONFIGURE_ATTEMPTS {
        if attempt > 0 {
            let _ = pin_render::gpu::wait_for_device(device, CONFIGURE_WAIT_TIMEOUT);
        }
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        surface.configure(device, config);
        if matches!(resolve(scope.pop()), Some(None)) {
            return true;
        }
    }
    false
}

/// How the presenter thread copies a frame into the swap-chain image.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CopyPath {
    /// `copy_texture_to_texture` (the surface allows `COPY_DST`; formats
    /// and extents are equal).
    CopyDst,
    /// A fullscreen-triangle render pass sampling the frame (the surface
    /// lacks `COPY_DST`).
    Blit,
}

impl CopyPath {
    /// The path for a surface whose capabilities allow `usages`.
    pub fn for_usages(usages: wgpu::TextureUsages) -> CopyPath {
        if usages.contains(wgpu::TextureUsages::COPY_DST) {
            CopyPath::CopyDst
        } else {
            CopyPath::Blit
        }
    }

    /// The usages to configure the surface (or a stand-in target) with.
    pub fn target_usages(self) -> wgpu::TextureUsages {
        match self {
            CopyPath::CopyDst => {
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_DST
            }
            CopyPath::Blit => wgpu::TextureUsages::RENDER_ATTACHMENT,
        }
    }
}

/// An offscreen frame texture of the hand-off pool: `format`, `size`
/// pixels, usable as render target, copy source and sampled texture.
pub fn offscreen_frame_texture(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    (width, height): (u32, u32),
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("presenter frame"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

const BLIT_SHADER: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VsOut {
    // One triangle covering the target; uv (0, 0) is the top-left texel.
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: VsOut;
    out.pos = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    out.uv = uv;
    return out;
}

@fragment
fn fs(v: VsOut) -> @location(0) vec4<f32> {
    return textureSample(src, samp, v.uv);
}
"#;

struct Blit {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

/// Copies a frame texture into a swap-chain image (or any target texture
/// of the same format) along a [`CopyPath`].
pub struct FrameCopy {
    path: CopyPath,
    blit: Option<Blit>,
}

impl FrameCopy {
    /// A copier into targets of `format` along `path` (the blit pipeline
    /// is built only for [`CopyPath::Blit`]).
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, path: CopyPath) -> FrameCopy {
        let blit = (path == CopyPath::Blit).then(|| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("presenter blit"),
                source: wgpu::ShaderSource::Wgsl(BLIT_SHADER.into()),
            });
            let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("presenter blit"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
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
                ],
            });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("presenter blit"),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("presenter blit"),
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
            // Nearest: equal sizes copy texel for texel.
            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("presenter blit"),
                mag_filter: wgpu::FilterMode::Nearest,
                min_filter: wgpu::FilterMode::Nearest,
                ..Default::default()
            });
            Blit {
                pipeline,
                layout,
                sampler,
            }
        });
        FrameCopy { path, blit }
    }

    /// The copy path.
    pub fn path(&self) -> CopyPath {
        self.path
    }

    /// Records the copy of `src` into `dst` (the overlapping extent for
    /// [`CopyPath::CopyDst`]; the whole of `dst` for [`CopyPath::Blit`]).
    pub fn encode(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::Texture,
        dst: &wgpu::Texture,
    ) {
        match &self.blit {
            None => {
                let extent = wgpu::Extent3d {
                    width: src.width().min(dst.width()),
                    height: src.height().min(dst.height()),
                    depth_or_array_layers: 1,
                };
                encoder.copy_texture_to_texture(src.as_image_copy(), dst.as_image_copy(), extent);
            }
            Some(b) => {
                let src_view = src.create_view(&wgpu::TextureViewDescriptor::default());
                let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());
                let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("presenter blit"),
                    layout: &b.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&src_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&b.sampler),
                        },
                    ],
                });
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("presenter blit"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &dst_view,
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
                pass.set_pipeline(&b.pipeline);
                pass.set_bind_group(0, &bind, &[]);
                pass.draw(0..3, 0..1);
            }
        }
    }
}

/// What the presenter thread owns besides the hand-off.
pub(crate) struct PresenterTarget {
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    /// Owned by the thread; dropped when [`run_presenter`] returns, before
    /// the window guard (`spawn_guarded`).
    pub(crate) surface: wgpu::Surface<'static>,
    /// Already applied once on the render thread before the hand-over.
    pub(crate) config: wgpu::SurfaceConfiguration,
    pub(crate) path: CopyPath,
    pub(crate) gpu_lock: std::sync::Arc<Mutex<()>>,
}

/// Whether a frame of `frame_size` still matches the published window size
/// (`current`) and may be presented.
pub(crate) fn frame_still_current(frame_size: (u32, u32), current: (u32, u32)) -> bool {
    frame_size == current
}

/// The presenter thread body: until the hand-off closes, takes the latest
/// frame, reconfigures on a size change (or after a failed configure),
/// acquires, copies, submits and presents, then returns the slot.
pub(crate) fn run_presenter(h: &Handoff<wgpu::Texture>, target: PresenterTarget) {
    let PresenterTarget {
        device,
        queue,
        surface,
        mut config,
        path,
        gpu_lock,
    } = target;
    let copy = FrameCopy::new(&device, config.format, path);
    let mut configured = true;
    while let Some(frame) = h.wait_take() {
        let Some(src) = frame.payload().cloned() else {
            h.release(frame, Outcome::Skipped);
            continue;
        };
        let size = frame.size();
        if (config.width, config.height) != size {
            config.width = size.0.max(1);
            config.height = size.1.max(1);
            configured = false;
        }
        if !configured {
            configured = configure_surface(&device, &surface, &config, &gpu_lock);
            if !configured {
                h.record_configure_failure();
                h.release(frame, Outcome::Skipped);
                continue;
            }
        }
        if !frame_still_current(size, h.size()) {
            // Resized while this frame was in flight: never present it.
            h.release(frame, Outcome::Discarded);
            continue;
        }
        let (image, suboptimal) = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => (t, false),
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => (t, true),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                configured = false;
                h.release(frame, Outcome::Skipped);
                continue;
            }
            _ => {
                // Occluded, timeout, other: drop the frame, keep running.
                h.release(frame, Outcome::Skipped);
                continue;
            }
        };
        if !frame_still_current(size, h.size()) {
            // Resized while `get_current_texture` blocked: drop the acquired
            // image without presenting it; the next frame reconfigures.
            drop(image);
            h.release(frame, Outcome::Discarded);
            continue;
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("presenter copy"),
        });
        copy.encode(&device, &mut encoder, &src, &image.texture);
        {
            let _g = lock_gpu(&gpu_lock);
            queue.submit([encoder.finish()]);
        }
        queue.present(image);
        if suboptimal {
            configured = false;
        }
        h.release(frame, Outcome::Presented);
    }
    drop(surface);
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::TextureUsages as U;

    #[test]
    fn a_frame_is_current_only_at_the_published_size() {
        assert!(frame_still_current((640, 480), (640, 480)));
        assert!(!frame_still_current((640, 480), (800, 480)));
        assert!(!frame_still_current((640, 480), (640, 600)));
    }

    #[test]
    fn copy_dst_capable_surfaces_copy_others_blit() {
        assert_eq!(
            CopyPath::for_usages(U::RENDER_ATTACHMENT | U::COPY_DST),
            CopyPath::CopyDst
        );
        assert_eq!(
            CopyPath::for_usages(U::RENDER_ATTACHMENT | U::COPY_DST | U::COPY_SRC),
            CopyPath::CopyDst
        );
        assert_eq!(CopyPath::for_usages(U::RENDER_ATTACHMENT), CopyPath::Blit);
        assert_eq!(
            CopyPath::for_usages(U::RENDER_ATTACHMENT | U::TEXTURE_BINDING),
            CopyPath::Blit
        );
        assert!(CopyPath::CopyDst.target_usages().contains(U::COPY_DST));
        assert_eq!(CopyPath::Blit.target_usages(), U::RENDER_ATTACHMENT);
    }
}
