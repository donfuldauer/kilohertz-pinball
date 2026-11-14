//! Window events forwarded from the main thread to the render thread through
//! a `pin_core::spsc` ring (PLAN §2.2).

/// What happened to a window.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum WindowEventKind {
    /// Logical size changed.
    Resized {
        /// Width in logical pixels.
        w: u32,
        /// Height in logical pixels.
        h: u32,
    },
    /// Drawable (physical pixel) size changed: reconfigure the surface.
    PixelSizeChanged {
        /// Width in physical pixels.
        w: u32,
        /// Height in physical pixels.
        h: u32,
    },
    /// Window moved (desktop coordinates).
    Moved {
        /// Left edge.
        x: i32,
        /// Top edge.
        y: i32,
    },
    /// Window moved to another display (SDL display id).
    DisplayChanged {
        /// SDL display id.
        display_id: u32,
    },
    /// Window became visible.
    Shown,
    /// Window was hidden.
    Hidden,
    /// Window needs a redraw.
    Exposed,
    /// Window was minimised.
    Minimized,
    /// Window was maximised.
    Maximized,
    /// Window was restored.
    Restored,
    /// Window is fully covered.
    Occluded,
    /// Window gained keyboard focus.
    FocusGained,
    /// Window lost keyboard focus.
    FocusLost,
    /// The user asked to close the window.
    CloseRequested,
    /// Window entered fullscreen.
    EnterFullscreen,
    /// Window left fullscreen.
    LeaveFullscreen,
}

/// One window event, timestamped with SDL's nanosecond clock.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct WindowEvent {
    /// SDL event timestamp, nanoseconds.
    pub timestamp_ns: u64,
    /// Index of the window in `TopologyPlan::windows`.
    pub window: usize,
    /// The event.
    pub kind: WindowEventKind,
}
