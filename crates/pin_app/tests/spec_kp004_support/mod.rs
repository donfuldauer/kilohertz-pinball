//! Shared helpers for the KP-004 (#84) spec tests (REQ-VIEW-006).
#![allow(dead_code)]

use pin_app::platform::{EventForwarder, InputRouter};
use pin_app::ui::MenuController;
use pin_core::spsc::{SpscConsumer, channel};
use pin_core::{CabinetInput, InputEvent, InputEventKind};
use pin_render::topology::WindowEvent;
use sdl3::event::Event;
use sdl3::gamepad::Button;
use sdl3::joystick::JoystickId;
use sdl3::keyboard::{Mod, Scancode};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A unique, empty scratch directory below `CARGO_TARGET_TMPDIR`.
pub fn scratch(name: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("kp004_{name}_{}_{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

pub fn key(down: bool, sc: Scancode, ts: u64, repeat: bool) -> Event {
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

pub fn pad_button(down: bool, button: Button, ts: u64) -> Event {
    let which = JoystickId::new(7);
    if down {
        Event::GamepadButtonDown {
            timestamp: ts,
            which,
            button,
        }
    } else {
        Event::GamepadButtonUp {
            timestamp: ts,
            which,
            button,
        }
    }
}

pub struct Rig {
    pub fwd: EventForwarder,
    pub ring: SpscConsumer<InputEvent>,
    pub _windows: SpscConsumer<WindowEvent>,
    pub dir: PathBuf,
}

impl Rig {
    /// A forwarder with a menu on a fresh config path; `routed` adds an
    /// `InputRouter` on the auto-detected keyboard profile.
    pub fn new(name: &str, routed: bool) -> Rig {
        let dir = scratch(name);
        let (itx, irx) = channel::<InputEvent>(1024);
        let (wtx, wrx) = channel::<WindowEvent>(64);
        let mut fwd = EventForwarder::new(itx, wtx);
        if routed {
            fwd.set_router(InputRouter::new());
        }
        let (menu, _warnings) = MenuController::load(dir.join("config.toml"));
        fwd.set_menu(menu);
        Rig {
            fwd,
            ring: irx,
            _windows: wrx,
            dir,
        }
    }

    pub fn drain(&mut self) -> Vec<InputEvent> {
        let mut v = Vec::new();
        while let Some(e) = self.ring.pop() {
            v.push(e);
        }
        v
    }

    pub fn is_open(&self) -> bool {
        self.fwd.menu().expect("menu set").is_open()
    }

    pub fn send(&mut self, ev: Event) {
        let _ = self.fwd.forward(&ev);
    }

    pub fn esc(&mut self, ts: u64) {
        self.send(key(true, Scancode::Escape, ts, false));
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub fn is_button(e: &InputEvent, input: CabinetInput, pressed: bool) -> bool {
    matches!(e.kind, InputEventKind::Button { input: i, pressed: p } if i == input && p == pressed)
}
