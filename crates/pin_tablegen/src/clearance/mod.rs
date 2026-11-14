//! Table clearance validation for every ball size (PLAN 2.8).
//!
//! Owner: Subphase 2.8.
//!
//! [`validate_clearance`] checks, for every ball profile a table declares
//! (`balls.profiles`, `balls.restrict` and dispenser contents), that the
//! table's geometry can carry a ball of that diameter: lane widths, flipper
//! gaps, wireform gauges, scoop openings and VUK exits, and headroom under
//! upper decks. Each violation is a [`ClearanceError`] naming the offending
//! feature by file and key path, like `pin_assets::ValidationError`.
//! [`validate_bundle`] loads a bundle (`pin_assets::load_table` and the
//! playfield glTF) and runs it; `pin_tablegen validate <bundle>` is its
//! command line ([`run_validate_cli`]). Wiring it into the engine's load
//! path is Subphase 6.0's.
//!
//! # Restrictions
//!
//! A profile with `[balls] restrict` entries may only be on the union of
//! them: each entry allows its `layers`, optionally only with the ball
//! centre inside its plan-view `area` polygon (mm, even-odd rule; CR-005).
//! A profile without entries may be anywhere. A feature is checked for a
//! profile only where the profile may be (layer allowed, reference point
//! inside the area). Restrictions are declarations: the table's geometry
//! and rules keep the ball there; [`Feature::Restriction`] warnings point at
//! layer transitions and VUKs that would carry it elsewhere.
//!
//! # Checks
//!
//! All "required" widths are the ball diameter `d` plus
//! [`MIN_CLEARANCE_MM`].
//!
//! * **Lanes**: guide pairs `<base>.left` / `<base>.right` on one layer
//!   (the generator's `tablegen:lanes[i]`): smallest rail-to-rail distance.
//! * **Flipper gaps**: two flippers on one layer, both at rest: the
//!   smallest distance between the bats (a gap narrower than the ball
//!   wedges it; touching bats are no gap).
//! * **Wireforms**: the rails (gauge `g`, lines) must hold the ball at a
//!   contact angle `α` (from vertical, `sin α = g / d`) between
//!   [`WIREFORM_MIN_CONTACT_DEG`] and [`WIREFORM_MAX_CONTACT_DEG`].
//! * **Scoops** (`vuk` mechs): the held ball (centre at the scoop position
//!   plus `r` up, on the capture layer) must clear the pocket's walls.
//! * **VUK exits**: the ejected ball (centre at `position + exit_offset`, on
//!   `exit_layer`) must not overlap that layer's geometry by more than
//!   [`VUK_EXIT_TOLERANCE_MM`].
//! * **Deck headroom**: wherever a ball may rest on a walkable collider
//!   surface, a non-steep collider of another layer above it (an upper
//!   deck) must leave at least `d + 1 mm`.
//!
//! See DECISIONS.md "2.8 — Clearance validator public API" for the exact
//! rules.

mod checks;
mod cli;
mod geom;
mod headroom;
#[cfg(test)]
mod tests;

pub use cli::{VALIDATE_USAGE, run_validate_cli};

use core::fmt;
use geom::Polygon;
use glam::DVec2;
use pin_assets::{AssetError, GltfScene, LoadedTable, ValidationError, Vfs, load_gltf, load_table};
use pin_core::profiles::builtin_profile;

/// Clearance (mm) every width check adds to the ball diameter.
pub const MIN_CLEARANCE_MM: f32 = 1.0;
/// A collider surface less than this (mm) above a walkable one is part of
/// the same floor, not a ceiling.
pub const MIN_HEADROOM_GAP_MM: f32 = 1.0;
/// Smallest contact angle (degrees from vertical) at which wireform rails
/// hold a ball: below it the ball perches on top and rolls off sideways
/// under a lateral acceleration of `g·tan α` (0.36 g at 20°).
pub const WIREFORM_MIN_CONTACT_DEG: f32 = 20.0;
/// Largest contact angle (degrees from vertical) at which wireform rails
/// hold a ball: above it the ball sits between the rails, at 90° it drops
/// through.
pub const WIREFORM_MAX_CONTACT_DEG: f32 = 75.0;
/// Overlap (mm) an ejected VUK ball may have with its exit layer's
/// geometry (the solver pushes small overlaps out).
pub const VUK_EXIT_TOLERANCE_MM: f32 = 0.5;

/// The kind of feature a [`ClearanceError`] is about.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Feature {
    /// A lane between two guide rails.
    Lane,
    /// The gap between two flippers.
    FlipperGap,
    /// A wireform's rail gauge.
    Wireform,
    /// A scoop (VUK) pocket.
    Scoop,
    /// Where a VUK puts the ejected ball.
    VukExit,
    /// Headroom under an upper deck.
    DeckHeadroom,
    /// A `[balls] restrict` entry.
    Restriction,
}

impl Feature {
    /// Lower-case label used in messages (`"flipper gap"`).
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Feature::Lane => "lane",
            Feature::FlipperGap => "flipper gap",
            Feature::Wireform => "wireform",
            Feature::Scoop => "scoop",
            Feature::VukExit => "vuk exit",
            Feature::DeckHeadroom => "deck headroom",
            Feature::Restriction => "restriction",
        }
    }
}

/// One clearance finding: which feature cannot carry which ball profile.
#[derive(Clone, Debug, PartialEq)]
pub struct ClearanceError {
    /// What kind of feature.
    pub feature: Feature,
    /// The offending feature: spline id, `"<flipper a> / <flipper b>"`
    /// (`[[flippers]]` ids), mech id, glTF node name, or
    /// `"balls.restrict[i]"`.
    pub name: String,
    /// Ball profile name.
    pub profile: String,
    /// Bundle file: `table.toml` or the playfield glTF (`meta.playfield_mesh`).
    pub file: String,
    /// Key path in `file` (`splines[3]`, `flippers[0]`, `mechs[2]`,
    /// `mechs[2].exit_offset`, `meshes[4]`, `balls.restrict[1].area`,
    /// `layer_transitions[0]`).
    pub path: String,
    /// What the profile needs, mm (NaN where no length applies).
    pub required_mm: f32,
    /// What the geometry offers, mm (NaN where no length applies).
    pub actual_mm: f32,
    /// Human-readable description naming the feature, the profile and
    /// both numbers.
    pub message: String,
}

impl ClearanceError {
    /// The same finding as a bundle validation error (file, path, message).
    #[must_use]
    pub fn to_validation_error(&self) -> ValidationError {
        ValidationError::new(&self.file, &self.path, &self.message)
    }
}

impl fmt::Display for ClearanceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.to_validation_error(), f)
    }
}

impl std::error::Error for ClearanceError {}

/// Everything [`validate_clearance`] found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClearanceReport {
    /// Violations: the bundle cannot carry a declared ball. Sorted by
    /// (file, path, profile, message).
    pub errors: Vec<ClearanceError>,
    /// Findings that do not fail validation. Same order.
    pub warnings: Vec<ClearanceError>,
}

impl ClearanceReport {
    /// True when there is no error.
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// `Ok(warnings)` or `Err(errors)`, as bundle validation errors.
    ///
    /// # Errors
    /// Every clearance error, when there is at least one.
    pub fn into_result(self) -> Result<Vec<ValidationError>, Vec<ValidationError>> {
        let conv = |v: Vec<ClearanceError>| v.iter().map(ClearanceError::to_validation_error).collect();
        if self.errors.is_empty() {
            Ok(conv(self.warnings))
        } else {
            Err(conv(self.errors))
        }
    }
}

/// Where a profile may be.
#[derive(Clone, Debug)]
pub(crate) enum Allowed {
    /// No restriction: every layer, everywhere.
    Anywhere,
    /// The union of (layers, optional area) entries.
    Only(Vec<(Vec<u8>, Option<Polygon>)>),
}

impl Allowed {
    /// Whether the profile may be on `layer` at all.
    pub(crate) fn layer(&self, layer: u8) -> bool {
        match self {
            Allowed::Anywhere => true,
            Allowed::Only(e) => e.iter().any(|(l, _)| l.contains(&layer)),
        }
    }

    /// Whether the profile's centre may be at plan point `q` (m) on `layer`.
    pub(crate) fn at(&self, layer: u8, q: DVec2) -> bool {
        match self {
            Allowed::Anywhere => true,
            Allowed::Only(e) => e
                .iter()
                .any(|(l, a)| l.contains(&layer) && a.as_ref().is_none_or(|p| p.contains(q))),
        }
    }

    /// Whether the profile may be somewhere in the box `lo..hi` (m) on
    /// `layer`.
    pub(crate) fn in_box(&self, layer: u8, lo: DVec2, hi: DVec2) -> bool {
        match self {
            Allowed::Anywhere => true,
            Allowed::Only(e) => e.iter().any(|(l, a)| {
                l.contains(&layer) && a.as_ref().is_none_or(|p| p.overlaps_box(lo, hi))
            }),
        }
    }

    pub(crate) fn restricted(&self) -> bool {
        matches!(self, Allowed::Only(_))
    }
}

/// A declared ball profile with its restriction.
#[derive(Clone, Debug)]
pub(crate) struct Ball {
    pub name: String,
    /// Diameter, m.
    pub d: f64,
    pub allowed: Allowed,
}

impl Ball {
    pub(crate) fn r(&self) -> f64 {
        0.5 * self.d
    }

    /// `d + MIN_CLEARANCE_MM`, m.
    pub(crate) fn needed(&self) -> f64 {
        self.d + f64::from(MIN_CLEARANCE_MM) * 1.0e-3
    }

    pub(crate) fn d_mm(&self) -> f32 {
        (self.d * 1.0e3) as f32
    }
}

/// Collects findings.
#[derive(Default)]
pub(crate) struct Sink {
    pub errors: Vec<ClearanceError>,
    pub warnings: Vec<ClearanceError>,
}

/// Fields of one finding (file defaults to `table.toml`).
pub(crate) struct Finding<'a> {
    pub feature: Feature,
    pub name: &'a str,
    pub ball: &'a str,
    pub file: &'a str,
    pub path: String,
    pub required_m: f64,
    pub actual_m: f64,
    pub message: String,
}

impl Sink {
    fn make(f: Finding<'_>) -> ClearanceError {
        ClearanceError {
            feature: f.feature,
            name: f.name.to_owned(),
            profile: f.ball.to_owned(),
            file: f.file.to_owned(),
            path: f.path,
            required_mm: (f.required_m * 1.0e3) as f32,
            actual_mm: (f.actual_m * 1.0e3) as f32,
            message: f.message,
        }
    }

    pub(crate) fn error(&mut self, f: Finding<'_>) {
        self.errors.push(Self::make(f));
    }

    pub(crate) fn warning(&mut self, f: Finding<'_>) {
        self.warnings.push(Self::make(f));
    }
}

/// File name of `table.toml` in findings.
pub(crate) const TABLE: &str = pin_assets::validate::TABLE_FILE;

/// Formats metres as millimetres with one decimal.
pub(crate) fn mm(m: f64) -> String {
    format!("{:.1} mm", m * 1.0e3)
}

/// The declared profiles and their restrictions; bad areas are reported
/// (and the entry then allows its whole layers).
fn declared_balls(table: &LoadedTable, sink: &mut Sink) -> Vec<Ball> {
    let t = &table.manifest;
    let mut areas: Vec<Option<Polygon>> = Vec::with_capacity(t.balls.restrict.len());
    for (i, r) in t.balls.restrict.iter().enumerate() {
        if r.area.is_empty() {
            areas.push(None);
            continue;
        }
        let points: Vec<DVec2> = r
            .area
            .iter()
            .map(|p| DVec2::new(f64::from(p[0]), f64::from(p[1])) * 1.0e-3)
            .collect();
        let poly = Polygon { points };
        let problem = if poly.points.len() < 3 {
            Some(format!(
                "needs at least 3 points, got {}",
                poly.points.len()
            ))
        } else if !poly.points.iter().all(|p| p.is_finite()) {
            Some("has a non-finite coordinate".to_owned())
        } else if !(poly.area2().abs() > 1.0e-12) {
            Some("has no area".to_owned())
        } else {
            None
        };
        match problem {
            Some(m) => {
                let name = format!("balls.restrict[{i}]");
                sink.error(Finding {
                    feature: Feature::Restriction,
                    name: &name,
                    ball: &r.profile,
                    file: TABLE,
                    path: format!("balls.restrict[{i}].area"),
                    required_m: f64::NAN,
                    actual_m: f64::NAN,
                    message: format!("the area of the {:?} restriction {m}", r.profile),
                });
                areas.push(None);
            }
            None => areas.push(Some(poly)),
        }
    }
    let mut balls: Vec<Ball> = Vec::new();
    let names = t
        .balls
        .profiles
        .iter()
        .chain(t.balls.restrict.iter().map(|r| &r.profile))
        .chain(t.dispensers.iter().flat_map(|d| d.contents.iter()));
    for name in names {
        if balls.iter().any(|b| &b.name == name) {
            continue;
        }
        let Some(profile) = builtin_profile(name) else {
            continue;
        };
        let d = f64::from(profile.diameter_m);
        if !(d.is_finite() && d > 0.0) {
            continue;
        }
        let entries: Vec<(Vec<u8>, Option<Polygon>)> = t
            .balls
            .restrict
            .iter()
            .zip(&areas)
            .filter(|(r, _)| &r.profile == name)
            .map(|(r, a)| (r.layers.clone(), a.clone()))
            .collect();
        balls.push(Ball {
            name: name.clone(),
            d,
            allowed: if entries.is_empty() {
                Allowed::Anywhere
            } else {
                Allowed::Only(entries)
            },
        });
    }
    balls
}

/// Checks every ball profile `table` declares against the table's
/// geometry (`scene` is its `meta.playfield_mesh`, loaded with
/// `pin_assets::load_gltf`). See the [module docs](self).
///
/// Deterministic, never panics on a table that `load_table` accepted
/// (and tolerates inconsistent hand-built `LoadedTable`s).
#[must_use]
pub fn validate_clearance(table: &LoadedTable, scene: &GltfScene) -> ClearanceReport {
    validate_clearance_for(table, scene, None)
}

/// [`validate_clearance`] for ball diameters given directly (metres)
/// instead of the declared profiles, each unrestricted and named
/// `"<d> mm ball"`; for tests and tools that sweep sizes.
#[must_use]
pub fn validate_clearance_diameters(
    table: &LoadedTable,
    scene: &GltfScene,
    diameters_m: &[f32],
) -> ClearanceReport {
    validate_clearance_for(table, scene, Some(diameters_m))
}

fn validate_clearance_for(
    table: &LoadedTable,
    scene: &GltfScene,
    diameters: Option<&[f32]>,
) -> ClearanceReport {
    let mut sink = Sink::default();
    let balls = match diameters {
        None => declared_balls(table, &mut sink),
        Some(ds) => ds
            .iter()
            .filter(|d| d.is_finite() && **d > 0.0)
            .map(|&d| Ball {
                name: format!("{:.2} mm ball", d * 1.0e3),
                d: f64::from(d),
                allowed: Allowed::Anywhere,
            })
            .collect(),
    };
    let tris = geom::collider_triangles(scene);
    let grid = geom::TriGrid::new(&tris);
    let world = checks::World {
        table,
        scene,
        tris: &tris,
        grid: &grid,
        glb: table.manifest.meta.playfield_mesh.as_str(),
    };
    checks::lanes(&world, &balls, &mut sink);
    checks::flippers(&world, &balls, &mut sink);
    checks::wireforms(&world, &balls, &mut sink);
    checks::vuks(&world, &balls, &mut sink);
    checks::transitions(&world, &balls, &mut sink);
    headroom::decks(&world, &balls, &mut sink);
    let key = |e: &ClearanceError| {
        (
            e.file.clone(),
            e.path.clone(),
            e.profile.clone(),
            e.message.clone(),
        )
    };
    sink.errors.sort_by_key(key);
    sink.warnings.sort_by_key(key);
    ClearanceReport {
        errors: sink.errors,
        warnings: sink.warnings,
    }
}

/// Loads a bundle (`load_table`, then `load_gltf` of `meta.playfield_mesh`)
/// and runs [`validate_clearance`] on it.
///
/// # Errors
/// Whatever `load_table` / `load_gltf` report (manifest validation errors
/// come as [`AssetError::Invalid`]).
pub fn validate_bundle(vfs: &Vfs) -> Result<ClearanceReport, AssetError> {
    let table = load_table(vfs)?;
    let scene = load_gltf(vfs, &table.manifest.meta.playfield_mesh)?;
    Ok(validate_clearance(&table, &scene))
}
