//! The menu painted over the running game (REQ-VIEW-006, Subphase 4.4
//! deliverable 1), on the render thread.
//!
//! * [`MenuFrames`] (GPU-free): the latest menu state received from the
//!   main thread ([`MenuShare`](super::MenuShare)) and an egui context;
//!   while the menu is open it lays out one [`MenuView`] frame per call
//!   at 1 pixel per point, centred in
//!   [`menu_panel_rect_px`](pin_render::ui::menu_panel_rect_px). A closed
//!   menu produces nothing.
//! * [`MenuOverlay`]: [`MenuFrames`] plus `pin_render`'s
//!   [`OverlayPainter`] for one target format, in the style of the
//!   profiler HUD (`orchestrator::hud`): it paints over the frame already
//!   recorded into the encoder, loading (not clearing) it, after the table
//!   and before the present. A closed menu records no pass at all, so
//!   frames are unchanged.
//! * [`MenuDebugHooks`]: the overlay in the `SurfaceSet` debug view (no
//!   table loaded), wrapping the profiler's debug-view hooks.
//!
//! Pointer input is not forwarded to egui: the menu is driven by the
//! keyboard and controller through [`MenuInput`](super::MenuInput) on the
//! main thread, and the render thread only shows the state it receives.

use super::menu::MenuState;
use super::view::MenuView;
use pin_render::gpu::{SurfaceFrameHooks, wgpu};
use pin_render::topology::SurfaceRole;
use pin_render::ui::{OverlayFrame, OverlayPainter, egui, overlay_style, raw_input};

/// Pixels per point the overlay is laid out at (the render thread does not
/// know the display's content scale; 1 matches the HUD).
pub const MENU_PIXELS_PER_POINT: f32 = 1.0;

/// The GPU-free half of the overlay: the shown state and the egui context.
pub struct MenuFrames {
    ctx: egui::Context,
    view: MenuView,
    state: Option<MenuState>,
    frames: u64,
}

impl std::fmt::Debug for MenuFrames {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MenuFrames")
            .field("open", &self.is_open())
            .field("frames", &self.frames)
            .finish_non_exhaustive()
    }
}

impl Default for MenuFrames {
    fn default() -> Self {
        MenuFrames::new()
    }
}

impl MenuFrames {
    /// No state yet (nothing is shown).
    #[must_use]
    pub fn new() -> MenuFrames {
        let ctx = egui::Context::default();
        overlay_style(&ctx);
        MenuFrames {
            ctx,
            view: MenuView::new(),
            state: None,
            frames: 0,
        }
    }

    /// Shows `state` from the next frame on.
    pub fn set_state(&mut self, state: MenuState) {
        self.state = Some(state);
    }

    /// The shown state.
    #[must_use]
    pub fn state(&self) -> Option<&MenuState> {
        self.state.as_ref()
    }

    /// Whether the shown state is an open menu.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.state.as_ref().is_some_and(MenuState::is_open)
    }

    /// Frames laid out so far.
    #[must_use]
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// One tessellated menu frame for a target of `size_px`, or `None` when
    /// the menu is closed (or the target is empty). The inputs the widgets
    /// produce are dropped: the main thread owns the menu (no pointer input
    /// reaches this context).
    pub fn frame(&mut self, size_px: [u32; 2]) -> Option<OverlayFrame> {
        if !self.is_open() || size_px[0] == 0 || size_px[1] == 0 {
            return None;
        }
        let state = self.state.as_mut()?;
        let raw = raw_input(size_px, MENU_PIXELS_PER_POINT, Vec::new(), None);
        let (out, _inputs) = self.view.run_frame(&self.ctx, raw, state);
        self.frames += 1;
        Some(OverlayFrame::new(&self.ctx, out))
    }
}

/// The menu painted over a playfield target of one format.
pub struct MenuOverlay {
    frames: MenuFrames,
    painter: OverlayPainter,
    device: wgpu::Device,
    paints: u64,
}

impl std::fmt::Debug for MenuOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MenuOverlay")
            .field("frames", &self.frames)
            .field("format", &self.painter.format())
            .field("paints", &self.paints)
            .finish_non_exhaustive()
    }
}

impl MenuOverlay {
    /// An overlay for targets of `format` (check it with
    /// [`hud_format_supported`](crate::orchestrator::hud_format_supported)
    /// first), showing nothing until a state arrives.
    #[must_use]
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> MenuOverlay {
        MenuOverlay {
            frames: MenuFrames::new(),
            painter: OverlayPainter::new(device, format),
            device: device.clone(),
            paints: 0,
        }
    }

    /// The target format.
    #[must_use]
    pub fn format(&self) -> wgpu::TextureFormat {
        self.painter.format()
    }

    /// Shows `state` from the next paint on.
    pub fn set_state(&mut self, state: MenuState) {
        self.frames.set_state(state);
    }

    /// Whether the shown state is an open menu.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.frames.is_open()
    }

    /// Frames painted so far.
    #[must_use]
    pub fn paints(&self) -> u64 {
        self.paints
    }

    /// Records the menu over `target` (`size` pixels, this overlay's
    /// format) into `encoder` when the menu is open; returns whether it
    /// painted. A closed menu records nothing. Submit `encoder` before the
    /// next paint.
    pub fn paint(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: (u32, u32),
    ) -> bool {
        let Some(frame) = self.frames.frame([size.0, size.1]) else {
            return false;
        };
        let extra = self.painter.paint(
            &self.device,
            queue,
            encoder,
            target,
            [size.0, size.1],
            frame,
        );
        // The menu uses no egui paint callbacks.
        debug_assert!(extra.is_empty(), "menu paint produced callback commands");
        self.paints += 1;
        true
    }
}

/// The menu overlay for a playfield drawn in `format` on `ctx`'s device:
/// `None`, with one stderr note, when there is no playfield (`None`) or the
/// egui pipeline cannot draw onto that format
/// ([`hud_format_supported`](crate::orchestrator::hud_format_supported),
/// as for the profiler HUD). The game then runs without a visible menu (it
/// still opens and consumes inputs).
#[must_use]
pub fn menu_overlay_for(
    ctx: &pin_render::gpu::GpuContext,
    format: Option<wgpu::TextureFormat>,
) -> Option<MenuOverlay> {
    match format {
        Some(f)
            if crate::orchestrator::hud_format_supported(
                f,
                ctx.adapter().get_texture_format_features(f),
            ) =>
        {
            Some(MenuOverlay::new(ctx.device(), f))
        }
        Some(f) => {
            eprintln!(
                "pin_app: menu: the playfield format {f:?} cannot take the overlay; the menu is \
                 not drawn"
            );
            None
        }
        None => {
            eprintln!("pin_app: menu: no playfield surface; the menu is not drawn");
            None
        }
    }
}

/// The debug view's frame hooks with the menu painted last over the
/// playfield: every call goes to `inner` (the profiler's
/// `DebugViewHooks`, when there are any), then the open menu is painted.
pub struct MenuDebugHooks<'a> {
    inner: Option<&'a mut dyn SurfaceFrameHooks>,
    menu: &'a mut MenuOverlay,
}

impl<'a> MenuDebugHooks<'a> {
    /// `menu` over the frames of `inner` (or of the plain debug view).
    pub fn new(
        inner: Option<&'a mut dyn SurfaceFrameHooks>,
        menu: &'a mut MenuOverlay,
    ) -> MenuDebugHooks<'a> {
        MenuDebugHooks { inner, menu }
    }
}

impl SurfaceFrameHooks for MenuDebugHooks<'_> {
    fn begin_frame(&mut self) {
        if let Some(h) = self.inner.as_deref_mut() {
            h.begin_frame();
        }
    }

    fn wrap_submit(
        &mut self,
        frame: wgpu::CommandBuffer,
    ) -> arrayvec::ArrayVec<wgpu::CommandBuffer, 3> {
        match self.inner.as_deref_mut() {
            Some(h) => h.wrap_submit(frame),
            None => {
                let mut one = arrayvec::ArrayVec::new();
                one.push(frame);
                one
            }
        }
    }

    fn overlay(
        &mut self,
        role: SurfaceRole,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        size_px: (u32, u32),
    ) {
        if let Some(h) = self.inner.as_deref_mut() {
            h.overlay(role, queue, encoder, view, size_px);
        }
        if role == SurfaceRole::Playfield {
            self.menu.paint(queue, encoder, view, size_px);
        }
    }

    fn end_frame(&mut self, submit: &mut dyn FnMut(wgpu::CommandBuffer)) {
        if let Some(h) = self.inner.as_deref_mut() {
            h.end_frame(submit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{AppConfig, MenuInput};

    #[test]
    fn closed_or_empty_menu_lays_out_nothing() {
        let mut f = MenuFrames::default();
        assert!(f.state().is_none() && !f.is_open());
        assert!(f.frame([640, 480]).is_none());
        f.set_state(MenuState::new(AppConfig::default()));
        assert!(f.frame([640, 480]).is_none());
        let mut open = MenuState::default();
        open.apply(MenuInput::Toggle);
        f.set_state(open);
        assert!(f.is_open());
        assert!(f.frame([0, 480]).is_none());
        assert_eq!(f.frames(), 0);
        assert!(format!("{f:?}").contains("MenuFrames"));
    }

    #[test]
    fn open_menu_lays_out_inside_the_panel() {
        let mut f = MenuFrames::new();
        let mut open = MenuState::default();
        open.apply(MenuInput::Toggle);
        f.set_state(open);
        let size = [800u32, 600];
        let [x0, y0, x1, y1] = pin_render::ui::menu_panel_rect_px(size, MENU_PIXELS_PER_POINT);
        for _ in 0..2 {
            let frame = f.frame(size).expect("an open menu draws");
            assert!(!frame.is_empty());
            for p in &frame.primitives {
                let r = p.clip_rect;
                assert!(
                    r.min.x >= x0 as f32 - 0.5
                        && r.min.y >= y0 as f32 - 0.5
                        && r.max.x <= x1 as f32 + 0.5
                        && r.max.y <= y1 as f32 + 0.5
                        || r.width() <= 0.0
                        || r.height() <= 0.0
                        || primitive_inside(p, [x0, y0, x1, y1]),
                    "primitive outside the panel: {r:?}"
                );
            }
        }
        assert_eq!(f.frames(), 2);
    }

    /// Whether every vertex of a mesh primitive lies inside `rect`.
    fn primitive_inside(p: &egui::ClippedPrimitive, rect: [u32; 4]) -> bool {
        match &p.primitive {
            egui::epaint::Primitive::Mesh(m) => m.vertices.iter().all(|v| {
                v.pos.x >= rect[0] as f32 - 0.5
                    && v.pos.y >= rect[1] as f32 - 0.5
                    && v.pos.x <= rect[2] as f32 + 0.5
                    && v.pos.y <= rect[3] as f32 + 0.5
            }),
            egui::epaint::Primitive::Callback(_) => false,
        }
    }
}
