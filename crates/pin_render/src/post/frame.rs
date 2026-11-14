//! The whole frame: reflections, PBR and the post chain.

use super::{LdrImage, MotionBall, PostChain, PostConfig, PostFrame};
use crate::gpu::{GpuContext, GpuError};
use crate::pbr::{FrameDesc, HdrImage, PbrRenderer};
use crate::reflections::{
    MAX_PROBES, OctahedralProbes, ProbeConfig, ProbeStats, ReflectionSource, ReflectiveBall,
};

/// A ball in a [`FrameRenderer`] frame: its draw (index into
/// [`FrameDesc::draws`]), centre, velocity (world, SI) and radius.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SceneBall {
    /// Index of the ball's draw in [`FrameDesc::draws`].
    pub draw: usize,
    /// Centre (m).
    pub position: [f32; 3],
    /// Velocity (m/s).
    pub velocity: [f32; 3],
    /// Radius (m).
    pub radius: f32,
}

/// What [`FrameRenderer::render`] reads back.
#[derive(Clone, Debug)]
pub struct RenderedFrame {
    /// The display image (after the post chain).
    pub ldr: LdrImage,
    /// The PBR output with reflections, before post (linear).
    pub hdr: HdrImage,
    /// Reflection budget counters after this frame.
    pub probes: ProbeStats,
}

/// PBR + reflections + post for one view.
///
/// Per frame ([`step`](Self::step)): the reflection source prepares
/// (probe captures, each submitted on its own), then one command buffer
/// holds the main PBR pass (MSAA per [`PostConfig::aa`]) and the post chain
/// into the chain's output texture.
pub struct FrameRenderer {
    pbr: PbrRenderer,
    reflections: Box<dyn ReflectionSource>,
    post: PostChain,
    reflective: Vec<ReflectiveBall>,
    motion: Vec<MotionBall>,
}

impl FrameRenderer {
    /// A `width`×`height` renderer with octahedral probes
    /// ([`ProbeConfig::default`]) and `config`; output `Rgba8Unorm`.
    pub fn new(
        ctx: &GpuContext,
        width: u32,
        height: u32,
        config: PostConfig,
    ) -> Result<FrameRenderer, GpuError> {
        let pbr = PbrRenderer::new(ctx, width, height)?;
        let probes = OctahedralProbes::new(ctx, &pbr, ProbeConfig::default())?;
        Self::assemble(ctx, pbr, Box::new(probes), config)
    }

    /// Like [`new`](Self::new) with a reflection source built for the
    /// renderer's PBR pass (`make` gets the PBR renderer to take its
    /// [`reflection_layout`](PbrRenderer::reflection_layout) from).
    pub fn with_reflections(
        ctx: &GpuContext,
        width: u32,
        height: u32,
        config: PostConfig,
        make: impl FnOnce(&PbrRenderer) -> Result<Box<dyn ReflectionSource>, GpuError>,
    ) -> Result<FrameRenderer, GpuError> {
        let pbr = PbrRenderer::new(ctx, width, height)?;
        let source = make(&pbr)?;
        Self::assemble(ctx, pbr, source, config)
    }

    fn assemble(
        ctx: &GpuContext,
        mut pbr: PbrRenderer,
        reflections: Box<dyn ReflectionSource>,
        config: PostConfig,
    ) -> Result<FrameRenderer, GpuError> {
        let (w, h) = pbr.size();
        pbr.set_msaa_samples(config.aa.samples())?;
        let mut post = PostChain::new(ctx, w, h, wgpu::TextureFormat::Rgba8Unorm)?;
        post.set_config(config);
        Ok(FrameRenderer {
            pbr,
            reflections,
            post,
            reflective: Vec::with_capacity(MAX_PROBES),
            motion: Vec::with_capacity(MAX_PROBES),
        })
    }

    /// The PBR renderer (upload meshes with [`pbr_mut`](Self::pbr_mut)).
    pub fn pbr(&self) -> &PbrRenderer {
        &self.pbr
    }

    /// The PBR renderer, mutable.
    pub fn pbr_mut(&mut self) -> &mut PbrRenderer {
        &mut self.pbr
    }

    /// The post chain.
    pub fn post(&self) -> &PostChain {
        &self.post
    }

    /// The post chain, mutable (prefer [`set_config`](Self::set_config),
    /// which also applies the AA mode).
    pub fn post_mut(&mut self) -> &mut PostChain {
        &mut self.post
    }

    /// The reflection source.
    pub fn reflections(&self) -> &dyn ReflectionSource {
        self.reflections.as_ref()
    }

    /// Replaces the post configuration and applies its AA mode.
    pub fn set_config(&mut self, config: PostConfig) -> Result<(), GpuError> {
        self.pbr.set_msaa_samples(config.aa.samples())?;
        self.post.set_config(config);
        Ok(())
    }

    /// Resizes the PBR and post targets.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), GpuError> {
        self.pbr.resize(width, height)?;
        self.post.resize(width, height)
    }

    /// Renders one frame (reflections, PBR, post into
    /// [`PostChain::output_view`]) and submits it, without readback.
    pub fn step(
        &mut self,
        ctx: &GpuContext,
        frame: &FrameDesc<'_>,
        balls: &[SceneBall],
    ) -> Result<ProbeStats, GpuError> {
        self.reflective.clear();
        self.motion.clear();
        for b in balls {
            self.reflective.push(ReflectiveBall {
                position: b.position,
                radius: b.radius,
                draw: b.draw,
            });
            self.motion.push(MotionBall {
                position: b.position,
                velocity: b.velocity,
                radius: b.radius,
            });
        }
        self.reflections
            .prepare(ctx, &mut self.pbr, frame, &self.reflective)?;
        let mut encoder = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        let binding = self.reflections.binding();
        self.pbr
            .encode_reflected(&mut encoder, frame, Some(&binding));
        let post_frame = PostFrame {
            camera: frame.camera,
            balls: &self.motion,
        };
        let out = self.post.output_view().clone();
        self.post
            .encode(&mut encoder, self.pbr.hdr_view(), &post_frame, &out);
        ctx.queue().submit([encoder.finish()]);
        Ok(self.reflections.stats())
    }

    /// [`step`](Self::step), then reads back the display image and the
    /// PBR output.
    pub fn render(
        &mut self,
        ctx: &GpuContext,
        frame: &FrameDesc<'_>,
        balls: &[SceneBall],
    ) -> Result<RenderedFrame, GpuError> {
        let probes = self.step(ctx, frame, balls)?;
        let ldr = self.post.read_ldr(ctx)?;
        let hdr = super::readback::read_hdr_layer(ctx, self.pbr.hdr_texture(), 0)?;
        Ok(RenderedFrame { ldr, hdr, probes })
    }
}
