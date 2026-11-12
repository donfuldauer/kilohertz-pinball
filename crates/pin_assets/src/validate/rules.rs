//! The validation rules (beyond `pin_core`'s parse) and SI conversion.

use super::ids::suggest;
use super::{
    BACKGLASS_FILE, BackboxSi, DispenserSi, FlipperStageHint, IdKind, LoadedTable, MagnetSi,
    MechSi, ModeSi, ShotSi, SplineSi, SwitchSi, TABLE_FILE, TableIds, TransitionSi,
    ValidationError,
};
use crate::Vfs;
use crate::zip::normalize_path;
use glam::Vec3;
use pin_core::profiles::BUILTIN_PROFILE_NAMES;
use pin_core::schema::backglass::ScoreDisplayMode;
use pin_core::schema::table::{FlipperStagingDef, MechKind, SplineDef, SplineKind};
use pin_core::schema::{BackglassManifest, PhysicsDefaults, PhysicsParams, TableManifest};
use pin_core::units::{deg_to_rad, mm_to_m, mm3_to_m};
use pin_core::{
    CabinetInput, DiverterId, FlipperConfig, FlipperId, MagnetId, ProfileId, SceneId, SwitchId,
};
use pin_physics::world::{CubicSpline, GuideCurve, SplineError, Wireform};

/// Highest collision layer.
const MAX_LAYER: u8 = 7;
/// Highest switch and lamp number (256-entry runtime tables).
const MAX_SWITCH_LAMP: u16 = 255;
/// Guides whose ends are this close (m) are snapped closed.
const CLOSE_SNAP_M: f32 = 1.0e-6;
/// Through-point spacing ratio above which a spline gets a warning.
const SPACING_RATIO_WARN: f32 = 3.0;
/// Addressable persistent keys (`HostView::persist`).
const MAX_PERSIST_KEYS: usize = 64;

/// Everything computed from a valid table, waiting for the manifests.
pub(super) struct Parts {
    ids: TableIds,
    physics: PhysicsParams,
    flippers: Vec<FlipperConfig>,
    stages: Vec<FlipperStageHint>,
    switches: Vec<SwitchSi>,
    magnets: Vec<MagnetSi>,
    splines: Vec<SplineSi>,
    transitions: Vec<TransitionSi>,
    dispensers: Vec<DispenserSi>,
    mechs: Vec<MechSi>,
    shots: Vec<ShotSi>,
    modes: Vec<ModeSi>,
    backbox: Option<BackboxSi>,
}

impl Parts {
    pub(super) fn finish(
        self,
        manifest: TableManifest,
        backglass: BackglassManifest,
        warnings: Vec<ValidationError>,
    ) -> LoadedTable {
        LoadedTable {
            pitch_rad: deg_to_rad(manifest.meta.pitch_deg),
            manifest,
            backglass,
            ids: self.ids,
            physics: self.physics,
            flippers: self.flippers,
            flipper_stages: self.stages,
            switches: self.switches,
            magnets: self.magnets,
            splines: self.splines,
            transitions: self.transitions,
            dispensers: self.dispensers,
            mechs: self.mechs,
            shots: self.shots,
            modes: self.modes,
            backbox: self.backbox,
            warnings,
        }
    }
}

pub(super) struct Checker<'v> {
    vfs: &'v Vfs,
    pub(super) errors: Vec<ValidationError>,
    pub(super) warnings: Vec<ValidationError>,
    ids: TableIds,
}

/// Tracks first declarations of names and numbers within one kind.
struct Seen<'a, T> {
    items: Vec<(T, &'a str)>,
}

impl<'a, T: PartialEq> Seen<'a, T> {
    fn new() -> Self {
        Seen { items: Vec::new() }
    }

    /// The path of an earlier declaration of `key`, or records this one.
    fn check(&mut self, key: T, path: &'a str) -> Option<&'a str> {
        if let Some((_, first)) = self.items.iter().find(|(k, _)| *k == key) {
            return Some(first);
        }
        self.items.push((key, path));
        None
    }
}

fn fmt3(v: [f32; 3]) -> String {
    format!("[{}, {}, {}]", v[0], v[1], v[2])
}

impl<'v> Checker<'v> {
    pub(super) fn new(vfs: &'v Vfs) -> Self {
        Checker {
            vfs,
            errors: Vec::new(),
            warnings: Vec::new(),
            ids: TableIds::default(),
        }
    }

    fn err(&mut self, file: &str, path: impl Into<String>, message: impl Into<String>) {
        self.errors.push(ValidationError::new(file, path, message));
    }

    fn warn(&mut self, file: &str, path: impl Into<String>, message: impl Into<String>) {
        self.warnings.push(ValidationError::new(file, path, message));
    }

    fn layer(&mut self, file: &str, path: impl Into<String>, layer: u8) {
        if layer > MAX_LAYER {
            self.err(
                file,
                path,
                format!("layer {layer} is out of range (0..={MAX_LAYER})"),
            );
        }
    }

    /// Error for an unresolved `name` among `candidates`, with a suggestion.
    fn unknown<'a>(
        &mut self,
        file: &str,
        path: impl Into<String>,
        label: &str,
        name: &str,
        candidates: impl Iterator<Item = &'a str>,
    ) {
        let message = match suggest(name, candidates) {
            Some(s) => format!("unknown {label} {name:?} (did you mean {s:?}?)"),
            None => format!("unknown {label} {name:?}"),
        };
        self.err(file, path, message);
    }

    /// Resolves `name` in `kind`, reporting an error at `path` when it fails.
    fn resolve(&mut self, kind: IdKind, name: &str, path: impl Into<String>) -> Option<u16> {
        if let Some(id) = self.ids.get(kind, name) {
            return Some(id);
        }
        let candidates: Vec<String> = self
            .ids
            .entries(kind)
            .iter()
            .map(|(n, _)| n.clone())
            .collect();
        self.unknown(
            TABLE_FILE,
            path,
            kind.label(),
            name,
            candidates.iter().map(String::as_str),
        );
        None
    }

    /// Checks that `file` is a valid bundle path naming an existing file.
    fn file_exists(&mut self, manifest: &str, path: impl Into<String>, file: &str) {
        match normalize_path(file) {
            Ok(norm) if !norm.is_empty() => {
                if !self.vfs.exists(&norm) {
                    self.err(
                        manifest,
                        path,
                        format!("file {file:?} does not exist in the bundle"),
                    );
                }
            }
            Ok(_) => self.err(manifest, path, "file path is empty"),
            Err(e) => self.err(manifest, path, format!("invalid file path {file:?}: {e}")),
        }
    }

    /// Runs every check; returns the SI parts when the table parsed.
    pub(super) fn run(
        &mut self,
        table: Option<&TableManifest>,
        backglass: Option<&BackglassManifest>,
    ) -> Option<Parts> {
        let parts = table.map(|t| self.table(t));
        if let Some(b) = backglass {
            self.backglass(b, table.is_some());
        }
        let mut parts = parts?;
        parts.ids = core::mem::take(&mut self.ids);
        parts.backbox = backglass.and_then(|b| {
            b.backbox.as_ref().map(|bb| BackboxSi {
                scene: bb.scene.clone(),
                camera_position_m: mm3_to_m(bb.camera.position),
                camera_look_at_m: mm3_to_m(bb.camera.look_at),
                fov_rad: deg_to_rad(bb.camera.fov_deg),
                collision_layer: bb.collision_layer,
            })
        });
        Some(parts)
    }

    fn intern(&mut self, t: &TableManifest) {
        // Switches: id = number; cabinet switches are implicitly declared.
        let paths: Vec<String> = (0..t.switches.len())
            .map(|i| format!("switches[{i}]"))
            .collect();
        let mut names = Seen::new();
        let mut numbers = Seen::new();
        for (i, s) in t.switches.iter().enumerate() {
            let p = &paths[i];
            if let Some(first) = names.check(s.id.as_str(), p) {
                self.err(
                    TABLE_FILE,
                    format!("{p}.id"),
                    format!("duplicate switch id {:?} (first declared at {first})", s.id),
                );
                continue;
            }
            if s.number > MAX_SWITCH_LAMP {
                self.err(
                    TABLE_FILE,
                    format!("{p}.number"),
                    format!("switch number {} exceeds {MAX_SWITCH_LAMP}", s.number),
                );
            } else if let Some(first) = numbers.check(s.number, p) {
                self.err(
                    TABLE_FILE,
                    format!("{p}.number"),
                    format!("switch number {} is already used by {first}", s.number),
                );
            }
            self.layer(TABLE_FILE, format!("{p}.layer"), s.layer);
            self.ids.push(IdKind::Switch, &s.id, s.number);
        }
        let mut cabinet = Seen::new();
        for input in CabinetInput::ALL {
            let n = t.cabinet_switches.number(input);
            let path = format!("cabinet_switches.{}", input.name());
            if n > MAX_SWITCH_LAMP {
                self.err(
                    TABLE_FILE,
                    &path,
                    format!("cabinet switch number {n} exceeds {MAX_SWITCH_LAMP}"),
                );
                continue;
            }
            if let Some(first) = cabinet.check(n, input.name()) {
                self.err(
                    TABLE_FILE,
                    &path,
                    format!("cabinet switch number {n} is already used by cabinet_switches.{first}"),
                );
                continue;
            }
            match self.ids.get(IdKind::Switch, input.name()) {
                // A declared switch may carry the cabinet input's name only
                // if it has the cabinet switch's number.
                Some(declared) if declared != n => {
                    let at = t
                        .switches
                        .iter()
                        .position(|s| s.id == input.name())
                        .unwrap_or(0);
                    self.err(
                        TABLE_FILE,
                        format!("switches[{at}].id"),
                        format!(
                            "switch id {:?} is reserved for cabinet switch number {n}, \
                             but this switch has number {declared}",
                            input.name()
                        ),
                    );
                }
                Some(_) => {}
                None => self.ids.push(IdKind::Switch, input.name(), n),
            }
        }

        // Coils, lamps, magnets, dispensers: id = number.
        macro_rules! numbered {
            ($list:expr, $section:literal, $label:literal, $kind:expr, $max:expr) => {{
                let mut names = Seen::new();
                let mut numbers = Seen::new();
                let paths: Vec<String> = (0..$list.len())
                    .map(|i| format!(concat!($section, "[{}]"), i))
                    .collect();
                for (i, item) in $list.iter().enumerate() {
                    let p = &paths[i];
                    if let Some(first) = names.check(item.id.as_str(), p) {
                        self.err(
                            TABLE_FILE,
                            format!("{p}.id"),
                            format!(
                                concat!("duplicate ", $label, " id {:?} (first declared at {})"),
                                item.id, first
                            ),
                        );
                        continue;
                    }
                    if item.number > $max {
                        self.err(
                            TABLE_FILE,
                            format!("{p}.number"),
                            format!(
                                concat!($label, " number {} exceeds {}"),
                                item.number, $max
                            ),
                        );
                    } else if let Some(first) = numbers.check(item.number, p) {
                        self.err(
                            TABLE_FILE,
                            format!("{p}.number"),
                            format!(
                                concat!($label, " number {} is already used by {}"),
                                item.number, first
                            ),
                        );
                    }
                    self.ids.push($kind, &item.id, item.number);
                }
            }};
        }
        numbered!(t.coils, "coils", "coil", IdKind::Coil, u16::MAX);
        numbered!(t.lamps, "lamps", "lamp", IdKind::Lamp, MAX_SWITCH_LAMP);
        numbered!(t.magnets, "magnets", "magnet", IdKind::Magnet, u16::MAX);
        numbered!(
            t.dispensers,
            "dispensers",
            "dispenser",
            IdKind::Dispenser,
            u16::MAX
        );

        // Declaration-order kinds and kinds without ids (unique names only).
        self.ordered(
            t.flippers.iter().map(|f| f.id.as_str()),
            "flippers",
            Some(IdKind::Flipper),
        );
        self.ordered(
            t.sounds.iter().map(|s| s.id.as_str()),
            "sounds",
            Some(IdKind::Sound),
        );
        self.ordered(
            t.scenes.iter().map(|s| s.id.as_str()),
            "scenes",
            Some(IdKind::Scene),
        );
        self.ordered(t.mechs.iter().map(|m| m.id.as_str()), "mechs", None);
        let mut diverter = 0u16;
        for m in &t.mechs {
            if m.kind == MechKind::Diverter && self.ids.get(IdKind::Diverter, &m.id).is_none() {
                self.ids.push(IdKind::Diverter, &m.id, diverter);
                diverter = diverter.wrapping_add(1);
            }
        }
        self.ordered(t.splines.iter().map(|s| s.id.as_str()), "splines", None);
        self.ordered(
            t.layer_transitions.iter().map(|s| s.id.as_str()),
            "layer_transitions",
            None,
        );
        self.ordered(t.shots.iter().map(|s| s.id.as_str()), "shots", None);
        self.ordered(t.modes.iter().map(|s| s.id.as_str()), "modes", None);
        if let Some(persist) = &t.persist {
            let mut seen = Seen::new();
            let paths: Vec<String> = (0..persist.keys.len())
                .map(|i| format!("persist.keys[{i}]"))
                .collect();
            for (i, key) in persist.keys.iter().enumerate() {
                if let Some(first) = seen.check(key.as_str(), &paths[i]) {
                    self.err(
                        TABLE_FILE,
                        &paths[i],
                        format!("duplicate persist key {key:?} (first declared at {first})"),
                    );
                } else {
                    self.ids.push(IdKind::PersistKey, key, i as u16);
                }
            }
            if persist.keys.len() > MAX_PERSIST_KEYS {
                self.warn(
                    TABLE_FILE,
                    "persist.keys",
                    format!(
                        "{} persist keys declared; only the first {MAX_PERSIST_KEYS} are addressable",
                        persist.keys.len()
                    ),
                );
            }
        }
        for (i, name) in BUILTIN_PROFILE_NAMES.iter().enumerate() {
            self.ids.push(IdKind::Profile, name, i as u16);
        }
    }

    /// Unique names; with `kind`, ids in declaration order from 0.
    fn ordered<'a>(
        &mut self,
        names: impl Iterator<Item = &'a str>,
        section: &str,
        kind: Option<IdKind>,
    ) {
        let names: Vec<&str> = names.collect();
        let paths: Vec<String> = (0..names.len())
            .map(|i| format!("{section}[{i}]"))
            .collect();
        let mut seen = Seen::new();
        for (i, name) in names.iter().enumerate() {
            if let Some(first) = seen.check(*name, &paths[i]) {
                let label = kind.map_or(section.trim_end_matches('s'), IdKind::label);
                self.err(
                    TABLE_FILE,
                    format!("{}.id", paths[i]),
                    format!("duplicate {label} id {name:?} (first declared at {first})"),
                );
            } else if let Some(kind) = kind {
                self.ids.push(kind, name, i as u16);
            }
        }
    }

    fn table(&mut self, t: &TableManifest) -> Parts {
        self.intern(t);
        let physics = PhysicsDefaults::builtin().resolve(t.physics.as_ref());

        // Meta files.
        self.file_exists(TABLE_FILE, "meta.playfield_mesh", &t.meta.playfield_mesh);
        self.file_exists(TABLE_FILE, "meta.script_entry", &t.meta.script_entry);
        if t.meta.script_backend().is_none() {
            self.err(
                TABLE_FILE,
                "meta.script_entry",
                format!(
                    "script_entry {:?} must end in .lua or .wasm",
                    t.meta.script_entry
                ),
            );
        }

        // Balls.
        for (i, p) in t.balls.profiles.iter().enumerate() {
            self.resolve(IdKind::Profile, p, format!("balls.profiles[{i}]"));
        }
        for (i, r) in t.balls.restrict.iter().enumerate() {
            self.resolve(
                IdKind::Profile,
                &r.profile,
                format!("balls.restrict[{i}].profile"),
            );
            for (j, &l) in r.layers.iter().enumerate() {
                self.layer(TABLE_FILE, format!("balls.restrict[{i}].layers[{j}]"), l);
            }
        }

        // Switches (SI).
        let switches = t
            .switches
            .iter()
            .map(|s| SwitchSi {
                id: SwitchId(s.number),
                name: s.id.clone(),
                kind: s.kind,
                layer: s.layer,
                position_m: s.position.map(mm3_to_m),
                radius_m: s.radius.map(mm_to_m),
            })
            .collect();

        // Magnets.
        let magnets = t
            .magnets
            .iter()
            .map(|m| MagnetSi {
                id: MagnetId(m.number),
                name: m.id.clone(),
                position_m: mm3_to_m(m.position),
                k_coil: m.k_coil,
                d_min_m: mm_to_m(m.d_min),
                range_m: mm_to_m(m.range),
                tau_s: m.tau_ms.unwrap_or(physics.magnet_remanence_tau_ms) / 1000.0,
            })
            .collect();

        // Flippers.
        let mut stages = Vec::with_capacity(t.flippers.len());
        for (i, f) in t.flippers.iter().enumerate() {
            let p = format!("flippers[{i}]");
            self.resolve(IdKind::Coil, &f.coil, format!("{p}.coil"));
            self.resolve(IdKind::Switch, &f.eos_switch, format!("{p}.eos_switch"));
            if let FlipperStagingDef::AutoSensorThingFlip { trigger_switch, .. } = &f.staging {
                self.resolve(
                    IdKind::Switch,
                    trigger_switch,
                    format!("{p}.staging.trigger_switch"),
                );
            }
            self.layer(TABLE_FILE, format!("{p}.collision_layer"), f.collision_layer);
            let upper_input = matches!(
                f.staging,
                FlipperStagingDef::DedicatedButton { input }
                    | FlipperStagingDef::ParallelLinked { input }
                    if matches!(input, CabinetInput::LeftUpper | CabinetInput::RightUpper)
            );
            stages.push(if upper_input || f.collision_layer != 0 {
                FlipperStageHint::Upper
            } else {
                FlipperStageHint::Lower
            });
        }

        // Splines.
        let splines = t
            .splines
            .iter()
            .enumerate()
            .filter_map(|(i, s)| self.spline(i, s))
            .collect();

        // Layer transitions.
        let mut transitions = Vec::with_capacity(t.layer_transitions.len());
        for (i, lt) in t.layer_transitions.iter().enumerate() {
            let p = format!("layer_transitions[{i}]");
            self.layer(TABLE_FILE, format!("{p}.from_layer"), lt.from_layer);
            self.layer(TABLE_FILE, format!("{p}.to_layer"), lt.to_layer);
            let (min, max) = (lt.zone.min, lt.zone.max);
            let bad: Vec<&str> = ["x", "y", "z"]
                .into_iter()
                .enumerate()
                .filter(|&(a, _)| min[a] >= max[a])
                .map(|(_, n)| n)
                .collect();
            if !bad.is_empty() {
                self.err(
                    TABLE_FILE,
                    format!("{p}.zone"),
                    format!(
                        "zone min {} must be below max {} on every axis (fails on {})",
                        fmt3(min),
                        fmt3(max),
                        bad.join(", ")
                    ),
                );
            }
            transitions.push(TransitionSi {
                name: lt.id.clone(),
                from_layer: lt.from_layer,
                to_layer: lt.to_layer,
                zone_min_m: mm3_to_m(min),
                zone_max_m: mm3_to_m(max),
            });
        }

        // Dispensers.
        let mut dispensers = Vec::with_capacity(t.dispensers.len());
        for (i, d) in t.dispensers.iter().enumerate() {
            let contents = d
                .contents
                .iter()
                .enumerate()
                .filter_map(|(j, c)| {
                    self.resolve(IdKind::Profile, c, format!("dispensers[{i}].contents[{j}]"))
                        .map(ProfileId)
                })
                .collect();
            dispensers.push(DispenserSi {
                id: pin_core::DispenserId(d.number),
                name: d.id.clone(),
                exit_position_m: mm3_to_m(d.exit_position),
                exit_velocity_m_s: d.exit_velocity,
                contents,
            });
        }

        // Mechs.
        let mut mechs = Vec::with_capacity(t.mechs.len());
        for (i, m) in t.mechs.iter().enumerate() {
            let coil = m.coil.as_ref().and_then(|c| {
                self.resolve(IdKind::Coil, c, format!("mechs[{i}].coil"))
                    .map(pin_core::CoilId)
            });
            let switch = m.switch.as_ref().and_then(|s| {
                self.resolve(IdKind::Switch, s, format!("mechs[{i}].switch"))
                    .map(SwitchId)
            });
            let diverter = (m.kind == MechKind::Diverter)
                .then(|| self.ids.get(IdKind::Diverter, &m.id).map(DiverterId))
                .flatten();
            mechs.push(MechSi {
                name: m.id.clone(),
                kind: m.kind,
                coil,
                switch,
                diverter,
                position_m: m.position.map(mm3_to_m),
            });
        }

        // Shots.
        let mut shots = Vec::with_capacity(t.shots.len());
        for (i, s) in t.shots.iter().enumerate() {
            if s.sequence.is_empty() {
                self.err(
                    TABLE_FILE,
                    format!("shots[{i}].sequence"),
                    "shot sequence is empty",
                );
            }
            let sequence = s
                .sequence
                .iter()
                .enumerate()
                .filter_map(|(j, name)| {
                    self.resolve(IdKind::Switch, name, format!("shots[{i}].sequence[{j}]"))
                        .map(SwitchId)
                })
                .collect();
            shots.push(ShotSi {
                name: s.id.clone(),
                sequence,
                window_s: s.window_ms as f32 / 1000.0,
            });
        }

        // Modes.
        let mut modes = Vec::with_capacity(t.modes.len());
        for (i, m) in t.modes.iter().enumerate() {
            if let Some(scene) = self.resolve(
                IdKind::Scene,
                &m.start_scene,
                format!("modes[{i}].start_scene"),
            ) {
                modes.push(ModeSi {
                    name: m.id.clone(),
                    start_scene: SceneId(scene),
                });
            }
        }

        // Sounds and scenes.
        for (i, s) in t.sounds.iter().enumerate() {
            self.file_exists(TABLE_FILE, format!("sounds[{i}].file"), &s.file);
        }
        for (i, s) in t.scenes.iter().enumerate() {
            self.file_exists(TABLE_FILE, format!("scenes[{i}].file"), &s.file);
        }

        // DOF.
        if let Some(dof) = &t.dof {
            for (i, m) in dof.map.iter().enumerate() {
                if let Some(c) = &m.coil {
                    self.resolve(IdKind::Coil, c, format!("dof.map[{i}].coil"));
                }
                if let Some(s) = &m.switch {
                    self.resolve(IdKind::Switch, s, format!("dof.map[{i}].switch"));
                }
                if let Some(l) = &m.lamp {
                    self.resolve(IdKind::Lamp, l, format!("dof.map[{i}].lamp"));
                }
            }
        }

        // Flipper configs (only when every name resolved).
        let flippers = if self.errors.is_empty() {
            t.flippers
                .iter()
                .enumerate()
                .filter_map(|(i, f)| {
                    f.to_config(FlipperId(i as u16), &self.ids, &physics)
                        .map_err(|e| {
                            self.errors.push(ValidationError::new(
                                TABLE_FILE,
                                format!("flippers[{i}]"),
                                e.to_string(),
                            ))
                        })
                        .ok()
                })
                .collect()
        } else {
            Vec::new()
        };

        Parts {
            ids: TableIds::default(),
            physics,
            flippers,
            stages,
            switches,
            magnets,
            splines,
            transitions,
            dispensers,
            mechs,
            shots,
            modes,
            backbox: None,
        }
    }

    fn spline_error(&mut self, path: &str, e: SplineError) {
        self.err(
            TABLE_FILE,
            path,
            format!("spline does not construct: {e}"),
        );
    }

    fn spline(&mut self, i: usize, s: &SplineDef) -> Option<SplineSi> {
        let p = format!("splines[{i}]");
        self.layer(TABLE_FILE, format!("{p}.layer"), s.layer);
        if s.points.len() < 2 {
            self.err(
                TABLE_FILE,
                format!("{p}.points"),
                format!(
                    "a spline needs at least 2 points, got {}",
                    s.points.len()
                ),
            );
            return None;
        }
        let mut points: Vec<[f32; 3]> = s.points.iter().map(|&q| mm3_to_m(q)).collect();
        let mut ok = true;
        for j in 1..points.len() {
            if points[j] == points[j - 1] {
                self.err(
                    TABLE_FILE,
                    format!("{p}.points[{j}]"),
                    format!(
                        "point {} repeats the previous point {}",
                        fmt3(s.points[j]),
                        fmt3(s.points[j - 1])
                    ),
                );
                ok = false;
            }
        }
        let roll_deg: Vec<f32> = match &s.roll_deg {
            Some(r) => {
                if r.len() != s.points.len() {
                    self.err(
                        TABLE_FILE,
                        format!("{p}.roll_deg"),
                        format!(
                            "{} roll angles given for {} points (one per point is required)",
                            r.len(),
                            s.points.len()
                        ),
                    );
                    ok = false;
                }
                r.clone()
            }
            None => vec![0.0; s.points.len()],
        };
        let gauge_m = s.gauge.map(mm_to_m);
        if s.kind == SplineKind::Wireform {
            match s.gauge {
                Some(g) if g > 0.0 => {}
                Some(g) => {
                    self.err(
                        TABLE_FILE,
                        format!("{p}.gauge"),
                        format!("wireform gauge must be positive, got {g}"),
                    );
                    ok = false;
                }
                None => {
                    self.err(TABLE_FILE, format!("{p}.gauge"), "a wireform needs a gauge");
                    ok = false;
                }
            }
        }
        if !ok {
            return None;
        }

        // Near-closed guides are snapped closed (2.1 review).
        let first = Vec3::from_array(points[0]);
        let last_index = points.len() - 1;
        let closed = s.kind == SplineKind::Guide
            && first.distance(Vec3::from_array(points[last_index])) <= CLOSE_SNAP_M;
        if closed {
            points[last_index] = points[0];
            if points[last_index - 1] == points[last_index] {
                self.err(
                    TABLE_FILE,
                    format!("{p}.points[{last_index}]"),
                    "closing point repeats the previous point after snapping closed",
                );
                return None;
            }
        }
        let through: Vec<Vec3> = points.iter().map(|&q| Vec3::from_array(q)).collect();
        let built = if closed {
            CubicSpline::interpolate_closed(&through)
        } else {
            CubicSpline::interpolate(&through)
        };
        let spline = match built {
            Ok(spline) => spline,
            Err(SplineError::TooFewPoints) if closed => {
                self.err(
                    TABLE_FILE,
                    format!("{p}.points"),
                    format!(
                        "a closed guide needs at least 4 points (3 distinct), got {}",
                        points.len()
                    ),
                );
                return None;
            }
            Err(e) => {
                self.spline_error(&format!("{p}.points"), e);
                return None;
            }
        };
        let constructed = match s.kind {
            SplineKind::Guide => GuideCurve::new(spline, 0.0).map(drop),
            SplineKind::Wireform => {
                Wireform::new(spline, &roll_deg, gauge_m.unwrap_or(0.0), Vec3::Z).map(drop)
            }
        };
        if let Err(e) = constructed {
            self.spline_error(&p, e);
            return None;
        }

        // Uneven through-point spacing: a warning (2.1 HUMAN_QUEUE item).
        let spacing: Vec<f32> = through.windows(2).map(|w| w[0].distance(w[1])).collect();
        let (lo, hi) = spacing
            .iter()
            .fold((f32::INFINITY, 0.0f32), |(lo, hi), &d| (lo.min(d), hi.max(d)));
        if lo > 0.0 && hi / lo > SPACING_RATIO_WARN {
            self.warn(
                TABLE_FILE,
                format!("{p}.points"),
                format!(
                    "uneven through-point spacing: {:.1} mm to {:.1} mm (ratio {:.1} > {SPACING_RATIO_WARN}); \
                     the curve may overshoot between points",
                    lo * 1000.0,
                    hi * 1000.0,
                    hi / lo
                ),
            );
        }
        Some(SplineSi {
            name: s.id.clone(),
            kind: s.kind,
            layer: s.layer,
            gauge_m,
            points_m: points,
            roll_rad: roll_deg.iter().map(|&d| deg_to_rad(d)).collect(),
            closed,
        })
    }

    fn backglass(&mut self, b: &BackglassManifest, table_parsed: bool) {
        let f = BACKGLASS_FILE;
        for (i, layer) in b.translite.layers.iter().enumerate() {
            self.file_exists(f, format!("translite.layers[{i}].file"), &layer.file);
            if let Some(mask) = &layer.lamp_mask
                && !b.lamp_masks.iter().any(|m| &m.id == mask)
            {
                self.unknown(
                    f,
                    format!("translite.layers[{i}].lamp_mask"),
                    "lamp mask",
                    mask,
                    b.lamp_masks.iter().map(|m| m.id.as_str()),
                );
            }
        }
        let mut seen = Seen::new();
        let paths: Vec<String> = (0..b.lamp_masks.len())
            .map(|i| format!("lamp_masks[{i}]"))
            .collect();
        for (i, m) in b.lamp_masks.iter().enumerate() {
            let p = &paths[i];
            if let Some(first) = seen.check(m.id.as_str(), p) {
                self.err(
                    f,
                    format!("{p}.id"),
                    format!("duplicate lamp mask id {:?} (first declared at {first})", m.id),
                );
            }
            self.file_exists(f, format!("{p}.file"), &m.file);
            if table_parsed && self.ids.get(IdKind::Lamp, &m.lamp).is_none() {
                let candidates: Vec<String> = self
                    .ids
                    .entries(IdKind::Lamp)
                    .iter()
                    .map(|(n, _)| n.clone())
                    .collect();
                self.unknown(
                    f,
                    format!("{p}.lamp"),
                    "lamp",
                    &m.lamp,
                    candidates.iter().map(String::as_str),
                );
            }
        }
        let sd = &b.score_display;
        match sd.mode {
            ScoreDisplayMode::Dmd if sd.dmd.is_none() => self.err(
                f,
                "score_display.dmd",
                "score_display.mode is \"dmd\" but there is no score_display.dmd table",
            ),
            ScoreDisplayMode::Lcd if sd.lcd.is_none() => self.err(
                f,
                "score_display.lcd",
                "score_display.mode is \"lcd\" but there is no score_display.lcd table",
            ),
            _ => {}
        }
        if let Some(bb) = &b.backbox {
            self.file_exists(f, "backbox.scene", &bb.scene);
            self.layer(f, "backbox.collision_layer", bb.collision_layer);
        }
    }
}
