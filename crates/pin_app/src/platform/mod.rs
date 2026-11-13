//! SDL3 platform shell: subsystems, display topology, windows and the
//! main-thread event pump (PLAN §2.2, §4 Subphase 1.3).
//!
//! The main thread owns [`Platform`]. Each iteration it calls
//! [`Platform::pump`], which blocks in `SDL_WaitEventTimeout` (1 ms by
//! default), then drains the SDL queue completely (stopping early leaves SDL3's
//! poll sentinel queued, see DECISIONS 0.1) and hands every event to an
//! [`EventForwarder`]. Cabinet inputs go into the input ring (→ sim thread)
//! with SDL's nanosecond timestamp; window events go into the window ring
//! (→ render thread).
//!
//! Headless use (tests, CI, `--headless-ticks`): [`PlatformConfig::headless`]
//! selects SDL's dummy video and audio drivers, which still provide a display,
//! windows, virtual joysticks and the event queue.

mod display;
mod events;
pub mod keymap;
pub mod rumble;

pub use display::{DisplayTopologyManager, PlatformWindow, window_title};
pub use events::{EventForwarder, ForwardStats, Translated, translate};

use core::fmt;
use core::time::Duration;
use pin_render::topology::TopologyError;
use sdl3::event::Event;
use sdl3::gamepad::Gamepad;
use sdl3::joystick::JoystickId;
use sdl3::{
    AudioSubsystem, EventPump, EventSubsystem, GamepadSubsystem, JoystickSubsystem, Sdl,
    SensorSubsystem, VideoSubsystem,
};

/// Default timeout of the main-thread wait (PLAN §2.2).
pub const PUMP_TIMEOUT: Duration = Duration::from_millis(1);

/// Default input ring capacity (events between two sim ticks are few; this
/// covers several seconds of a stalled sim at human input rates).
pub const INPUT_RING_CAPACITY: usize = 1024;

/// Default window-event ring capacity.
pub const WINDOW_RING_CAPACITY: usize = 256;

/// Errors from the platform layer.
#[derive(Clone, Debug, PartialEq)]
pub enum PlatformError {
    /// An SDL call failed.
    Sdl(String),
    /// Window creation failed.
    Window(String),
    /// The requested topology is impossible.
    Topology(TopologyError),
}

impl PlatformError {
    pub(crate) fn sdl(e: impl fmt::Display) -> Self {
        PlatformError::Sdl(e.to_string())
    }
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlatformError::Sdl(e) => write!(f, "SDL error: {e}"),
            PlatformError::Window(e) => write!(f, "window creation failed: {e}"),
            PlatformError::Topology(e) => write!(f, "display topology: {e}"),
        }
    }
}

impl std::error::Error for PlatformError {}

impl From<TopologyError> for PlatformError {
    fn from(e: TopologyError) -> Self {
        PlatformError::Topology(e)
    }
}

/// How to start SDL.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct PlatformConfig {
    /// Use SDL's dummy video and audio drivers (no display server, no sound
    /// device).
    pub headless: bool,
}

/// Sets the SDL hints for `config`. Must run before [`sdl3::init`].
pub fn apply_hints(config: &PlatformConfig) {
    if config.headless {
        sdl3::hint::set("SDL_VIDEO_DRIVER", "dummy");
        sdl3::hint::set("SDL_AUDIO_DRIVER", "dummy");
    }
    // Cabinets run with the playfield window unfocused (backglass clicked,
    // DOF tools in front); input must keep flowing.
    sdl3::hint::set("SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS", "1");
}

/// What one [`Platform::pump`] call saw.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct PumpOutcome {
    /// SDL events handled.
    pub events: u32,
    /// A quit was requested.
    pub quit: bool,
    /// Displays changed; re-query with [`DisplayTopologyManager::refresh`].
    pub displays_changed: bool,
}

/// SDL with the video, audio, gamepad (and joystick) and sensor subsystems
/// initialised, plus the event pump. Lives on the main thread.
pub struct Platform {
    // Field order is drop order: gamepads and the pump go before subsystems,
    // the `Sdl` context last.
    gamepads: Vec<Gamepad>,
    event_pump: EventPump,
    events: EventSubsystem,
    sensor: SensorSubsystem,
    gamepad: GamepadSubsystem,
    joystick: JoystickSubsystem,
    audio: AudioSubsystem,
    video: VideoSubsystem,
    _sdl: Sdl,
}

impl Platform {
    /// Initialises SDL and its subsystems. Call once per process, on the main
    /// thread.
    pub fn init(config: PlatformConfig) -> Result<Self, PlatformError> {
        apply_hints(&config);
        let sdl = sdl3::init().map_err(PlatformError::sdl)?;
        let video = sdl.video().map_err(PlatformError::sdl)?;
        let audio = sdl.audio().map_err(PlatformError::sdl)?;
        let joystick = sdl.joystick().map_err(PlatformError::sdl)?;
        let gamepad = sdl.gamepad().map_err(PlatformError::sdl)?;
        let sensor = sdl.sensor().map_err(PlatformError::sdl)?;
        let events = sdl.event().map_err(PlatformError::sdl)?;
        let event_pump = sdl.event_pump().map_err(PlatformError::sdl)?;
        let mut platform = Self {
            gamepads: Vec::new(),
            event_pump,
            events,
            sensor,
            gamepad,
            joystick,
            audio,
            video,
            _sdl: sdl,
        };
        // Gamepads present at start-up also arrive as GamepadAdded events, but
        // open them now so input works before the first pump.
        if let Ok(ids) = platform.gamepad.gamepads() {
            for id in ids {
                platform.open_gamepad(id);
            }
        }
        Ok(platform)
    }

    /// The video subsystem (displays, windows).
    pub fn video(&self) -> &VideoSubsystem {
        &self.video
    }

    /// The audio subsystem.
    pub fn audio(&self) -> &AudioSubsystem {
        &self.audio
    }

    /// The joystick subsystem (also used to attach virtual cabinets, D31).
    pub fn joystick(&self) -> &JoystickSubsystem {
        &self.joystick
    }

    /// The gamepad subsystem.
    pub fn gamepad(&self) -> &GamepadSubsystem {
        &self.gamepad
    }

    /// The sensor subsystem.
    pub fn sensor(&self) -> &SensorSubsystem {
        &self.sensor
    }

    /// The event subsystem (push events, user events).
    pub fn events(&self) -> &EventSubsystem {
        &self.events
    }

    /// Joystick ids of the open gamepads.
    pub fn open_gamepads(&self) -> Vec<JoystickId> {
        self.gamepads.iter().filter_map(|g| g.id().ok()).collect()
    }

    fn open_gamepad(&mut self, id: JoystickId) {
        let already = self.gamepads.iter().any(|g| g.id().ok() == Some(id));
        if !already && let Ok(g) = self.gamepad.open(id) {
            self.gamepads.push(g);
        }
    }

    fn handle(&mut self, forwarder: &mut EventForwarder, event: &Event, out: &mut PumpOutcome) {
        out.events += 1;
        match forwarder.forward(event) {
            Translated::Quit => out.quit = true,
            Translated::DisplaysChanged => out.displays_changed = true,
            Translated::GamepadAdded(id) => self.open_gamepad(id),
            Translated::GamepadRemoved(id) => self.gamepads.retain(|g| g.id().ok() != Some(id)),
            Translated::Input(_) | Translated::Window { .. } | Translated::Ignored => {}
        }
    }

    /// One main-thread iteration: waits up to `timeout` for an event, then
    /// drains the queue completely, forwarding every event.
    ///
    /// A zero timeout only drains (used by tests, which never sleep).
    pub fn pump(&mut self, forwarder: &mut EventForwarder, timeout: Duration) -> PumpOutcome {
        let mut out = PumpOutcome::default();
        let first = if timeout.is_zero() {
            self.event_pump.poll_event()
        } else {
            self.event_pump.wait_event_timeout(timeout)
        };
        if let Some(ev) = first {
            self.handle(forwarder, &ev, &mut out);
            // Drain fully. `poll_event` is used instead of holding a
            // `poll_iter` borrow, so `handle` can open gamepads.
            while let Some(ev) = self.event_pump.poll_event() {
                self.handle(forwarder, &ev, &mut out);
            }
        }
        out
    }
}
