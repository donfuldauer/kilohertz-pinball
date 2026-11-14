//! Lane, flipper-gap, wireform, scoop / VUK and restriction checks.

use super::geom::{self, Tri, TriGrid, WALKABLE_NZ};
use super::{
    Ball, Feature, Finding, Sink, TABLE, VUK_EXIT_TOLERANCE_MM, WIREFORM_MAX_CONTACT_DEG,
    WIREFORM_MIN_CONTACT_DEG, mm,
};
use glam::{DVec2, DVec3, Vec3};
use pin_assets::{GltfScene, LoadedTable};
use pin_core::PhysicsDefaults;
use pin_core::flipper::FlipperShape;
use pin_core::schema::table::{MechKind, SplineKind};
use pin_physics::ball::MovingBodies;
use pin_physics::flipper::FlipperSet;
use pin_physics::world::CubicSpline;

/// Spacing (m) of the samples along lane rails and wireform centrelines.
const SAMPLE_STEP_M: f64 = 2.0e-3;

/// A guide wall seen in plan view.
pub(super) struct GuideWall {
    pub layer: u8,
    /// The guide's plan view (z = 0).
    pub flat: CubicSpline,
    /// Lowest point of the wall's bottom edge, m.
    pub z_lo: f64,
}

/// What the checks read.
pub(super) struct World<'a> {
    pub table: &'a LoadedTable,
    pub scene: &'a GltfScene,
    pub tris: &'a [Tri],
    pub grid: &'a TriGrid,
    /// The playfield glTF's file name (findings on meshes).
    pub glb: &'a str,
}

impl World<'_> {
    pub fn guide_walls(&self) -> Vec<GuideWall> {
        let mut out = Vec::new();
        for s in &self.table.splines {
            if s.kind != SplineKind::Guide {
                continue;
            }
            let flat: Vec<Vec3> = s
                .points_m
                .iter()
                .map(|p| Vec3::new(p[0], p[1], 0.0))
                .collect();
            let Some(spline) = geom::build_spline(&flat, s.closed) else {
                continue;
            };
            let z_lo = s
                .points_m
                .iter()
                .map(|p| f64::from(p[2]))
                .fold(f64::INFINITY, f64::min);
            out.push(GuideWall {
                layer: s.layer,
                flat: spline,
                z_lo,
            });
        }
        out
    }

    /// Smallest distance from `c` to the colliders of `layer` accepted by
    /// `keep`, looked for within `reach` (returns `reach` when none is
    /// closer). Guide walls of the layer count when their bottom edge is
    /// below `c.z + top` (their height is not known here: taken as tall).
    fn clearance(
        &self,
        guides: &[GuideWall],
        layer: u8,
        c: DVec3,
        reach: f64,
        top: f64,
        keep: impl Fn(&Tri, DVec3) -> bool,
    ) -> f64 {
        let mut best = reach;
        let mut idx = Vec::new();
        let r = DVec2::splat(reach);
        self.grid
            .query(self.tris, c.truncate() - r, c.truncate() + r, &mut idx);
        for &i in &idx {
            let t = &self.tris[i as usize];
            if t.layer != layer || t.min.z > c.z + reach || t.max.z < c.z - reach {
                continue;
            }
            let q = t.closest(c);
            if keep(t, q) {
                best = best.min(q.distance(c));
            }
        }
        for g in guides
            .iter()
            .filter(|g| g.layer == layer && g.z_lo <= c.z + top)
        {
            let cp = g
                .flat
                .closest_point(Vec3::new(c.x as f32, c.y as f32, 0.0), None);
            if cp.distance.is_finite() {
                best = best.min(f64::from(cp.distance));
            }
        }
        best
    }
}

fn vec3(p: [f32; 3]) -> Vec3 {
    Vec3::from_array(p)
}

/// Lane widths: guide pairs `<base>.left` / `<base>.right` on one layer.
pub(super) fn lanes(w: &World<'_>, balls: &[Ball], sink: &mut Sink) {
    let splines = &w.table.splines;
    for (i, left) in splines.iter().enumerate() {
        if left.kind != SplineKind::Guide {
            continue;
        }
        let Some(base) = left.name.strip_suffix(".left") else {
            continue;
        };
        let Some(right) = splines.iter().find(|o| {
            o.kind == SplineKind::Guide
                && o.layer == left.layer
                && o.name.strip_suffix(".right") == Some(base)
        }) else {
            continue;
        };
        let pts =
            |s: &pin_assets::SplineSi| s.points_m.iter().map(|&p| vec3(p)).collect::<Vec<_>>();
        let (Some(ls), Some(rs)) = (
            geom::build_spline(&pts(left), left.closed),
            geom::build_spline(&pts(right), right.closed),
        ) else {
            continue;
        };
        // (width, plan midpoint) per left-rail sample.
        let mut seed = None;
        let widths: Vec<(f64, DVec2)> = geom::sample_params(&ls, SAMPLE_STEP_M)
            .into_iter()
            .filter_map(|t| {
                let p = ls.point(t);
                let cp = rs.closest_point(p, seed);
                seed = Some(cp.t);
                let d = f64::from(cp.distance);
                d.is_finite()
                    .then(|| (d, ((p + cp.point) * 0.5).as_dvec3().truncate()))
            })
            .collect();
        for ball in balls {
            if !ball.allowed.layer(left.layer) {
                continue;
            }
            let narrowest = widths
                .iter()
                .filter(|(_, mid)| ball.allowed.at(left.layer, *mid))
                .map(|(d, _)| *d)
                .fold(f64::INFINITY, f64::min);
            if narrowest < ball.needed() {
                sink.error(Finding {
                    feature: Feature::Lane,
                    name: base,
                    ball: &ball.name,
                    file: TABLE,
                    path: format!("splines[{i}]"),
                    required_m: ball.needed(),
                    actual_m: narrowest,
                    message: format!(
                        "lane {base:?} (guides {:?} / {:?}, layer {}) is {} wide; ball {:?} ({}) needs {}",
                        left.name,
                        right.name,
                        left.layer,
                        mm(narrowest),
                        ball.name,
                        mm(ball.d),
                        mm(ball.needed())
                    ),
                });
            }
        }
    }
}

/// How far (m) a bat reaches from its pivot (upper bound).
fn bat_reach(shape: &FlipperShape) -> f64 {
    let v = match *shape {
        FlipperShape::StraightCapsule {
            length_mm,
            base_radius_mm,
            tip_radius_mm,
        } => length_mm + base_radius_mm.max(tip_radius_mm),
        FlipperShape::CurvedBanana {
            length_mm,
            arc_radius_mm,
            tube_radius_mm,
            ..
        } => length_mm + arc_radius_mm.min(length_mm) + tube_radius_mm,
    };
    let v = f64::from(v) * 1.0e-3;
    if v.is_finite() { v.max(0.0) } else { 0.0 }
}

/// Most coarse-grid points per axis of the flipper-gap search.
const GAP_GRID_SIDE: f64 = 300.0;

/// The narrowest gap between two bats at rest: `(gap, midpoint)`, from
/// minimising `d_a(x) + d_b(x)` over the mid-height plane of bat `a`.
fn bat_gap(
    a: &FlipperSet,
    b: &FlipperSet,
    normal: Vec3,
    reach: (f64, f64),
) -> Option<(f64, DVec3)> {
    let id_a = a.id_of(0)?;
    let id_b = b.id_of(0)?;
    let pa = a.pivot_m(id_a)?.as_dvec3();
    let pb = b.pivot_m(id_b)?.as_dvec3();
    let dir = a.bat_direction(id_a)?;
    let n = normal.as_dvec3().normalize_or_zero();
    let u = dir.as_dvec3();
    let v = n.cross(u);
    if !(n.is_finite() && v.length_squared() > 0.5) {
        return None;
    }
    let lift = n * (0.5 * f64::from(a.tuning().bat_height_m));
    let to = |q: DVec2| pa + lift + u * q.x + v * q.y;
    let f = |q: DVec2| -> f64 {
        let x = to(q).as_vec3();
        match (a.closest_point(0, 0.0, x), b.closest_point(0, 0.0, x)) {
            (Some(da), Some(db)) => f64::from(da.distance) + f64::from(db.distance),
            _ => f64::INFINITY,
        }
    };
    let rel = DVec2::new((pb - pa).dot(u), (pb - pa).dot(v));
    let lo = DVec2::splat(-reach.0).min(rel - DVec2::splat(reach.1));
    let hi = DVec2::splat(reach.0).max(rel + DVec2::splat(reach.1));
    let step = ((hi - lo).max_element() / GAP_GRID_SIDE).max(1.0e-3);
    let (nx, ny) = (
        ((hi.x - lo.x) / step).ceil() as usize,
        ((hi.y - lo.y) / step).ceil() as usize,
    );
    let mut best = (f64::INFINITY, DVec2::ZERO);
    for j in 0..=ny {
        for i in 0..=nx {
            let q = lo + DVec2::new(i as f64, j as f64) * step;
            let val = f(q);
            if val < best.0 {
                best = (val, q);
            }
        }
    }
    if !best.0.is_finite() {
        return None;
    }
    // Pattern search down to 1 µm.
    let mut h = step;
    while h > 1.0e-6 {
        let mut moved = false;
        for d in [
            DVec2::X,
            DVec2::NEG_X,
            DVec2::Y,
            DVec2::NEG_Y,
            DVec2::ONE,
            -DVec2::ONE,
            DVec2::new(1.0, -1.0),
            DVec2::new(-1.0, 1.0),
        ] {
            let q = best.1 + d * h;
            let val = f(q);
            if val < best.0 {
                best = (val, q);
                moved = true;
            }
        }
        if !moved {
            h *= 0.5;
        }
    }
    Some((best.0, to(best.1)))
}

/// Gaps between flippers on the same layer, both at rest.
pub(super) fn flippers(w: &World<'_>, balls: &[Ball], sink: &mut Sink) {
    let cfgs = &w.table.flippers;
    let name = |i: usize| -> String {
        w.table
            .manifest
            .flippers
            .get(i)
            .map_or_else(|| format!("flipper {}", cfgs[i].id.0), |d| d.id.clone())
    };
    let built: Vec<Option<FlipperSet>> = cfgs
        .iter()
        .map(|c| FlipperSet::new(core::slice::from_ref(c)).ok())
        .collect();
    for (i, c) in cfgs.iter().enumerate() {
        let partner = cfgs
            .iter()
            .enumerate()
            .any(|(j, o)| j != i && o.collision_layer == c.collision_layer);
        if built[i].is_none() && partner {
            if let Err(e) = FlipperSet::new(core::slice::from_ref(c)) {
                let n = name(i);
                sink.warning(Finding {
                    feature: Feature::FlipperGap,
                    name: &n,
                    ball: "",
                    file: TABLE,
                    path: format!("flippers[{i}]"),
                    required_m: f64::NAN,
                    actual_m: f64::NAN,
                    message: format!(
                        "flipper {n:?}: gaps to other flippers not checked (the physics cannot build it: {e})"
                    ),
                });
            }
        }
    }
    let largest = balls.iter().map(Ball::needed).fold(0.0, f64::max);
    for i in 0..cfgs.len() {
        for j in i + 1..cfgs.len() {
            if cfgs[i].collision_layer != cfgs[j].collision_layer {
                continue;
            }
            let (Some(a), Some(b)) = (&built[i], &built[j]) else {
                continue;
            };
            let reach = (bat_reach(&cfgs[i].shape), bat_reach(&cfgs[j].shape));
            let pivots = DVec3::from_array(cfgs[i].base_position_mm.map(f64::from))
                .distance(DVec3::from_array(cfgs[j].base_position_mm.map(f64::from)))
                * 1.0e-3;
            if !(pivots - reach.0 - reach.1 < largest) {
                continue;
            }
            let normal = Vec3::from_array(cfgs[i].surface_normal);
            let Some((gap, mid)) = bat_gap(a, b, normal, reach) else {
                continue;
            };
            if !(gap > 0.0) {
                continue;
            }
            let layer = cfgs[i].collision_layer;
            let pair = format!("{} / {}", name(i), name(j));
            for ball in balls {
                if gap < ball.needed() && ball.allowed.at(layer, mid.truncate()) {
                    sink.error(Finding {
                        feature: Feature::FlipperGap,
                        name: &pair,
                        ball: &ball.name,
                        file: TABLE,
                        path: format!("flippers[{i}]"),
                        required_m: ball.needed(),
                        actual_m: gap,
                        message: format!(
                            "flippers {pair:?} (layer {layer}) leave a {} gap at rest; ball {:?} ({}) would wedge in it (needs {})",
                            mm(gap),
                            ball.name,
                            mm(ball.d),
                            mm(ball.needed())
                        ),
                    });
                }
            }
        }
    }
}

/// Wireform gauges: the contact angle of the ball on the rails.
pub(super) fn wireforms(w: &World<'_>, balls: &[Ball], sink: &mut Sink) {
    let lo_sin = libm::sin(f64::from(WIREFORM_MIN_CONTACT_DEG).to_radians());
    let hi_sin = libm::sin(f64::from(WIREFORM_MAX_CONTACT_DEG).to_radians());
    for (i, s) in w.table.splines.iter().enumerate() {
        if s.kind != SplineKind::Wireform {
            continue;
        }
        let Some(g) = s
            .gauge_m
            .map(f64::from)
            .filter(|g| g.is_finite() && *g > 0.0)
        else {
            continue;
        };
        let pts: Vec<Vec3> = s.points_m.iter().map(|&p| vec3(p)).collect();
        let samples: Vec<DVec2> = match geom::build_spline(&pts, s.closed) {
            Some(sp) => geom::sample_params(&sp, 5.0 * SAMPLE_STEP_M)
                .into_iter()
                .map(|t| sp.point(t).as_dvec3().truncate())
                .collect(),
            None => pts.iter().map(|p| p.as_dvec3().truncate()).collect(),
        };
        for ball in balls {
            if !samples.iter().any(|&q| ball.allowed.at(s.layer, q)) {
                continue;
            }
            let (min_g, max_g) = (ball.d * lo_sin, ball.d * hi_sin);
            let sin = (g / ball.d).min(1.0);
            let alpha = libm::asin(sin).to_degrees();
            let (required, what) = if g < min_g {
                (
                    min_g,
                    format!(
                        "is too narrow: the ball perches on the rails at {alpha:.1}° from vertical (at least {WIREFORM_MIN_CONTACT_DEG}° needs a gauge of at least {})",
                        mm(min_g)
                    ),
                )
            } else if g > max_g {
                let how = if g >= ball.d {
                    "drops through the rails".to_owned()
                } else {
                    format!("sits {alpha:.1}° deep between the rails")
                };
                (
                    max_g,
                    format!(
                        "is too wide: the ball {how} (at most {WIREFORM_MAX_CONTACT_DEG}° needs a gauge of at most {})",
                        mm(max_g)
                    ),
                )
            } else {
                continue;
            };
            sink.error(Finding {
                feature: Feature::Wireform,
                name: &s.name,
                ball: &ball.name,
                file: TABLE,
                path: format!("splines[{i}]"),
                required_m: required,
                actual_m: g,
                message: format!(
                    "wireform {:?} (layer {}, gauge {}) {what} for ball {:?} ({})",
                    s.name,
                    s.layer,
                    mm(g),
                    ball.name,
                    mm(ball.d)
                ),
            });
        }
    }
}

/// Scoop pockets and VUK exits.
pub(super) fn vuks(w: &World<'_>, balls: &[Ball], sink: &mut Sink) {
    let defaults = PhysicsDefaults::builtin().mechs.vuk;
    let guides = w.guide_walls();
    for (i, m) in w.table.manifest.mechs.iter().enumerate() {
        if m.kind != MechKind::Vuk {
            continue;
        }
        let Some(pos) = m.position else { continue };
        let pos = DVec3::from_array(pos.map(f64::from)) * 1.0e-3;
        if !pos.is_finite() {
            continue;
        }
        let p = m.params();
        let layer = p.layer.unwrap_or(0);
        let exit_layer = p.exit_layer.unwrap_or(defaults.exit_layer);
        let offset = p.exit_offset.map_or(
            DVec3::from_array(defaults.exit_offset.map(f64::from)),
            |o| DVec3::from_array(o.map(f64::from)) * 1.0e-3,
        );
        for ball in balls {
            if !ball.allowed.at(layer, pos.truncate()) {
                continue;
            }
            let r = ball.r();
            // Scoop pocket: walls (steeper than 45°) and anything above the
            // held ball's centre.
            let c = pos + DVec3::Z * r;
            let half = 0.5 * ball.needed();
            let free = w.clearance(&guides, layer, c, half + 1.0e-3, r, |t, q| {
                t.nz() < WALKABLE_NZ || q.z > c.z
            });
            if 2.0 * free < ball.needed() {
                sink.error(Finding {
                    feature: Feature::Scoop,
                    name: &m.id,
                    ball: &ball.name,
                    file: TABLE,
                    path: format!("mechs[{i}]"),
                    required_m: ball.needed(),
                    actual_m: 2.0 * free,
                    message: format!(
                        "scoop of vuk {:?} (layer {layer}) is {} across at the hold point; ball {:?} ({}) needs {}",
                        m.id,
                        mm(2.0 * free),
                        ball.name,
                        mm(ball.d),
                        mm(ball.needed())
                    ),
                });
            }
            // Exit: the ejected ball must not overlap its exit layer.
            let e = pos + offset;
            if e.is_finite() {
                let tol = f64::from(VUK_EXIT_TOLERANCE_MM) * 1.0e-3;
                let need = r - tol;
                let gap = w.clearance(&guides, exit_layer, e, r + 1.0e-3, r, |_, _| true);
                if gap < need {
                    sink.error(Finding {
                        feature: Feature::VukExit,
                        name: &m.id,
                        ball: &ball.name,
                        file: TABLE,
                        path: format!("mechs[{i}].exit_offset"),
                        required_m: need,
                        actual_m: gap,
                        message: format!(
                            "vuk {:?} ejects ball {:?} ({}) to a point {} from layer {exit_layer}'s geometry; it overlaps it by {} (at most {} allowed)",
                            m.id,
                            ball.name,
                            mm(ball.d),
                            mm(gap),
                            mm(r - gap),
                            mm(tol)
                        ),
                    });
                }
            }
            if !ball.allowed.layer(exit_layer) {
                sink.warning(Finding {
                    feature: Feature::Restriction,
                    name: &m.id,
                    ball: &ball.name,
                    file: TABLE,
                    path: format!("mechs[{i}].exit_layer"),
                    required_m: f64::NAN,
                    actual_m: f64::NAN,
                    message: format!(
                        "vuk {:?} can capture ball {:?} on layer {layer} and eject it onto layer {exit_layer}, which its restriction does not allow",
                        m.id, ball.name
                    ),
                });
            }
        }
    }
}

/// Layer transitions that lead a restricted ball off its layers.
pub(super) fn transitions(w: &World<'_>, balls: &[Ball], sink: &mut Sink) {
    for (i, t) in w.table.transitions.iter().enumerate() {
        let lo = DVec2::new(f64::from(t.zone_min_m[0]), f64::from(t.zone_min_m[1]));
        let hi = DVec2::new(f64::from(t.zone_max_m[0]), f64::from(t.zone_max_m[1]));
        for ball in balls.iter().filter(|b| b.allowed.restricted()) {
            if ball.allowed.in_box(t.from_layer, lo, hi) && !ball.allowed.layer(t.to_layer) {
                sink.warning(Finding {
                    feature: Feature::Restriction,
                    name: &t.name,
                    ball: &ball.name,
                    file: TABLE,
                    path: format!("layer_transitions[{i}]"),
                    required_m: f64::NAN,
                    actual_m: f64::NAN,
                    message: format!(
                        "layer transition {:?} ({} -> {}) is within ball {:?}'s restriction and leads to layer {}, which it does not allow",
                        t.name, t.from_layer, t.to_layer, ball.name, t.to_layer
                    ),
                });
            }
        }
    }
}
