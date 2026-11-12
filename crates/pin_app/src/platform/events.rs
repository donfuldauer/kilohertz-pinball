//! SDL event translation and forwarding into the SPSC rings (PLAN §2.2).
//!
//! [`translate`] is pure (no SDL calls), so the mapping is unit-testable without
//! initialising SDL. [`EventForwarder`] owns the producer ends of the two rings
//! and pushes translated events; it never blocks: when a ring is full the event
//! is dropped and counted in [`ForwardStats`].

use super::keymap;
use arrayvec::ArrayVec;
use pin_core::InputEvent;
use pin_core::spsc::SpscProducer;
use pin_render::topology::{WindowEvent, WindowEventKind};
use sdl3::event::{Event, WindowEvent as SdlWindowEvent};
use sdl3::joystick::JoystickId;

/// The meaning of one SDL event for the engine.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Translated {
    /// A cabinet input event for the sim thread.
    Input(InputEvent),
    /// A window event for the render thread (keyed by SDL window id).
    Window {
        /// SDL event timestamp, nanoseconds.
        timestamp_ns: u64,
        /// SDL window id.
        window_id: u32,
        /// The event.
        kind: WindowEventKind,
    },
    /// The application should quit.
    Quit,
    /// Displays were added, removed or changed: re-query and re-plan.
    DisplaysChanged,
    /// A gamepad was connected; the platform opens it.
    GamepadAdded(JoystickId),
    /// A gamepad was disconnected; the platform closes it.
    GamepadRemoved(JoystickId),
    /// Not relevant to the engine.
    Ignored,
}

fn window_kind(e: SdlWindowEvent) -> Option<WindowEventKind> {
    let size = |w: i32, h: i32| (w.max(0) as u32, h.max(0) as u32);
    Some(match e {
        SdlWindowEvent::Resized(w, h) => {
            let (w, h) = size(w, h);
            WindowEventKind::Resized { w, h }
        }
        SdlWindowEvent::PixelSizeChanged(w, h) => {
            let (w, h) = size(w, h);
            WindowEventKind::PixelSizeChanged { w, h }
        }
        SdlWindowEvent::Moved(x, y) => WindowEventKind::Moved { x, y },
        SdlWindowEvent::DisplayChanged(d) => WindowEventKind::DisplayChanged {
            display_id: d as u32,
        },
        SdlWindowEvent::Shown => WindowEventKind::Shown,
        SdlWindowEvent::Hidden => WindowEventKind::Hidden,
        SdlWindowEvent::Exposed => WindowEventKind::Exposed,
        SdlWindowEvent::Minimized => WindowEventKind::Minimized,
        SdlWindowEvent::Maximized => WindowEventKind::Maximized,
        SdlWindowEvent::Restored => WindowEventKind::Restored,
        SdlWindowEvent::Occluded => WindowEventKind::Occluded,
        SdlWindowEvent::FocusGained => WindowEventKind::FocusGained,
        SdlWindowEvent::FocusLost => WindowEventKind::FocusLost,
        SdlWindowEvent::CloseRequested => WindowEventKind::CloseRequested,
        SdlWindowEvent::EnterFullscreen => WindowEventKind::EnterFullscreen,
        SdlWindowEvent::LeaveFullscreen => WindowEventKind::LeaveFullscreen,
        SdlWindowEvent::None
        | SdlWindowEvent::MouseEnter
        | SdlWindowEvent::MouseLeave
        | SdlWindowEvent::HitTest(..)
        | SdlWindowEvent::ICCProfChanged => return None,
    })
}

/// Translates one SDL event using the default keymap. The SDL nanosecond
/// timestamp is carried over unchanged. Key repeats are ignored.
pub fn translate(event: &Event) -> Translated {
    let ts = event.get_timestamp();
    match *event {
        Event::Quit { .. } | Event::AppTerminating { .. } => Translated::Quit,
        Event::KeyDown {
            scancode: Some(sc),
            repeat: false,
            ..
        } => keymap::key_input(sc)
            .map_or(Translated::Ignored, |i| {
                Translated::Input(InputEvent::button(ts, i, true))
            }),
        Event::KeyUp {
            scancode: Some(sc),
            repeat: false,
            ..
        } => keymap::key_input(sc)
            .map_or(Translated::Ignored, |i| {
                Translated::Input(InputEvent::button(ts, i, false))
            }),
        Event::GamepadButtonDown { button, .. } => keymap::gamepad_button_input(button)
            .map_or(Translated::Ignored, |i| {
                Translated::Input(InputEvent::button(ts, i, true))
            }),
        Event::GamepadButtonUp { button, .. } => keymap::gamepad_button_input(button)
            .map_or(Translated::Ignored, |i| {
                Translated::Input(InputEvent::button(ts, i, false))
            }),
        Event::GamepadAxisMotion { axis, value, .. } => keymap::gamepad_axis_input(axis, value)
            .map_or(Translated::Ignored, |(a, v)| {
                Translated::Input(InputEvent::axis(ts, a, v))
            }),
        Event::GamepadAdded { which, .. } => Translated::GamepadAdded(which),
        Event::GamepadRemoved { which, .. } => Translated::GamepadRemoved(which),
        Event::Window {
            window_id,
            win_event,
            ..
        } => window_kind(win_event).map_or(Translated::Ignored, |kind| Translated::Window {
            timestamp_ns: ts,
            window_id,
            kind,
        }),
        Event::Display { .. } => Translated::DisplaysChanged,
        _ => Translated::Ignored,
    }
}

/// Counters kept by [`EventForwarder`].
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct ForwardStats {
    /// Input events pushed into the input ring.
    pub input_forwarded: u64,
    /// Input events dropped because the input ring was full.
    pub input_dropped: u64,
    /// Window events pushed into the window ring.
    pub window_forwarded: u64,
    /// Window events dropped because the window ring was full.
    pub window_dropped: u64,
    /// Window events for windows that are not registered.
    pub window_unknown: u64,
}

/// Pushes translated events into the input ring (→ sim) and the window ring
/// (→ render).
pub struct EventForwarder {
    input: SpscProducer<InputEvent>,
    window: SpscProducer<WindowEvent>,
    /// SDL window id of each `TopologyPlan` window, by plan index.
    window_ids: ArrayVec<u32, 3>,
    stats: ForwardStats,
}

impl EventForwarder {
    /// A forwarder writing into the given ring producers.
    pub fn new(input: SpscProducer<InputEvent>, window: SpscProducer<WindowEvent>) -> Self {
        Self {
            input,
            window,
            window_ids: ArrayVec::new(),
            stats: ForwardStats::default(),
        }
    }

    /// Sets the SDL window ids of the plan's windows, in plan order; window
    /// events carry the plan index. At most three ids are kept.
    pub fn register_windows(&mut self, sdl_window_ids: &[u32]) {
        self.window_ids.clear();
        for id in sdl_window_ids.iter().take(3) {
            self.window_ids.push(*id);
        }
    }

    /// Counters so far.
    pub fn stats(&self) -> ForwardStats {
        self.stats
    }

    /// Translates `event` and pushes it into the matching ring. Returns the
    /// translation so the caller can act on the remaining kinds (quit,
    /// gamepads, display changes).
    pub fn forward(&mut self, event: &Event) -> Translated {
        let t = translate(event);
        match t {
            Translated::Input(ev) => {
                if self.input.push(ev).is_ok() {
                    self.stats.input_forwarded += 1;
                } else {
                    self.stats.input_dropped += 1;
                }
            }
            Translated::Window {
                timestamp_ns,
                window_id,
                kind,
            } => match self.window_ids.iter().position(|id| *id == window_id) {
                Some(window) => {
                    let ev = WindowEvent {
                        timestamp_ns,
                        window,
                        kind,
                    };
                    if self.window.push(ev).is_ok() {
                        self.stats.window_forwarded += 1;
                    } else {
                        self.stats.window_dropped += 1;
                    }
                }
                None => self.stats.window_unknown += 1,
            },
            _ => {}
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pin_core::spsc::channel;
    use pin_core::{CabinetAxis, CabinetInput};
    use sdl3::gamepad::{Axis, Button};
    use sdl3::keyboard::{Mod, Scancode};

    fn key(down: bool, sc: Scancode, ts: u64, repeat: bool) -> Event {
        if down {
            Event::KeyDown {
                timestamp: ts,
                window_id: 1,
                keycode: None,
                scancode: Some(sc),
                keymod: Mod::NOMOD,
                repeat,
                which: 0,
                raw: 0,
            }
        } else {
            Event::KeyUp {
                timestamp: ts,
                window_id: 1,
                keycode: None,
                scancode: Some(sc),
                keymod: Mod::NOMOD,
                repeat,
                which: 0,
                raw: 0,
            }
        }
    }

    fn win(ts: u64, window_id: u32, e: SdlWindowEvent) -> Event {
        Event::Window {
            timestamp: ts,
            window_id,
            win_event: e,
        }
    }

    #[test]
    fn keys_translate_with_timestamps() {
        assert_eq!(
            translate(&key(true, Scancode::LShift, 123, false)),
            Translated::Input(InputEvent::button(123, CabinetInput::LeftFlipper, true))
        );
        assert_eq!(
            translate(&key(false, Scancode::RCtrl, 456, false)),
            Translated::Input(InputEvent::button(456, CabinetInput::RightUpper, false))
        );
        assert_eq!(
            translate(&key(true, Scancode::LShift, 1, true)),
            Translated::Ignored
        );
        assert_eq!(
            translate(&key(true, Scancode::Q, 1, false)),
            Translated::Ignored
        );
        assert_eq!(
            translate(&key(false, Scancode::Q, 1, false)),
            Translated::Ignored
        );
    }

    #[test]
    fn gamepad_translates() {
        let which = JoystickId::new(7);
        assert_eq!(
            translate(&Event::GamepadButtonDown {
                timestamp: 10,
                which,
                button: Button::RightShoulder
            }),
            Translated::Input(InputEvent::button(10, CabinetInput::RightFlipper, true))
        );
        assert_eq!(
            translate(&Event::GamepadButtonUp {
                timestamp: 11,
                which,
                button: Button::Start
            }),
            Translated::Input(InputEvent::button(11, CabinetInput::Start, false))
        );
        assert_eq!(
            translate(&Event::GamepadButtonUp {
                timestamp: 11,
                which,
                button: Button::North
            }),
            Translated::Ignored
        );
        assert_eq!(
            translate(&Event::GamepadButtonDown {
                timestamp: 11,
                which,
                button: Button::North
            }),
            Translated::Ignored
        );
        assert_eq!(
            translate(&Event::GamepadAxisMotion {
                timestamp: 12,
                which,
                axis: Axis::TriggerLeft,
                value: 32767
            }),
            Translated::Input(InputEvent::axis(12, CabinetAxis::LeftFlipper, 1.0))
        );
        assert_eq!(
            translate(&Event::GamepadAxisMotion {
                timestamp: 12,
                which,
                axis: Axis::LeftX,
                value: 3
            }),
            Translated::Ignored
        );
        assert_eq!(
            translate(&Event::GamepadAdded {
                timestamp: 1,
                which
            }),
            Translated::GamepadAdded(which)
        );
        assert_eq!(
            translate(&Event::GamepadRemoved {
                timestamp: 1,
                which
            }),
            Translated::GamepadRemoved(which)
        );
    }

    #[test]
    fn window_and_app_events() {
        assert_eq!(translate(&Event::Quit { timestamp: 0 }), Translated::Quit);
        assert_eq!(
            translate(&Event::AppTerminating { timestamp: 0 }),
            Translated::Quit
        );
        assert_eq!(
            translate(&Event::AppLowMemory { timestamp: 0 }),
            Translated::Ignored
        );
        let cases = [
            (
                SdlWindowEvent::Resized(800, -1),
                WindowEventKind::Resized { w: 800, h: 0 },
            ),
            (
                SdlWindowEvent::PixelSizeChanged(1600, 1200),
                WindowEventKind::PixelSizeChanged { w: 1600, h: 1200 },
            ),
            (
                SdlWindowEvent::Moved(-5, 7),
                WindowEventKind::Moved { x: -5, y: 7 },
            ),
            (
                SdlWindowEvent::DisplayChanged(3),
                WindowEventKind::DisplayChanged { display_id: 3 },
            ),
            (SdlWindowEvent::Shown, WindowEventKind::Shown),
            (SdlWindowEvent::Hidden, WindowEventKind::Hidden),
            (SdlWindowEvent::Exposed, WindowEventKind::Exposed),
            (SdlWindowEvent::Minimized, WindowEventKind::Minimized),
            (SdlWindowEvent::Maximized, WindowEventKind::Maximized),
            (SdlWindowEvent::Restored, WindowEventKind::Restored),
            (SdlWindowEvent::Occluded, WindowEventKind::Occluded),
            (SdlWindowEvent::FocusGained, WindowEventKind::FocusGained),
            (SdlWindowEvent::FocusLost, WindowEventKind::FocusLost),
            (SdlWindowEvent::CloseRequested, WindowEventKind::CloseRequested),
            (SdlWindowEvent::EnterFullscreen, WindowEventKind::EnterFullscreen),
            (SdlWindowEvent::LeaveFullscreen, WindowEventKind::LeaveFullscreen),
        ];
        for (sdl, kind) in cases {
            assert_eq!(
                translate(&win(99, 4, sdl)),
                Translated::Window {
                    timestamp_ns: 99,
                    window_id: 4,
                    kind
                }
            );
        }
        for sdl in [
            SdlWindowEvent::None,
            SdlWindowEvent::MouseEnter,
            SdlWindowEvent::MouseLeave,
            SdlWindowEvent::HitTest(1, 2),
            SdlWindowEvent::ICCProfChanged,
        ] {
            assert_eq!(translate(&win(1, 1, sdl)), Translated::Ignored);
        }
    }

    #[test]
    fn forwarder_routes_counts_and_drops() {
        let (itx, mut irx) = channel(2);
        let (wtx, mut wrx) = channel(1);
        let mut f = EventForwarder::new(itx, wtx);
        f.register_windows(&[40, 41, 42, 43]);

        for (i, sc) in [Scancode::LShift, Scancode::RShift, Scancode::Space]
            .into_iter()
            .enumerate()
        {
            f.forward(&key(true, sc, 100 + i as u64, false));
        }
        assert_eq!(
            irx.pop(),
            Some(InputEvent::button(100, CabinetInput::LeftFlipper, true))
        );
        assert_eq!(
            irx.pop(),
            Some(InputEvent::button(101, CabinetInput::RightFlipper, true))
        );
        assert_eq!(irx.pop(), None);

        f.forward(&win(5, 41, SdlWindowEvent::Exposed));
        f.forward(&win(6, 41, SdlWindowEvent::Exposed));
        f.forward(&win(7, 43, SdlWindowEvent::Exposed)); // fourth id not kept
        f.forward(&win(8, 99, SdlWindowEvent::Exposed));
        assert_eq!(
            wrx.pop(),
            Some(WindowEvent {
                timestamp_ns: 5,
                window: 1,
                kind: WindowEventKind::Exposed
            })
        );
        assert_eq!(wrx.pop(), None);
        assert_eq!(
            f.forward(&Event::Quit { timestamp: 1 }),
            Translated::Quit
        );
        assert_eq!(
            f.stats(),
            ForwardStats {
                input_forwarded: 2,
                input_dropped: 1,
                window_forwarded: 1,
                window_dropped: 1,
                window_unknown: 2,
            }
        );
    }
}
