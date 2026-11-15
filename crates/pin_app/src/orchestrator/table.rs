//! Loading a table bundle into the engine (Subphase 6.0): manifest
//! validation, the 2.8 clearance validator for every declared ball profile,
//! then [`Simulation::load`]; and [`TableSim`], the [`SimStep`] the sim
//! thread drives, which also hands the tick's haptic impulses to the audio
//! thread.

use pin_assets::{GltfScene, LoadedTable, Vfs, load_gltf, load_table};
use pin_core::input::HapticsTelemetryEvent;
use pin_core::{FrameStateSnapshot, InputEvent, SimStep};
use pin_input::haptics::HapticsProducer;
use pin_render::debug::DebugLine;
use pin_sim::{SimConfig, Simulation};
use pin_tablegen::clearance::{ClearanceError, validate_clearance};
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

/// Opens `bundle` (folder, `.zip` or `.pinbundle`), validates its manifests
/// (`pin_assets::load_table`), loads the playfield glTF and runs the 2.8
/// clearance validator (`pin_tablegen::clearance`) for every declared ball
/// profile.
///
/// # Errors
/// A message naming the bundle and every problem: manifest errors as
/// `file: path: message`, clearance errors as `<feature> <name> (<profile>
/// ball): <file>: <path>: <message>`.
pub fn validate_table(bundle: &Path) -> Result<ValidatedTable, String> {
    let name = bundle.display();
    let vfs = Vfs::open(bundle).map_err(|e| format!("table {name}: {e}"))?;
    let table = load_table(&vfs).map_err(|e| format!("table {name}: {e}"))?;
    let scene = load_gltf(&vfs, &table.manifest.meta.playfield_mesh)
        .map_err(|e| format!("table {name}: {e}"))?;
    let report = validate_clearance(&table, &scene);
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
    })
}

/// [`validate_table`], then [`Simulation::load`]. Warnings are printed to
/// stderr (`pin_app: table warning: ...`). This is the engine's table load
/// path (`pin_app --table <path>`).
///
/// # Errors
/// [`validate_table`]'s errors, or the [`pin_sim::SimError`] text.
pub fn load_validated(bundle: &Path, cfg: SimConfig) -> Result<Simulation, String> {
    let checked = validate_table(bundle)?;
    for w in &checked.warnings {
        eprintln!("pin_app: table warning: {w}");
    }
    Simulation::load(bundle, cfg).map_err(|e| format!("table {}: {e}", bundle.display()))
}

/// The table [`SimStep`]: steps the [`Simulation`] and pushes each tick's
/// haptic impulses into the audio thread's ring (wait-free, no allocation;
/// a full ring drops the newest and counts it).
pub struct TableSim {
    sim: Simulation,
    haptics: Option<HapticsProducer<HapticsTelemetryEvent>>,
}

impl TableSim {
    /// Wraps `sim`; `haptics` receives every haptic impulse.
    pub fn new(sim: Simulation, haptics: Option<HapticsProducer<HapticsTelemetryEvent>>) -> Self {
        Self { sim, haptics }
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
    }
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

/// Runs `ticks` ticks of the table at `bundle` on simulated time with no
/// input (`pin_app --headless-ticks N --table <path>`), reading every
/// snapshot back through the triple buffer. Returns the last tick read back
/// and the simulation's state hash.
///
/// # Errors
/// Load errors ([`load_validated`]) and non-monotonic snapshots.
pub fn run_headless_table(bundle: &Path, ticks: u64) -> Result<(u64, u64), String> {
    let sim = load_validated(bundle, SimConfig::default())?;
    let (last, table) = super::run_headless_returning(TableSim::new(sim, None), ticks)?;
    Ok((last, table.sim().state_hash()))
}
