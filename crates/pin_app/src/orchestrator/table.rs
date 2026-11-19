//! Loading a table bundle into the engine (Subphase 6.0): the bundle is
//! copied once into an immutable in-memory [`BundleSnapshot`]; manifest
//! validation and the 2.8 clearance validator (every declared ball profile)
//! run on that snapshot, and [`Simulation::load_snapshot`] builds the
//! simulation from the same snapshot, so a bundle replaced on disk between
//! the two steps can never skip validation. [`TableSim`] is the [`SimStep`]
//! the sim thread drives; it also hands the tick's haptic impulses, sound
//! commands and display commands (scenes, score reports) to the audio and
//! render threads through wait-free rings.

use pin_assets::{GltfScene, LoadedTable, load_gltf, load_table};
use pin_audio::mixer::{SoundSender, SoundTrigger};
use pin_core::input::HapticsTelemetryEvent;
use pin_core::{FrameStateSnapshot, InputEvent, SceneId, SimStep};
use pin_input::haptics::HapticsProducer;
use pin_render::debug::DebugLine;
use pin_sim::load::BundleSnapshot;
use pin_sim::{SimConfig, Simulation};
use pin_tablegen::clearance::{
    ClearanceError, load_backbox_colliders, validate_clearance_with_backbox,
};
use std::path::Path;

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
/// haptic impulses and sound triggers into the audio thread's rings and its
/// display commands into the render thread's ring (all wait-free, no
/// allocation; a full ring drops the newest and counts it).
pub struct TableSim {
    sim: Simulation,
    haptics: Option<HapticsProducer<HapticsTelemetryEvent>>,
    sounds: Option<SoundSender>,
    displays: Option<HapticsProducer<DisplayCommand>>,
}

impl TableSim {
    /// Wraps `sim`; `haptics` receives every haptic impulse.
    pub fn new(sim: Simulation, haptics: Option<HapticsProducer<HapticsTelemetryEvent>>) -> Self {
        Self {
            sim,
            haptics,
            sounds: None,
            displays: None,
        }
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
