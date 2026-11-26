//! SDL event translation and forwarding into the SPSC rings (PLAN §2.2).
//!
//! [`translate`] is pure (no SDL calls), so the mapping is unit-testable without
//! initialising SDL. [`EventForwarder`] owns the producer ends of the two rings
//! and pushes translated events; it never blocks: when a ring is full the event
//! is dropped and counted in [`ForwardStats`].
//!
//! Cabinet input: without an [`InputRouter`] the forwarder uses the fixed
//! 1.3 keymap ([`translate`]); with one ([`EventForwarder::set_router`], what
//! the app does) keys and gamepads go through their device profiles and
//! [`HeldInputs`](super::HeldInputs), and a cabinet edge reaches the input
//! ring only when `HeldInputs::set` reports one (issue #9b).
//!
//! Menu (issue #84, REQ-VIEW-006): with a [`MenuController`]
//! ([`EventForwarder::set_menu`]), `Esc` and the controller menu button
//! toggle the menu before the router sees them, and every cabinet event goes
//! through [`MenuController::route`] (consumed while the menu is open)
//! instead of straight into the input ring. Without one nothing changes.

use super::input_router::{InputRouter, gamepad_button};
use super::keymap;
use crate::ui::{MenuController, is_menu_toggle_button, is_menu_toggle_key};
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
        } => keymap::key_input(sc).map_or(Translated::Ignored, |i| {
            Translated::Input(InputEvent::button(ts, i, true))
        }),
        Event::KeyUp {
            scancode: Some(sc),
            repeat: false,
            ..
        } => keymap::key_input(sc).map_or(Translated::Ignored, |i| {
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
    /// Profile-based input path (`None`: the 1.3 keymap).
    router: Option<InputRouter>,
    /// The in-game menu (`None`: no menu, cabinet events go straight to
    /// the input ring).
    menu: Option<MenuController>,
}

impl EventForwarder {
    /// A forwarder writing into the given ring producers.
    pub fn new(input: SpscProducer<InputEvent>, window: SpscProducer<WindowEvent>) -> Self {
        Self {
            input,
            window,
            window_ids: ArrayVec::new(),
            stats: ForwardStats::default(),
            router: None,
            menu: None,
        }
    }

    /// Re-points the forwarder at new rings (the table browser restarts the
    /// engine threads on a table swap, issue #85): events forwarded after
    /// this call go to `input` and `window` only. The menu, the router (and
    /// its held inputs and gamepads), the registered windows and the
    /// counters are kept; the old producers are dropped.
    pub fn replace_channels(
        &mut self,
        input: SpscProducer<InputEvent>,
        window: SpscProducer<WindowEvent>,
    ) {
        self.input = input;
        self.window = window;
    }

    /// Puts the in-game menu between the router and the input ring (see
    /// the [module docs](self)): `Esc` / the controller menu button toggle
    /// it, and while it is open cabinet events drive it and never reach
    /// the simulation.
    pub fn set_menu(&mut self, menu: MenuController) {
        self.menu = Some(menu);
    }

    /// The menu, if set.
    pub fn menu(&self) -> Option<&MenuController> {
        self.menu.as_ref()
    }

    /// The menu, if set (e.g. to take its config changes).
    pub fn menu_mut(&mut self) -> Option<&mut MenuController> {
        self.menu.as_mut()
    }

    /// Removes and returns the menu (the forwarder then behaves as without
    /// one).
    pub fn take_menu(&mut self) -> Option<MenuController> {
        self.menu.take()
    }

    /// An input event for the ring: through the menu when there is one.
    fn deliver(
        input: &mut SpscProducer<InputEvent>,
        stats: &mut ForwardStats,
        menu: Option<&mut MenuController>,
        ev: InputEvent,
    ) {
        match menu {
            Some(m) => m.route(ev, &mut |e| Self::push_input(input, stats, e)),
            None => Self::push_input(input, stats, ev),
        }
    }

    /// With a menu: handles the toggle key / button (and swallows their
    /// releases and repeats). `true` when `event` was such an event.
    fn menu_toggle(&mut self, event: &Event) -> bool {
        let Some(menu) = self.menu.as_mut() else {
            return false;
        };
        let (down, repeat) = match *event {
            Event::KeyDown {
                scancode: Some(sc),
                repeat,
                ..
            } if is_menu_toggle_key(sc as u32) => (true, repeat),
            Event::KeyUp {
                scancode: Some(sc), ..
            } if is_menu_toggle_key(sc as u32) => (false, false),
            Event::GamepadButtonDown { button, .. }
                if gamepad_button(button).is_some_and(is_menu_toggle_button) =>
            {
                (true, false)
            }
            Event::GamepadButtonUp { button, .. }
                if gamepad_button(button).is_some_and(is_menu_toggle_button) =>
            {
                (false, false)
            }
            _ => return false,
        };
        if down && !repeat {
            let (input, stats) = (&mut self.input, &mut self.stats);
            menu.toggle(event.get_timestamp(), &mut |e| {
                Self::push_input(input, stats, e);
            });
        }
        true
    }

    /// Routes cabinet input through `router` (device profiles and
    /// [`HeldInputs`](super::HeldInputs)) instead of the 1.3 keymap.
    pub fn set_router(&mut self, router: InputRouter) {
        self.router = Some(router);
    }

    /// Replaces the input router (a live input-profile change, issue #84).
    /// Everything the old router holds is released first, at `ts_ns`, through
    /// the same path as [`poll`](Self::poll) (the menu, then the input ring),
    /// so the ring holds those releases before any event of `router` and no
    /// input stays held in the simulation, the menu's live readings or a
    /// router. Without an old router this is [`set_router`](Self::set_router).
    pub fn replace_router(&mut self, ts_ns: u64, router: InputRouter) {
        let Self {
            input,
            stats,
            router: old,
            menu,
            ..
        } = self;
        if let Some(r) = old.as_mut() {
            r.release_all(ts_ns, &mut |ev| {
                Self::deliver(input, stats, menu.as_mut(), ev);
            });
        }
        *old = Some(router);
    }

    /// The input router, if set.
    pub fn router(&self) -> Option<&InputRouter> {
        self.router.as_ref()
    }

    /// The input router, if set (e.g. to change a device profile).
    pub fn router_mut(&mut self) -> Option<&mut InputRouter> {
        self.router.as_mut()
    }

    fn push_input(input: &mut SpscProducer<InputEvent>, stats: &mut ForwardStats, ev: InputEvent) {
        if input.push(ev).is_ok() {
            stats.input_forwarded += 1;
        } else {
            stats.input_dropped += 1;
        }
    }

    /// A gamepad was connected (the platform passes its SDL GUID): the
    /// router picks its profile. No effect without a router.
    pub fn gamepad_added(&mut self, which: u32, guid: Option<pin_input::SdlGuid>) {
        if let Some(r) = self.router.as_mut() {
            let _ = r.add_gamepad(which, guid);
        }
    }

    /// Time-driven input at `now_ns` (SDL ns clock): ends digital nudges.
    /// Called by [`Platform::pump`](super::Platform::pump) every iteration.
    pub fn poll(&mut self, now_ns: u64) {
        let Self {
            input,
            stats,
            router,
            menu,
            ..
        } = self;
        if let Some(r) = router.as_mut() {
            r.poll(now_ns, &mut |ev| {
                Self::deliver(input, stats, menu.as_mut(), ev);
            });
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
    ///
    /// With a menu ([`set_menu`](Self::set_menu)), a non-repeat `Esc` key
    /// press or controller menu button press toggles it (any releases go
    /// into the input ring with that event's timestamp) and returns
    /// [`Translated::Ignored`], as do their releases and repeats; every
    /// cabinet event goes through [`MenuController::route`], and the
    /// returned [`Translated::Input`] is the last event that reached the
    /// ring ([`Translated::Ignored`] when the menu consumed them all).
    pub fn forward(&mut self, event: &Event) -> Translated {
        if self.menu_toggle(event) {
            return Translated::Ignored;
        }
        let t = translate(event);
        if let Some(router) = self.router.as_mut() {
            let (input, stats, menu) = (&mut self.input, &mut self.stats, &mut self.menu);
            let mut last = None;
            let handled = router.route(event, &mut |ev| match menu.as_mut() {
                Some(m) => m.route(ev, &mut |e| {
                    last = Some(e);
                    Self::push_input(input, stats, e);
                }),
                None => {
                    last = Some(ev);
                    Self::push_input(input, stats, ev);
                }
            });
            if handled {
                return match t {
                    Translated::GamepadRemoved(_) => t,
                    _ => last.map_or(Translated::Ignored, Translated::Input),
                };
            }
        }
        match t {
            Translated::Input(ev) => {
                if let Some(m) = self.menu.as_mut() {
                    let (input, stats) = (&mut self.input, &mut self.stats);
                    let mut last = None;
                    m.route(ev, &mut |e| {
                        last = Some(e);
                        Self::push_input(input, stats, e);
                    });
                    return last.map_or(Translated::Ignored, Translated::Input);
                }
                Self::push_input(&mut self.input, &mut self.stats, ev);
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
            (
                SdlWindowEvent::CloseRequested,
                WindowEventKind::CloseRequested,
            ),
            (
                SdlWindowEvent::EnterFullscreen,
                WindowEventKind::EnterFullscreen,
            ),
            (
                SdlWindowEvent::LeaveFullscreen,
                WindowEventKind::LeaveFullscreen,
            ),
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
        assert_eq!(f.forward(&Event::Quit { timestamp: 1 }), Translated::Quit);
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

    #[test]
    fn menu_sits_between_router_and_ring() {
        let dir = std::env::temp_dir().join(format!("pin_app_events_menu_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (itx, mut irx) = channel(16);
        let (wtx, _wrx) = channel(4);
        let mut f = EventForwarder::new(itx, wtx);
        f.set_router(InputRouter::new());
        f.set_menu(MenuController::load(dir.join("config.toml")).0);
        assert_eq!(
            f.forward(&key(true, Scancode::LShift, 5, false)),
            Translated::Input(InputEvent::button(5, CabinetInput::LeftFlipper, true))
        );
        assert_eq!(
            f.forward(&key(true, Scancode::Escape, 6, false)),
            Translated::Ignored
        );
        assert!(f.menu().unwrap().is_open());
        // Consumed while open: nothing reaches the ring.
        assert_eq!(
            f.forward(&key(false, Scancode::LShift, 7, false)),
            Translated::Ignored
        );
        f.poll(8);
        assert_eq!(
            f.forward(&key(false, Scancode::Escape, 9, false)),
            Translated::Ignored
        );
        assert_eq!(
            irx.pop(),
            Some(InputEvent::button(5, CabinetInput::LeftFlipper, true))
        );
        assert_eq!(
            irx.pop(),
            Some(InputEvent::button(6, CabinetInput::LeftFlipper, false))
        );
        assert_eq!(irx.pop(), None);
        assert_eq!(f.stats().input_forwarded, 2);
        // Window events are unchanged by the menu.
        f.register_windows(&[1]);
        f.forward(&win(10, 1, SdlWindowEvent::Exposed));
        assert_eq!(f.stats().window_forwarded, 1);
        assert!(f.menu_mut().is_some());
        assert!(f.take_menu().is_some() && f.menu().is_none());
    }

    #[test]
    fn menu_without_router_routes_the_keymap_path() {
        let dir = std::env::temp_dir().join(format!("pin_app_events_menu2_{}", std::process::id()));
        let (itx, mut irx) = channel(16);
        let (wtx, _wrx) = channel(4);
        let mut f = EventForwarder::new(itx, wtx);
        f.set_menu(MenuController::load(dir.join("config.toml")).0);
        assert_eq!(
            f.forward(&key(true, Scancode::RShift, 1, false)),
            Translated::Input(InputEvent::button(1, CabinetInput::RightFlipper, true))
        );
        let which = JoystickId::new(2);
        f.forward(&Event::GamepadButtonDown {
            timestamp: 2,
            which,
            button: Button::Guide,
        });
        f.forward(&Event::GamepadButtonUp {
            timestamp: 3,
            which,
            button: Button::Guide,
        });
        assert!(f.menu().unwrap().is_open());
        assert_eq!(
            f.forward(&key(false, Scancode::RShift, 4, false)),
            Translated::Ignored
        );
        assert_eq!(
            irx.pop(),
            Some(InputEvent::button(1, CabinetInput::RightFlipper, true))
        );
        assert_eq!(
            irx.pop(),
            Some(InputEvent::button(2, CabinetInput::RightFlipper, false))
        );
        assert_eq!(irx.pop(), None);
        // A repeat Esc press is swallowed and toggles nothing.
        f.forward(&key(true, Scancode::Escape, 5, true));
        assert!(f.menu().unwrap().is_open());
    }
    /// PR #197 finding 1 (issue #84 live input-profile change): replacing
    /// the router releases what the old one holds at the swap timestamp,
    /// ahead of any event of the new router, and the later physical release
    /// adds nothing.
    #[test]
    fn router_swap_releases_old_holds() {
        let (itx, mut irx) = channel(16);
        let (wtx, _wrx) = channel(4);
        let mut f = EventForwarder::new(itx, wtx);
        f.set_router(InputRouter::new());
        f.forward(&key(true, Scancode::LShift, 5, false));
        f.replace_router(20, InputRouter::new());
        f.forward(&key(true, Scancode::RShift, 25, false));
        f.forward(&key(false, Scancode::LShift, 30, false));
        f.poll(40);
        assert_eq!(
            irx.pop(),
            Some(InputEvent::button(5, CabinetInput::LeftFlipper, true))
        );
        assert_eq!(
            irx.pop(),
            Some(InputEvent::button(20, CabinetInput::LeftFlipper, false))
        );
        assert_eq!(
            irx.pop(),
            Some(InputEvent::button(25, CabinetInput::RightFlipper, true))
        );
        assert_eq!(irx.pop(), None);
        let r = f.router().unwrap();
        assert!(!r.held().is_held(CabinetInput::LeftFlipper));
        assert_eq!(f.stats().input_forwarded, 3);
        // Without an old router it only installs the new one.
        let (itx, mut irx) = channel(4);
        let (wtx, _wrx) = channel(1);
        let mut g = EventForwarder::new(itx, wtx);
        g.replace_router(1, InputRouter::new());
        assert!(g.router().is_some());
        assert_eq!(irx.pop(), None);
    }

    /// Finding 1 with the menu open (where the app changes the profile):
    /// the old router's release updates the menu's live readings and is
    /// consumed by the open menu (opening already released the sim).
    #[test]
    fn router_swap_releases_old_holds_with_menu_open() {
        let dir = std::env::temp_dir().join(format!("pin_app_events_swap_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (itx, mut irx) = channel(16);
        let (wtx, _wrx) = channel(4);
        let mut f = EventForwarder::new(itx, wtx);
        f.set_router(InputRouter::new());
        f.set_menu(MenuController::load(dir.join("config.toml")).0);
        f.forward(&key(true, Scancode::Escape, 1, false));
        f.forward(&key(false, Scancode::Escape, 2, false));
        assert!(f.menu().unwrap().is_open());
        // Pressed while open: consumed, but shown as held in the monitor.
        f.forward(&key(true, Scancode::LShift, 5, false));
        let monitor = |f: &EventForwarder| {
            f.menu()
                .unwrap()
                .menu()
                .monitor()
                .is_pressed(CabinetInput::LeftFlipper)
        };
        assert!(monitor(&f));
        f.replace_router(20, InputRouter::new());
        assert!(!monitor(&f), "monitor reading released by the swap");
        assert!(
            !f.router()
                .unwrap()
                .held()
                .is_held(CabinetInput::LeftFlipper)
        );
        assert!(f.menu().unwrap().is_open());
        assert_eq!(irx.pop(), None, "the open menu consumes it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Issue #85: after `replace_channels` events reach the new rings only;
    /// the registered windows and the counters are kept.
    #[test]
    fn replace_channels_redirects_to_the_new_rings() {
        let (itx, mut irx) = channel(8);
        let (wtx, mut wrx) = channel(8);
        let mut f = EventForwarder::new(itx, wtx);
        f.register_windows(&[42]);
        f.forward(&key(true, Scancode::LShift, 1, false));
        f.forward(&win(2, 42, SdlWindowEvent::Exposed));
        assert!(irx.pop().is_some() && wrx.pop().is_some());
        let (itx2, mut irx2) = channel(8);
        let (wtx2, mut wrx2) = channel(8);
        f.replace_channels(itx2, wtx2);
        f.forward(&key(false, Scancode::LShift, 3, false));
        f.forward(&win(4, 42, SdlWindowEvent::Exposed));
        assert!(irx.pop().is_none() && wrx.pop().is_none(), "old rings");
        assert_eq!(
            irx2.pop(),
            Some(InputEvent::button(3, CabinetInput::LeftFlipper, false))
        );
        assert!(wrx2.pop().is_some());
        assert_eq!(f.stats().input_forwarded, 2);
        assert_eq!(f.stats().window_forwarded, 2);
    }
}
