//! `pin_guest_sdk`: write Kilohertz Pinball table rules in Rust and compile
//! them to WebAssembly (`wasm32-unknown-unknown`). `no_std`, no allocator.
//!
//! The host is `pin_script::wasm::WasmScriptBackend`. It mirrors the Lua
//! `pinball` API one-to-one, with the same argument validation and the same
//! fault ("tilt") semantics: an invalid argument, a trap or running out of
//! fuel aborts the callback, drops the commands it queued, releases held
//! coils and stops calling the rules until they are reloaded.
//!
//! # Writing rules in Rust
//!
//! ```ignore
//! #![no_std]
//! use pin_guest_sdk::{self as pin, Rules};
//!
//! struct MyRules { score: i64 }
//!
//! impl Rules for MyRules {
//!     fn on_switch_edge(&mut self, sw: u16, closed: bool) {
//!         if closed {
//!             self.score += 10;
//!             pin::set_score(1, self.score);
//!             pin::pulse_coil(sw, 20);
//!         }
//!     }
//! }
//!
//! pin_guest_sdk::export_rules!(MyRules = MyRules { score: 0 });
//!
//! #[cfg(target_arch = "wasm32")]
//! #[panic_handler]
//! fn panic(_: &core::panic::PanicInfo) -> ! {
//!     core::arch::wasm32::unreachable()
//! }
//! ```
//!
//! Build with `crate-type = ["cdylib"]`, `panic = "abort"` and
//! `cargo build --release --target wasm32-unknown-unknown`. Put the
//! `#[panic_handler]` in the rules crate (behind
//! `#[cfg(target_arch = "wasm32")]`), never in this SDK, so the SDK also
//! builds for the host. On non-Wasm targets the import wrappers panic; only
//! the pure helpers ([`pack_state`], [`Kind`], …) are usable there.
//!
//! [`export_rules!`] exports `on_init`, `on_tick`, `on_switch_edge`,
//! `save_state`, `alloc_state` and `restore_state` for a [`Rules`] value kept
//! in a static. Alternatively write the exports by hand as
//! `#[unsafe(no_mangle)] pub extern "C" fn on_tick() { … }`.
//!
//! # The ABI (for C, AssemblyScript, TinyGo, WAT, …)
//!
//! A rules module is a core Wasm module (no WASI). It may import only these
//! functions from the module `"pinball"`; ids are `i32` in `0..=65535`,
//! booleans are `i32` 0 or 1 (anything else traps):
//!
//! ```text
//! pulse_coil(coil: i32, ms: i32)          ms in 0..=65535
//! hold_coil(coil: i32, on: i32)
//! set_magnet(magnet: i32, power: f32)     power in [0, 1]
//! set_zipper(flipper: i32, closed: i32)
//! set_diverter(diverter: i32, on: i32)
//! spawn_ball(dispenser: i32, profile: i32)
//! set_lamp(lamp: i32, intensity: f32)     intensity in [0, 1]
//! set_gi_tint(r: f32, g: f32, b: f32, a: f32)   each in [0, 1]
//! set_score(player: i32, score: i64)      player in 1..=4
//! show_scene(scene: i32)
//! play_sound(sound: i32, gain: f32)       gain finite and >= 0
//! persist_set(key: i32, value: i64)       key in 0..64
//! persist_get(key: i32) -> i64
//! switch(id: i32) -> i32                  1 if closed
//! tick() -> i64                           sim time, ms
//! balls_in_play() -> i32
//! random(m: i64, n: i64) -> i64           uniform in [m, n]; m > n traps
//! id_of(kind: i32, name_ptr: i32, name_len: i32) -> i32   id or -1
//! ```
//!
//! `id_of` kinds (see [`Kind`]): 0 sw, 1 coil, 2 lamp, 3 magnet, 4 diverter,
//! 5 flipper, 6 dispenser, 7 profile, 8 sound, 9 scene, 10 persist_key. The
//! name is UTF-8 bytes in guest memory; an out-of-bounds name or unknown kind
//! traps. `id_of` works everywhere; every other import works only inside the
//! three callbacks below (elsewhere it traps).
//!
//! Exports:
//!
//! ```text
//! memory                                   (required) the linear memory
//! on_init()                                (required) once after load
//! on_tick()                                (required) every 1 ms tick
//! on_switch_edge(id: i32, closed: i32)     (required) per switch edge
//! save_state() -> i64                      (optional) (ptr << 32) | len
//! alloc_state(len: i32) -> i32             (optional) buffer for restore
//! restore_state(ptr: i32, len: i32)        (optional)
//! ```
//!
//! Hot swap: the host instantiates the new module, calls the old
//! `save_state` (blob of at most [`MAX_STATE_BYTES`] bytes in the old
//! memory), then in the new module `alloc_state(len)` (skipped for an empty
//! blob, which is delivered as `restore_state(0, 0)`), copies the blob to the
//! returned address and calls `restore_state(ptr, len)`. Without
//! `alloc_state` a non-empty blob is not delivered.
//!
//! Limits: the memory is capped (`memory.grow` past the cap returns −1), and
//! every callback has a fuel budget (≈ one unit per Wasm operator; the
//! engine uses 100 000 per callback and a 2 MiB memory cap).

#![cfg_attr(not(test), no_std)]

use core::cell::UnsafeCell;

/// Largest state blob the host carries across a hot swap, bytes.
pub const MAX_STATE_BYTES: usize = 64 * 1024;

macro_rules! imports {
    ($( $(#[$m:meta])* fn $name:ident($($a:ident: $t:ty),*) $(-> $r:ty)?; )*) => {
        /// The raw `pinball` imports, exactly as the host exposes them.
        #[cfg(target_arch = "wasm32")]
        pub mod raw {
            #[link(wasm_import_module = "pinball")]
            unsafe extern "C" {
                $( $(#[$m])* pub safe fn $name($($a: $t),*) $(-> $r)?; )*
            }
        }

        /// Host-side stand-ins for the raw imports: they exist so the SDK
        /// builds and type-checks on every target, and panic when called.
        #[cfg(not(target_arch = "wasm32"))]
        pub mod raw {
            $(
                $(#[$m])*
                #[allow(unused_variables)]
                pub fn $name($($a: $t),*) $(-> $r)? {
                    panic!(concat!("pinball.", stringify!($name), " is only available on wasm32"))
                }
            )*
        }
    };
}

imports! {
    /// `pulse_coil(coil, ms)`.
    fn pulse_coil(coil: i32, ms: i32);
    /// `hold_coil(coil, on)`.
    fn hold_coil(coil: i32, on: i32);
    /// `set_magnet(magnet, power)`.
    fn set_magnet(magnet: i32, power: f32);
    /// `set_zipper(flipper, closed)`.
    fn set_zipper(flipper: i32, closed: i32);
    /// `set_diverter(diverter, on)`.
    fn set_diverter(diverter: i32, on: i32);
    /// `spawn_ball(dispenser, profile)`.
    fn spawn_ball(dispenser: i32, profile: i32);
    /// `set_lamp(lamp, intensity)`.
    fn set_lamp(lamp: i32, intensity: f32);
    /// `set_gi_tint(r, g, b, a)`.
    fn set_gi_tint(r: f32, g: f32, b: f32, a: f32);
    /// `set_score(player, score)`.
    fn set_score(player: i32, score: i64);
    /// `show_scene(scene)`.
    fn show_scene(scene: i32);
    /// `play_sound(sound, gain)`.
    fn play_sound(sound: i32, gain: f32);
    /// `persist_set(key, value)`.
    fn persist_set(key: i32, value: i64);
    /// `persist_get(key)`.
    fn persist_get(key: i32) -> i64;
    /// `switch(id)`.
    fn switch(id: i32) -> i32;
    /// `tick()`.
    fn tick() -> i64;
    /// `balls_in_play()`.
    fn balls_in_play() -> i32;
    /// `random(m, n)`.
    fn random(m: i64, n: i64) -> i64;
    /// `id_of(kind, name_ptr, name_len)`.
    fn id_of(kind: i32, name_ptr: i32, name_len: i32) -> i32;
}

/// The name tables `id_of` searches (the `NameTables` order).
#[repr(i32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Switches (`pinball.sw`).
    Switch = 0,
    /// Coils.
    Coil = 1,
    /// Lamps.
    Lamp = 2,
    /// Magnets.
    Magnet = 3,
    /// Diverters.
    Diverter = 4,
    /// Flippers (for `set_zipper`).
    Flipper = 5,
    /// Ball dispensers.
    Dispenser = 6,
    /// Ball profiles.
    Profile = 7,
    /// Sounds.
    Sound = 8,
    /// Display scenes.
    Scene = 9,
    /// Persistent slots.
    PersistKey = 10,
}

/// Pulses `coil` for `ms` milliseconds.
pub fn pulse_coil(coil: u16, ms: u16) {
    raw::pulse_coil(i32::from(coil), i32::from(ms));
}

/// Holds (`on`) or releases `coil`.
pub fn hold_coil(coil: u16, on: bool) {
    raw::hold_coil(i32::from(coil), i32::from(on));
}

/// Sets a magnet's power (`0..=1`; anything else faults the rules).
pub fn set_magnet(magnet: u16, power: f32) {
    raw::set_magnet(i32::from(magnet), power);
}

/// Closes (`true`) or opens a flipper zipper.
pub fn set_zipper(flipper: u16, closed: bool) {
    raw::set_zipper(i32::from(flipper), i32::from(closed));
}

/// Switches a diverter.
pub fn set_diverter(diverter: u16, on: bool) {
    raw::set_diverter(i32::from(diverter), i32::from(on));
}

/// Spawns a ball of `profile` at `dispenser`.
pub fn spawn_ball(dispenser: u16, profile: u16) {
    raw::spawn_ball(i32::from(dispenser), i32::from(profile));
}

/// Sets a lamp's intensity (`0..=1`).
pub fn set_lamp(lamp: u16, intensity: f32) {
    raw::set_lamp(i32::from(lamp), intensity);
}

/// Sets the general-illumination tint (each `0..=1`).
pub fn set_gi_tint(r: f32, g: f32, b: f32, a: f32) {
    raw::set_gi_tint(r, g, b, a);
}

/// Sets the score of `player` (`1..=4`).
pub fn set_score(player: u8, score: i64) {
    raw::set_score(i32::from(player), score);
}

/// Shows a display scene.
pub fn show_scene(scene: u16) {
    raw::show_scene(i32::from(scene));
}

/// Plays a sound at `gain` (finite, `>= 0`).
pub fn play_sound(sound: u16, gain: f32) {
    raw::play_sound(i32::from(sound), gain);
}

/// Writes a persistent value (`key < 64`).
pub fn persist_set(key: u16, value: i64) {
    raw::persist_set(i32::from(key), value);
}

/// Reads a persistent value (`key < 64`).
pub fn persist_get(key: u16) -> i64 {
    raw::persist_get(i32::from(key))
}

/// True if switch `id` is closed.
pub fn switch(id: u16) -> bool {
    raw::switch(i32::from(id)) != 0
}

/// Sim time in ms (the tick number).
pub fn tick() -> i64 {
    raw::tick()
}

/// Balls currently in play.
pub fn balls_in_play() -> i32 {
    raw::balls_in_play()
}

/// A uniform integer in `[m, n]` from the host PCG (Lua's
/// `pinball.random(m, n)`); `m > n` faults the rules.
pub fn random(m: i64, n: i64) -> i64 {
    raw::random(m, n)
}

/// A uniform integer in `[1, n]` (Lua's `pinball.random(n)`).
pub fn random_to(n: i64) -> i64 {
    raw::random(1, n)
}

/// The id of `name` in table `kind`, or `None` if the table has no such name.
pub fn id_of(kind: Kind, name: &str) -> Option<u16> {
    // Guest pointers and lengths are 32-bit on wasm32.
    let id = raw::id_of(
        kind as i32,
        name.as_ptr() as usize as i32,
        name.len() as i32,
    );
    u16::try_from(id).ok()
}

/// `id_of(Kind::Switch, name)`.
pub fn sw(name: &str) -> Option<u16> {
    id_of(Kind::Switch, name)
}

/// `id_of(Kind::Coil, name)`.
pub fn coil(name: &str) -> Option<u16> {
    id_of(Kind::Coil, name)
}

/// `id_of(Kind::Lamp, name)`.
pub fn lamp(name: &str) -> Option<u16> {
    id_of(Kind::Lamp, name)
}

/// `id_of(Kind::Sound, name)`.
pub fn sound(name: &str) -> Option<u16> {
    id_of(Kind::Sound, name)
}

/// `id_of(Kind::PersistKey, name)`.
pub fn persist_key(name: &str) -> Option<u16> {
    id_of(Kind::PersistKey, name)
}

/// Packs a state blob's address and length as `save_state` returns them.
pub const fn pack_state(ptr: u32, len: u32) -> i64 {
    (((ptr as u64) << 32) | len as u64) as i64
}

/// The inverse of [`pack_state`].
pub const fn unpack_state(packed: i64) -> (u32, u32) {
    ((packed as u64 >> 32) as u32, packed as u64 as u32)
}

/// Table rules. Every method has an empty default.
pub trait Rules {
    /// Called once after the table loads.
    fn on_init(&mut self) {}
    /// Called every tick, after the tick's switch edges.
    fn on_tick(&mut self) {}
    /// Called for each switch edge.
    fn on_switch_edge(&mut self, _switch: u16, _closed: bool) {}
    /// State to carry into the next version of the rules on hot swap (at
    /// most [`MAX_STATE_BYTES`] bytes; longer blobs are not carried).
    fn save_state(&mut self) -> &[u8] {
        &[]
    }
    /// Receives the previous version's `save_state` blob after a hot swap.
    fn restore_state(&mut self, _blob: &[u8]) {}
}

/// A single-threaded global cell for the rules value (Wasm guests have one
/// thread and the host never re-enters an export).
pub struct RulesCell<T>(UnsafeCell<T>);

// SAFETY: wasm32-unknown-unknown guests are single-threaded and the host
// calls one export at a time, never re-entrantly.
unsafe impl<T> Sync for RulesCell<T> {}

impl<T> RulesCell<T> {
    /// Wraps the initial rules value.
    pub const fn new(v: T) -> Self {
        Self(UnsafeCell::new(v))
    }

    /// Mutable access from an export.
    ///
    /// # Safety
    /// No other reference from [`Self::get`] may be live (true for exports
    /// called one at a time by the host).
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn get(&self) -> &mut T {
        // SAFETY: guaranteed by the caller.
        unsafe { &mut *self.0.get() }
    }
}

/// The buffer `alloc_state` hands to the host for an incoming state blob.
pub struct StateBuffer(UnsafeCell<[u8; MAX_STATE_BYTES]>);

// SAFETY: as for `RulesCell`.
unsafe impl Sync for StateBuffer {}

impl Default for StateBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl StateBuffer {
    /// An empty buffer.
    pub const fn new() -> Self {
        Self(UnsafeCell::new([0; MAX_STATE_BYTES]))
    }

    /// The guest address of the buffer for a blob of `len` bytes, or −1 if
    /// `len` is negative or too large.
    pub fn alloc(&self, len: i32) -> i32 {
        match usize::try_from(len) {
            Ok(n) if n <= MAX_STATE_BYTES => self.0.get() as usize as i32,
            _ => -1,
        }
    }

    /// The first `len` bytes the host copied in (empty for a bad `len`).
    ///
    /// # Safety
    /// The host must not be writing the buffer (true inside an export).
    pub unsafe fn contents(&self, len: i32) -> &[u8] {
        // SAFETY: guaranteed by the caller.
        let buf = unsafe { &*self.0.get() };
        usize::try_from(len)
            .ok()
            .and_then(|n| buf.get(..n))
            .unwrap_or(&[])
    }
}

/// Exports a [`Rules`] implementation: `export_rules!(MyRules = MyRules::new());`
/// (the initialiser must be a constant expression). Defines the exports
/// `on_init`, `on_tick`, `on_switch_edge`, `save_state`, `alloc_state` and
/// `restore_state`.
#[macro_export]
macro_rules! export_rules {
    ($ty:ty = $init:expr) => {
        static __PIN_RULES: $crate::RulesCell<$ty> = $crate::RulesCell::new($init);
        static __PIN_STATE_IN: $crate::StateBuffer = $crate::StateBuffer::new();

        /// Export `on_init`.
        #[unsafe(no_mangle)]
        pub extern "C" fn on_init() {
            // SAFETY: exports run one at a time.
            $crate::Rules::on_init(unsafe { __PIN_RULES.get() });
        }

        /// Export `on_tick`.
        #[unsafe(no_mangle)]
        pub extern "C" fn on_tick() {
            // SAFETY: exports run one at a time.
            $crate::Rules::on_tick(unsafe { __PIN_RULES.get() });
        }

        /// Export `on_switch_edge`.
        #[unsafe(no_mangle)]
        pub extern "C" fn on_switch_edge(id: i32, closed: i32) {
            // SAFETY: exports run one at a time. The host passes ids in
            // 0..=65535.
            $crate::Rules::on_switch_edge(unsafe { __PIN_RULES.get() }, id as u16, closed != 0);
        }

        /// Export `save_state`.
        #[unsafe(no_mangle)]
        pub extern "C" fn save_state() -> i64 {
            // SAFETY: exports run one at a time.
            let blob = $crate::Rules::save_state(unsafe { __PIN_RULES.get() });
            $crate::pack_state(blob.as_ptr() as usize as u32, blob.len() as u32)
        }

        /// Export `alloc_state`.
        #[unsafe(no_mangle)]
        pub extern "C" fn alloc_state(len: i32) -> i32 {
            __PIN_STATE_IN.alloc(len)
        }

        /// Export `restore_state`.
        #[unsafe(no_mangle)]
        pub extern "C" fn restore_state(_ptr: i32, len: i32) {
            // SAFETY: exports run one at a time; the host wrote the blob into
            // the buffer `alloc_state` returned before this call.
            let blob = unsafe { __PIN_STATE_IN.contents(len) };
            $crate::Rules::restore_state(unsafe { __PIN_RULES.get() }, blob);
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_round_trip() {
        assert_eq!(pack_state(0x10, 5), 0x10_0000_0005);
        assert_eq!(
            unpack_state(pack_state(u32::MAX, u32::MAX)),
            (u32::MAX, u32::MAX)
        );
        assert_eq!(unpack_state(pack_state(1024, 0)), (1024, 0));
    }

    #[test]
    fn kinds_follow_name_table_order() {
        assert_eq!(Kind::Switch as i32, 0);
        assert_eq!(Kind::Profile as i32, 7);
        assert_eq!(Kind::PersistKey as i32, 10);
    }

    #[test]
    fn state_buffer_bounds() {
        let b = StateBuffer::default();
        assert_eq!(b.alloc(-1), -1);
        assert_eq!(b.alloc(MAX_STATE_BYTES as i32 + 1), -1);
        assert_ne!(b.alloc(0), -1);
        // SAFETY: nothing else touches `b`.
        unsafe {
            assert_eq!(b.contents(3), &[0, 0, 0]);
            assert!(b.contents(-1).is_empty());
            assert!(b.contents(MAX_STATE_BYTES as i32 + 1).is_empty());
        }
        let c = RulesCell::new(5u8);
        // SAFETY: single reference.
        unsafe {
            *c.get() += 1;
            assert_eq!(*c.get(), 6);
        }
    }

    #[test]
    #[should_panic(expected = "only available on wasm32")]
    fn imports_panic_on_host() {
        pulse_coil(1, 2);
    }
}
