//! Display topology: which windows exist, where, and which viewports each
//! window shows (PLAN §4 Subphase 1.3).
//!
//! Pure geometry: no SDL, no GPU. `pin_app::platform` turns SDL's display list
//! into [`DisplayInfo`]s, calls [`plan_topology`], and creates the windows the
//! resulting [`TopologyPlan`] describes.
//!
//! * **Single-monitor mode:** one borderless fullscreen window on the chosen
//!   display. The playfield fills the window; backglass and score display are
//!   picture-in-picture (PiP) rectangles anchored to a corner.
//! * **Cabinet mode:** one borderless fullscreen window per role that has its
//!   own display (playfield, backglass, score; up to three). A role without a
//!   display becomes a PiP on the playfield window ([`DEFAULT_BACKGLASS_PIP`],
//!   [`DEFAULT_SCORE_PIP`]).
//!
//! Display numbers in [`TopologyMode`] refer to [`DisplayInfo::index`].

use arrayvec::ArrayVec;
use core::fmt;

mod window_event;

pub use window_event::{WindowEvent, WindowEventKind};

/// An axis-aligned rectangle in pixels. `x`/`y` is the top-left corner.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub w: u32,
    /// Height.
    pub h: u32,
}

impl Rect {
    /// A rectangle from its top-left corner and size.
    pub const fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    /// Right edge (exclusive), widened so it cannot overflow.
    pub const fn right(&self) -> i64 {
        self.x as i64 + self.w as i64
    }

    /// Bottom edge (exclusive), widened so it cannot overflow.
    pub const fn bottom(&self) -> i64 {
        self.y as i64 + self.h as i64
    }

    /// `true` if the two rectangles share a region of positive area
    /// (touching edges do not count).
    pub const fn overlaps(&self, other: &Rect) -> bool {
        (self.x as i64) < other.right()
            && (other.x as i64) < self.right()
            && (self.y as i64) < other.bottom()
            && (other.y as i64) < self.bottom()
    }

    /// `true` if `inner` lies entirely inside `self`.
    pub const fn contains(&self, inner: &Rect) -> bool {
        inner.x >= self.x
            && inner.y >= self.y
            && inner.right() <= self.right()
            && inner.bottom() <= self.bottom()
    }
}

/// One physical display as reported by the platform layer.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct DisplayInfo {
    /// Display number used by [`TopologyMode`].
    pub index: usize,
    /// Desktop-coordinate bounds of the display.
    pub bounds: Rect,
    /// Refresh rate in Hz (0 if unknown).
    pub refresh_hz: f32,
    /// Content (DPI) scale, 1.0 = 96 dpi-equivalent.
    pub content_scale: f32,
}

/// What a viewport shows.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SurfaceRole {
    /// The 3D playfield.
    Playfield,
    /// The backglass art (4:3 when shown as a PiP).
    Backglass,
    /// The score display / DMD (4:1 when shown as a PiP).
    Score,
}

impl SurfaceRole {
    /// PiP aspect ratio as `(width, height)`; `None` for the playfield, which
    /// is never a PiP.
    pub const fn pip_aspect(self) -> Option<(u32, u32)> {
        match self {
            SurfaceRole::Playfield => None,
            SurfaceRole::Backglass => Some((4, 3)),
            SurfaceRole::Score => Some((4, 1)),
        }
    }
}

/// Window corner a PiP is anchored to.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Corner {
    /// Top-left.
    TopLeft,
    /// Top-right.
    TopRight,
    /// Bottom-left.
    BottomLeft,
    /// Bottom-right.
    BottomRight,
}

/// Where and how large a PiP is.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PipPlacement {
    /// Anchor corner (margin 0).
    pub corner: Corner,
    /// PiP width as a fraction of the window width, in `(0, 1]`.
    pub width_fraction: f32,
}

/// Default backglass PiP when a role has no display of its own.
pub const DEFAULT_BACKGLASS_PIP: PipPlacement = PipPlacement {
    corner: Corner::TopRight,
    width_fraction: 0.25,
};

/// Default score PiP when a role has no display of its own.
pub const DEFAULT_SCORE_PIP: PipPlacement = PipPlacement {
    corner: Corner::BottomRight,
    width_fraction: 0.25,
};

/// How surfaces map onto displays.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum TopologyMode {
    /// One window; backglass and score are PiPs on the playfield.
    SingleMonitor {
        /// Display the window covers.
        display: usize,
        /// Backglass PiP.
        backglass: PipPlacement,
        /// Score PiP.
        score: PipPlacement,
    },
    /// One window per role with its own display.
    Cabinet {
        /// Playfield display.
        playfield: usize,
        /// Backglass display, or `None` for a PiP on the playfield.
        backglass: Option<usize>,
        /// Score display, or `None` for a PiP on the playfield.
        score: Option<usize>,
    },
}

/// One window to create.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowPlan {
    /// [`DisplayInfo::index`] of the display the window covers.
    pub display: usize,
    /// Window bounds in desktop coordinates (the display's bounds).
    pub bounds: Rect,
    /// Borderless fullscreen (desktop mode, no mode switch).
    pub borderless_fullscreen: bool,
    /// Viewports in window-local coordinates. The first entry is the window's
    /// own role covering the full window; PiPs follow.
    pub viewports: ArrayVec<(SurfaceRole, Rect), 3>,
}

impl WindowPlan {
    /// The role that fills this window.
    pub fn primary_role(&self) -> Option<SurfaceRole> {
        self.viewports.first().map(|(r, _)| *r)
    }

    /// The viewport of `role` in this window, if any.
    pub fn viewport(&self, role: SurfaceRole) -> Option<Rect> {
        self.viewports
            .iter()
            .find(|(r, _)| *r == role)
            .map(|(_, rect)| *rect)
    }
}

/// The windows to create. The playfield window is always first.
#[derive(Clone, Debug, PartialEq)]
pub struct TopologyPlan {
    /// Windows, playfield first.
    pub windows: ArrayVec<WindowPlan, 3>,
}

impl TopologyPlan {
    /// The window and viewport that show `role`.
    pub fn find(&self, role: SurfaceRole) -> Option<(usize, Rect)> {
        self.windows
            .iter()
            .enumerate()
            .find_map(|(i, w)| w.viewport(role).map(|r| (i, r)))
    }
}

/// Why a topology cannot be planned.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TopologyError {
    /// The display list is empty.
    NoDisplays,
    /// No display has this index.
    DisplayOutOfRange(usize),
    /// Two roles were assigned the same display.
    DuplicateDisplay(usize),
    /// A PiP for this role is invalid: width fraction not finite or outside
    /// `(0, 1]`, a zero-sized result, not fitting inside the window, or (for
    /// [`SurfaceRole::Score`]) overlapping the backglass PiP.
    InvalidPip(SurfaceRole),
}

impl fmt::Display for TopologyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TopologyError::NoDisplays => write!(f, "no displays available"),
            TopologyError::DisplayOutOfRange(i) => write!(f, "display {i} does not exist"),
            TopologyError::DuplicateDisplay(i) => {
                write!(f, "display {i} is assigned to more than one role")
            }
            TopologyError::InvalidPip(role) => write!(f, "invalid {role:?} picture-in-picture"),
        }
    }
}

impl std::error::Error for TopologyError {}

/// The PiP rectangle for `role` inside a `window_w × window_h` window, in
/// window-local coordinates.
///
/// Width is `floor(width_fraction × window_w)`, height follows the role's
/// aspect (backglass 4:3, score 4:1) rounded down; the rectangle is anchored to
/// the corner with no margin.
pub fn pip_rect(
    role: SurfaceRole,
    placement: &PipPlacement,
    window_w: u32,
    window_h: u32,
) -> Result<Rect, TopologyError> {
    let frac = placement.width_fraction;
    let Some((aw, ah)) = role.pip_aspect() else {
        return Err(TopologyError::InvalidPip(role));
    };
    if !frac.is_finite() || frac <= 0.0 || frac > 1.0 {
        return Err(TopologyError::InvalidPip(role));
    }
    // f64 keeps `frac × window_w` exact for every u32 width and f32 fraction.
    let w = (f64::from(frac) * f64::from(window_w)).floor() as u32;
    let h = (u64::from(w) * u64::from(ah) / u64::from(aw)) as u32;
    if w == 0 || h == 0 || h > window_h {
        return Err(TopologyError::InvalidPip(role));
    }
    let right = (window_w - w) as i32;
    let bottom = (window_h - h) as i32;
    let (x, y) = match placement.corner {
        Corner::TopLeft => (0, 0),
        Corner::TopRight => (right, 0),
        Corner::BottomLeft => (0, bottom),
        Corner::BottomRight => (right, bottom),
    };
    Ok(Rect { x, y, w, h })
}

fn find_display(displays: &[DisplayInfo], index: usize) -> Result<&DisplayInfo, TopologyError> {
    displays
        .iter()
        .find(|d| d.index == index)
        .ok_or(TopologyError::DisplayOutOfRange(index))
}

fn full_window(bounds: &Rect) -> Rect {
    Rect::new(0, 0, bounds.w, bounds.h)
}

/// The playfield window with optional backglass/score PiPs.
fn playfield_window(
    display: &DisplayInfo,
    backglass: Option<&PipPlacement>,
    score: Option<&PipPlacement>,
) -> Result<WindowPlan, TopologyError> {
    let b = display.bounds;
    let mut viewports = ArrayVec::new();
    viewports.push((SurfaceRole::Playfield, full_window(&b)));
    let bg = backglass
        .map(|p| pip_rect(SurfaceRole::Backglass, p, b.w, b.h))
        .transpose()?;
    let sc = score
        .map(|p| pip_rect(SurfaceRole::Score, p, b.w, b.h))
        .transpose()?;
    if let (Some(bg), Some(sc)) = (bg, sc)
        && bg.overlaps(&sc)
    {
        return Err(TopologyError::InvalidPip(SurfaceRole::Score));
    }
    if let Some(bg) = bg {
        viewports.push((SurfaceRole::Backglass, bg));
    }
    if let Some(sc) = sc {
        viewports.push((SurfaceRole::Score, sc));
    }
    Ok(WindowPlan {
        display: display.index,
        bounds: b,
        borderless_fullscreen: true,
        viewports,
    })
}

fn role_window(display: &DisplayInfo, role: SurfaceRole) -> WindowPlan {
    let mut viewports = ArrayVec::new();
    viewports.push((role, full_window(&display.bounds)));
    WindowPlan {
        display: display.index,
        bounds: display.bounds,
        borderless_fullscreen: true,
        viewports,
    }
}

/// Plans the windows and viewports for `mode` on `displays`.
///
/// Checks run in this order: empty display list, unknown display numbers
/// (playfield, backglass, score), duplicate displays, PiP geometry (backglass,
/// score, overlap).
pub fn plan_topology(
    displays: &[DisplayInfo],
    mode: &TopologyMode,
) -> Result<TopologyPlan, TopologyError> {
    if displays.is_empty() {
        return Err(TopologyError::NoDisplays);
    }
    let mut windows = ArrayVec::new();
    match *mode {
        TopologyMode::SingleMonitor {
            display,
            ref backglass,
            ref score,
        } => {
            let d = find_display(displays, display)?;
            windows.push(playfield_window(d, Some(backglass), Some(score))?);
        }
        TopologyMode::Cabinet {
            playfield,
            backglass,
            score,
        } => {
            let pf = find_display(displays, playfield)?;
            let bg = backglass.map(|i| find_display(displays, i)).transpose()?;
            let sc = score.map(|i| find_display(displays, i)).transpose()?;
            if backglass == Some(playfield) || score == Some(playfield) {
                return Err(TopologyError::DuplicateDisplay(playfield));
            }
            if let (Some(b), Some(s)) = (backglass, score)
                && b == s
            {
                return Err(TopologyError::DuplicateDisplay(b));
            }
            windows.push(playfield_window(
                pf,
                bg.is_none().then_some(&DEFAULT_BACKGLASS_PIP),
                sc.is_none().then_some(&DEFAULT_SCORE_PIP),
            )?);
            if let Some(bg) = bg {
                windows.push(role_window(bg, SurfaceRole::Backglass));
            }
            if let Some(sc) = sc {
                windows.push(role_window(sc, SurfaceRole::Score));
            }
        }
    }
    Ok(TopologyPlan { windows })
}

#[cfg(test)]
mod tests;
