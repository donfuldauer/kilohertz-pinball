//! Building a [`Simulation`] from a table bundle.
//!
//! Owner: Subphase 2.5.
//!
//! [`Simulation::load`] opens the bundle with [`Vfs::open`] (folder or zip),
//! validates it with [`load_table`], and wires every part (all setup-time,
//! allocating):
//!
//! * **Static world** from `meta.playfield_mesh` (glTF, table frame: `x`
//!   across, `y` up the table, `z` the playfield normal, metres): every node
//!   with extras `collider: true` becomes a two-sided triangle mesh on
//!   `layer_bit(collision_layer)` (default layer 0), with the material named
//!   by its glTF material (the generator writes the physics material name
//!   there; unknown names are wood).
//! * **Splines**: guides become [`GuideCurve`]s with the playfield wall height
//!   (`[playfield] wall_height` of a `layout.toml` in the bundle, else
//!   [`DEFAULT_WALL_HEIGHT_MM`]); wireforms become [`Wireform`]s (gauge,
//!   `roll_deg`, up hint `+z`). Each collides on its own layer only.
//! * **Gravity** `9.81 m/s²` tilted by `meta.pitch_deg` down `−y`; then the
//!   2.7 hooks ([`register_all`]), after which gravity never changes.
//! * **Magnets** (main deck), **sensors** for every switch with a position
//!   and radius (or bound through glTF extras `switch`), **flippers**
//!   ([`FlipperSet`], body set 0) with the input staging router, **mechs**
//!   ([`MechSet`], body set 1) with the tilt bob on the `tilt` cabinet
//!   switch, and the diverter / gate / Newton-ball bindings described in
//!   DECISIONS.md "2.5 — diverters, gates and Newton balls".
//! * **Script**: Lua or Wasm by the extension of `meta.script_entry`, with
//!   [`SimConfig::lua`] / [`SimConfig::wasm`] (the engine limits by
//!   default); `on_init` runs at tick 0 and its commands are applied.
//! * **Playfield glass** (a1-glass): once every source has fed the table
//!   bounds (collider meshes, splines, flippers, mechs, dispensers), a static
//!   [`Material::Acrylic`] box on every layer is added as the last collider
//!   and the world is built. Its underside is at `height`
//!   (mm, of the bundle's optional `glass.toml`) when that file exists, which must be at
//!   least the table geometry's max `z` (else [`SimError::Build`]);
//!   otherwise it is the old escape ceiling (bounds max `z` +
//!   [`BOUNDS_MARGIN_M`]). It is [`GLASS_THICKNESS_M`] thick and covers the
//!   bounds in `x`/`y` (with the margin). The escape box keeps its `x`/`y`
//!   and floor; its top is the glass's top face + [`BOUNDS_MARGIN_M`]
//!   ([`Simulation::escape_bounds`]). The glass never feeds the bounds.
//!
//! [`Simulation::load_snapshot`] builds from a [`BundleSnapshot`] (an
//! immutable in-memory copy, Subphase 6.0) so a caller can validate and
//! simulate exactly the same bytes; [`Simulation::load`] is
//! [`Simulation::load_from_vfs`] on a freshly opened bundle.

use arrayvec::ArrayVec;
use glam::{Mat4, Quat, Vec3};
use pin_assets::validate::FlipperStageHint;
use pin_assets::{IdKind, LoadedTable, TableIds, Vfs, load_gltf, load_table};
use pin_core::ids::{
    CoilId, DispenserId, DiverterId, FlipperId, LampId, MagnetId, PersistKey, ProfileId, SceneId,
    SoundId, SwitchId,
};
use pin_core::input::CabinetInput;
use pin_core::profiles::{BUILTIN_PROFILE_NAMES, builtin_profile};
use pin_core::schema::table::{MechKind, SplineKind, SwitchKind};
use pin_core::schema::{GLASS_FILE, GlassManifest, LayoutManifest};
use pin_core::snapshot::{FrameStateSnapshot, MAX_BALLS};
use pin_core::{
    BallProfile, HostCommandQueue, HostCtx, HostView, Material, Pcg32, PhysicsDefaults,
    ScriptError, SeedableRng,
};
use pin_input::{FlipperStage, FlipperStagingRouter, IngestConfig, InputIngest, StagingEntry};
use pin_physics::ball::{
    BallEvents, BallSolver, BallSolverConfig, Magnet, NewtonChain, Sensor, SensorKind,
};
use pin_physics::ball_ext::register_all;
use pin_physics::flipper::FlipperSet;
use pin_physics::mech::{MechSet, MechSpec};
use pin_physics::world::{
    ColliderHandle, CubicSpline, GuideCurve, LayerTransition, StaticShape, StaticWorldBuilder,
    TransitionSet, Wireform, layers,
};
use pin_script::NameTables;
use pin_script::lua::LuaScriptBackend;
use pin_script::wasm::WasmScriptBackend;
use std::path::Path;

use crate::hash::Fnv64;
use crate::sim::{
    DiverterBinding, FlipperDrive, GateBinding, NewtonBinding, Script, SimConfig, SimError,
    Simulation, TroughZone,
};

/// Guide wall height when the bundle has no `layout.toml`, mm (the
/// Appendix A.3 example's `wall_height`).
pub const DEFAULT_WALL_HEIGHT_MM: f32 = 28.0;

/// Margin around the table's geometry beyond which a ball has escaped, m.
pub const BOUNDS_MARGIN_M: f32 = 0.05;

/// Thickness of the playfield glass, m. Its underside sits at the bundle's
/// `glass.toml` `height` when present, else [`BOUNDS_MARGIN_M`] above the
/// table's bounds max `z`; the escape ceiling is [`BOUNDS_MARGIN_M`] above
/// its top face.
pub const GLASS_THICKNESS_M: f32 = 0.025;

/// Height of a switch sensor's centre above the switch's playfield
/// position: the centre height of a standard 27 mm ball, m.
pub const SENSOR_LIFT_M: f32 = 0.0135;

/// Length of a Newton-ball mech's path when `length` is not given, mm.
pub const DEFAULT_NEWTON_PATH_MM: f32 = 50.0;

/// Gravity, m/s².
pub const GRAVITY_M_S2: f32 = 9.81;

mod snapshot;

pub use snapshot::BundleSnapshot;

fn build_err(what: impl core::fmt::Display) -> SimError {
    SimError::Build(what.to_string())
}

/// FNV-1a 64 of every file of `vfs` (sorted paths; path bytes, length,
/// contents).
pub fn bundle_hash(vfs: &Vfs) -> Result<u64, SimError> {
    let mut names = vfs.list();
    names.sort();
    let mut h = Fnv64::new();
    for name in &names {
        let bytes = vfs.read(name)?;
        h.write(name.as_bytes());
        h.write_u8(0);
        h.write_u64(bytes.len() as u64);
        h.write(&bytes);
    }
    Ok(h.finish())
}

fn names_of<I: Copy>(ids: &TableIds, kind: IdKind, f: impl Fn(u16) -> I) -> Vec<(String, I)> {
    ids.entries(kind)
        .iter()
        .map(|(n, i)| (n.clone(), f(*i)))
        .collect()
}

/// The name tables the script sees (`pinball.sw`, …).
#[must_use]
pub fn name_tables(ids: &TableIds) -> NameTables {
    NameTables {
        switches: names_of(ids, IdKind::Switch, SwitchId),
        coils: names_of(ids, IdKind::Coil, CoilId),
        lamps: names_of(ids, IdKind::Lamp, LampId),
        magnets: names_of(ids, IdKind::Magnet, MagnetId),
        diverters: names_of(ids, IdKind::Diverter, DiverterId),
        flippers: names_of(ids, IdKind::Flipper, FlipperId),
        dispensers: names_of(ids, IdKind::Dispenser, DispenserId),
        profiles: names_of(ids, IdKind::Profile, ProfileId),
        sounds: names_of(ids, IdKind::Sound, SoundId),
        scenes: names_of(ids, IdKind::Scene, SceneId),
        persist: names_of(ids, IdKind::PersistKey, PersistKey),
    }
}

/// Builds the rules backend for `script_entry` with the configured limits.
fn build_script(
    entry: &str,
    source: &[u8],
    names: &NameTables,
    cfg: &SimConfig,
) -> Result<Script, ScriptError> {
    let ext = Path::new(entry)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("lua") => Ok(Script::Lua(Box::new(LuaScriptBackend::new(
            source, names, cfg.lua,
        )?))),
        Some("wasm") => Ok(Script::Wasm(Box::new(WasmScriptBackend::new(
            source, names, cfg.wasm,
        )?))),
        // Any other extension: the same error `load_backend_with` gives.
        _ => match pin_script::load_backend_with(entry, source, names, cfg.lua, cfg.wasm) {
            Err(e) => Err(e),
            Ok(_) => Err(ScriptError::UnsupportedBackend(entry.to_owned())),
        },
    }
}

/// The ball profiles the solver knows, indexed by `ProfileId` (every
/// built-in, the Glo-Ball with the table's Magnus coefficient).
fn profiles(table: &LoadedTable) -> Vec<BallProfile> {
    BUILTIN_PROFILE_NAMES
        .iter()
        .filter_map(|&n| {
            let mut p = builtin_profile(n)?;
            if n == "uv_globall" {
                p.magnus_coeff = table.physics.globall_magnus_coeff;
            }
            Some(p)
        })
        .collect()
}

struct Bounds {
    min: Vec3,
    max: Vec3,
}

impl Bounds {
    fn new() -> Self {
        Self {
            min: Vec3::splat(f32::INFINITY),
            max: Vec3::splat(f32::NEG_INFINITY),
        }
    }

    fn add(&mut self, p: Vec3) {
        if p.is_finite() {
            self.min = self.min.min(p);
            self.max = self.max.max(p);
        }
    }

    /// Highest `z` of the geometry, before the margin (`-inf` when empty).
    fn max_z(&self) -> f32 {
        self.max.z
    }

    fn finish(&self) -> (Vec3, Vec3) {
        if self.min.x > self.max.x {
            return (Vec3::splat(-1.0), Vec3::splat(1.0));
        }
        (
            self.min - Vec3::splat(BOUNDS_MARGIN_M),
            self.max + Vec3::splat(BOUNDS_MARGIN_M),
        )
    }
}

/// The playfield glass for the finished table bounds `(lo, hi)` (margin
/// included): a box covering `lo..hi` in `x`/`y` whose underside is at
/// `under` (m). Returns the shape and the escape bounds (`lo` and `hi` in
/// `x`/`y` and the floor, with the top at the glass's top face +
/// [`BOUNDS_MARGIN_M`]).
fn glass_lid(lo: Vec3, hi: Vec3, under: f32) -> (StaticShape, (Vec3, Vec3)) {
    let half = GLASS_THICKNESS_M * 0.5;
    let shape = StaticShape::Cuboid {
        center: Vec3::new((lo.x + hi.x) * 0.5, (lo.y + hi.y) * 0.5, under + half),
        half_extents: Vec3::new((hi.x - lo.x) * 0.5, (hi.y - lo.y) * 0.5, half),
        rotation: Quat::IDENTITY,
    };
    let top = under + GLASS_THICKNESS_M;
    (shape, (lo, Vec3::new(hi.x, hi.y, top + BOUNDS_MARGIN_M)))
}

/// Underside of the playfield glass, m: `glass_height_m` when the bundle's
/// `glass.toml` sets it (an error if it is below the geometry's max `z`, `max_z`, before
/// the margin; equal is allowed), else the finished bounds' top `hi_z`
/// (max `z` + [`BOUNDS_MARGIN_M`]).
fn glass_underside(glass_height_m: Option<f32>, max_z: f32, hi_z: f32) -> Result<f32, SimError> {
    let Some(g) = glass_height_m else {
        return Ok(hi_z);
    };
    if g < max_z {
        return Err(build_err(format!(
            "glass.toml height {:.3} mm is below the table geometry's max z {:.3} mm",
            g * 1.0e3,
            max_z * 1.0e3
        )));
    }
    Ok(g)
}

impl Simulation {
    /// The box outside which a ball counts as escaped and is removed
    /// (`(min, max)`, m): the table bounds plus [`BOUNDS_MARGIN_M`] in `x`,
    /// `y` and below, and the playfield glass's top face plus
    /// [`BOUNDS_MARGIN_M`] above. The glass's underside (the bundle's
    /// `glass.toml` `height` when present, else the table bounds' max `z` +
    /// [`BOUNDS_MARGIN_M`]) is `max.z − GLASS_THICKNESS_M − BOUNDS_MARGIN_M`
    /// (to f32 rounding).
    #[must_use]
    pub fn escape_bounds(&self) -> ([f32; 3], [f32; 3]) {
        (self.bounds.0.to_array(), self.bounds.1.to_array())
    }
}

/// The playfield wall height of the bundle's `layout.toml`, m.
fn layout_wall_height(vfs: &Vfs) -> Result<f32, SimError> {
    const LAYOUT: &str = "layout.toml";
    if !vfs.exists(LAYOUT) {
        return Ok(DEFAULT_WALL_HEIGHT_MM * 1.0e-3);
    }
    let bytes = vfs.read(LAYOUT)?;
    let text = String::from_utf8(bytes).map_err(|e| build_err(format!("{LAYOUT}: {e}")))?;
    let layout =
        LayoutManifest::from_toml_str(&text).map_err(|e| build_err(format!("{LAYOUT}: {e}")))?;
    let h = layout.playfield.wall_height;
    if !(h.is_finite() && h > 0.0) {
        return Err(build_err(format!(
            "{LAYOUT}: playfield.wall_height {h} must be positive"
        )));
    }
    Ok(h * 1.0e-3)
}

/// The glass underside height of the bundle's optional `glass.toml`, m
/// (`None` when the file is absent).
fn glass_height_m(vfs: &Vfs) -> Result<Option<f32>, SimError> {
    if !vfs.exists(GLASS_FILE) {
        return Ok(None);
    }
    let bytes = vfs.read(GLASS_FILE)?;
    let text = String::from_utf8(bytes).map_err(|e| build_err(format!("{GLASS_FILE}: {e}")))?;
    let glass =
        GlassManifest::from_toml_str(&text).map_err(|e| build_err(format!("{GLASS_FILE}: {e}")))?;
    Ok(Some(glass.height * 1.0e-3))
}

/// Collider nodes named after a mech: `<id>` / `<id>.on` (on) and
/// `<id>.off` (off).
fn bound_colliders(
    named: &[(String, ColliderHandle)],
    id: &str,
) -> (Vec<ColliderHandle>, Vec<ColliderHandle>) {
    let mut on = Vec::new();
    let mut off = Vec::new();
    for (name, h) in named {
        if let Some(rest) = name.strip_prefix(id) {
            match rest {
                "" | ".on" => on.push(*h),
                ".off" => off.push(*h),
                _ => {}
            }
        }
    }
    (on, off)
}

impl Simulation {
    /// Loads the bundle at `bundle` (a folder, `.zip` or `.pinbundle`),
    /// builds the physics, input and script, runs the script's `on_init` and
    /// applies its commands. The tick counter starts at 0.
    ///
    /// Run it on a thread with at least
    /// [`pin_script::wasm::MIN_HOST_STACK_BYTES`] of stack for a Wasm script
    /// (see [`Simulation`]).
    ///
    /// # Errors
    /// [`SimError::Asset`] (bundle, manifest or glTF), [`SimError::Script`]
    /// (script load or the `on_init` top level), [`SimError::Build`]
    /// (physics construction).
    pub fn load(bundle: &Path, cfg: SimConfig) -> Result<Simulation, SimError> {
        let vfs = Vfs::open(bundle)?;
        Simulation::load_from_vfs(&vfs, cfg)
    }

    /// [`load`](Self::load) from a [`BundleSnapshot`]: every file is read
    /// from the snapshot's in-memory copy, never from disk, so the
    /// simulation is built from exactly the bytes a validator saw in the
    /// same snapshot ([`Simulation::table_hash`] equals
    /// [`BundleSnapshot::bundle_hash`]).
    ///
    /// # Errors
    /// As [`load`](Self::load).
    pub fn load_snapshot(
        snapshot: &BundleSnapshot,
        cfg: SimConfig,
    ) -> Result<Simulation, SimError> {
        Simulation::load_from_vfs(snapshot.vfs(), cfg)
    }

    /// [`load`](Self::load) from an open [`Vfs`] (a folder `Vfs` reads its
    /// files from disk during the call; use [`load_snapshot`](Self::load_snapshot)
    /// for an immutable copy).
    ///
    /// # Errors
    /// As [`load`](Self::load).
    pub fn load_from_vfs(vfs: &Vfs, cfg: SimConfig) -> Result<Simulation, SimError> {
        let table_hash = bundle_hash(vfs)?;
        let table = load_table(vfs)?;
        let scene = load_gltf(vfs, &table.manifest.meta.playfield_mesh)?;
        let wall_height = layout_wall_height(vfs)?;
        let glass_m = glass_height_m(vfs)?;
        let mut bounds = Bounds::new();

        // --- Static world. ---
        let mut builder = StaticWorldBuilder::new();
        let mut materials: Vec<(ColliderHandle, Material)> = Vec::new();
        let mut named: Vec<(String, ColliderHandle)> = Vec::new();
        for mesh in scene.meshes.iter().filter(|m| m.extras.collider) {
            let m = Mat4::from_cols_array_2d(&mesh.transform);
            let vertices: Vec<Vec3> = mesh
                .positions
                .iter()
                .map(|&p| m.transform_point3(Vec3::from_array(p)))
                .collect();
            for &v in &vertices {
                bounds.add(v);
            }
            let indices: Vec<[u32; 3]> = mesh
                .indices
                .chunks_exact(3)
                .map(|t| [t[0], t[1], t[2]])
                .collect();
            let layer = mesh.extras.collision_layer.unwrap_or(0);
            let h = builder.add_collider(
                StaticShape::TriMesh { vertices, indices },
                layers::layer_bit(layer),
                true,
            );
            let material = mesh
                .material
                .and_then(|i| scene.materials.get(i))
                .and_then(|m| Material::from_name(&m.name))
                .unwrap_or(Material::Wood);
            materials.push((h, material));
            named.push((mesh.name.clone(), h));
        }
        // The world is built after the last `bounds.add` (dispensers), so the
        // playfield glass can sit on the final bounds; collider state set
        // before that (diverters' "on" colliders) waits in this list.
        let mut disabled_at_load: Vec<ColliderHandle> = Vec::new();

        // --- Splines. ---
        let mut guides = Vec::new();
        let mut guide_layers = Vec::new();
        let mut wireforms = Vec::new();
        let mut wireform_layers = Vec::new();
        for s in &table.splines {
            let through: Vec<Vec3> = s.points_m.iter().map(|&p| Vec3::from_array(p)).collect();
            for &p in &through {
                bounds.add(p);
            }
            let spline = if s.closed {
                CubicSpline::interpolate_closed(&through)
            } else {
                CubicSpline::interpolate(&through)
            }
            .map_err(|e| build_err(format!("spline {}: {e}", s.name)))?;
            match s.kind {
                SplineKind::Guide => {
                    let g = GuideCurve::new(spline, wall_height)
                        .map_err(|e| build_err(format!("guide {}: {e}", s.name)))?;
                    guides.push(g);
                    guide_layers.push(layers::layer_bit(s.layer));
                }
                SplineKind::Wireform => {
                    let roll: Vec<f32> = table
                        .manifest
                        .splines
                        .iter()
                        .find(|d| d.id == s.name)
                        .and_then(|d| d.roll_deg.clone())
                        .unwrap_or_else(|| vec![0.0; through.len()]);
                    let w = Wireform::new(spline, &roll, s.gauge_m.unwrap_or(0.0), Vec3::Z)
                        .map_err(|e| build_err(format!("wireform {}: {e}", s.name)))?;
                    wireforms.push(w);
                    wireform_layers.push(layers::layer_bit(s.layer));
                }
            }
        }

        // --- Solver: gravity first, then the 2.7 hooks. ---
        let defaults = PhysicsDefaults::builtin();
        let mut solver_cfg = BallSolverConfig::from_defaults(&defaults);
        let pitch = table.pitch_rad;
        solver_cfg.gravity = Vec3::new(
            0.0,
            -GRAVITY_M_S2 * libm::sinf(pitch),
            -GRAVITY_M_S2 * libm::cosf(pitch),
        );
        solver_cfg.rolling_resistance = table.physics.rolling_resistance;
        solver_cfg.magnet_remanence_tau_s = table.physics.magnet_remanence_tau_ms * 1.0e-3;
        let mut solver = BallSolver::new(solver_cfg, &profiles(&table));
        for (h, m) in &materials {
            solver.set_collider_material(*h, *m);
        }
        for (i, &mask) in guide_layers.iter().enumerate() {
            solver.set_guide_layers(i, mask);
        }
        for (i, &mask) in wireform_layers.iter().enumerate() {
            solver.set_wireform_layers(i, mask);
        }
        let remanence = register_all(&mut solver);

        let mut magnets = Vec::new();
        for m in &table.magnets {
            let sid = solver.add_magnet(Magnet {
                position: Vec3::from_array(m.position_m),
                k_coil: m.k_coil,
                d_min: m.d_min_m,
                range: m.range_m,
                layer_mask: layers::MAIN_DECK,
            });
            solver.set_magnet_remanence_tau(sid, m.tau_s);
            magnets.push((m.id, sid));
        }

        // --- Sensors and trough zones. ---
        let mut troughs = Vec::new();
        for sw in &table.switches {
            let zone = match (sw.position_m, sw.radius_m) {
                (Some(p), Some(r)) => Some((Vec3::from_array(p) + Vec3::Z * SENSOR_LIFT_M, r)),
                _ => scene
                    .meshes
                    .iter()
                    .find(|m| m.extras.switch.as_deref() == Some(sw.name.as_str()))
                    .and_then(|mesh| {
                        let m = Mat4::from_cols_array_2d(&mesh.transform);
                        let mut b = Bounds::new();
                        for &p in &mesh.positions {
                            b.add(m.transform_point3(Vec3::from_array(p)));
                        }
                        (b.min.x <= b.max.x).then(|| {
                            let c = (b.min + b.max) * 0.5;
                            let half = (b.max - b.min) * 0.5;
                            (c + Vec3::Z * SENSOR_LIFT_M, half.x.max(half.y).max(0.005))
                        })
                    }),
            };
            let Some((center, radius)) = zone else {
                continue;
            };
            if !(center.is_finite() && radius.is_finite() && radius > 0.0) {
                continue;
            }
            let kind = if sw.kind == SwitchKind::Eddy {
                SensorKind::Eddy
            } else {
                SensorKind::Opto
            };
            let layer_mask = layers::layer_bit(sw.layer);
            solver.add_sensor(Sensor {
                switch: sw.id,
                kind,
                center,
                radius,
                layer_mask,
            });
            if sw.kind == SwitchKind::Trough {
                troughs.push(TroughZone {
                    switch: sw.id,
                    center,
                    radius,
                    layer_mask,
                });
            }
        }

        // --- Flippers and staging. ---
        let flippers = FlipperSet::new(&table.flippers).map_err(build_err)?;
        let mut drives = ArrayVec::new();
        let mut staging = Vec::new();
        for (i, c) in table.flippers.iter().enumerate() {
            bounds.add(Vec3::from_array(c.base_position_mm) * 1.0e-3);
            drives.push(FlipperDrive {
                id: c.id,
                coil: c.coil,
                eos_switch: c.eos_switch,
                staged: false,
                held: false,
                pulse_until: 0,
                energised: false,
            });
            let stage = match table.flipper_stages.get(i) {
                Some(FlipperStageHint::Upper) => FlipperStage::Upper,
                _ => FlipperStage::Lower,
            };
            staging.push(StagingEntry {
                flipper: c.id,
                mode: c.staging,
                stage,
            });
        }
        let router = FlipperStagingRouter::new(&staging).map_err(build_err)?;

        // --- Mechs (and the bindings 2.4 leaves inert). ---
        let mut specs = Vec::new();
        let mut diverters = Vec::new();
        let mut gates = Vec::new();
        let mut newtons = Vec::new();
        let standard = crate::sim::profile_id("standard_steel").unwrap_or(ProfileId(0));
        for mech in &table.mechs {
            let def = table
                .manifest
                .mechs
                .iter()
                .find(|d| d.id == mech.name)
                .ok_or_else(|| build_err(format!("mech {} missing from table.toml", mech.name)))?;
            let spec = MechSpec::from_def(def, mech.coil, mech.switch);
            bounds.add(spec.position);
            specs.push(spec);
            match mech.kind {
                MechKind::Diverter => {
                    let (on, off) = bound_colliders(&named, &mech.name);
                    if let Some(id) = mech.diverter {
                        disabled_at_load.extend_from_slice(&on);
                        diverters.push(DiverterBinding {
                            id,
                            on,
                            off,
                            active: false,
                        });
                    }
                }
                MechKind::Gate => {
                    let (colliders, _) = bound_colliders(&named, &mech.name);
                    gates.push(GateBinding {
                        coil: mech.coil,
                        switch: mech.switch,
                        colliders,
                        held: false,
                        pulse_until: 0,
                        open: false,
                    });
                }
                MechKind::NewtonBall => {
                    let angle = def.angle_deg.unwrap_or(90.0).to_radians();
                    let dir = Vec3::new(libm::cosf(angle), libm::sinf(angle), 0.0);
                    let len = def.length.unwrap_or(DEFAULT_NEWTON_PATH_MM) * 1.0e-3;
                    let r = builtin_profile("standard_steel").map_or(0.0135, |p| p.radius_m());
                    let start = spec.position + Vec3::Z * r;
                    let path = CubicSpline::interpolate(&[start, start + dir * len])
                        .map_err(|e| build_err(format!("newton_ball {}: {e}", mech.name)))?;
                    let mut chain = NewtonChain::new(path, standard, &[0.0])
                        .map_err(|e| build_err(format!("newton_ball {}: {e}", mech.name)))?;
                    chain.set_layer_mask(layers::layer_bit(def.layer.unwrap_or(0)));
                    let index = solver.add_newton_chain(chain);
                    newtons.push(NewtonBinding {
                        chain: index,
                        switch: mech.switch,
                    });
                }
                _ => {}
            }
        }
        let mut mechs = MechSet::new(&specs, &defaults).map_err(|e| build_err(format!("{e:?}")))?;
        // Eject scatter: one Pcg32 per mech derived from the sim seed (only
        // VUKs draw; the sim's own generator is untouched).
        mechs.set_scatter_seed(cfg.seed);
        let cabinet =
            CabinetInput::ALL.map(|c| table.ids.switch(c.name()).unwrap_or(SwitchId(u16::MAX)));
        mechs.set_tilt_switch(Some(cabinet[CabinetInput::Tilt.index()]));
        let mut zones: Vec<LayerTransition> = table
            .transitions
            .iter()
            .map(|t| LayerTransition {
                from_layer: t.from_layer,
                to_layer: t.to_layer,
                min: Vec3::from_array(t.zone_min_m),
                max: Vec3::from_array(t.zone_max_m),
            })
            .collect();
        zones.extend_from_slice(mechs.transitions().zones());

        let dispensers = table
            .dispensers
            .iter()
            .map(|d| {
                bounds.add(Vec3::from_array(d.exit_position_m));
                (
                    d.id,
                    Vec3::from_array(d.exit_position_m),
                    Vec3::from_array(d.exit_velocity_m_s),
                )
            })
            .collect();

        // --- Playfield glass, then the world. ---
        // The ceiling is taken from the finished bounds (or the layout's
        // glass.toml) before the glass exists; the glass never feeds
        // `bounds`. Added last, so every existing collider handle is
        // unchanged.
        let (lo, hi) = bounds.finish();
        let under = glass_underside(glass_m, bounds.max_z(), hi.z)?;
        let (glass, escape_bounds) = glass_lid(lo, hi, under);
        let glass = builder.add_collider(glass, layers::ALL, true);
        solver.set_collider_material(glass, Material::Acrylic);
        let mut world = builder.build();
        for &h in &disabled_at_load {
            world.set_enabled(h, false);
        }

        // --- Script. ---
        let names = name_tables(&table.ids);
        let source = vfs.read(&table.manifest.meta.script_entry)?;
        let script = build_script(&table.manifest.meta.script_entry, &source, &names, &cfg)?;

        let mut sim = Simulation {
            cfg,
            ids: table.ids,
            table_hash,
            world,
            guides,
            wireforms,
            transitions: TransitionSet::new(zones),
            solver,
            flippers,
            drives,
            mechs,
            remanence,
            magnets,
            dispensers,
            diverters,
            gates,
            newtons,
            troughs,
            bounds: escape_bounds,
            ingest: InputIngest::new(IngestConfig::default()),
            router,
            router_out: ArrayVec::new(),
            cabinet,
            script,
            view: HostView::default(),
            rng: Pcg32::seed_from_u64(cfg.seed),
            cmd: HostCommandQueue::default(),
            pending: ArrayVec::new(),
            cab_bits: [0; 4],
            phys_bits: [0; 4],
            drain_bits: [0; 4],
            forced_mask: [0; 4],
            forced_val: [0; 4],
            switch_hits: [0; 256],
            scores: [0; 256],
            scores_reported: [0; 4],
            lamps: [0.0; 256],
            gi: [1.0; 4],
            snapshot: FrameStateSnapshot::default(),
            events: BallEvents::new(),
            haptics: ArrayVec::new(),
            out_scenes: ArrayVec::new(),
            out_sounds: ArrayVec::new(),
            out_scores: ArrayVec::new(),
            generations: [0; MAX_BALLS],
            prev_positions: [Vec3::ZERO; MAX_BALLS],
            tick: 0,
            counters: Default::default(),
        };
        // Initial switch states (EOS closed at rest, ...) without edges.
        sim.collect_physics_bits();
        sim.view.switches = sim.compose_switches();
        sim.view.balls_in_play = sim.solver.ball_count().min(255) as u8;
        {
            let mut ctx = HostCtx {
                view: &sim.view,
                rng: &mut sim.rng,
                cmd: &mut sim.cmd,
            };
            match &mut sim.script {
                Script::Lua(b) => pin_core::TableScriptBackend::on_init(b.as_mut(), &mut ctx),
                Script::Wasm(b) => pin_core::TableScriptBackend::on_init(b.as_mut(), &mut ctx),
            }
        }
        // No physics runs in load: on_init pulses count from tick 1.
        sim.apply_commands(sim.tick + 1);
        sim.update_flipper_drive();
        sim.collect_flipper_haptics();
        sim.update_gates();
        sim.publish_snapshot();
        sim.haptics.clear();
        Ok(sim)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glass_sits_on_the_old_ceiling_and_raises_only_the_escape_top() {
        let mut b = Bounds::new();
        b.add(Vec3::new(-0.01, 0.02, -0.03));
        b.add(Vec3::new(0.514, 1.07, 0.12));
        let (lo, hi) = b.finish();
        let (shape, (elo, ehi)) = glass_lid(lo, hi, hi.z);
        let StaticShape::Cuboid {
            center,
            half_extents,
            rotation,
        } = shape
        else {
            panic!("glass is not a cuboid: {shape:?}");
        };
        assert_eq!(rotation, Quat::IDENTITY);
        const { assert!(GLASS_THICKNESS_M >= 0.020) };
        let under = center.z - half_extents.z;
        let top = center.z + half_extents.z;
        assert!(
            (under - hi.z).abs() <= 1.0e-6,
            "underside {under} vs {}",
            hi.z
        );
        assert!((top - under - GLASS_THICKNESS_M).abs() <= 1.0e-6);
        for (c, h, l, u) in [
            (center.x, half_extents.x, lo.x, hi.x),
            (center.y, half_extents.y, lo.y, hi.y),
        ] {
            assert!((c - h - l).abs() <= 1.0e-6 && (c + h - u).abs() <= 1.0e-6);
        }
        // x/y and the floor keep the old escape box; the top clears the glass.
        assert_eq!(elo, lo);
        assert_eq!((ehi.x, ehi.y), (hi.x, hi.y));
        assert!(ehi.z >= top + BOUNDS_MARGIN_M - 1.0e-6);
        assert!((ehi.z - (hi.z + GLASS_THICKNESS_M + BOUNDS_MARGIN_M)).abs() <= 1.0e-6);
    }

    #[test]
    fn empty_bounds_still_get_a_glass() {
        let (lo, hi) = Bounds::new().finish();
        let (shape, (elo, ehi)) = glass_lid(lo, hi, hi.z);
        assert!(matches!(shape, StaticShape::Cuboid { .. }));
        assert_eq!(elo, Vec3::splat(-1.0));
        assert_eq!((ehi.x, ehi.y), (1.0, 1.0));
        assert!(ehi.z > 1.0 + GLASS_THICKNESS_M);
    }

    #[test]
    fn glass_height_sets_the_underside_and_the_escape_top() {
        let mut b = Bounds::new();
        b.add(Vec3::new(0.0, 0.0, 0.0));
        b.add(Vec3::new(0.514, 1.07, 0.092));
        let (lo, hi) = b.finish();
        let under = glass_underside(Some(0.112), b.max_z(), hi.z).unwrap();
        assert_eq!(under, 0.112);
        let (shape, (elo, ehi)) = glass_lid(lo, hi, under);
        let StaticShape::Cuboid {
            center,
            half_extents,
            ..
        } = shape
        else {
            panic!("glass is not a cuboid: {shape:?}");
        };
        assert!((center.z - half_extents.z - 0.112).abs() <= 1.0e-6);
        assert!((half_extents.z * 2.0 - GLASS_THICKNESS_M).abs() <= 1.0e-6);
        assert_eq!(elo, lo);
        assert_eq!((ehi.x, ehi.y), (hi.x, hi.y));
        assert!((ehi.z - (0.112 + GLASS_THICKNESS_M + BOUNDS_MARGIN_M)).abs() <= 1.0e-6);
    }

    #[test]
    fn glass_height_absent_is_the_old_ceiling_and_equal_max_z_is_allowed() {
        assert_eq!(glass_underside(None, 0.03, 0.08).unwrap(), 0.08);
        assert_eq!(glass_underside(Some(0.03), 0.03, 0.08).unwrap(), 0.03);
        assert_eq!(
            glass_underside(Some(1.0), f32::NEG_INFINITY, 1.0).unwrap(),
            1.0
        );
    }

    #[test]
    fn glass_height_below_the_geometry_is_an_error() {
        let Err(SimError::Build(msg)) = glass_underside(Some(0.02), 0.028, 0.078) else {
            panic!("expected a build error");
        };
        assert!(msg.contains("glass.toml height"), "{msg}");
        assert!(msg.contains("20") && msg.contains("28"), "{msg}");
    }
}
