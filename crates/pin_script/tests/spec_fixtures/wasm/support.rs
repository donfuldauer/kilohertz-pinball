//! Shared harness for the black-box spec tests of subphase 4.5 (Wasm backend).
//!
//! Everything here goes through the brief's FIXED API only:
//! `pin_script::wasm::{WasmConfig, WasmScriptBackend, PreparedModule}`,
//! `NameTables` and the `pin_core` scripting contract. Guest modules are
//! hand-written WAT against the fixed `pinball` import ABI.
#![allow(dead_code)]

use pin_core::{
    CoilId, DispenserId, DiverterId, FlipperId, HostCommandQueue, HostCtx, HostView, LampId,
    MagnetId, Pcg32, PersistKey, ProfileId, Rng, SceneId, SeedableRng, SoundId, SwitchEdge,
    SwitchId, TableScriptBackend,
};
use pin_script::NameTables;
use pin_script::wasm::{WasmConfig, WasmScriptBackend};
use std::sync::mpsc;
use std::time::Duration;

/// Every import of the fixed ABI, with the `$name` the fixtures call it by.
pub const IMPORTS: &str = r#"
  (import "pinball" "pulse_coil" (func $pulse_coil (param i32 i32)))
  (import "pinball" "hold_coil" (func $hold_coil (param i32 i32)))
  (import "pinball" "set_magnet" (func $set_magnet (param i32 f32)))
  (import "pinball" "set_zipper" (func $set_zipper (param i32 i32)))
  (import "pinball" "set_diverter" (func $set_diverter (param i32 i32)))
  (import "pinball" "spawn_ball" (func $spawn_ball (param i32 i32)))
  (import "pinball" "set_lamp" (func $set_lamp (param i32 f32)))
  (import "pinball" "set_gi_tint" (func $set_gi_tint (param f32 f32 f32 f32)))
  (import "pinball" "set_score" (func $set_score (param i32 i64)))
  (import "pinball" "show_scene" (func $show_scene (param i32)))
  (import "pinball" "play_sound" (func $play_sound (param i32 f32)))
  (import "pinball" "persist_set" (func $persist_set (param i32 i64)))
  (import "pinball" "persist_get" (func $persist_get (param i32) (result i64)))
  (import "pinball" "switch" (func $switch (param i32) (result i32)))
  (import "pinball" "tick" (func $tick (result i64)))
  (import "pinball" "balls_in_play" (func $balls_in_play (result i32)))
  (import "pinball" "random" (func $random (param i64 i64) (result i64)))
  (import "pinball" "id_of" (func $id_of (param i32 i32 i32) (result i32)))
"#;

/// `(module <all imports> <body>)`. The body declares memory and exports.
pub fn module(body: &str) -> String {
    format!("(module {IMPORTS} {body})")
}

/// A complete guest: one page of exported memory, the three callbacks with the
/// given bodies (instruction text; `on_switch_edge` has params `$id`, `$closed`).
pub fn guest(on_init: &str, on_tick: &str, on_switch_edge: &str) -> String {
    module(&format!(
        r#"(memory (export "memory") 1)
           (func (export "on_init") {on_init})
           (func (export "on_tick") {on_tick})
           (func (export "on_switch_edge") (param $id i32) (param $closed i32) {on_switch_edge})"#
    ))
}

/// A guest whose only behaviour is `on_tick`.
pub fn tick_guest(on_tick: &str) -> String {
    guest("", on_tick, "")
}

/// The world a backend sees: view, seeded generator and command queue.
pub struct Host {
    pub view: HostView,
    pub rng: Pcg32,
    pub cmd: HostCommandQueue,
}

impl Host {
    pub fn new(seed: u64) -> Self {
        Self {
            view: HostView::default(),
            rng: Pcg32::seed_from_u64(seed),
            cmd: HostCommandQueue::default(),
        }
    }

    pub fn ctx(&mut self) -> HostCtx<'_> {
        HostCtx {
            view: &self.view,
            rng: &mut self.rng,
            cmd: &mut self.cmd,
        }
    }

    /// Calls `on_init` without clearing the queue first.
    pub fn init(&mut self, b: &mut dyn TableScriptBackend) {
        b.on_init(&mut self.ctx());
    }

    /// Clears the queue (as the sim does every tick), advances time, calls `on_tick`.
    pub fn tick(&mut self, b: &mut dyn TableScriptBackend) {
        self.cmd.clear();
        self.view.tick += 1;
        b.on_tick(&mut self.ctx());
    }

    /// Calls `on_tick` without clearing the queue or advancing time.
    pub fn tick_keep(&mut self, b: &mut dyn TableScriptBackend) {
        b.on_tick(&mut self.ctx());
    }

    /// Clears the queue, updates the switch bitset and delivers the edge.
    pub fn edge(&mut self, b: &mut dyn TableScriptBackend, switch: u16, closed: bool) {
        self.cmd.clear();
        self.view.set_switch(SwitchId(switch), closed);
        self.edge_keep(b, switch, closed);
    }

    /// Delivers an edge without clearing the queue or touching the bitset.
    pub fn edge_keep(&mut self, b: &mut dyn TableScriptBackend, switch: u16, closed: bool) {
        let edge = if closed {
            SwitchEdge::Closed
        } else {
            SwitchEdge::Opened
        };
        b.on_switch_edge(SwitchId(switch), edge, &mut self.ctx());
    }
}

fn named<T>(items: &[(&str, u16)], make: impl Fn(u16) -> T) -> Vec<(String, T)> {
    items
        .iter()
        .map(|&(n, id)| (n.to_owned(), make(id)))
        .collect()
}

/// Name tables with distinctive, non-contiguous ids so mix-ups are visible.
/// `left_sling` and `trough` exist under several kinds with different ids.
pub fn names() -> NameTables {
    NameTables {
        switches: named(
            &[
                ("start", 0),
                ("left_sling", 7),
                ("right_sling", 8),
                ("trough_1", 20),
                ("spinner", 63),
                ("far", 255),
            ],
            SwitchId,
        ),
        coils: named(
            &[
                ("trough_eject", 1),
                ("left_sling", 3),
                ("knocker", 31),
                ("hold_gate", 40),
            ],
            CoilId,
        ),
        lamps: named(&[("shoot_again", 2), ("jackpot", 200)], LampId),
        magnets: named(&[("center", 4)], MagnetId),
        diverters: named(&[("ramp_gate", 5)], DiverterId),
        flippers: named(&[("left", 0), ("right", 1), ("upper", 6)], FlipperId),
        dispensers: named(&[("trough", 9)], DispenserId),
        profiles: named(&[("steel", 0), ("ceramic", 11)], ProfileId),
        sounds: named(&[("bell", 12), ("drain", 13)], SoundId),
        scenes: named(&[("attract", 14), ("multiball", 15)], SceneId),
        persist: named(
            &[("high_score_1", 0), ("credits", 16), ("last", 63)],
            PersistKey,
        ),
    }
}

pub fn cfg(memory_limit_bytes: usize, fuel_per_callback: u64) -> WasmConfig {
    WasmConfig {
        memory_limit_bytes,
        fuel_per_callback,
    }
}

/// Builds a backend with the shared name tables and default limits; panics on error.
pub fn backend(source: &str) -> WasmScriptBackend {
    backend_with(source, WasmConfig::default())
}

pub fn backend_with(source: &str, cfg: WasmConfig) -> WasmScriptBackend {
    match WasmScriptBackend::new(source.as_bytes(), &names(), cfg) {
        Ok(b) => b,
        Err(e) => panic!("fixture failed to load: {e}"),
    }
}

/// Runs `f` on a helper thread and fails the test if it does not come back.
///
/// This is a hang guard, not a timing assertion (rule 5): an unbounded guest
/// loop (fuel metering missing) would otherwise block the whole test binary.
/// The limit is orders of magnitude above anything a bounded callback needs.
pub fn watchdog<T: Send + 'static>(what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    let handle = std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    match rx.recv_timeout(Duration::from_secs(300)) {
        Ok(v) => v,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            panic!("{what}: guest execution was not bounded (no result after 300 s)")
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => match handle.join() {
            Err(p) => std::panic::resume_unwind(p),
            Ok(()) => unreachable!("worker ended without sending"),
        },
    }
}

/// Lua 5.4 `math.random(lo, hi)` over the host PCG, written from the Lua 5.4
/// reference (`project` in lmathlib.c): one 64-bit draw, masked rejection.
/// This is the test's own oracle for `pinball.random`.
pub fn lua_random(lo: i64, hi: i64, rng: &mut Pcg32) -> i64 {
    assert!(lo <= hi);
    let mut ran = rng.next_u64();
    let n = (hi as u64).wrapping_sub(lo as u64);
    if n & n.wrapping_add(1) == 0 {
        ran &= n;
    } else {
        let mut lim = n;
        lim |= lim >> 1;
        lim |= lim >> 2;
        lim |= lim >> 4;
        lim |= lim >> 8;
        lim |= lim >> 16;
        lim |= lim >> 32;
        loop {
            ran &= lim;
            if ran <= n {
                break;
            }
            ran = rng.next_u64();
        }
    }
    (lo as u64).wrapping_add(ran) as i64
}

/// The f32 `x` as a driver argument (its bit pattern in the low 32 bits).
pub fn fa(x: f32) -> i64 {
    i64::from(x.to_bits())
}

pub const DRIVER_WAT: &str = include_str!("driver.wat");
pub const DRIVER_WASM: &[u8] = include_bytes!("driver.wasm");

/// Driver operations (the `id` passed to the driver's `on_switch_edge`).
pub mod op {
    pub const PULSE_COIL: u16 = 0;
    pub const HOLD_COIL: u16 = 1;
    pub const SET_MAGNET: u16 = 2;
    pub const SET_ZIPPER: u16 = 3;
    pub const SET_DIVERTER: u16 = 4;
    pub const SPAWN_BALL: u16 = 5;
    pub const SET_LAMP: u16 = 6;
    pub const SET_GI_TINT: u16 = 7;
    pub const SET_SCORE: u16 = 8;
    pub const SHOW_SCENE: u16 = 9;
    pub const PLAY_SOUND: u16 = 10;
    pub const PERSIST_SET: u16 = 11;
    pub const PERSIST_GET: u16 = 12;
    pub const SWITCH: u16 = 13;
    pub const TICK: u16 = 14;
    pub const BALLS_IN_PLAY: u16 = 15;
    pub const RANDOM: u16 = 16;
    pub const ID_OF: u16 = 17;
    pub const UNREACHABLE: u16 = 100;
    pub const SPIN: u16 = 101;
    pub const DIV_ZERO: u16 = 102;
    pub const OOB_LOAD: u16 = 103;
    pub const RECURSE: u16 = 104;
    pub const MEMORY_GROW: u16 = 105;
    pub const MEMORY_SIZE: u16 = 106;
    pub const NAN_ADD: u16 = 107;
    pub const NULL_INDIRECT: u16 = 108;
    pub const COUNT_LOOP: u16 = 109;
    pub const PULSE_40: u16 = 111;
    pub const SPIN_CALLING_HOST: u16 = 112;
    pub const HOLD_RANGE: u16 = 113;
    pub const OOB_STORE: u16 = 114;
}

/// The driver guest (`driver.wat`): `on_switch_edge(op, closed)` performs one
/// operation whose arguments are read from persist slots 0..=3 of the view;
/// value-returning operations report through `set_score(1, value)`. An
/// `Opened` edge first queues a fixed set of marker commands (see
/// [`assert_markers`]) so rollback is observable. `on_init` shows scene 1,
/// `on_tick` shows scene 7.
pub struct Driver {
    pub b: WasmScriptBackend,
    pub h: Host,
}

impl Driver {
    pub fn new() -> Self {
        Self::with_cfg(WasmConfig::default())
    }

    pub fn with_cfg(cfg: WasmConfig) -> Self {
        Self {
            b: backend_with(DRIVER_WAT, cfg),
            h: Host::new(0x5eed),
        }
    }

    pub fn binary() -> Self {
        Self {
            b: WasmScriptBackend::new(DRIVER_WASM, &names(), WasmConfig::default())
                .unwrap_or_else(|e| panic!("binary driver failed to load: {e}")),
            h: Host::new(0x5eed),
        }
    }

    fn set_args(&mut self, args: &[i64]) {
        assert!(args.len() <= 4);
        for (slot, &a) in self.h.view.persist.iter_mut().zip(args) {
            *slot = a;
        }
    }

    /// Clears the queue and runs `op(args)`.
    pub fn call(&mut self, op: u16, args: &[i64]) -> &HostCommandQueue {
        self.h.cmd.clear();
        self.call_keep(op, args)
    }

    /// Runs `op(args)` without clearing the queue.
    pub fn call_keep(&mut self, op: u16, args: &[i64]) -> &HostCommandQueue {
        self.set_args(args);
        self.h.edge_keep(&mut self.b, op, true);
        &self.h.cmd
    }

    /// Clears the queue, queues the marker commands, then runs `op(args)`.
    pub fn call_marked(&mut self, op: u16, args: &[i64]) -> &HostCommandQueue {
        self.h.cmd.clear();
        self.set_args(args);
        self.h.edge_keep(&mut self.b, op, false);
        &self.h.cmd
    }

    /// Runs a value-returning op and returns the reported value (None if the
    /// call faulted or reported nothing).
    pub fn value(&mut self, op: u16, args: &[i64]) -> Option<i64> {
        let q = self.call(op, args);
        assert!(q.scores.len() <= 1);
        q.scores.first().map(|&(p, v)| {
            assert_eq!(p, 0, "driver reports through player 1");
            v
        })
    }

    pub fn tick(&mut self) -> &HostCommandQueue {
        self.h.tick(&mut self.b);
        &self.h.cmd
    }
}

/// The commands the driver queues first on an `Opened` edge.
pub fn markers() -> HostCommandQueue {
    let mut q = HostCommandQueue::default();
    q.pulse_coil(CoilId(9), 9).unwrap();
    q.hold_coil(CoilId(8), true).unwrap();
    q.set_lamp(LampId(9), 1.0).unwrap();
    q.set_gi_tint([0.5; 4]);
    q.set_score(3, 99).unwrap();
    q.write_persist(PersistKey(9), 9).unwrap();
    q
}
