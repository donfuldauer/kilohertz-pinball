//! [`DisplayTopologyManager`]: SDL display enumeration and window creation for
//! a [`TopologyPlan`] (PLAN §4 Subphase 1.3).

use super::PlatformError;
use pin_render::topology::{
    DisplayInfo, Rect, SurfaceRole, TopologyError, TopologyMode, TopologyPlan, WindowPlan,
    plan_topology,
};
use sdl3::VideoSubsystem;
use sdl3::video::Window;

/// One created window and the plan it was created from.
pub struct PlatformWindow {
    /// Index in `TopologyPlan::windows`.
    pub plan_index: usize,
    /// What the window shows.
    pub plan: WindowPlan,
    /// The SDL window. Surfaces are created from it on the main thread and
    /// handed to the render thread (PLAN §2.2).
    pub window: Window,
}

impl PlatformWindow {
    /// SDL window id (as carried by SDL window events).
    pub fn id(&self) -> u32 {
        self.window.id()
    }
}

/// Window title for a window whose primary role is `role`.
pub fn window_title(role: Option<SurfaceRole>) -> &'static str {
    match role {
        Some(SurfaceRole::Backglass) => "Kilohertz Pinball - Backglass",
        Some(SurfaceRole::Score) => "Kilohertz Pinball - Score",
        Some(SurfaceRole::Playfield) | None => "Kilohertz Pinball",
    }
}

/// Converts an SDL rectangle to a topology rectangle (negative sizes become 0).
fn to_rect(r: sdl3::rect::Rect) -> Rect {
    Rect::new(r.x(), r.y(), r.width(), r.height())
}

/// Queries SDL displays and creates the windows of a [`TopologyPlan`].
#[derive(Clone, Debug, Default)]
pub struct DisplayTopologyManager {
    displays: Vec<DisplayInfo>,
}

impl DisplayTopologyManager {
    /// A manager over an explicit display list (for tests and tools).
    pub fn from_displays(displays: Vec<DisplayInfo>) -> Self {
        Self { displays }
    }

    /// Queries SDL's current displays. `DisplayInfo::index` is the position in
    /// SDL's display list.
    pub fn query(video: &VideoSubsystem) -> Result<Self, PlatformError> {
        let mut m = Self::default();
        m.refresh(video)?;
        Ok(m)
    }

    /// Re-queries the displays (call after `Translated::DisplaysChanged`).
    pub fn refresh(&mut self, video: &VideoSubsystem) -> Result<(), PlatformError> {
        let list = video.displays().map_err(PlatformError::sdl)?;
        let mut displays = Vec::with_capacity(list.len());
        for (index, d) in list.iter().enumerate() {
            let bounds = to_rect(d.get_bounds().map_err(PlatformError::sdl)?);
            let refresh_hz = d.get_mode().map(|m| m.refresh_rate).unwrap_or(0.0);
            let content_scale = d.get_content_scale().unwrap_or(1.0);
            displays.push(DisplayInfo {
                index,
                bounds,
                refresh_hz,
                content_scale,
            });
        }
        self.displays = displays;
        Ok(())
    }

    /// The displays from the last query.
    pub fn displays(&self) -> &[DisplayInfo] {
        &self.displays
    }

    /// Plans `mode` on the current displays.
    pub fn plan(&self, mode: &TopologyMode) -> Result<TopologyPlan, TopologyError> {
        plan_topology(&self.displays, mode)
    }

    /// Creates one SDL window per planned window, positioned on its display,
    /// borderless and (if planned) desktop fullscreen, with high pixel
    /// density so the drawable size is in physical pixels.
    pub fn create_windows(
        video: &VideoSubsystem,
        plan: &TopologyPlan,
    ) -> Result<Vec<PlatformWindow>, PlatformError> {
        let mut out = Vec::with_capacity(plan.windows.len());
        for (plan_index, wp) in plan.windows.iter().enumerate() {
            let mut b = video.window(
                window_title(wp.primary_role()),
                wp.bounds.w.max(1),
                wp.bounds.h.max(1),
            );
            b.position(wp.bounds.x, wp.bounds.y)
                .borderless()
                .high_pixel_density();
            if wp.borderless_fullscreen {
                b.fullscreen();
            }
            let window = b
                .build()
                .map_err(|e| PlatformError::Window(e.to_string()))?;
            out.push(PlatformWindow {
                plan_index,
                plan: wp.clone(),
                window,
            });
        }
        Ok(out)
    }
}
