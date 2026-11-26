//! Loading a table bundle into the engine (Subphase 6.0): the bundle is
//! copied once into an immutable in-memory [`BundleSnapshot`]; manifest
//! validation and the 2.8 clearance validator (every declared ball profile)
//! run on that snapshot, and [`Simulation::load_snapshot`] builds the
//! simulation from the same snapshot, so a bundle replaced on disk between
//! the two steps can never skip validation. [`TableSim`] is the [`SimStep`]
//! the sim thread drives; it also hands the tick's haptic impulses, sound
//! commands and display commands (scenes, score reports) to the audio and
//! render threads through wait-free rings, the impulses to the main
//! thread's rumble driver, and each tick's cabinet state to the UDP
//! telemetry encoder (KP-006).

use super::audio::{RegisteredSound, TelemetryFeed, TelemetryMap, TelemetrySources, table_samples};
use super::trace::FlipperBindings;
use super::view::TableVisuals;
use crate::ui::{TableEntry, scan_tables};
use pin_assets::{GltfScene, LoadedTable, load_gltf, load_table};
use pin_audio::mixer::{
    DecodeBudget, MixerConfig, SoundReceiver, SoundSender, SoundTrigger, trigger_ring,
};
use pin_core::haptics::HapticEvent;
use pin_core::input::HapticsTelemetryEvent;
use pin_core::schema::table::TableManifest;
use pin_core::{CoilId, FlipperId, FrameStateSnapshot, InputEvent, SceneId, SimStep};
use pin_input::haptics::{
    DEFAULT_HAPTICS_RING_CAPACITY, HapticsConsumer, HapticsProducer, haptics_ring,
};
use pin_render::debug::DebugLine;
use pin_sim::load::BundleSnapshot;
use pin_sim::{SimConfig, Simulation};
use pin_tablegen::clearance::{
    ClearanceError, load_backbox_colliders, validate_clearance_with_backbox,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;

/// A bundle that passed [`validate_table`].
pub struct ValidatedTable {
    /// The validated manifests (SI).
    pub table: LoadedTable,
    /// The playfield glTF.
    pub scene: GltfScene,
    /// Non-fatal findings: manifest warnings (`file: path: message`), then
    /// clearance warnings (feature, name, profile and the finding).
    pub warnings: Vec<String>,
    /// The in-memory copy of the bundle that was validated; build the
    /// simulation from it ([`load_checked`]), never from the path again.
    pub snapshot: BundleSnapshot,
}

fn clearance_line(e: &ClearanceError) -> String {
    format!(
        "{} {} ({} ball): {}",
        e.feature.label(),
        e.name,
        e.profile,
        e.to_validation_error()
    )
}

/// Copies `bundle` (folder, `.zip` or `.pinbundle`) into an immutable
/// in-memory [`BundleSnapshot`] (each file read once) and validates that
/// copy with [`validate_snapshot`].
///
/// # Errors
/// A message naming the bundle and every problem: manifest errors as
/// `file: path: message`, clearance errors as `<feature> <name> (<profile>
/// ball): <file>: <path>: <message>`.
pub fn validate_table(bundle: &Path) -> Result<ValidatedTable, String> {
    let name = bundle.display().to_string();
    let snapshot = BundleSnapshot::capture(bundle).map_err(|e| format!("table {name}: {e}"))?;
    validate_snapshot(&name, snapshot)
}

/// Validates `snapshot` (named `name` in messages): its manifests
/// (`pin_assets::load_table`), the playfield glTF and the 2.8 clearance
/// validator (`pin_tablegen::clearance`) for every declared ball profile.
/// Everything is read from the snapshot.
///
/// # Errors
/// As [`validate_table`].
pub fn validate_snapshot(name: &str, snapshot: BundleSnapshot) -> Result<ValidatedTable, String> {
    let vfs = snapshot.vfs();
    let table = load_table(vfs).map_err(|e| format!("table {name}: {e}"))?;
    let scene = load_gltf(vfs, &table.manifest.meta.playfield_mesh)
        .map_err(|e| format!("table {name}: {e}"))?;
    // A generated backbox's colliders (layer 4) carry the VUK exit surface.
    let backbox = load_backbox_colliders(vfs).map_err(|e| format!("table {name}: {e}"))?;
    let report = validate_clearance_with_backbox(&table, &scene, backbox.as_ref());
    if !report.is_ok() {
        let lines: Vec<String> = report.errors.iter().map(clearance_line).collect();
        return Err(format!(
            "table {name}: clearance check failed:\n  {}",
            lines.join("\n  ")
        ));
    }
    let mut warnings: Vec<String> = table.warnings.iter().map(ToString::to_string).collect();
    warnings.extend(report.warnings.iter().map(clearance_line));
    Ok(ValidatedTable {
        table,
        scene,
        warnings,
        snapshot,
    })
}

/// Builds the [`Simulation`] from the snapshot `checked` was validated on
/// ([`Simulation::load_snapshot`]); the bundle path is not touched again.
///
/// # Errors
/// The [`pin_sim::SimError`] text.
pub fn load_checked(checked: &ValidatedTable, cfg: SimConfig) -> Result<Simulation, String> {
    Simulation::load_snapshot(&checked.snapshot, cfg)
        .map_err(|e| format!("table {:?}: {e}", checked.table.manifest.meta.name.as_str()))
}

/// [`validate_table`], then [`load_checked`] on the same snapshot (the path
/// is opened once). Warnings are printed to stderr (`pin_app: table
/// warning: ...`). This is the engine's table load path (`pin_app --table
/// <path>`).
///
/// # Errors
/// [`validate_table`]'s errors, or the [`pin_sim::SimError`] text.
pub fn load_validated(bundle: &Path, cfg: SimConfig) -> Result<Simulation, String> {
    let checked = validate_table(bundle)?;
    for w in &checked.warnings {
        eprintln!("pin_app: table warning: {w}");
    }
    Simulation::load_snapshot(&checked.snapshot, cfg)
        .map_err(|e| format!("table {}: {e}", bundle.display()))
}

/// A display update from the sim thread to the render thread (the score
/// display and the backglass): one per scene shown and per score report.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DisplayCommand {
    /// The rules showed scene `SceneId` (`HostCommandQueue::display_scenes`).
    Scene(SceneId),
    /// The rules reported `score` for `player` (0-based).
    Score {
        /// Player, 0-based.
        player: u8,
        /// Absolute score.
        score: i64,
    },
}

/// Default capacity of the display command ring (a burst of scene and
/// score updates per frame is a handful; overflow drops and counts).
pub const DISPLAY_RING_CAPACITY: usize = 256;

/// Default capacity of the sound trigger ring.
pub const SOUND_RING_CAPACITY: usize = 256;

/// The table [`SimStep`]: steps the [`Simulation`] and pushes each tick's
/// haptic impulses and sound triggers into the audio thread's rings, its
/// display commands into the render thread's ring, its impulses into the
/// rumble ring ([`set_rumble`](Self::set_rumble)) and its cabinet state
/// into the telemetry encoder ([`set_telemetry`](Self::set_telemetry)) (all
/// wait-free, no allocation; a full ring drops the newest and counts it).
pub struct TableSim {
    sim: Simulation,
    haptics: Option<HapticsProducer<HapticsTelemetryEvent>>,
    sounds: Option<SoundSender>,
    displays: Option<HapticsProducer<DisplayCommand>>,
    rumble: Option<HapticsProducer<HapticEvent>>,
    telemetry: Option<Box<TelemetryTap>>,
    /// `(flipper, its coil)` of every flipper, for the coil states.
    flipper_coils: Vec<(FlipperId, CoilId)>,
}

/// Coils (by `CoilId`) the telemetry state tracks from host commands.
const TRACKED_COILS: usize = 256;

/// The telemetry feed and the coil and magnet state it is derived from
/// (allocated once in [`TableSim::set_telemetry`]).
struct TelemetryTap {
    feed: TelemetryFeed,
    sources: TelemetrySources,
    /// Coils held on by a host command (`HostCommandQueue::coil_holds`).
    held: [bool; TRACKED_COILS],
    /// First tick a host-pulsed coil is off again (a pulse of `d` ms issued
    /// at tick `t` is energised for ticks `t..t + d`).
    pulse_until: [u64; TRACKED_COILS],
}

impl TelemetryTap {
    /// Applies the host commands of the tick `tick` (pulses, holds, magnet
    /// powers) to the tracked state. Never allocates.
    fn apply_commands(&mut self, tick: u64, cmds: &pin_core::script::HostCommandQueue) {
        for &(c, d) in &cmds.coil_pulses {
            if let Some(p) = self.pulse_until.get_mut(usize::from(c.0)) {
                *p = (*p).max(tick.saturating_add(u64::from(d)));
            }
        }
        for &(c, on) in &cmds.coil_holds {
            let i = usize::from(c.0);
            if let Some(h) = self.held.get_mut(i) {
                *h = on;
                if !on {
                    // Like the mechs: releasing a hold ends a running pulse.
                    self.pulse_until[i] = 0;
                }
            }
        }
        for &(m, power) in &cmds.magnets {
            if let Some(v) = self.sources.magnets.get_mut(usize::from(m.0)) {
                *v = power;
            }
        }
    }

    /// The coil states at `tick`: host holds and pulses, flipper coils whose
    /// flipper is energised, and every mapped coil a mech reports on.
    fn update_coils(&mut self, tick: u64, sim: &Simulation, flipper_coils: &[(FlipperId, CoilId)]) {
        self.host_coils(tick);
        let flippers = sim.flipper_set();
        for &(f, c) in flipper_coils {
            if flippers.is_energised(f)
                && let Some(v) = self.sources.coils.get_mut(usize::from(c.0))
            {
                *v = true;
            }
        }
        let mechs = sim.mech_set();
        for &(_, c) in &self.feed.map().coils {
            if let Some(v) = self.sources.coils.get_mut(usize::from(c))
                && !*v
                && mechs.coil_on(CoilId(c))
            {
                *v = true;
            }
        }
    }

    /// The coil states at `tick` from the host holds and pulses alone.
    fn host_coils(&mut self, tick: u64) {
        for ((c, &h), &until) in self
            .sources
            .coils
            .iter_mut()
            .zip(&self.held)
            .zip(&self.pulse_until)
        {
            *c = h || tick < until;
        }
    }
}

impl TableSim {
    /// Wraps `sim`; `haptics` receives every haptic impulse.
    pub fn new(sim: Simulation, haptics: Option<HapticsProducer<HapticsTelemetryEvent>>) -> Self {
        Self {
            sim,
            haptics,
            sounds: None,
            displays: None,
            rumble: None,
            telemetry: None,
            flipper_coils: Vec::new(),
        }
    }

    /// Sends every haptic impulse ([`Simulation::haptic_events`]) to the
    /// main thread's rumble driver as well (SPEC §4 3.4).
    pub fn set_rumble(&mut self, tx: HapticsProducer<HapticEvent>) {
        self.rumble = Some(tx);
    }

    /// Feeds the cabinet state to `feed` every tick (SPEC §4 3.4, KP-006):
    /// switches ([`Simulation::switch_bits`]), coils (energised flippers
    /// from [`set_flipper_coils`](Self::set_flipper_coils), host coil holds
    /// and pulses, mech coils), lamps and GI (the snapshot), magnets (the
    /// latest host magnet power) and the tick's haptic impulses. Load time:
    /// allocates the tracking state once. At tick 0 the `on_init` commands
    /// are applied at once.
    pub fn set_telemetry(&mut self, feed: TelemetryFeed) {
        let mut tap = Box::new(TelemetryTap {
            feed,
            sources: TelemetrySources::default(),
            held: [false; TRACKED_COILS],
            pulse_until: [0; TRACKED_COILS],
        });
        tap.sources.switch_bits = self.sim.switch_bits();
        if self.sim.tick() == 0 {
            tap.apply_commands(0, self.sim.last_commands());
        }
        self.telemetry = Some(tap);
    }

    /// The coil of each flipper (`(FlipperId, CoilId)`, load time): a
    /// flipper's coil reads energised on the telemetry while the flipper is.
    pub fn set_flipper_coils(&mut self, pairs: &[(FlipperId, CoilId)]) {
        self.flipper_coils = pairs.to_vec();
    }

    /// The telemetry feed, if one is attached.
    pub fn telemetry(&self) -> Option<&TelemetryFeed> {
        self.telemetry.as_ref().map(|t| &t.feed)
    }

    /// The telemetry sources of the last tick, if a feed is attached.
    pub fn telemetry_sources(&self) -> Option<&TelemetrySources> {
        self.telemetry.as_ref().map(|t| &t.sources)
    }

    /// Haptic impulses dropped because the rumble ring was full.
    pub fn rumble_dropped(&self) -> u64 {
        self.rumble.as_ref().map_or(0, HapticsProducer::dropped)
    }

    /// Sends every sound command ([`Simulation::sound_commands`]) to the
    /// 3.3 mixer's trigger ring. At tick 0 the `on_init` sounds are sent at
    /// once.
    pub fn with_sounds(mut self, mut sounds: SoundSender) -> Self {
        if self.sim.tick() == 0 {
            push_sounds(&self.sim, &mut sounds);
        }
        self.sounds = Some(sounds);
        self
    }

    /// Sends every display scene and score report
    /// ([`Simulation::display_scenes`], [`Simulation::score_reports`]) to
    /// the render thread. At tick 0 the `on_init` ones (the attract scene)
    /// are sent at once.
    pub fn with_displays(mut self, mut displays: HapticsProducer<DisplayCommand>) -> Self {
        if self.sim.tick() == 0 {
            push_displays(&self.sim, &mut displays);
        }
        self.displays = Some(displays);
        self
    }

    /// Sound triggers dropped because the ring was full.
    pub fn sounds_dropped(&self) -> u64 {
        self.sounds.as_ref().map_or(0, SoundSender::dropped)
    }

    /// Display commands dropped because the ring was full.
    pub fn displays_dropped(&self) -> u64 {
        self.displays.as_ref().map_or(0, HapticsProducer::dropped)
    }

    /// The simulation.
    pub fn sim(&self) -> &Simulation {
        &self.sim
    }

    /// The simulation, mutably (state injection in tests).
    pub fn sim_mut(&mut self) -> &mut Simulation {
        &mut self.sim
    }

    /// Unwraps the simulation.
    pub fn into_inner(self) -> Simulation {
        self.sim
    }

    /// Haptic impulses dropped because the ring was full.
    pub fn haptics_dropped(&self) -> u64 {
        self.haptics.as_ref().map_or(0, HapticsProducer::dropped)
    }
}

impl SimStep for TableSim {
    fn step_1ms(&mut self, tick: u64, inputs: &[InputEvent], out: &mut FrameStateSnapshot) {
        self.sim.step_1ms(tick, inputs, out);
        if let Some(h) = self.haptics.as_mut() {
            for e in self.sim.haptic_events() {
                h.push(*e);
            }
        }
        if let Some(s) = self.sounds.as_mut() {
            push_sounds(&self.sim, s);
        }
        if let Some(d) = self.displays.as_mut() {
            push_displays(&self.sim, d);
        }
        if let Some(r) = self.rumble.as_mut() {
            for e in self.sim.haptic_events() {
                r.push(HapticEvent::from(*e));
            }
        }
        if let Some(t) = self.telemetry.as_deref_mut() {
            t.apply_commands(tick, self.sim.last_commands());
            t.sources.switch_bits = self.sim.switch_bits();
            t.update_coils(tick, &self.sim, &self.flipper_coils);
            t.feed.tick(tick, out, &t.sources, self.sim.haptic_events());
        }
    }
}

fn push_sounds(sim: &Simulation, out: &mut SoundSender) {
    for &(sound, gain) in sim.sound_commands() {
        out.send(SoundTrigger { sound, gain });
    }
}

fn push_displays(sim: &Simulation, out: &mut HapticsProducer<DisplayCommand>) {
    for &(player, score) in sim.score_reports() {
        out.push(DisplayCommand::Score { player, score });
    }
    for &id in sim.display_scenes() {
        out.push(DisplayCommand::Scene(id));
    }
}

/// The playfield position `[x, y]` (mm, table frame) of every haptic
/// source the simulation can emit that the table data places: pin_sim
/// pushes a coil's impulses with `source = CoilId.0` (the same id as
/// `pin_core::haptics::class_source(SOURCE_CLASS_COIL, coil)` for coils
/// below 4096) for flipper coils (rising edge of the energised state) and for every
/// pulsed or held coil, so each flipper's coil sits at its pivot
/// (`base_position_mm`) and each mech's coil (slingshot, bumper, kicker,
/// ...) at the mech's `position`. A coil driving several placed elements
/// keeps the first (flippers, then mechs in declaration order). Coils
/// without a placed element are not listed (they play at the centre).
/// Sorted by source id.
pub fn table_source_positions(table: &LoadedTable) -> Vec<(u16, [f32; 2])> {
    let mut out: Vec<(u16, [f32; 2])> = Vec::new();
    let mut add = |source: u16, p: [f32; 3]| {
        if p[0].is_finite() && p[1].is_finite() && !out.iter().any(|(s, _)| *s == source) {
            out.push((source, [p[0], p[1]]));
        }
    };
    for f in &table.flippers {
        add(f.coil.0, f.base_position_mm);
    }
    for m in &table.mechs {
        if let (Some(coil), Some(p)) = (m.coil, m.position_m) {
            add(coil.0, p.map(|v| v * 1000.0));
        }
    }
    out.sort_by_key(|(s, _)| *s);
    out
}

/// Plan-view outline of a flipper for the debug view: pivot, length and
/// rest angle (table frame, m / rad).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FlipperOutline {
    /// Pivot `[x, y, z]`, m.
    pub base_m: [f32; 3],
    /// Bat length, m.
    pub length_m: f32,
    /// Rest angle about +z from +x, rad.
    pub rest_rad: f32,
}

impl FlipperOutline {
    /// The outlines of `table`'s flippers, in `FlipperId` order.
    pub fn of(table: &LoadedTable) -> Vec<FlipperOutline> {
        table
            .flippers
            .iter()
            .map(|f| {
                let length_mm = match f.shape {
                    pin_core::flipper::FlipperShape::StraightCapsule { length_mm, .. }
                    | pin_core::flipper::FlipperShape::CurvedBanana { length_mm, .. } => length_mm,
                };
                FlipperOutline {
                    base_m: f.base_position_mm.map(|v| v * 1.0e-3),
                    length_m: length_mm * 1.0e-3,
                    rest_rad: f.rest_angle_deg.to_radians(),
                }
            })
            .collect()
    }

    /// The bat line at `angle_rad` from rest.
    pub fn line(&self, angle_rad: f32, color: [f32; 4]) -> DebugLine {
        let a = self.rest_rad + angle_rad;
        let [x, y, z] = self.base_m;
        DebugLine::new(
            self.base_m,
            [x + self.length_m * a.cos(), y + self.length_m * a.sin(), z],
            color,
        )
    }
}

/// Colour of the static table lines in the debug view.
pub const TABLE_LINE_COLOR: [f32; 4] = [0.35, 0.45, 0.6, 1.0];

/// Plan-view debug lines of the table's colliders: every triangle edge of a
/// collider mesh that is not shared by two coplanar-in-plan triangles is
/// drawn once (walls and posts become their outlines). At most `max` lines.
pub fn table_lines(scene: &GltfScene, max: usize) -> Vec<DebugLine> {
    let mut out: Vec<DebugLine> = Vec::new();
    let mut seen: Vec<([i32; 2], [i32; 2])> = Vec::new();
    let key = |p: [f32; 3]| [(p[0] * 1.0e4) as i32, (p[1] * 1.0e4) as i32];
    for mesh in scene.meshes.iter().filter(|m| m.extras.collider) {
        let m = mesh.transform;
        let tf = |p: [f32; 3]| {
            let mut o = [0.0f32; 3];
            for (r, v) in o.iter_mut().enumerate() {
                *v = m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r];
            }
            o
        };
        for t in mesh.indices.chunks_exact(3) {
            let pts = [t[0], t[1], t[2]]
                .map(|i| mesh.positions.get(i as usize).copied().map_or([0.0; 3], tf));
            for (a, b) in [(0, 1), (1, 2), (2, 0)] {
                let (ka, kb) = (key(pts[a]), key(pts[b]));
                if ka == kb {
                    continue; // vertical edge: a point in plan view
                }
                let k = if ka <= kb { (ka, kb) } else { (kb, ka) };
                if seen.contains(&k) {
                    continue;
                }
                if out.len() >= max {
                    return out;
                }
                seen.push(k);
                let flat = |p: [f32; 3]| [p[0], p[1], 0.0];
                out.push(DebugLine::new(flat(pts[a]), flat(pts[b]), TABLE_LINE_COLOR));
            }
        }
    }
    out
}

/// Stack of every thread that loads or steps a [`Simulation`]
/// ([`pin_sim::cli::SIM_THREAD_STACK_BYTES`], 16 MiB): a Wasm rules script
/// needs [`pin_script::wasm::MIN_HOST_STACK_BYTES`], and the simulation
/// (about 70 KiB by value) is moved through several frames while it is
/// built and wrapped, which overflows a 1 MiB main-thread stack (Windows).
pub const SIM_STACK_BYTES: usize = pin_sim::cli::SIM_THREAD_STACK_BYTES;

/// Runs `f` on a new thread with a [`SIM_STACK_BYTES`] stack and returns
/// its result (a panic in `f` is resumed on the caller).
///
/// # Errors
/// The thread could not be spawned.
pub fn on_sim_stack<R: Send + 'static>(
    name: &str,
    f: impl FnOnce() -> R + Send + 'static,
) -> Result<R, String> {
    let h = std::thread::Builder::new()
        .name(name.to_owned())
        .stack_size(SIM_STACK_BYTES)
        .spawn(f)
        .map_err(|e| format!("spawning the {name} thread: {e}"))?;
    Ok(h.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
}

/// Runs `ticks` ticks of the table at `bundle` on simulated time with no
/// input (`pin_app --headless-ticks N --table <path>`), reading every
/// snapshot back through the triple buffer. Returns the last tick read back
/// and the simulation's state hash. Load and run happen on a thread with a
/// [`SIM_STACK_BYTES`] stack ([`on_sim_stack`]), never on the caller's.
///
/// # Errors
/// Load errors ([`load_validated`]) and non-monotonic snapshots.
pub fn run_headless_table(bundle: &Path, ticks: u64) -> Result<(u64, u64), String> {
    let bundle = bundle.to_path_buf();
    on_sim_stack("headless-table", move || {
        let sim = load_validated(&bundle, SimConfig::default())?;
        let (last, table) = super::run_headless_returning(TableSim::new(sim, None), ticks)?;
        Ok((last, table.sim().state_hash()))
    })?
}

/// Most table lines drawn in the debug view of an app table.
const MAX_TABLE_LINES: usize = 8192;

/// A table loaded for the app (`pin_app --table <path>`, or picked in the
/// table browser): validated and simulated from one in-memory bundle
/// snapshot, with what the render and audio threads need.
pub struct LoadedApp {
    /// The simulation, wrapped with its haptics, sound and display rings.
    pub sim: Box<TableSim>,
    /// The PBR renderer's view of the table (meshes, backglass, displays).
    pub visuals: TableVisuals,
    /// The table's decoded sounds, registered with the mixer.
    pub samples: Vec<RegisteredSound>,
    /// Plan-view collider lines for the debug view ([`table_lines`]).
    pub table_lines: Vec<DebugLine>,
    /// Flipper outlines for the debug view ([`FlipperOutline::of`]).
    pub flippers: Vec<FlipperOutline>,
    /// Haptic source positions ([`table_source_positions`]).
    pub source_positions: Vec<(u16, [f32; 2])>,
    /// The audio thread's end of the haptics ring.
    pub haptics: HapticsConsumer<HapticsTelemetryEvent>,
    /// The mixer's end of the sound trigger ring.
    pub sounds: SoundReceiver,
    /// The render thread's end of the display command ring.
    pub displays: HapticsConsumer<DisplayCommand>,
    /// The flipper coils of the latency trace (`--latency-trace`).
    pub bindings: FlipperBindings,
    /// The table manifest (a clone made at load; `[dof]` for the DOF
    /// bridge).
    pub manifest: TableManifest,
    /// The telemetry ids of the table's elements (built at load from the
    /// manifest numbers and the interned ids).
    pub telemetry_map: TelemetryMap,
}

/// Validates and loads the table at `path` for the app: [`validate_table`]
/// then [`load_checked`] on the same snapshot, on a [`SIM_STACK_BYTES`]
/// thread ([`on_sim_stack`], named `table-load`; the simulation is built,
/// wrapped and boxed there, only the box reaches the caller). Sounds are
/// decoded at `mixer.sample_rate`. Prints the warnings and one `pin_app:
/// table "<name>" loaded (...)` line on stderr.
///
/// Sounds are charged to the default decoded-audio budget
/// ([`DecodeBudget::DEFAULT_BYTES`], 512 MiB); the app passes the player's
/// setting through [`load_app_table_with_budget`].
///
/// # Errors
/// The validation or load error text, or a failed thread spawn. A panic in
/// the load is resumed on the caller.
pub fn load_app_table(path: &Path, mixer: MixerConfig) -> Result<LoadedApp, String> {
    load_app_table_with_budget(path, mixer, DecodeBudget::DEFAULT_BYTES)
}

/// [`load_app_table`] with the bundle's decoded-audio budget in bytes
/// (`audio.decoded_budget_mib` × 1 MiB, SPEC §4 4.4; D25): the sounds are
/// decoded by [`table_samples`] against one budget of
/// `decoded_budget_bytes`; a sound that does not fit stays silent and its
/// error is printed with the other sound warnings.
///
/// # Errors
/// As [`load_app_table`].
pub fn load_app_table_with_budget(
    path: &Path,
    mixer: MixerConfig,
    decoded_budget_bytes: u64,
) -> Result<LoadedApp, String> {
    let path = path.to_path_buf();
    on_sim_stack("table-load", move || -> Result<LoadedApp, String> {
        let checked = validate_table(&path)?;
        for w in &checked.warnings {
            eprintln!("pin_app: table warning: {w}");
        }
        let sim = load_checked(&checked, SimConfig::default())?;
        let (htx, hrx) = haptics_ring(DEFAULT_HAPTICS_RING_CAPACITY);
        let (stx, srx) = trigger_ring(SOUND_RING_CAPACITY);
        let (dtx, drx) = haptics_ring(DISPLAY_RING_CAPACITY);
        let mut sim = Box::new(
            TableSim::new(sim, Some(htx))
                .with_sounds(stx)
                .with_displays(dtx),
        );
        let flipper_coils: Vec<(FlipperId, CoilId)> = checked
            .table
            .flippers
            .iter()
            .map(|f| (f.id, f.coil))
            .collect();
        sim.set_flipper_coils(&flipper_coils);
        let ids = &checked.table.ids;
        let telemetry_map =
            TelemetryMap::from_manifest(&checked.table.manifest, |k, n| ids.get(k, n));
        let visuals = TableVisuals::of(&checked);
        for w in &visuals.warnings {
            eprintln!("pin_app: table display warning: {w}");
        }
        let (samples, errors) = table_samples(&checked, mixer.sample_rate, decoded_budget_bytes);
        for e in &errors {
            eprintln!("pin_app: table sound warning: {e}");
        }
        eprintln!(
            "pin_app: table {:?} loaded ({} switches, {} mechs, {} sounds)",
            checked.table.manifest.meta.name,
            checked.table.switches.len(),
            checked.table.mechs.len(),
            samples.len()
        );
        Ok(LoadedApp {
            sim,
            table_lines: table_lines(&checked.scene, MAX_TABLE_LINES),
            flippers: FlipperOutline::of(&checked.table),
            bindings: FlipperBindings::of_flippers(&checked.table.flippers),
            source_positions: table_source_positions(&checked.table),
            visuals,
            samples,
            haptics: hrx,
            sounds: srx,
            displays: drx,
            manifest: checked.table.manifest.clone(),
            telemetry_map,
        })
    })?
}

/// A request to the app's [`IoWorker`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IoRequest {
    /// Scan this folder for table bundles ([`scan_tables`]).
    Scan(PathBuf),
    /// Validate and load the bundle at `path` ([`load_app_table`]) for the
    /// menu's load request `id`.
    Load {
        /// The menu's load id (`MenuState::pending_load_id`).
        id: u64,
        /// The bundle (folder, `.zip` or `.pinbundle`).
        path: PathBuf,
    },
}

/// A reply of the [`IoWorker`], one per request, in request order.
pub enum IoReply {
    /// A [`IoRequest::Scan`] finished (a missing or unreadable folder gives
    /// an empty list).
    Scanned {
        /// The folder scanned.
        root: PathBuf,
        /// The bundles found, sorted ([`scan_tables`]).
        tables: Vec<TableEntry>,
    },
    /// A [`IoRequest::Load`] succeeded.
    Loaded {
        /// The request's load id.
        id: u64,
        /// The bundle.
        path: PathBuf,
        /// The loaded table, ready for the engine threads.
        app: Box<LoadedApp>,
    },
    /// A [`IoRequest::Load`] failed (validation, load or a panic in the
    /// load, with its message).
    LoadFailed {
        /// The request's load id.
        id: u64,
        /// The bundle.
        path: PathBuf,
        /// Why (never empty).
        error: String,
    },
}

impl std::fmt::Debug for IoReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IoReply::Scanned { root, tables } => f
                .debug_struct("Scanned")
                .field("root", root)
                .field("tables", &tables.len())
                .finish(),
            IoReply::Loaded { id, path, .. } => f
                .debug_struct("Loaded")
                .field("id", id)
                .field("path", path)
                .finish_non_exhaustive(),
            IoReply::LoadFailed { id, path, error } => f
                .debug_struct("LoadFailed")
                .field("id", id)
                .field("path", path)
                .field("error", error)
                .finish(),
        }
    }
}

/// The text of a caught panic payload.
fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    let text = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned());
    format!("panicked: {text}")
}

/// Serves one request; never panics. A load reads the decoded-audio budget
/// from `budget` when it starts.
fn serve(req: IoRequest, mixer: MixerConfig, budget: &AtomicU64) -> IoReply {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    match req {
        IoRequest::Scan(root) => {
            let tables = catch_unwind(AssertUnwindSafe(|| scan_tables(&root))).unwrap_or_default();
            IoReply::Scanned { root, tables }
        }
        IoRequest::Load { id, path } => {
            let bytes = budget.load(Ordering::Acquire);
            let result = catch_unwind(AssertUnwindSafe(|| {
                load_app_table_with_budget(&path, mixer, bytes)
            }))
            .unwrap_or_else(|p| Err(panic_text(p.as_ref())));
            match result {
                Ok(app) => IoReply::Loaded {
                    id,
                    path,
                    app: Box::new(app),
                },
                Err(e) => {
                    let error = if e.is_empty() {
                        format!("table {}: load failed", path.display())
                    } else {
                        e
                    };
                    IoReply::LoadFailed { id, path, error }
                }
            }
        }
    }
}

/// The app's I/O worker (R4: the only thread that touches the file system
/// at runtime): one background thread named `io-worker` that serves
/// [`IoRequest`]s in order (a scan with [`scan_tables`], a load with
/// [`load_app_table`] on its big-stack `table-load` thread) and returns one
/// [`IoReply`] per request through a channel, in request order. A request
/// never panics the worker (a panic inside a load becomes
/// [`IoReply::LoadFailed`] with the panic message). The main thread only
/// posts requests and applies replies.
///
/// Loads use the decoded-audio budget last set with
/// [`set_decoded_budget_bytes`](Self::set_decoded_budget_bytes)
/// ([`DecodeBudget::DEFAULT_BYTES`] until then), read when each load starts.
///
/// Dropping the worker stops it: the request in flight completes, requests
/// still queued are skipped, and the thread is joined.
pub struct IoWorker {
    requests: Option<mpsc::Sender<IoRequest>>,
    replies: mpsc::Receiver<IoReply>,
    stop: Arc<AtomicBool>,
    decoded_budget: Arc<AtomicU64>,
    handle: Option<JoinHandle<()>>,
}

impl IoWorker {
    /// Starts the worker; loads decode sounds at `mixer.sample_rate`. If the
    /// thread cannot be spawned, a line is printed on stderr and the worker
    /// answers nothing ([`recv`](Self::recv) returns `None`).
    #[must_use]
    pub fn spawn(mixer: MixerConfig) -> IoWorker {
        let (req_tx, req_rx) = mpsc::channel::<IoRequest>();
        let (rep_tx, rep_rx) = mpsc::channel::<IoReply>();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let decoded_budget = Arc::new(AtomicU64::new(DecodeBudget::DEFAULT_BYTES));
        let budget = Arc::clone(&decoded_budget);
        let spawned = std::thread::Builder::new()
            .name("io-worker".to_owned())
            .spawn(move || {
                while let Ok(req) = req_rx.recv() {
                    if flag.load(Ordering::Acquire) {
                        break;
                    }
                    if rep_tx.send(serve(req, mixer, &budget)).is_err() {
                        break;
                    }
                }
            });
        let handle = match spawned {
            Ok(h) => Some(h),
            Err(e) => {
                eprintln!("pin_app: spawning the io-worker thread: {e}");
                None
            }
        };
        IoWorker {
            requests: handle.as_ref().map(|_| req_tx),
            replies: rep_rx,
            stop,
            decoded_budget,
            handle,
        }
    }

    /// Sets the decoded-audio budget, in bytes, of the loads that start
    /// after this call (`audio.decoded_budget_mib` × 1 MiB; a change applies
    /// to the next table load, never to one in flight).
    pub fn set_decoded_budget_bytes(&self, bytes: u64) {
        self.decoded_budget.store(bytes, Ordering::Release);
    }

    /// The decoded-audio budget, in bytes, the next load will use.
    pub fn decoded_budget_bytes(&self) -> u64 {
        self.decoded_budget.load(Ordering::Acquire)
    }

    /// Queues `req` (never blocks).
    pub fn request(&self, req: IoRequest) {
        if let Some(tx) = &self.requests {
            let _ = tx.send(req);
        }
    }

    /// The next reply if one is ready (never blocks).
    pub fn try_recv(&self) -> Option<IoReply> {
        self.replies.try_recv().ok()
    }

    /// Blocks for the next reply; `None` once the worker has stopped (or
    /// never started).
    pub fn recv(&self) -> Option<IoReply> {
        self.replies.recv().ok()
    }
}

impl Drop for IoWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Closing the request channel ends the worker's loop.
        drop(self.requests.take());
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "pin_app_io_worker_{tag}_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// R4 / KP-005: requests are served in order; a missing folder scans
    /// empty and a missing or broken bundle fails without stopping the
    /// worker.
    #[test]
    fn io_worker_serves_in_order_and_survives_failures() {
        let d = temp("order");
        std::fs::create_dir_all(d.join("tables/broken")).unwrap();
        std::fs::write(d.join("tables/broken/table.toml"), "not = [toml").unwrap();
        let w = IoWorker::spawn(MixerConfig::default());
        w.request(IoRequest::Scan(d.join("missing")));
        w.request(IoRequest::Load {
            id: 1,
            path: d.join("tables/broken"),
        });
        w.request(IoRequest::Load {
            id: 2,
            path: d.join("nope"),
        });
        w.request(IoRequest::Scan(d.join("tables")));
        match w.recv() {
            Some(IoReply::Scanned { root, tables }) => {
                assert_eq!(root, d.join("missing"));
                assert!(tables.is_empty());
            }
            other => panic!("{other:?}"),
        }
        for want in [1, 2] {
            match w.recv() {
                Some(IoReply::LoadFailed { id, error, .. }) => {
                    assert_eq!(id, want);
                    assert!(!error.is_empty());
                }
                other => panic!("{other:?}"),
            }
        }
        match w.recv() {
            Some(IoReply::Scanned { tables, .. }) => {
                assert_eq!(tables.len(), 1);
                assert!(!tables[0].is_ready());
            }
            other => panic!("{other:?}"),
        }
        assert!(w.try_recv().is_none());
        let rendered = format!(
            "{:?}",
            IoReply::Scanned {
                root: d.clone(),
                tables: Vec::new()
            }
        );
        assert!(rendered.contains("Scanned"));
        drop(w);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Dropping the worker with requests queued joins it without serving
    /// them all.
    #[test]
    fn io_worker_drop_joins() {
        let d = temp("drop");
        let w = IoWorker::spawn(MixerConfig::default());
        for i in 0..64 {
            w.request(IoRequest::Load {
                id: i,
                path: d.join(format!("missing{i}")),
            });
        }
        drop(w);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn panic_text_of_payloads() {
        let p: Box<dyn std::any::Any + Send> = Box::new("boom");
        assert_eq!(panic_text(p.as_ref()), "panicked: boom");
        let p: Box<dyn std::any::Any + Send> = Box::new(String::from("bang"));
        assert_eq!(panic_text(p.as_ref()), "panicked: bang");
        let p: Box<dyn std::any::Any + Send> = Box::new(5u8);
        assert_eq!(panic_text(p.as_ref()), "panicked: unknown panic");
    }

    /// KP-006 (SPEC §4 3.4): a host pulse of `d` ms issued at tick `t`
    /// reads energised for exactly ticks `t..t + d`; a hold stays on until
    /// released (which also ends a pulse); magnet powers follow the latest
    /// command; ids beyond the tracked range are ignored.
    #[test]
    fn kp006_coil_pulse_and_hold_states() {
        use pin_core::MagnetId;
        use pin_core::script::HostCommandQueue;
        let (tx, _rx) = pin_audio::telemetry::telemetry_channel(8);
        let mut tap = TelemetryTap {
            feed: TelemetryFeed::new(tx, TelemetryMap::default()),
            sources: TelemetrySources::default(),
            held: [false; TRACKED_COILS],
            pulse_until: [0; TRACKED_COILS],
        };
        let mut q = HostCommandQueue::default();
        q.pulse_coil(CoilId(4), 30).unwrap();
        q.pulse_coil(CoilId(5), 0).unwrap();
        q.pulse_coil(CoilId(900), 30).unwrap();
        q.hold_coil(CoilId(6), true).unwrap();
        q.hold_coil(CoilId(700), true).unwrap();
        q.set_magnet(MagnetId(2), 0.75).unwrap();
        q.set_magnet(MagnetId(64), 0.75).unwrap();
        tap.apply_commands(100, &q);
        let mut on_ticks = 0;
        for t in 100..200 {
            tap.host_coils(t);
            assert!(!tap.sources.coils[5], "a 0 ms pulse never energises");
            assert!(tap.sources.coils[6], "held at {t}");
            if tap.sources.coils[4] {
                assert!((100..130).contains(&t), "tick {t}");
                on_ticks += 1;
            }
        }
        assert_eq!(on_ticks, 30);
        assert_eq!(tap.sources.magnets[2], 0.75);
        // A shorter pulse does not cut a longer one; a released hold ends
        // a pulse.
        let mut q = HostCommandQueue::default();
        q.pulse_coil(CoilId(4), 50).unwrap();
        q.pulse_coil(CoilId(6), 50).unwrap();
        tap.apply_commands(200, &q);
        let mut q = HostCommandQueue::default();
        q.pulse_coil(CoilId(4), 5).unwrap();
        q.hold_coil(CoilId(6), false).unwrap();
        tap.apply_commands(210, &q);
        tap.host_coils(240);
        assert!(tap.sources.coils[4] && !tap.sources.coils[6]);
        tap.host_coils(250);
        assert!(!tap.sources.coils[4]);
    }
}
