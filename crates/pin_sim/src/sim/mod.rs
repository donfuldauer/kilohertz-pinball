//! The [`Simulation`] type, its configuration, errors and the test API
//! (state injection and queries).
//!
//! Owner: Subphase 2.5.

use arrayvec::ArrayVec;
use core::fmt;
use glam::Vec3;
use pin_assets::{AssetError, TableIds};
use pin_core::ids::{CoilId, DispenserId, DiverterId, FlipperId, MagnetId, ProfileId, SwitchId};
use pin_core::input::HapticsTelemetryEvent;
use pin_core::profiles::BUILTIN_PROFILE_NAMES;
use pin_core::script::MAX_SWITCHES;
use pin_core::snapshot::{BallState, FrameStateSnapshot, MAX_BALLS, MAX_FLIPPERS};
use pin_core::{HostCommandQueue, HostView, Pcg32, Rng, ScriptError, SwitchEdge};
use pin_input::{FlipperStagingRouter, InputIngest};
use pin_physics::ball::{BallEvents, BallSolver};
use pin_physics::ball_ext::Remanence;
use pin_physics::flipper::FlipperSet;
use pin_physics::mech::MechSet;
use pin_physics::world::{ColliderHandle, GuideCurve, StaticWorld, TransitionSet, Wireform};
use pin_script::lua::{LuaConfig, LuaScriptBackend};
use pin_script::wasm::{WasmConfig, WasmScriptBackend};

use crate::hash::Fnv64;

/// Players a table can report scores for (`HostCommandQueue::scores`
/// indices `0..MAX_PLAYERS`; Lua's `set_score` takes players 1..=4). Reports
/// for higher indices are ignored.
pub const MAX_PLAYERS: u8 = 4;

/// Default RNG seed of [`SimConfig`] (`0x4B485A`, "KHZ").
pub const DEFAULT_SEED: u64 = 0x4B_485A;

/// Capacity of the per-tick haptic event buffer ([`Simulation::haptic_events`]).
pub const HAPTICS_CAPACITY: usize = 32;

/// Capacity of the switch-edge queue between two script deliveries; edges
/// beyond it are dropped and counted ([`SimCounters::edges_dropped`]).
pub const MAX_PENDING_EDGES: usize = 512;

/// Simulation settings.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SimConfig {
    /// Seed of the sim-owned [`Pcg32`] the script draws from (recorded in
    /// replays).
    pub seed: u64,
    /// Lua limits; default [`LuaConfig::engine`] (100 000 instructions,
    /// 2 MiB per callback).
    pub lua: LuaConfig,
    /// Wasm limits; default [`WasmConfig::engine`].
    pub wasm: WasmConfig,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            seed: DEFAULT_SEED,
            lua: LuaConfig::engine(),
            wasm: WasmConfig::engine(),
        }
    }
}

/// Why a [`Simulation`] call failed.
#[derive(Clone, Debug, PartialEq)]
pub enum SimError {
    /// The bundle could not be opened, read or validated (the
    /// [`AssetError`] text is in [`Display`](fmt::Display)).
    Asset(AssetError),
    /// The rules script could not be loaded.
    Script(ScriptError),
    /// A table element could not be turned into physics (spline, flipper,
    /// mech, staging or Newton chain), with the element named.
    Build(String),
    /// No built-in ball profile has this name.
    UnknownProfile(String),
    /// The table declares no switch (or cabinet switch) with this name.
    UnknownSwitch(String),
    /// All [`MAX_BALLS`] ball slots are taken.
    BallArenaFull,
    /// A spawn with a non-finite position or velocity, or a layer above 7.
    InvalidBall(String),
}

impl fmt::Display for SimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SimError::Asset(e) => write!(f, "bundle error: {e}"),
            SimError::Script(e) => write!(f, "rules script error: {e}"),
            SimError::Build(m) => write!(f, "table build error: {m}"),
            SimError::UnknownProfile(p) => write!(f, "unknown ball profile {p:?}"),
            SimError::UnknownSwitch(s) => write!(f, "unknown switch {s:?}"),
            SimError::BallArenaFull => write!(f, "all {MAX_BALLS} ball slots are in use"),
            SimError::InvalidBall(m) => write!(f, "invalid ball: {m}"),
        }
    }
}

impl std::error::Error for SimError {}

impl From<AssetError> for SimError {
    fn from(e: AssetError) -> Self {
        SimError::Asset(e)
    }
}

impl From<ScriptError> for SimError {
    fn from(e: ScriptError) -> Self {
        SimError::Script(e)
    }
}

/// A ball on the table. Handles are not reused: once the ball drains or is
/// removed, [`Simulation::ball`] returns `None` for it even when a new ball
/// takes the same solver slot.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct BallHandle {
    pub(crate) slot: u8,
    pub(crate) generation: u32,
}

impl BallHandle {
    /// The solver slot (0..16) the ball occupies.
    #[must_use]
    pub fn slot(self) -> u8 {
        self.slot
    }
}

/// Things the simulation dropped or removed, since load.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct SimCounters {
    /// Switch edges dropped because [`MAX_PENDING_EDGES`] were waiting.
    pub edges_dropped: u32,
    /// Haptic events dropped because [`HAPTICS_CAPACITY`] were collected in
    /// one tick.
    pub haptics_dropped: u32,
    /// Input events dropped because the ingest buffer was full.
    pub inputs_dropped: u32,
    /// Script `spawn_ball` commands that found no free slot, no such
    /// dispenser or no such profile.
    pub spawns_failed: u32,
    /// Ball events lost because the per-tick event list was full.
    pub ball_events_dropped: u32,
    /// Balls removed by a trough switch (drained).
    pub balls_drained: u32,
    /// Balls removed for leaving the playfield bounds or turning non-finite.
    pub balls_escaped: u32,
}

/// The rules backend, kept concrete so its fault state and overflow counter
/// stay readable (DECISIONS.md "2.5 — script backend construction").
pub(crate) enum Script {
    Lua(Box<LuaScriptBackend>),
    Wasm(Box<WasmScriptBackend>),
}

/// The drive of one flipper coil: staging button, script hold, script pulse.
#[derive(Copy, Clone, Debug)]
pub(crate) struct FlipperDrive {
    pub id: FlipperId,
    pub coil: CoilId,
    pub eos_switch: SwitchId,
    pub staged: bool,
    pub held: bool,
    /// Energised for ticks `< pulse_until`.
    pub pulse_until: u64,
    pub energised: bool,
}

/// A diverter mech bound to collider nodes (DECISIONS.md "2.5 — diverters,
/// gates and Newton balls").
#[derive(Clone, Debug)]
pub(crate) struct DiverterBinding {
    pub id: DiverterId,
    pub on: Vec<ColliderHandle>,
    pub off: Vec<ColliderHandle>,
    pub active: bool,
}

/// A gate mech: solid colliders that its coil opens.
#[derive(Clone, Debug)]
pub(crate) struct GateBinding {
    pub coil: Option<CoilId>,
    pub switch: Option<SwitchId>,
    pub colliders: Vec<ColliderHandle>,
    pub held: bool,
    pub pulse_until: u64,
    pub open: bool,
}

/// A newton_ball mech: one captive ball on a short straight path.
#[derive(Copy, Clone, Debug)]
pub(crate) struct NewtonBinding {
    pub chain: usize,
    pub switch: Option<SwitchId>,
}

/// A trough switch zone: a ball whose centre enters it has drained.
#[derive(Copy, Clone, Debug)]
pub(crate) struct TroughZone {
    pub center: Vec3,
    pub radius: f32,
    pub layer_mask: u8,
}

/// A switch edge waiting for the script.
#[derive(Copy, Clone, Debug)]
pub(crate) struct PendingEdge {
    pub switch: SwitchId,
    pub edge: SwitchEdge,
    /// Tick in which the edge happened.
    pub tick: u64,
}

/// A deterministic, GPU-free table simulation (PLAN.md §2.2, Subphase 2.5).
///
/// [`load`](Simulation::load) builds it from a bundle and runs the script's
/// `on_init`; every [`step_1ms`](pin_core::sim::SimStep::step_1ms) (or
/// [`advance`](Simulation::advance)) runs the fixed tick order:
///
/// 1. drain the input events with timestamp ≤ the tick's end into the
///    ingest, update the cabinet switches and the flipper staging;
/// 2. deliver the switch edges collected since the last delivery to the
///    script (`on_switch_edge`, in the order they happened), then `on_tick`;
/// 3. apply the `HostCommandQueue` (coils, magnets, zippers, diverters,
///    spawns, lamps, scores, persistent values), then clear it;
/// 4. step physics: flippers (body set 0), mechs (body set 1), balls with
///    CCD sub-steps, the Magnus/remanence hooks, layer transitions, drains
///    and escapes;
/// 5. collect the new switch states and edges and the haptic events;
/// 6. publish the [`FrameStateSnapshot`].
///
/// Edges from step 1 (cabinet buttons) are delivered in step 2 of the same
/// tick; edges from step 5 and from [`set_switch`](Simulation::set_switch)
/// in step 2 of the next tick.
///
/// **Thread stack:** a table with a Wasm script calls into wasmtime, which
/// needs at least [`pin_script::wasm::MIN_HOST_STACK_BYTES`] (2 MiB) of
/// stack on the thread that steps the simulation (and loads it: `on_init`
/// runs in `load`). The default test-harness and main-thread stacks may be
/// smaller, so tests and the `pin_sim` binary run the simulation on a
/// spawned thread with a larger stack.
///
/// **Allocation:** stepping never allocates in Rust engine code (D5). The
/// Lua VM allocates in its capped arena when the script creates objects, and
/// now and then even when it does not: the collector shrinks the VM's
/// call-info list and the next call regrows it. Those allocations happen
/// inside [`ExemptScope`](crate::alloc_guard::ExemptScope)s around the Lua
/// calls (PLAN.md §2.3 rule 4; DECISIONS.md "2.5 — allocation guard and the
/// Lua arena"). Wasm calls are not exempt.
pub struct Simulation {
    pub(crate) cfg: SimConfig,
    pub(crate) ids: TableIds,
    pub(crate) table_hash: u64,
    pub(crate) world: StaticWorld,
    pub(crate) guides: Vec<GuideCurve>,
    pub(crate) wireforms: Vec<Wireform>,
    pub(crate) transitions: TransitionSet,
    pub(crate) solver: BallSolver,
    pub(crate) flippers: FlipperSet,
    pub(crate) drives: ArrayVec<FlipperDrive, MAX_FLIPPERS>,
    pub(crate) mechs: MechSet,
    pub(crate) remanence: Remanence,
    /// Table magnet id → solver magnet id.
    pub(crate) magnets: Vec<(MagnetId, MagnetId)>,
    /// Dispenser exits: id, position, velocity.
    pub(crate) dispensers: Vec<(DispenserId, Vec3, Vec3)>,
    pub(crate) diverters: Vec<DiverterBinding>,
    pub(crate) gates: Vec<GateBinding>,
    pub(crate) newtons: Vec<NewtonBinding>,
    pub(crate) troughs: Vec<TroughZone>,
    pub(crate) bounds: (Vec3, Vec3),
    pub(crate) ingest: InputIngest,
    pub(crate) router: FlipperStagingRouter,
    pub(crate) router_out: ArrayVec<(FlipperId, bool), 16>,
    /// Switch of each cabinet input, in `CabinetInput::ALL` order.
    pub(crate) cabinet: [SwitchId; 8],
    pub(crate) script: Script,
    pub(crate) view: HostView,
    pub(crate) rng: Pcg32,
    pub(crate) cmd: HostCommandQueue,
    pub(crate) pending: ArrayVec<PendingEdge, MAX_PENDING_EDGES>,
    pub(crate) cab_bits: [u64; 4],
    pub(crate) phys_bits: [u64; 4],
    pub(crate) forced_mask: [u64; 4],
    pub(crate) forced_val: [u64; 4],
    pub(crate) switch_hits: [u32; MAX_SWITCHES],
    pub(crate) scores: [i64; 256],
    pub(crate) scores_reported: [u64; 4],
    pub(crate) lamps: [f32; 256],
    pub(crate) gi: [f32; 4],
    pub(crate) snapshot: FrameStateSnapshot,
    pub(crate) events: BallEvents,
    pub(crate) haptics: ArrayVec<HapticsTelemetryEvent, HAPTICS_CAPACITY>,
    pub(crate) generations: [u32; MAX_BALLS],
    pub(crate) prev_positions: [Vec3; MAX_BALLS],
    pub(crate) tick: u64,
    pub(crate) counters: SimCounters,
}

pub(crate) fn bit(bits: &[u64; 4], n: usize) -> bool {
    n < MAX_SWITCHES && bits[n / 64] & (1u64 << (n % 64)) != 0
}

pub(crate) fn set_bit(bits: &mut [u64; 4], n: usize, on: bool) {
    if n < MAX_SWITCHES {
        let b = 1u64 << (n % 64);
        if on {
            bits[n / 64] |= b;
        } else {
            bits[n / 64] &= !b;
        }
    }
}

fn check_ball_input(layer: u8, p: [f32; 3], v: [f32; 3]) -> Result<(), SimError> {
    if layer > 7 {
        return Err(SimError::InvalidBall(format!("layer {layer} (0..=7)")));
    }
    if p.iter().chain(&v).any(|x| !x.is_finite()) {
        return Err(SimError::InvalidBall(
            "position and velocity must be finite".into(),
        ));
    }
    Ok(())
}

impl Simulation {
    /// The configuration it was loaded with.
    #[must_use]
    pub fn config(&self) -> SimConfig {
        self.cfg
    }

    /// The last completed tick (0 before the first).
    #[must_use]
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// Spawns a ball of the built-in `profile` (e.g. `"standard_steel"`) on
    /// layer 0 at `position_m` (centre, table frame, m) with `velocity_m_s`.
    ///
    /// # Errors
    /// [`SimError::UnknownProfile`], [`SimError::BallArenaFull`],
    /// [`SimError::InvalidBall`] for non-finite input.
    pub fn spawn_ball(
        &mut self,
        profile: &str,
        position_m: [f32; 3],
        velocity_m_s: [f32; 3],
    ) -> Result<BallHandle, SimError> {
        self.spawn_ball_on_layer(profile, 0, position_m, velocity_m_s)
    }

    /// [`spawn_ball`](Self::spawn_ball) on collision layer `layer` (0..=7).
    ///
    /// # Errors
    /// As [`spawn_ball`](Self::spawn_ball); a layer above 7 is
    /// [`SimError::InvalidBall`].
    pub fn spawn_ball_on_layer(
        &mut self,
        profile: &str,
        layer: u8,
        position_m: [f32; 3],
        velocity_m_s: [f32; 3],
    ) -> Result<BallHandle, SimError> {
        let id = profile_id(profile).ok_or_else(|| SimError::UnknownProfile(profile.into()))?;
        check_ball_input(layer, position_m, velocity_m_s)?;
        if self.solver.ball_count() >= MAX_BALLS {
            return Err(SimError::BallArenaFull);
        }
        let h =
            pin_physics::debug::spawn_ball(&mut self.solver, id, layer, position_m, velocity_m_s)
                .ok_or(SimError::BallArenaFull)?;
        Ok(self.adopt(h))
    }

    /// Registers a ball the solver just added: new generation, previous
    /// position for the transition test.
    pub(crate) fn adopt(&mut self, h: pin_physics::ball::BallHandle) -> BallHandle {
        let slot = usize::from(h.0);
        self.generations[slot] = self.generations[slot].wrapping_add(1);
        if let Some(b) = self.solver.ball(h) {
            self.prev_positions[slot] = b.position;
        }
        BallHandle {
            slot: h.0,
            generation: self.generations[slot],
        }
    }

    /// Removes the ball in solver slot `slot` (its handle goes stale).
    pub(crate) fn remove_slot(&mut self, slot: usize) {
        let h = pin_physics::ball::BallHandle(slot as u8);
        if self.solver.remove_ball(h).is_some() {
            self.generations[slot] = self.generations[slot].wrapping_add(1);
        }
    }

    fn physics_handle(&self, h: BallHandle) -> Option<pin_physics::ball::BallHandle> {
        let slot = usize::from(h.slot);
        let ph = pin_physics::ball::BallHandle(h.slot);
        (slot < MAX_BALLS
            && self.generations[slot] == h.generation
            && self.solver.ball(ph).is_some())
        .then_some(ph)
    }

    /// The state of ball `h`; `None` once it drained or was removed.
    #[must_use]
    pub fn ball(&self, h: BallHandle) -> Option<BallState> {
        let ph = self.physics_handle(h)?;
        pin_physics::debug::ball_state(&self.solver, ph)
    }

    /// Overwrites the full state of ball `h` (position, orientation,
    /// velocities, profile, layer). Returns `false` for a stale handle or an
    /// invalid state (non-finite, unknown profile, layer above 7).
    pub fn set_ball_state(&mut self, h: BallHandle, state: &BallState) -> bool {
        match self.physics_handle(h) {
            Some(ph) => {
                let ok = pin_physics::debug::set_ball_state(&mut self.solver, ph, state);
                if ok {
                    self.prev_positions[usize::from(h.slot)] = Vec3::from_array(state.position_m);
                }
                ok
            }
            None => false,
        }
    }

    /// Removes ball `h`; `false` when it is already gone.
    pub fn remove_ball(&mut self, h: BallHandle) -> bool {
        match self.physics_handle(h) {
            Some(_) => {
                self.remove_slot(usize::from(h.slot));
                true
            }
            None => false,
        }
    }

    /// Handles of every ball on the table, ascending slot.
    #[must_use]
    pub fn ball_handles(&self) -> ArrayVec<BallHandle, MAX_BALLS> {
        self.solver
            .balls()
            .map(|(h, _)| BallHandle {
                slot: h.0,
                generation: self.generations[usize::from(h.0)],
            })
            .collect()
    }

    /// Balls on the table.
    #[must_use]
    pub fn balls_in_play(&self) -> usize {
        self.solver.ball_count()
    }

    /// Forces switch `name` (a `[[switches]]` id or a `[cabinet_switches]`
    /// key such as `"start"`) closed or open until
    /// [`release_switch`](Self::release_switch). The state changes at once;
    /// a change is an edge delivered to the script at the next tick and
    /// counted by [`switch_hits`](Self::switch_hits) when closing. A forced
    /// cabinet switch also acts as its button for flipper staging.
    ///
    /// # Errors
    /// [`SimError::UnknownSwitch`].
    pub fn set_switch(&mut self, name: &str, closed: bool) -> Result<(), SimError> {
        let n = self.switch_number(name)?;
        set_bit(&mut self.forced_mask, n, true);
        set_bit(&mut self.forced_val, n, closed);
        self.update_switches(self.tick);
        Ok(())
    }

    /// Ends a [`set_switch`](Self::set_switch) override: the switch follows
    /// its input or physics again (from now on).
    ///
    /// # Errors
    /// [`SimError::UnknownSwitch`].
    pub fn release_switch(&mut self, name: &str) -> Result<(), SimError> {
        let n = self.switch_number(name)?;
        set_bit(&mut self.forced_mask, n, false);
        self.update_switches(self.tick);
        Ok(())
    }

    fn switch_number(&self, name: &str) -> Result<usize, SimError> {
        self.switch_id(name)
            .map(SwitchId::index)
            .filter(|&n| n < MAX_SWITCHES)
            .ok_or_else(|| SimError::UnknownSwitch(name.into()))
    }

    /// The id of switch `name`.
    #[must_use]
    pub fn switch_id(&self, name: &str) -> Option<SwitchId> {
        self.ids.switch(name)
    }

    /// Runs `ticks` ticks with no input events.
    pub fn advance(&mut self, ticks: u64) {
        for _ in 0..ticks {
            let t = self.tick + 1;
            self.step_tick(t, &[]);
        }
    }

    /// The last score the script reported for `player`, numbered 0-based
    /// like `HostCommandQueue::scores` (pin_core is the contract): Lua's
    /// `pinball.set_score(1, …)` is `score(0)`. `None` before any report for
    /// that player and for `player >= MAX_PLAYERS`.
    #[must_use]
    pub fn score(&self, player: u8) -> Option<i64> {
        let i = usize::from(player);
        (player < MAX_PLAYERS && bit(&self.scores_reported, i)).then(|| self.scores[i])
    }

    /// Whether switch `name` is closed (`None` for an unknown name).
    #[must_use]
    pub fn switch_closed(&self, name: &str) -> Option<bool> {
        let n = self.switch_number(name).ok()?;
        Some(bit(&self.view.switches, n))
    }

    /// Closing edges of switch `name` since load (`None` for an unknown
    /// name).
    #[must_use]
    pub fn switch_hits(&self, name: &str) -> Option<u32> {
        let n = self.switch_number(name).ok()?;
        Some(self.switch_hits[n])
    }

    /// The 256-bit switch state the script sees.
    #[must_use]
    pub fn switch_bits(&self) -> [u64; 4] {
        self.view.switches
    }

    /// The snapshot published by the last tick (after `load`: the state
    /// after `on_init`, tick 0).
    #[must_use]
    pub fn snapshot(&self) -> &FrameStateSnapshot {
        &self.snapshot
    }

    /// Haptic impulses of the last tick (coil fires and flipper energise
    /// edges; `source` = coil id, strength 1). Subphase 6.0 drains them to
    /// the haptics bus.
    #[must_use]
    pub fn haptic_events(&self) -> &[HapticsTelemetryEvent] {
        &self.haptics
    }

    /// Drop and removal counters.
    #[must_use]
    pub fn counters(&self) -> SimCounters {
        self.counters
    }

    /// Script commands dropped because a `HostCommandQueue` list was full
    /// (the backend's counter, §3.4).
    #[must_use]
    pub fn script_queue_overflows(&self) -> u32 {
        match &self.script {
            Script::Lua(b) => b.overflow_count(),
            Script::Wasm(b) => b.overflow_count(),
        }
    }

    /// Whether the script is in its safe "tilt" state after a fault (it is
    /// no longer called; ticks continue).
    #[must_use]
    pub fn script_faulted(&self) -> bool {
        match &self.script {
            Script::Lua(b) => b.is_faulted(),
            Script::Wasm(b) => b.is_faulted(),
        }
    }

    /// Script faults since load.
    #[must_use]
    pub fn script_fault_count(&self) -> u32 {
        match &self.script {
            Script::Lua(b) => b.fault_count(),
            Script::Wasm(b) => b.fault_count(),
        }
    }

    /// The error behind the last script fault.
    #[must_use]
    pub fn script_error(&self) -> Option<&ScriptError> {
        match &self.script {
            Script::Lua(b) => b.last_error(),
            Script::Wasm(b) => b.last_error(),
        }
    }

    /// The persistent value at `key` (0 when out of range).
    #[must_use]
    pub fn persist(&self, key: u16) -> i64 {
        self.view
            .persist
            .get(usize::from(key))
            .copied()
            .unwrap_or(0)
    }

    /// Magnetisation `M` of ball `h` (Subphase 2.7 remanence; 0 when gone).
    #[must_use]
    pub fn magnetisation(&self, h: BallHandle) -> f32 {
        self.physics_handle(h)
            .map_or(0.0, |ph| self.remanence.magnetisation(ph))
    }

    /// Flipper `name`'s angle relative to rest (rad) and angular velocity
    /// (rad/s), signed like its sweep.
    #[must_use]
    pub fn flipper(&self, name: &str) -> Option<(f32, f32)> {
        let id = self.ids.flipper(name)?;
        self.flippers.body_index(id)?;
        Some((
            self.flippers.relative_angle_rad(id),
            self.flippers.angular_velocity(id),
        ))
    }

    /// The physics state for the debug HUD and white-box tests.
    #[must_use]
    pub fn solver(&self) -> &BallSolver {
        &self.solver
    }

    /// The flipper set.
    #[must_use]
    pub fn flipper_set(&self) -> &FlipperSet {
        &self.flippers
    }

    /// The mech set.
    #[must_use]
    pub fn mech_set(&self) -> &MechSet {
        &self.mechs
    }

    /// The table's interned names.
    #[must_use]
    pub fn ids(&self) -> &TableIds {
        &self.ids
    }

    /// FNV-1a 64 of the bundle's files as loaded (sorted paths; for each:
    /// path bytes, length, contents), so a folder and a zip with the same
    /// files hash alike.
    #[must_use]
    pub fn table_hash(&self) -> u64 {
        self.table_hash
    }

    /// FNV-1a 64 ([`crate::hash`]) over, in this order: the tick; for each
    /// of the 16 ball slots a presence byte and, for a ball, the exact `f32`
    /// bits of position, velocity, angular velocity and orientation, its
    /// layer and profile; the 256 switch-hit counts; the switch bitset;
    /// every reported score (queue player index and value, ascending); every
    /// flipper's relative angle and angular velocity (body order); and the
    /// RNG state (the next `u64` a clone of the sim's `Pcg32` would draw).
    #[must_use]
    pub fn state_hash(&self) -> u64 {
        let mut h = Fnv64::new();
        h.write_u64(self.tick);
        for slot in 0..MAX_BALLS {
            match self.solver.ball(pin_physics::ball::BallHandle(slot as u8)) {
                Some(b) => {
                    h.write_u8(1);
                    for v in [b.position, b.velocity, b.angular_velocity] {
                        h.write_f32(v.x);
                        h.write_f32(v.y);
                        h.write_f32(v.z);
                    }
                    let q = b.orientation;
                    for c in [q.x, q.y, q.z, q.w] {
                        h.write_f32(c);
                    }
                    h.write_u8(b.layer);
                    h.write(&b.profile.0.to_le_bytes());
                }
                None => h.write_u8(0),
            }
        }
        for &n in &self.switch_hits {
            h.write_u32(n);
        }
        for w in self.view.switches {
            h.write_u64(w);
        }
        for p in 0..256usize {
            if bit(&self.scores_reported, p) {
                h.write_u8(p as u8);
                h.write_i64(self.scores[p]);
            }
        }
        for d in &self.drives {
            h.write_f32(self.flippers.relative_angle_rad(d.id));
            h.write_f32(self.flippers.angular_velocity(d.id));
        }
        h.write_u64(self.rng.clone().next_u64());
        h.finish()
    }
}

/// The id of a built-in profile name (index in `BUILTIN_PROFILE_NAMES`).
pub(crate) fn profile_id(name: &str) -> Option<ProfileId> {
    BUILTIN_PROFILE_NAMES
        .iter()
        .position(|&n| n == name)
        .map(|i| ProfileId(i as u16))
}
