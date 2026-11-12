//! Scripting contract (PLAN.md §3.4, D5, D10, D11, D12).
//!
//! Scripts read the world through [`HostView`], draw randomness from the
//! sim-owned [`Pcg32`], and act only by pushing commands into the fixed-capacity
//! [`HostCommandQueue`], which the sim applies and clears every tick. Nothing
//! here allocates after construction.

use crate::Pcg32;
use crate::ids::{
    CoilId, DispenserId, DiverterId, FlipperId, LampId, MagnetId, PersistKey, ProfileId, SceneId,
    SoundId, SwitchId,
};
use arrayvec::ArrayVec;
use core::fmt;

/// A switch transition delivered to the script.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SwitchEdge {
    /// The switch closed (ball present / button pressed).
    Closed,
    /// The switch opened.
    Opened,
}

/// Number of switches tracked in [`HostView::switches`].
pub const MAX_SWITCHES: usize = 256;
/// Number of persistent slots in [`HostView::persist`].
pub const MAX_PERSIST: usize = 64;

/// Read-only world state visible to the script.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct HostView {
    /// Sim time, ms (= tick number).
    pub tick: u64,
    /// Switch states, a 256-bit bitset indexed by `SwitchId`.
    pub switches: [u64; 4],
    /// Balls currently in play.
    pub balls_in_play: u8,
    /// Persistent values (high scores, audits, settings), indexed by `PersistKey`.
    pub persist: [i64; MAX_PERSIST],
}

impl Default for HostView {
    fn default() -> Self {
        Self {
            tick: 0,
            switches: [0; 4],
            balls_in_play: 0,
            persist: [0; MAX_PERSIST],
        }
    }
}

impl HostView {
    /// True if `switch` is closed. Ids ≥ 256 are reported open.
    #[inline]
    pub fn switch_closed(&self, switch: SwitchId) -> bool {
        let i = switch.index();
        i < MAX_SWITCHES && self.switches[i / 64] & (1u64 << (i % 64)) != 0
    }

    /// Sets the state of `switch`; ids ≥ 256 are ignored.
    #[inline]
    pub fn set_switch(&mut self, switch: SwitchId, closed: bool) {
        let i = switch.index();
        if i < MAX_SWITCHES {
            let bit = 1u64 << (i % 64);
            if closed {
                self.switches[i / 64] |= bit;
            } else {
                self.switches[i / 64] &= !bit;
            }
        }
    }

    /// The persistent value at `key`, or 0 when out of range.
    #[inline]
    pub fn persist_value(&self, key: PersistKey) -> i64 {
        self.persist.get(key.index()).copied().unwrap_or(0)
    }
}

/// A command did not fit in its fixed-capacity list and was dropped (§3.4).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct QueueOverflow;

impl fmt::Display for QueueOverflow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("host command queue full; command dropped")
    }
}

impl std::error::Error for QueueOverflow {}

/// Commands a script issues during one callback, applied by the sim in tick
/// step 3 and then cleared. Capacities are fixed (D5).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HostCommandQueue {
    /// `(coil, duration_ms)`.
    pub coil_pulses: ArrayVec<(CoilId, u16), 32>,
    /// `(coil, on)`.
    pub coil_holds: ArrayVec<(CoilId, bool), 32>,
    /// `(magnet, power 0..=1)`.
    pub magnets: ArrayVec<(MagnetId, f32), 8>,
    /// `(flipper, extended)`.
    pub zippers: ArrayVec<(FlipperId, bool), 8>,
    /// `(diverter, active)`.
    pub diverters: ArrayVec<(DiverterId, bool), 16>,
    /// `(dispenser, profile)`.
    pub ball_spawns: ArrayVec<(DispenserId, ProfileId), 8>,
    /// `(lamp, intensity 0..=1)`.
    pub lamps: ArrayVec<(LampId, f32), 256>,
    /// General-illumination tint `[r, g, b, intensity]`.
    pub gi_tint: Option<[f32; 4]>,
    /// `(player, absolute score)`.
    pub scores: ArrayVec<(u8, i64), 4>,
    /// Display scenes to show.
    pub display_scenes: ArrayVec<SceneId, 8>,
    /// `(sound, gain)`.
    pub sounds: ArrayVec<(SoundId, f32), 16>,
    /// `(key, value)`.
    pub persist_writes: ArrayVec<(PersistKey, i64), 8>,
}

fn push<T, const N: usize>(list: &mut ArrayVec<T, N>, item: T) -> Result<(), QueueOverflow> {
    list.try_push(item).map_err(|_| QueueOverflow)
}

impl HostCommandQueue {
    /// Queues a coil pulse of `duration_ms`.
    pub fn pulse_coil(&mut self, coil: CoilId, duration_ms: u16) -> Result<(), QueueOverflow> {
        push(&mut self.coil_pulses, (coil, duration_ms))
    }

    /// Queues a coil hold on/off.
    pub fn hold_coil(&mut self, coil: CoilId, on: bool) -> Result<(), QueueOverflow> {
        push(&mut self.coil_holds, (coil, on))
    }

    /// Queues a magnet power change.
    pub fn set_magnet(&mut self, magnet: MagnetId, power: f32) -> Result<(), QueueOverflow> {
        push(&mut self.magnets, (magnet, power))
    }

    /// Queues a zipper extend/retract.
    pub fn set_zipper(&mut self, flipper: FlipperId, extended: bool) -> Result<(), QueueOverflow> {
        push(&mut self.zippers, (flipper, extended))
    }

    /// Queues a diverter change.
    pub fn set_diverter(&mut self, diverter: DiverterId, on: bool) -> Result<(), QueueOverflow> {
        push(&mut self.diverters, (diverter, on))
    }

    /// Queues a ball spawn.
    pub fn spawn_ball(
        &mut self,
        dispenser: DispenserId,
        profile: ProfileId,
    ) -> Result<(), QueueOverflow> {
        push(&mut self.ball_spawns, (dispenser, profile))
    }

    /// Queues a lamp intensity.
    pub fn set_lamp(&mut self, lamp: LampId, intensity: f32) -> Result<(), QueueOverflow> {
        push(&mut self.lamps, (lamp, intensity))
    }

    /// Sets the GI tint (last write wins; never overflows).
    pub fn set_gi_tint(&mut self, tint: [f32; 4]) {
        self.gi_tint = Some(tint);
    }

    /// Reports a player's absolute score.
    pub fn set_score(&mut self, player: u8, score: i64) -> Result<(), QueueOverflow> {
        push(&mut self.scores, (player, score))
    }

    /// Queues a display scene.
    pub fn show_scene(&mut self, scene: SceneId) -> Result<(), QueueOverflow> {
        push(&mut self.display_scenes, scene)
    }

    /// Queues a sound.
    pub fn play_sound(&mut self, sound: SoundId, gain: f32) -> Result<(), QueueOverflow> {
        push(&mut self.sounds, (sound, gain))
    }

    /// Queues a persistent write.
    pub fn write_persist(&mut self, key: PersistKey, value: i64) -> Result<(), QueueOverflow> {
        push(&mut self.persist_writes, (key, value))
    }

    /// Total queued commands (GI tint counts as one).
    pub fn len(&self) -> usize {
        self.coil_pulses.len()
            + self.coil_holds.len()
            + self.magnets.len()
            + self.zippers.len()
            + self.diverters.len()
            + self.ball_spawns.len()
            + self.lamps.len()
            + usize::from(self.gi_tint.is_some())
            + self.scores.len()
            + self.display_scenes.len()
            + self.sounds.len()
            + self.persist_writes.len()
    }

    /// True when nothing is queued.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Empties every list without releasing storage (none is heap-allocated).
    pub fn clear(&mut self) {
        self.coil_pulses.clear();
        self.coil_holds.clear();
        self.magnets.clear();
        self.zippers.clear();
        self.diverters.clear();
        self.ball_spawns.clear();
        self.lamps.clear();
        self.gi_tint = None;
        self.scores.clear();
        self.display_scenes.clear();
        self.sounds.clear();
        self.persist_writes.clear();
    }
}

/// Everything a script callback receives.
pub struct HostCtx<'a> {
    /// Read-only world state.
    pub view: &'a HostView,
    /// Sim-owned, seeded generator, recorded in replays.
    pub rng: &'a mut Pcg32,
    /// Command output.
    pub cmd: &'a mut HostCommandQueue,
}

/// A table rules backend (Lua 5.4 or Wasm). Selected by the extension of
/// `script_entry` in `table.toml`.
pub trait TableScriptBackend: Send {
    /// Called once after the table loads.
    fn on_init(&mut self, ctx: &mut HostCtx);
    /// Called every tick (tick step 2), after switch edges.
    fn on_tick(&mut self, ctx: &mut HostCtx);
    /// Called for each switch edge collected in the previous tick.
    fn on_switch_edge(&mut self, switch: SwitchId, edge: SwitchEdge, ctx: &mut HostCtx);
    /// Called on the sim thread at a tick boundary with source already read by the I/O worker.
    fn apply_reload(&mut self, source: &[u8]) -> Result<(), ScriptError>;
}

/// Script failures. A failed script drops into a safe "tilt" state (§3.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScriptError {
    /// Source failed to compile or load.
    Load(String),
    /// A callback raised an error.
    Runtime(String),
    /// The per-callback instruction (or fuel) budget was exhausted.
    BudgetExceeded,
    /// The script exceeded its memory cap.
    MemoryLimit,
    /// The entry file's extension names no known backend.
    UnsupportedBackend(String),
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScriptError::Load(m) => write!(f, "script load error: {m}"),
            ScriptError::Runtime(m) => write!(f, "script runtime error: {m}"),
            ScriptError::BudgetExceeded => f.write_str("script instruction budget exceeded"),
            ScriptError::MemoryLimit => f.write_str("script memory limit exceeded"),
            ScriptError::UnsupportedBackend(e) => write!(f, "unsupported script backend: {e}"),
        }
    }
}

impl std::error::Error for ScriptError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SeedableRng;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn set_then_get_leaves_other_bits(bits in any::<[u64; 4]>(), id in any::<u16>(),
                                          closed in any::<bool>()) {
            let mut v = HostView { switches: bits, ..HostView::default() };
            v.set_switch(SwitchId(id), closed);
            if usize::from(id) < MAX_SWITCHES {
                prop_assert_eq!(v.switch_closed(SwitchId(id)), closed);
                let i = usize::from(id);
                for (w, (&got, &orig)) in v.switches.iter().zip(bits.iter()).enumerate() {
                    let mask = if w == i / 64 { !(1u64 << (i % 64)) } else { !0 };
                    prop_assert_eq!(got & mask, orig & mask);
                }
            } else {
                prop_assert_eq!(v.switches, bits);
                prop_assert!(!v.switch_closed(SwitchId(id)));
            }
        }

        #[test]
        fn switch_closed_reads_the_bitset(bits in any::<[u64; 4]>(), id in 0u16..256) {
            let v = HostView { switches: bits, ..HostView::default() };
            let i = usize::from(id);
            prop_assert_eq!(v.switch_closed(SwitchId(id)), bits[i / 64] >> (i % 64) & 1 == 1);
        }
    }

    #[test]
    fn switch_bitset() {
        let mut v = HostView::default();
        for i in [0u16, 63, 64, 200, 255] {
            assert!(!v.switch_closed(SwitchId(i)));
            v.set_switch(SwitchId(i), true);
            assert!(v.switch_closed(SwitchId(i)));
        }
        v.set_switch(SwitchId(64), false);
        assert!(!v.switch_closed(SwitchId(64)));
        assert!(v.switch_closed(SwitchId(63)));
        v.set_switch(SwitchId(300), true);
        assert!(!v.switch_closed(SwitchId(300)));
        v.persist[3] = 42;
        assert_eq!(v.persist_value(PersistKey(3)), 42);
        assert_eq!(v.persist_value(PersistKey(999)), 0);
    }

    #[test]
    fn queue_capacities_and_overflow() {
        let mut q = HostCommandQueue::default();
        assert!(q.is_empty());
        for i in 0..32 {
            q.pulse_coil(CoilId(i), 10).unwrap();
            q.hold_coil(CoilId(i), true).unwrap();
        }
        assert_eq!(q.pulse_coil(CoilId(0), 1), Err(QueueOverflow));
        assert_eq!(q.hold_coil(CoilId(0), true), Err(QueueOverflow));
        for i in 0..8 {
            q.set_magnet(MagnetId(i), 0.5).unwrap();
            q.set_zipper(FlipperId(i), true).unwrap();
            q.spawn_ball(DispenserId(i), ProfileId(0)).unwrap();
            q.show_scene(SceneId(i)).unwrap();
            q.write_persist(PersistKey(i), 1).unwrap();
        }
        assert!(q.set_magnet(MagnetId(0), 1.0).is_err());
        assert!(q.set_zipper(FlipperId(0), false).is_err());
        assert!(q.spawn_ball(DispenserId(0), ProfileId(0)).is_err());
        assert!(q.show_scene(SceneId(0)).is_err());
        assert!(q.write_persist(PersistKey(0), 0).is_err());
        for i in 0..16 {
            q.set_diverter(DiverterId(i), true).unwrap();
            q.play_sound(SoundId(i), 1.0).unwrap();
        }
        assert!(q.set_diverter(DiverterId(0), true).is_err());
        assert!(q.play_sound(SoundId(0), 1.0).is_err());
        for i in 0..256 {
            q.set_lamp(LampId(i), 1.0).unwrap();
        }
        assert!(q.set_lamp(LampId(0), 1.0).is_err());
        for p in 0..4 {
            q.set_score(p, 1000).unwrap();
        }
        assert!(q.set_score(4, 0).is_err());
        q.set_gi_tint([1.0; 4]);
        assert_eq!(q.len(), 32 * 2 + 8 * 5 + 16 * 2 + 256 + 4 + 1);
        q.clear();
        assert!(q.is_empty());
        assert_eq!(q, HostCommandQueue::default());
        assert!(!QueueOverflow.to_string().is_empty());
    }

    struct Echo {
        ticks: u32,
    }

    impl TableScriptBackend for Echo {
        fn on_init(&mut self, ctx: &mut HostCtx) {
            ctx.cmd.set_score(0, 0).unwrap();
        }
        fn on_tick(&mut self, ctx: &mut HostCtx) {
            self.ticks += 1;
            let r = crate::Rng::next_u32(ctx.rng);
            ctx.cmd.set_score(0, i64::from(r % 10)).unwrap();
        }
        fn on_switch_edge(&mut self, switch: SwitchId, edge: SwitchEdge, ctx: &mut HostCtx) {
            if edge == SwitchEdge::Closed && ctx.view.switch_closed(switch) {
                ctx.cmd.pulse_coil(CoilId(switch.0), 20).unwrap();
            }
        }
        fn apply_reload(&mut self, source: &[u8]) -> Result<(), ScriptError> {
            if source.is_empty() {
                Err(ScriptError::Load("empty".into()))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn backend_is_object_safe_and_send() {
        let mut view = HostView::default();
        view.set_switch(SwitchId(7), true);
        let mut rng = Pcg32::seed_from_u64(1);
        let mut cmd = HostCommandQueue::default();
        let mut backend: Box<dyn TableScriptBackend> = Box::new(Echo { ticks: 0 });
        let mut ctx = HostCtx {
            view: &view,
            rng: &mut rng,
            cmd: &mut cmd,
        };
        backend.on_init(&mut ctx);
        backend.on_tick(&mut ctx);
        backend.on_switch_edge(SwitchId(7), SwitchEdge::Closed, &mut ctx);
        assert_eq!(cmd.coil_pulses.as_slice(), &[(CoilId(7), 20)]);
        assert_eq!(cmd.scores.len(), 2);
        assert!(backend.apply_reload(b"").is_err());
        assert!(backend.apply_reload(b"x").is_ok());
        fn assert_send<T: Send + ?Sized>() {}
        assert_send::<dyn TableScriptBackend>();
    }

    #[test]
    fn script_error_display() {
        for e in [
            ScriptError::Load("a".into()),
            ScriptError::Runtime("b".into()),
            ScriptError::BudgetExceeded,
            ScriptError::MemoryLimit,
            ScriptError::UnsupportedBackend(".py".into()),
        ] {
            assert!(!e.to_string().is_empty());
        }
    }
}
