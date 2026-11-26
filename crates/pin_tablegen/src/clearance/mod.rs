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
//! [`validate_bundle`] loads a bundle (`pin_assets::load_table`, the
//! playfield glTF and, when present, the backbox colliders) and runs it; `pin_tablegen validate <bundle>` is its
//! command line ([`run_validate_cli`]). Wiring it into the engine's load
//! path is Subphase 6.0's.
//!
//! # Restrictions
//!
//! A profile with `[balls] restrict` entries may only be on the union of
//! them: each entry allows its `layers`, optionally only with the ball
//! centre inside its plan-view `area` polygon (mm, even-odd rule; CR-005).
//! A profile without entries may be anywhere. An entry's area must hold
//! its profile's ball: when the largest circle inside it (the even-odd
//! region's, so retraced edges do not count) is narrower than the ball
//! diameter, that is an error at `balls.restrict[i].area`. The verdict is
//! decided with a 1 µm radius bracket (a ball up to 1 µm wider in radius,
//! 2 µm in diameter, than the largest circle still fits; a search that
//! cannot decide within its cell budget reports nothing); only
//! the size the message reports is approximate, within
//! [`INSCRIBED_CIRCLE_TOLERANCE_MM`] and never overstated. A feature is checked for a
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
//! # Backbox
//!
//! A bundle's `backbox_colliders.glb` ([`validate_clearance_with_backbox`])
//! adds its collider nodes as geometry of their own `collision_layer` (the
//! backbox layer): floors, walls and ceilings of that layer for the VUK
//! exit, scoop and headroom checks. They never become layer-0 geometry.
//! As in the engine's loader (`pin_sim::load::backbox_colliders`), every
//! node must be a collider (`extras.collider` true) naming a backbox layer
//! (`extras.collision_layer` 4..=7); a node that is not is a
//! [`Feature::BackboxCollider`] error naming the file and the node, with
//! the loader's wording, and adds no geometry (there is no default layer).
//! A VUK exit on a backbox layer is also supported by a walkable backbox
//! collider of the layer below it within 100 mm in plan (the shelf around
//! the entry hole the generated VUK kicks the ball up through), the reach
//! guides and wireforms already have.
//!
//! # Spatial index
//!
//! The geometric checks find nearby triangles through a spatial index
//! ([`SpatialIndex`]). The default ([`SpatialIndex::Fine`]) is one
//! bounding-volume hierarchy per (collision layer, slope class) bucket, so
//! a query visits only the layers, slopes and heights its check can use;
//! [`SpatialIndex::Legacy`] is the original uniform plan-view grid (cells
//! of at least 50 mm), kept as the reference. Both return the same
//! candidates the checks act on, in the same ascending order, so every
//! report is identical; [`validate_clearance_indexed`] runs either and
//! counts the narrow-phase work ([`IndexStats`]). See DECISIONS.md
//! "#135 — clearance spatial index".
//!
//! See DECISIONS.md "2.8 — Clearance validator public API" and
//! `docs/decisions/2026-11-18-fix-2.8-backbox-clearance.md` for the exact
//! rules.

mod checks;
mod cli;
mod geom;
mod headroom;
mod inscribed;
#[cfg(test)]
mod tests;

pub use cli::{VALIDATE_USAGE, run_validate_cli, validate_cli_output};

use core::fmt;
use geom::Polygon;
use glam::DVec2;
use pin_assets::{
    AssetError, BACKBOX_COLLIDERS_FILE, GltfLimits, GltfScene, LoadedTable, MAX_BACKBOX_COLLIDERS,
    MeshData, ValidationError, Vfs, load_gltf, load_gltf_with_limits, load_table,
};
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
/// Tolerance (mm) of the largest-circle size reported for a `[balls]
/// restrict` area that cannot hold its ball: the reported diameter is never
/// above the true one and at most this much below it. Whether the ball
/// fits is decided with a 1 µm radius bracket (2 µm in diameter), not
/// with this tolerance.
pub const INSCRIBED_CIRCLE_TOLERANCE_MM: f32 = 0.5;

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
    /// A node of `backbox_colliders.glb` the engine would refuse.
    BackboxCollider,
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
            Feature::BackboxCollider => "backbox collider",
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
    /// Ball profile name (empty for a finding about no particular ball:
    /// a [`Feature::BackboxCollider`] error).
    pub profile: String,
    /// Bundle file: `table.toml`, the playfield glTF (`meta.playfield_mesh`)
    /// or `backbox_colliders.glb`.
    pub file: String,
    /// Key path in `file` (`splines[3]`, `flippers[0]`, `mechs[2]`,
    /// `mechs[2].exit_offset`, `meshes[4]`, `balls.restrict[1].area`,
    /// `layer_transitions[0]`).
    pub path: String,
    /// What the profile needs, mm (0 where no length applies: restriction
    /// shape findings, backbox collider findings and the flipper-build
    /// warning).
    pub required_mm: f32,
    /// What the geometry offers, mm (0 where no length applies).
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
        let conv =
            |v: Vec<ClearanceError>| v.iter().map(ClearanceError::to_validation_error).collect();
        if self.errors.is_empty() {
            Ok(conv(self.warnings))
        } else {
            Err(conv(self.errors))
        }
    }
}

/// Where a profile may be.
#[derive(Clone, Debug)]
enum Allowed {
    /// No restriction: every layer, everywhere.
    Anywhere,
    /// The union of (layers, optional area) entries.
    Only(Vec<(Vec<u8>, Option<Polygon>)>),
}

impl Allowed {
    /// Whether the profile may be on `layer` at all.
    fn layer(&self, layer: u8) -> bool {
        match self {
            Allowed::Anywhere => true,
            Allowed::Only(e) => e.iter().any(|(l, _)| l.contains(&layer)),
        }
    }

    /// Whether the profile's centre may be at plan point `q` (m) on `layer`.
    fn at(&self, layer: u8, q: DVec2) -> bool {
        match self {
            Allowed::Anywhere => true,
            Allowed::Only(e) => e
                .iter()
                .any(|(l, a)| l.contains(&layer) && a.as_ref().is_none_or(|p| p.covers(q))),
        }
    }

    /// Whether the profile's centre may be anywhere on the polyline through
    /// `path` (m) on `layer`: at a vertex, or where a segment between two
    /// vertices enters or touches (runs along the boundary of) an allowed
    /// area lying between them.
    fn on_path(&self, layer: u8, path: &[DVec2]) -> bool {
        match self {
            Allowed::Anywhere => true,
            Allowed::Only(e) => e.iter().any(|(l, a)| {
                l.contains(&layer)
                    && a.as_ref().is_none_or(|p| {
                        path.iter().any(|&q| p.covers(q))
                            || path.windows(2).any(|s| p.meets_segment(s[0], s[1]))
                    })
            }),
        }
    }

    /// Whether a ball of radius `r` whose centre the profile allows on
    /// `layer` can cover plan point `q` (m): the area contains `q` or comes
    /// within `r` of it.
    fn near(&self, layer: u8, q: DVec2, r: f64) -> bool {
        match self {
            Allowed::Anywhere => true,
            Allowed::Only(e) => e.iter().any(|(l, a)| {
                l.contains(&layer)
                    && a.as_ref()
                        .is_none_or(|p| p.contains(q) || p.boundary_distance(q) <= r)
            }),
        }
    }

    /// Whether the profile may be somewhere in the box `lo..hi` (m) on
    /// `layer`.
    fn in_box(&self, layer: u8, lo: DVec2, hi: DVec2) -> bool {
        match self {
            Allowed::Anywhere => true,
            Allowed::Only(e) => e.iter().any(|(l, a)| {
                l.contains(&layer) && a.as_ref().is_none_or(|p| p.overlaps_box(lo, hi))
            }),
        }
    }

    fn restricted(&self) -> bool {
        matches!(self, Allowed::Only(_))
    }
}

/// A declared ball profile with its restriction.
#[derive(Clone, Debug)]
struct Ball {
    pub name: String,
    /// Diameter, m.
    pub d: f64,
    pub allowed: Allowed,
}

impl Ball {
    fn r(&self) -> f64 {
        0.5 * self.d
    }

    /// `d + MIN_CLEARANCE_MM`, m.
    fn needed(&self) -> f64 {
        self.d + f64::from(MIN_CLEARANCE_MM) * 1.0e-3
    }
}

/// Collects findings.
#[derive(Default)]
struct Sink {
    pub errors: Vec<ClearanceError>,
    pub warnings: Vec<ClearanceError>,
}

/// Fields of one finding (file defaults to `table.toml`).
struct Finding<'a> {
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

    fn error(&mut self, f: Finding<'_>) {
        self.errors.push(Self::make(f));
    }

    fn warning(&mut self, f: Finding<'_>) {
        self.warnings.push(Self::make(f));
    }
}

/// File name of `table.toml` in findings.
const TABLE: &str = pin_assets::validate::TABLE_FILE;

/// Formats metres as millimetres with one decimal.
fn mm(m: f64) -> String {
    format!("{:.1} mm", m * 1.0e3)
}

/// What is wrong with a restriction's area.
enum Problem {
    /// Malformed polygon (the text completes "the area ... ").
    Shape(String),
    /// No disc of the profile's diameter `d` (m) fits; the largest that
    /// does is `fits` (m) across (a lower bound within
    /// [`INSCRIBED_CIRCLE_TOLERANCE_MM`]).
    TooSmall { d: f64, fits: f64 },
}

/// The declared profiles and their restrictions; bad areas are reported
/// (and the entry then allows its whole layers), and so are areas that
/// cannot hold their profile's ball (`inscribed::fits` decides the
/// largest inscribed circle is below the ball diameter; an undecided
/// search is not reported).
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
        } else if !poly.has_filled_area() {
            Some("has no area".to_owned())
        } else {
            None
        };
        let problem = problem.map(Problem::Shape).or_else(|| {
            let d = builtin_profile(&r.profile).map(|p| f64::from(p.diameter_m))?;
            if !(d.is_finite() && d > 0.0) {
                return None;
            }
            if inscribed::fits(&poly, d) != inscribed::Fit::TooSmall {
                return None;
            }
            // Too small for certain; the size reported is a lower bound
            // within the tolerance (below `d`, since the true one is).
            let tol = f64::from(INSCRIBED_CIRCLE_TOLERANCE_MM) * 1.0e-3;
            let fits = inscribed::largest_inscribed_diameter(&poly, 0.25 * tol);
            Some(Problem::TooSmall { d, fits })
        });
        match problem {
            Some(Problem::TooSmall { d, fits }) => {
                let name = format!("balls.restrict[{i}]");
                sink.error(Finding {
                    feature: Feature::Restriction,
                    name: &name,
                    ball: &r.profile,
                    file: TABLE,
                    path: format!("balls.restrict[{i}].area"),
                    required_m: d,
                    actual_m: fits,
                    message: format!(
                        "the area of the {:?} restriction cannot hold the ball: the largest circle that fits inside it is {} across; ball {:?} is {}",
                        r.profile,
                        mm(fits),
                        r.profile,
                        mm(d)
                    ),
                });
                // The declaration stands (the error fails validation).
                areas.push(Some(poly));
            }
            Some(Problem::Shape(m)) => {
                let name = format!("balls.restrict[{i}]");
                sink.error(Finding {
                    feature: Feature::Restriction,
                    name: &name,
                    ball: &r.profile,
                    file: TABLE,
                    path: format!("balls.restrict[{i}].area"),
                    required_m: 0.0,
                    actual_m: 0.0,
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
    validate_clearance_for(table, scene, None, None)
}

/// [`validate_clearance`] with the bundle's backbox colliders (its
/// [`BACKBOX_COLLIDERS_FILE`], as [`load_backbox_colliders`] reads it):
/// their collider nodes are geometry of their `collision_layer` (the
/// backbox layer), so they support VUK exits and count as walls, floors and
/// ceilings of that layer in every check, exactly like playfield meshes of
/// the layer. A node the engine's loader refuses (not a collider, or no
/// `collision_layer` in 4..=7) is a [`Feature::BackboxCollider`] error and
/// adds no geometry. Findings on them name that file and `meshes[i]` within
/// it. With `backbox: None` this is [`validate_clearance`].
#[must_use]
pub fn validate_clearance_with_backbox(
    table: &LoadedTable,
    scene: &GltfScene,
    backbox: Option<&GltfScene>,
) -> ClearanceReport {
    validate_clearance_for(table, scene, backbox, None)
}

/// The layer of a backbox collider node, or why the engine's loader
/// (`pin_sim::load::backbox_colliders`) refuses it, in the loader's words.
fn backbox_node_layer(mesh: &MeshData) -> Result<u8, String> {
    if !mesh.extras.collider {
        return Err(format!(
            "node {:?} is not a collider (extras.collider must be true)",
            mesh.name
        ));
    }
    match mesh.extras.collision_layer {
        Some(l @ 4..=7) => Ok(l),
        other => Err(format!(
            "node {:?}: extras.collision_layer must be a backbox layer 4..=7, got {other:?}",
            mesh.name
        )),
    }
}

/// One [`Feature::BackboxCollider`] error per node of `backbox` that
/// [`backbox_node_layer`] refuses, in node order.
fn check_backbox_nodes(backbox: &GltfScene, sink: &mut Sink) {
    for (i, mesh) in backbox.meshes.iter().enumerate() {
        if let Err(message) = backbox_node_layer(mesh) {
            sink.error(Finding {
                feature: Feature::BackboxCollider,
                name: &mesh.name,
                ball: "",
                file: BACKBOX_COLLIDERS_FILE,
                path: format!("meshes[{i}]"),
                required_m: 0.0,
                actual_m: 0.0,
                message,
            });
        }
    }
}

/// The bundle's [`BACKBOX_COLLIDERS_FILE`], loaded with the engine's
/// budgets (at most [`MAX_BACKBOX_COLLIDERS`] nodes and meshes), or `None`
/// when the bundle has no such file.
///
/// # Errors
/// Whatever `load_gltf_with_limits` reports for the file.
pub fn load_backbox_colliders(vfs: &Vfs) -> Result<Option<GltfScene>, AssetError> {
    if !vfs.exists(BACKBOX_COLLIDERS_FILE) {
        return Ok(None);
    }
    let limits = GltfLimits {
        max_nodes: MAX_BACKBOX_COLLIDERS,
        max_meshes: MAX_BACKBOX_COLLIDERS,
        ..GltfLimits::default()
    };
    load_gltf_with_limits(vfs, BACKBOX_COLLIDERS_FILE, &limits).map(Some)
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
    validate_clearance_for(table, scene, None, Some(diameters_m))
}

/// Which spatial index the validator uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SpatialIndex {
    /// The original uniform plan-view grid (cells of at least 50 mm); kept as the reference.
    Legacy,
    /// The finer index (default).
    #[default]
    Fine,
}

/// Work counters of one validation run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IndexStats {
    /// Narrow-phase candidates examined: every (support, ceiling) triangle pair that the
    /// headroom check takes past the index and examines, plus every triangle the sphere and
    /// support checks (World::supported / World::clearance / laterally_free) take past the
    /// index and test. Deterministic: same inputs, same count.
    pub pairs_examined: u64,
}

/// validate_clearance_for with a chosen index; `diameters_m: None` uses the declared
/// profiles, `Some(ds)` as validate_clearance_diameters does. `backbox` is `None`.
///
/// Every [`SpatialIndex`] gives the same [`ClearanceReport`] (errors and
/// warnings, in the same order, with the same values and messages); only
/// the work done, counted in [`IndexStats`], differs.
#[must_use]
pub fn validate_clearance_indexed(
    table: &LoadedTable,
    scene: &GltfScene,
    diameters_m: Option<&[f32]>,
    index: SpatialIndex,
) -> (ClearanceReport, IndexStats) {
    validate_clearance_indexed_for(table, scene, None, diameters_m, index)
}

fn validate_clearance_for(
    table: &LoadedTable,
    scene: &GltfScene,
    backbox: Option<&GltfScene>,
    diameters: Option<&[f32]>,
) -> ClearanceReport {
    validate_clearance_indexed_for(table, scene, backbox, diameters, SpatialIndex::Fine).0
}

fn validate_clearance_indexed_for(
    table: &LoadedTable,
    scene: &GltfScene,
    backbox: Option<&GltfScene>,
    diameters: Option<&[f32]>,
    index: SpatialIndex,
) -> (ClearanceReport, IndexStats) {
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
    let mut tris = geom::collider_triangles(scene);
    if let Some(bb) = backbox {
        check_backbox_nodes(bb, &mut sink);
        geom::append_collider_triangles(
            bb,
            |m| backbox_node_layer(m).ok(),
            scene.meshes.len(),
            &mut tris,
        );
    }
    let index = geom::TriIndex::new(&tris, index);
    let world = checks::World {
        table,
        scene,
        tris: &tris,
        index: &index,
        pairs: core::cell::Cell::new(0),
        glb: table.manifest.meta.playfield_mesh.as_str(),
        backbox,
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
    let stats = IndexStats {
        pairs_examined: world.pairs.get(),
    };
    (
        ClearanceReport {
            errors: sink.errors,
            warnings: sink.warnings,
        },
        stats,
    )
}

/// Loads a bundle (`load_table`, `load_gltf` of `meta.playfield_mesh` and,
/// when present, [`load_backbox_colliders`]) and runs
/// [`validate_clearance_with_backbox`] on it.
///
/// # Errors
/// Whatever `load_table` / `load_gltf` / [`load_backbox_colliders`] report
/// (manifest validation errors come as [`AssetError::Invalid`]).
pub fn validate_bundle(vfs: &Vfs) -> Result<ClearanceReport, AssetError> {
    let table = load_table(vfs)?;
    let scene = load_gltf(vfs, &table.manifest.meta.playfield_mesh)?;
    let backbox = load_backbox_colliders(vfs)?;
    Ok(validate_clearance_with_backbox(
        &table,
        &scene,
        backbox.as_ref(),
    ))
}

/// #135: the fine spatial index against the legacy grid on whole tables.
#[cfg(test)]
mod index_tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_dir(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    /// The shipped table `name` (`tables/<name>`), generated and loaded as
    /// its `build.sh` does, without the placeholder media kit.
    fn shipped(name: &str) -> (LoadedTable, GltfScene) {
        let src = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tables")
            .join(name);
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir: PathBuf = std::env::temp_dir().join(format!(
            "pin_tablegen_index_{name}_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        copy_dir(&src, &dir);
        let layout =
            crate::layout::parse_layout(&std::fs::read_to_string(src.join("layout.toml")).unwrap())
                .unwrap();
        let mut base = pin_core::schema::TableManifest::from_toml_str(
            &std::fs::read_to_string(src.join("table.toml")).unwrap(),
        )
        .unwrap();
        // Media the validator never reads: the apex fixture's backglass
        // (with a placeholder backbox scene) and no sounds.
        base.sounds.clear();
        let apex = Path::new(env!("CARGO_MANIFEST_DIR")).join("../pin_assets/tests/fixtures/apex");
        copy_dir(&apex.join("backglass"), &dir.join("backglass"));
        std::fs::copy(apex.join("backglass.toml"), dir.join("backglass.toml")).unwrap();
        std::fs::write(dir.join("backbox.glb"), b"placeholder").unwrap();
        let generated = crate::generate(&layout, &base).unwrap_or_else(|e| panic!("{e}"));
        crate::write_bundle(&generated, &dir).unwrap();
        let vfs = Vfs::open(&dir).unwrap();
        let table = load_table(&vfs).unwrap_or_else(|e| panic!("{name}: {e}"));
        let scene = load_gltf(&vfs, &table.manifest.meta.playfield_mesh).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        (table, scene)
    }

    /// A flat collider sheet of `nx × ny` quads (two triangles each) over
    /// `x0..x0 + w`, `y0..y0 + h` at height `z`, on `layer`.
    fn sheet(name: &str, layer: u8, [x0, y0, w, h, z]: [f32; 5], nx: u32, ny: u32) -> MeshData {
        let mut positions = Vec::new();
        for j in 0..=ny {
            for i in 0..=nx {
                let (u, v) = (i as f32 / nx as f32, j as f32 / ny as f32);
                positions.push([x0 + u * w, y0 + v * h, z]);
            }
        }
        let mut indices = Vec::new();
        let at = |i: u32, j: u32| j * (nx + 1) + i;
        for j in 0..ny {
            for i in 0..nx {
                let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
                indices.extend_from_slice(&[a, b, c, a, c, d]);
            }
        }
        MeshData {
            name: name.to_owned(),
            positions,
            indices,
            transform: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
            extras: pin_assets::NodeExtras {
                collision_layer: Some(layer),
                collider: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// A dense synthetic playfield: a `4 k²`-triangle floor (layer 0,
    /// 514 × 1070 mm) under a `k²`-triangle deck (layer 1, `deck_z` m)
    /// over its upper-right quarter.
    fn dense(k: u32, deck_z: f32) -> GltfScene {
        GltfScene {
            meshes: vec![
                sheet("floor", 0, [0.0, 0.0, 0.514, 1.07, 0.0], k, 2 * k),
                sheet("deck", 1, [0.26, 0.7, 0.254, 0.37, deck_z], k / 2, k),
            ],
            ..Default::default()
        }
    }

    /// Debug text of a report (`required_mm` / `actual_mm` may be NaN,
    /// so `PartialEq` alone is not enough).
    fn text(r: &ClearanceReport) -> String {
        format!("{:?}\n{:?}", r.errors, r.warnings)
    }

    /// SPEC 2.8 (#135): on a dense scene, with a deck high enough and with
    /// one too low for the large ball, both indexes give the same report
    /// and the fine one examines far fewer candidates.
    #[test]
    fn dense_scene_gives_identical_reports_and_fewer_pairs() {
        let (table, _) = shipped("greybox");
        let ds = [0.027f32, 0.0794];
        for deck in [0.2f32, 0.06] {
            let scene = dense(41, deck);
            let (fine, fs) =
                validate_clearance_indexed(&table, &scene, Some(&ds), SpatialIndex::Fine);
            let (legacy, ls) =
                validate_clearance_indexed(&table, &scene, Some(&ds), SpatialIndex::Legacy);
            assert_eq!(text(&fine), text(&legacy), "deck at {deck}");
            assert_eq!(
                text(&fine),
                text(&validate_clearance_diameters(&table, &scene, &ds))
            );
            let low = fine
                .errors
                .iter()
                .any(|e| e.feature == Feature::DeckHeadroom);
            assert_eq!(low, deck < 0.1, "{fine:?}");
            assert!(fs.pairs_examined < ls.pairs_examined, "{fs:?} {ls:?}");
            // Deterministic counters.
            let again = validate_clearance_indexed(&table, &scene, Some(&ds), SpatialIndex::Fine);
            assert_eq!(again.1, fs);
        }
    }

    /// SPEC 2.8 (#135): on a uniformly dense floor under a deck the ball
    /// clears, nine times the triangles costs the fine index at most nine
    /// times the candidates; the legacy grid's 50 mm cells cost more than
    /// that (its candidates per support grow with the density).
    #[test]
    fn fine_pairs_grow_at_most_linearly_on_a_dense_floor() {
        let (table, _) = shipped("greybox");
        let run = |k: u32, index: SpatialIndex| {
            let scene = dense(k, 0.2);
            validate_clearance_indexed(&table, &scene, Some(&[0.027]), index)
                .1
                .pairs_examined
        };
        // 5 k² triangles: 8 405 and 75 645.
        let (small, big) = (run(41, SpatialIndex::Fine), run(123, SpatialIndex::Fine));
        assert!(small > 0 && big <= 9 * small, "{small} -> {big}");
        let (small, big) = (
            run(41, SpatialIndex::Legacy),
            run(123, SpatialIndex::Legacy),
        );
        assert!(big > 9 * small, "{small} -> {big}");
    }

    #[test]
    fn index_choice_defaults_to_the_fine_index() {
        assert_eq!(SpatialIndex::default(), SpatialIndex::Fine);
        assert_eq!(IndexStats::default().pairs_examined, 0);
        // An empty scene examines nothing with either index.
        let (table, _) = shipped("greybox");
        let empty = GltfScene::default();
        let (fine, fs) = validate_clearance_indexed(&table, &empty, None, SpatialIndex::Fine);
        let (legacy, ls) = validate_clearance_indexed(&table, &empty, None, SpatialIndex::Legacy);
        assert_eq!(text(&fine), text(&legacy));
        assert_eq!((fs.pairs_examined, ls.pairs_examined), (0, 0));
    }

    /// SPEC 2.8, 6.0 (#135): the four shipped tables get the same report
    /// from either index (declared profiles and a failing diameter sweep),
    /// and the default entry points use the fine one.
    #[test]
    fn shipped_tables_give_identical_reports_with_either_index() {
        for name in ["greybox", "apex_circuit", "sky_deck", "feature_lab"] {
            let (table, scene) = shipped(name);
            let (fine, fs) = validate_clearance_indexed(&table, &scene, None, SpatialIndex::Fine);
            let (legacy, ls) =
                validate_clearance_indexed(&table, &scene, None, SpatialIndex::Legacy);
            assert_eq!(text(&fine), text(&legacy), "{name}");
            assert_eq!(
                text(&fine),
                text(&validate_clearance(&table, &scene)),
                "{name}"
            );
            assert!(
                fs.pairs_examined <= ls.pairs_examined,
                "{name}: {fs:?} {ls:?}"
            );
            // A diameter sweep that fails somewhere on every table.
            let ds = [0.01588f32, 0.027, 0.0794, 0.12];
            let (fine, _) =
                validate_clearance_indexed(&table, &scene, Some(&ds), SpatialIndex::Fine);
            let (legacy, _) =
                validate_clearance_indexed(&table, &scene, Some(&ds), SpatialIndex::Legacy);
            assert!(!fine.errors.is_empty(), "{name}");
            assert_eq!(text(&fine), text(&legacy), "{name}");
            assert_eq!(
                text(&fine),
                text(&validate_clearance_diameters(&table, &scene, &ds))
            );
        }
    }
}
