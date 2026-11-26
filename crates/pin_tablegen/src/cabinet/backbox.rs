//! The cabinet's backbox (`cabinet.backbox.*`) and the inside art blades
//! (`cabinet.blade.*`) as millimetre meshes in the cabinet frame with `z`
//! above the floor (REQ-CAB-001, REQ-CAB-003).
//!
//! Every size and position comes from `[backbox]`, `[body]` and `[mount]`;
//! the only constants are the design values below (docs/DECISIONS.md
//! "#111 KP-030 — Backbox model: nodes, materials and design values").
//! Not to be confused with `crate::backbox`, the Subphase 5.5 mini
//! playfield.
//!
//! # Face coordinates
//!
//! The backbox front ("the face") is addressed by `(u, v, w)`: `u` across
//! from the backbox centreline (right positive), `v` up the face from its
//! bottom edge, both in the face plane, and `w` the depth behind the face
//! along its inward normal. A flat front is the plane `y = front_y`; an
//! angled one leans back [`FRONT_LEAN_DEG`] from vertical about its bottom
//! edge. The backbox bottom is level at `zb = floor_height_mm(front_y,
//! bottom_z)`.

use super::mesh::{MeshMm, P3};
use super::parts::{Part, slab};
use super::{err, finite, non_negative, positive};
use crate::GenError;
use pin_core::fmath::{max_f64, min_f64};
use pin_core::schema::cabinet::{
    BackboxFrontProfile, BackboxTopProfile, CabinetGeometry, MAX_GRILLES,
};

/// Lean of an angled backbox front from vertical, degrees.
pub(crate) const FRONT_LEAN_DEG: f64 = 10.0;
/// Slope of an angled backbox top, down toward the rear, degrees.
pub(crate) const TOP_SLOPE_DEG: f64 = 10.0;
/// Sag of an arched top at its sides as a fraction of the backbox width.
pub(crate) const ARCH_SAG_FRACTION: f64 = 0.06;
/// Segments of an arched top across the width.
pub(crate) const ARCH_SEGMENTS: usize = 32;
/// How far the score-display quad sits behind the face, mm.
pub(crate) const DISPLAY_RECESS: f64 = 5.0;
/// How far a speaker grille stands in front of the face, mm.
pub(crate) const GRILLE_PROUD: f64 = 2.0;
/// Segments of a speaker grille disc.
pub(crate) const GRILLE_SEGMENTS: usize = 32;
/// Hinge bracket plate width (across), thickness (along `y`) and reach
/// above the backbox bottom, mm.
pub(crate) const HINGE_WIDTH: f64 = 50.0;
/// Hinge bracket plate thickness along `y`, mm.
pub(crate) const HINGE_THICKNESS: f64 = 6.0;
/// Hinge bracket reach above the backbox bottom, mm.
pub(crate) const HINGE_REACH: f64 = 80.0;
/// Art blade panel thickness, mm.
pub(crate) const BLADE_THICKNESS: f64 = 3.0;
/// Largest playfield pitch magnitude the blades accept, degrees.
const MAX_PITCH_DEG: f64 = 89.0;
/// Smallest blade height (top over bottom edge), mm.
const MIN_BLADE_HEIGHT: f64 = 1.0;

fn rad(deg: f64) -> f64 {
    deg * core::f64::consts::PI / 180.0
}

/// The backbox front and top in the cabinet frame, mm.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Face {
    /// Width `W`.
    pub w: f64,
    /// Height `H`.
    pub h: f64,
    /// Depth `D`.
    pub d: f64,
    /// Panel and plate thickness `T` (`body.wall_thickness`).
    pub t: f64,
    /// `y` of the face's bottom edge (`front_y`).
    pub y0: f64,
    /// Height of the level backbox bottom above the floor.
    pub zb: f64,
    /// Sine and cosine of the front lean (0 and 1 for a flat front).
    pub s: f64,
    /// Cosine of the front lean.
    pub c: f64,
    /// Tangent of the top slope (0 unless the top is angled).
    pub k: f64,
    /// `y` of the face's top edge.
    pub y_ft: f64,
    /// Arched-top sag at the sides (0 unless the top is arched).
    pub sag: f64,
}

impl Face {
    /// The face of `g`'s backbox.
    pub(crate) fn new(g: &CabinetGeometry) -> Face {
        let bb = &g.backbox;
        let (s, c) = match bb.front_profile {
            BackboxFrontProfile::Flat => (0.0, 1.0),
            BackboxFrontProfile::Angled => {
                let a = rad(FRONT_LEAN_DEG);
                (libm::sin(a), libm::cos(a))
            }
        };
        let (w, h) = (f64::from(bb.width), f64::from(bb.height));
        let y0 = f64::from(bb.front_y);
        Face {
            w,
            h,
            d: f64::from(bb.depth),
            t: f64::from(g.body.wall_thickness),
            y0,
            zb: g.floor_height_mm(y0, f64::from(bb.bottom_z)),
            s,
            c,
            k: match bb.top_profile {
                BackboxTopProfile::Angled => libm::tan(rad(TOP_SLOPE_DEG)),
                _ => 0.0,
            },
            y_ft: y0 + h * s / c,
            sag: match bb.top_profile {
                BackboxTopProfile::Arched => ARCH_SAG_FRACTION * w,
                _ => 0.0,
            },
        }
    }

    /// The cabinet-frame point at face coordinates `(u, v)`, `w` behind the
    /// face.
    pub(crate) fn p(&self, u: f64, v: f64, w: f64) -> P3 {
        [
            u,
            self.y0 + v * self.s + w * self.c,
            self.zb + v * self.c - w * self.s,
        ]
    }

    /// Face coordinates `(u, v)` of a cabinet-frame point (projected on the
    /// face plane along its normal).
    pub(crate) fn uv_of(&self, p: P3) -> (f64, f64) {
        (p[0], (p[1] - self.y0) * self.s + (p[2] - self.zb) * self.c)
    }

    /// Top height above `zb` at the front top edge, at `x` (the arch).
    pub(crate) fn htop(&self, x: f64) -> f64 {
        let r = 2.0 * x / self.w;
        self.h - self.sag * r * r
    }

    /// The top surface's height above the floor at `(x, y)`.
    pub(crate) fn z_top(&self, x: f64, y: f64) -> f64 {
        self.zb + self.htop(x) - (y - self.y_ft) * self.k
    }

    /// `v` where the plane `w` behind the face meets the top surface
    /// lowered by `dz`, at `u = x`.
    pub(crate) fn v_top(&self, x: f64, w: f64, dz: f64) -> f64 {
        (self.htop(x) - dz + w * self.s - (self.y0 + w * self.c - self.y_ft) * self.k)
            / (self.c + self.s * self.k)
    }

    /// `v` of the point at height `z` on the plane `w` behind the face.
    pub(crate) fn v_at_z(&self, z: f64, w: f64) -> f64 {
        (z - self.zb + w * self.s) / self.c
    }

    /// `y` of the backbox rear.
    pub(crate) fn rear(&self) -> f64 {
        self.y0 + self.d
    }

    /// The `x` samples of the top across the width (one segment unless the
    /// top is arched).
    fn top_samples(&self) -> Vec<f64> {
        let hw = self.w * 0.5;
        if self.sag > 0.0 {
            (0..=ARCH_SEGMENTS)
                .map(|i| -hw + self.w * i as f64 / ARCH_SEGMENTS as f64)
                .collect()
        } else {
            vec![-hw, hw]
        }
    }
}

/// A plate edge's `v`: the bottom (level with `zb` on every plane), a fixed
/// `v`, or the top surface.
#[derive(Clone, Copy)]
enum Edge {
    Bottom,
    Fixed(f64),
    Top,
}

/// The edge's `v` on the plane `w` behind the face, never below the level
/// bottom: on an angled front a hole edge closer to the face bottom than
/// `w * tan(lean)` would otherwise put the plate's rear below `zb`.
fn edge_v(f: &Face, e: Edge, u: f64, w: f64) -> f64 {
    let bottom = f.v_at_z(f.zb, w);
    match e {
        Edge::Bottom => bottom,
        Edge::Fixed(v) => max_f64(v, bottom),
        Edge::Top => max_f64(f.v_top(u, w, 0.0), bottom),
    }
}

/// One closed column piece of a plate: `u` in `ua..ub`, `v` from `lo` to
/// `hi`, `w` from 0 to `T` (skipped when it has no height on the face).
fn plate_piece(m: &mut MeshMm, f: &Face, ua: f64, ub: f64, lo: Edge, hi: Edge) {
    let height = |u: f64| edge_v(f, hi, u, 0.0) - edge_v(f, lo, u, 0.0);
    if ub - ua <= 1e-9 || min_f64(height(ua), height(ub)) <= 1e-9 {
        return;
    }
    // Counter-clockwise in (u, v), i.e. about the outward normal.
    let ring = |w: f64| -> Vec<P3> {
        vec![
            f.p(ua, edge_v(f, lo, ua, w), w),
            f.p(ub, edge_v(f, lo, ub, w), w),
            f.p(ub, edge_v(f, hi, ub, w), w),
            f.p(ua, edge_v(f, hi, ua, w), w),
        ]
    };
    m.loft(&[ring(f.t), ring(0.0)]);
}

/// Sorted, de-duplicated column breakpoints across the face: the sides,
/// the hole's sides `±hole_half` and `extra`.
fn breakpoints(f: &Face, hole_half: f64, extra: &[f64]) -> Vec<f64> {
    let hw = f.w * 0.5;
    let mut us = vec![-hw, hw, -min_f64(hole_half, hw), min_f64(hole_half, hw)];
    us.extend_from_slice(extra);
    us.sort_by(f64::total_cmp);
    us.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    us
}

/// A front plate `T` thick from `lo` to `hi` across the whole width with a
/// rectangular through hole `u` in `±hole_half`, `v` in `hole_lo..hole_hi`.
fn plate(f: &Face, us: &[f64], lo: Edge, hi: Edge, hole: (f64, f64, f64)) -> MeshMm {
    let (hole_half, hole_lo, hole_hi) = hole;
    let mut m = MeshMm::default();
    for pair in us.windows(2) {
        let (ua, ub) = (pair[0], pair[1]);
        if ((ua + ub) * 0.5).abs() < hole_half {
            plate_piece(&mut m, f, ua, ub, lo, Edge::Fixed(hole_lo));
            plate_piece(&mut m, f, ua, ub, Edge::Fixed(hole_hi), hi);
        } else {
            plate_piece(&mut m, f, ua, ub, lo, hi);
        }
    }
    m
}

/// `cabinet.backbox.shell`: bottom, left, right, rear and top panels, each
/// `T` thick, behind the front plates.
pub(crate) fn shell(g: &CabinetGeometry) -> Part {
    let f = Face::new(g);
    let (t, yr, zb, hw) = (f.t, f.rear(), f.zb, f.w * 0.5);
    // Where the front plates' rear plane meets a height z, and the top.
    let back_y = |z: f64| f.p(0.0, f.v_at_z(z, t), t)[1];
    let back_top = |x: f64, dz: f64| f.p(x, f.v_top(x, t, dz), t);
    let mut m = MeshMm::default();
    // Rings across x, counter-clockwise in (y, z), i.e. about +x.
    let (yb0, yb1) = (back_y(zb), back_y(zb + t));
    let bottom = |x: f64| vec![[x, yb0, zb], [x, yr, zb], [x, yr, zb + t], [x, yb1, zb + t]];
    m.loft(&[bottom(-hw), bottom(hw)]);
    let side = |x: f64| {
        vec![
            [x, yb1, zb + t],
            [x, yr, zb + t],
            [x, yr, f.z_top(x, yr) - t],
            back_top(x, t),
        ]
    };
    m.loft(&[side(-hw), side(-hw + t)]);
    m.loft(&[side(hw - t), side(hw)]);
    let rear = |x: f64| {
        vec![
            [x, yr - t, zb + t],
            [x, yr, zb + t],
            [x, yr, f.z_top(x, yr) - t],
            [x, yr - t, f.z_top(x, yr - t) - t],
        ]
    };
    let top = |x: f64| {
        vec![
            back_top(x, t),
            [x, yr, f.z_top(x, yr) - t],
            [x, yr, f.z_top(x, yr)],
            back_top(x, 0.0),
        ]
    };
    for pair in f.top_samples().windows(2) {
        m.loft(&[rear(pair[0]), rear(pair[1])]);
        m.loft(&[top(pair[0]), top(pair[1])]);
    }
    Part::fixed(m)
}

/// `cabinet.backbox.speaker_panel`: the front plate below
/// `speaker_panel_height` with the score-display window through it.
pub(crate) fn speaker_panel(g: &CabinetGeometry) -> Part {
    let f = Face::new(g);
    let sd = &g.backbox.score_display;
    let half = f64::from(sd.width) * 0.5;
    let (cv, dh) = (f64::from(sd.centre_v), f64::from(sd.height) * 0.5);
    let us = breakpoints(&f, half, &[]);
    let sph = f64::from(g.backbox.speaker_panel_height);
    Part::fixed(plate(
        &f,
        &us,
        Edge::Bottom,
        Edge::Fixed(sph),
        (half, cv - dh, cv + dh),
    ))
}

/// `cabinet.backbox.translite_frame`: the front plate from
/// `speaker_panel_height` up to the top with the translite opening through
/// it.
pub(crate) fn translite_frame(g: &CabinetGeometry) -> Part {
    let f = Face::new(g);
    let o = &g.backbox.translite_opening;
    let half = f64::from(o.width) * 0.5;
    let lo = f64::from(o.bottom);
    let samples = f.top_samples();
    let us = breakpoints(&f, half, &samples);
    let sph = f64::from(g.backbox.speaker_panel_height);
    Part::fixed(plate(
        &f,
        &us,
        Edge::Fixed(sph),
        Edge::Top,
        (half, lo, lo + f64::from(o.height)),
    ))
}

/// A face-parallel quad's UVs: `(0, 0)` at its upper-left corner seen from
/// the front, `(1, 1)` at its lower-right.
fn quad_uvs(m: &mut MeshMm, f: &Face, u0: f64, v_top: f64, width: f64, height: f64) {
    m.set_uvs(|p| {
        let (u, v) = f.uv_of(p);
        [(u - u0) / width, (v_top - v) / height]
    });
}

/// `cabinet.backbox.translite`: the backglass sheet behind the translite
/// frame, centred on the opening, its front face UV-mapped to the image.
pub(crate) fn translite(g: &CabinetGeometry) -> Part {
    let f = Face::new(g);
    let bb = &g.backbox;
    let (bw, bh) = (
        f64::from(bb.backglass.width),
        f64::from(bb.backglass.height),
    );
    let th = f64::from(bb.backglass.thickness);
    let o = &bb.translite_opening;
    let vc = f64::from(o.bottom) + f64::from(o.height) * 0.5;
    let (u0, u1, v0, v1) = (-bw * 0.5, bw * 0.5, vc - bh * 0.5, vc + bh * 0.5);
    let ring = |w: f64| -> Vec<P3> {
        vec![
            f.p(u0, v0, w),
            f.p(u1, v0, w),
            f.p(u1, v1, w),
            f.p(u0, v1, w),
        ]
    };
    let mut m = MeshMm::default();
    m.loft(&[ring(f.t + th), ring(f.t)]);
    quad_uvs(&mut m, &f, u0, v1, bw, bh);
    Part::fixed(m)
}

/// `cabinet.backbox.display`: one quad in the score-display window facing
/// the player, [`DISPLAY_RECESS`] behind the face, UV-mapped to the image.
pub(crate) fn display(g: &CabinetGeometry) -> Part {
    let f = Face::new(g);
    let sd = &g.backbox.score_display;
    let (dw, dh) = (f64::from(sd.width), f64::from(sd.height));
    let cv = f64::from(sd.centre_v);
    let (u0, u1, v0, v1) = (-dw * 0.5, dw * 0.5, cv - dh * 0.5, cv + dh * 0.5);
    let r = DISPLAY_RECESS;
    let mut m = MeshMm::default();
    m.polygon(&[
        f.p(u0, v0, r),
        f.p(u1, v0, r),
        f.p(u1, v1, r),
        f.p(u0, v1, r),
    ]);
    quad_uvs(&mut m, &f, u0, v1, dw, dh);
    Part::fixed(m)
}

/// `cabinet.backbox.grilles`: one disc per grille, standing
/// [`GRILLE_PROUD`] in front of the face (empty with no grilles).
pub(crate) fn grilles(g: &CabinetGeometry) -> Part {
    let f = Face::new(g);
    let mut m = MeshMm::default();
    for gr in &g.backbox.grilles {
        let (u, v) = (f64::from(gr.u), f64::from(gr.v));
        let r = f64::from(gr.diameter) * 0.5;
        // Counter-clockwise in (u, v): about the outward normal, the
        // direction from the first ring (on the face) to the second.
        let ring = |w: f64| -> Vec<P3> {
            (0..GRILLE_SEGMENTS)
                .map(|i| {
                    let a = 2.0 * core::f64::consts::PI * i as f64 / GRILLE_SEGMENTS as f64;
                    f.p(u + r * libm::cos(a), v + r * libm::sin(a), w)
                })
                .collect()
        };
        m.loft(&[ring(0.0), ring(-GRILLE_PROUD)]);
    }
    Part::fixed(m)
}

/// `cabinet.backbox.hinge`: two bracket plates, mirrored about `x = 0`, at
/// the backbox rear, from the lower of the backbox bottom and the body's
/// rear wall top up to [`HINGE_REACH`] above the backbox bottom.
pub(crate) fn hinge(g: &CabinetGeometry) -> Part {
    let f = Face::new(g);
    let rear_top = g.floor_height_mm(f64::from(g.body.length), f64::from(g.body.rear_height));
    let (z0, z1) = (min_f64(f.zb, rear_top), f.zb + HINGE_REACH);
    let yr = f.rear();
    let y0 = max_f64(f.y0, yr - HINGE_THICKNESS);
    let half = min_f64(f.w, f64::from(g.body.width)) * 0.5;
    let x_out = max_f64(0.0, half - f.t);
    let x_in = max_f64(0.0, x_out - HINGE_WIDTH);
    let mut m = MeshMm::default();
    m.aabb([x_in, y0, z0], [x_out, yr, z1]);
    m.aabb([-x_out, y0, z0], [-x_in, yr, z1]);
    Part::fixed(m)
}

/// The blades' bottom (the playfield plane at pitch, roll ignored) and top
/// (two glass-channel widths under the side wall top) above the floor at
/// `y`.
fn blade_edges(g: &CabinetGeometry) -> impl Fn(f64) -> (f64, f64) + '_ {
    let b = &g.body;
    let (fh, rh, l) = (
        f64::from(b.front_height),
        f64::from(b.rear_height),
        f64::from(b.length),
    );
    let cw = f64::from(b.glass_channel.width);
    let mt = &g.mount;
    let (inset, mh) = (f64::from(mt.inset), f64::from(mt.height));
    let tp = libm::tan(rad(f64::from(mt.pitch_deg)));
    move |y: f64| {
        let w_top = fh + (rh - fh) * y / l;
        (
            g.floor_height_mm(y, mh + (y - inset) * tp),
            g.floor_height_mm(y, w_top - 2.0 * cw),
        )
    }
}

/// `cabinet.blade.left` and `cabinet.blade.right`: [`BLADE_THICKNESS`]
/// panels on the side walls' inner faces from the playfield front
/// (`mount.inset`) to the rear wall, between the playfield plane and the
/// glass channel.
pub(crate) fn blades(g: &CabinetGeometry) -> [Part; 2] {
    let edges = blade_edges(g);
    let xi = f64::from(g.body.width) * 0.5 - f64::from(g.body.wall_thickness);
    let (y0, y1) = (
        f64::from(g.mount.inset),
        f64::from(g.body.length) - f64::from(g.body.wall_thickness),
    );
    let lo = |y: f64| edges(y).0;
    let hi = |y: f64| edges(y).1;
    let mut left = MeshMm::default();
    slab(&mut left, -xi, -xi + BLADE_THICKNESS, y0, y1, &lo, &hi);
    let mut right = MeshMm::default();
    slab(&mut right, xi - BLADE_THICKNESS, xi, y0, y1, &lo, &hi);
    [Part::fixed(left), Part::fixed(right)]
}

/// Value checks of `[backbox]` (section G.1 of the backbox contract).
fn check_values(g: &CabinetGeometry) -> Result<(), GenError> {
    let bb = &g.backbox;
    positive("backbox.width", bb.width)?;
    positive("backbox.height", bb.height)?;
    positive("backbox.depth", bb.depth)?;
    non_negative("backbox.front_y", bb.front_y)?;
    non_negative("backbox.bottom_z", bb.bottom_z)?;
    positive("backbox.speaker_panel_height", bb.speaker_panel_height)?;
    let bg = &bb.backglass;
    positive("backbox.backglass.width", bg.width)?;
    positive("backbox.backglass.height", bg.height)?;
    positive("backbox.backglass.thickness", bg.thickness)?;
    let o = &bb.translite_opening;
    positive("backbox.translite_opening.width", o.width)?;
    positive("backbox.translite_opening.height", o.height)?;
    positive("backbox.translite_opening.bottom", o.bottom)?;
    let sd = &bb.score_display;
    positive("backbox.score_display.width", sd.width)?;
    positive("backbox.score_display.height", sd.height)?;
    positive("backbox.score_display.centre_v", sd.centre_v)?;
    if sd.pixels.contains(&0) {
        return Err(err(
            "backbox.score_display.pixels",
            format!("both counts must be > 0, got {:?}", sd.pixels),
        ));
    }
    for (i, gr) in bb.grilles.iter().enumerate() {
        finite(&format!("backbox.grilles[{i}].u"), &[gr.u])?;
        positive(&format!("backbox.grilles[{i}].v"), gr.v)?;
        positive(&format!("backbox.grilles[{i}].diameter"), gr.diameter)?;
    }
    if bb.grilles.len() > MAX_GRILLES {
        return Err(err(
            "backbox.grilles",
            format!("{} grilles listed, at most {MAX_GRILLES}", bb.grilles.len()),
        ));
    }
    Ok(())
}

/// `Err(key)` unless `lo..hi` lies in `0..=top` (1 µm slack for rounding).
fn within(key: &str, what: &str, lo: f64, hi: f64, top: f64) -> Result<(), GenError> {
    const SLACK: f64 = 1e-3;
    if lo >= -SLACK && hi <= top + SLACK {
        Ok(())
    } else {
        Err(err(
            key,
            format!("{what} spans {lo}..{hi} mm, outside 0..{top} mm"),
        ))
    }
}

/// Fit checks of `[backbox]` (section G.2 of the backbox contract, after
/// the speaker panel's own fit under the backbox height).
fn check_fit(g: &CabinetGeometry) -> Result<(), GenError> {
    let bb = &g.backbox;
    let (w, h) = (f64::from(bb.width), f64::from(bb.height));
    let hw = w * 0.5;
    let sph = f64::from(bb.speaker_panel_height);
    if sph >= h {
        return Err(err(
            "backbox.speaker_panel_height",
            format!("{sph} mm leaves no room for the translite under the {h} mm backbox height"),
        ));
    }
    let o = &bb.translite_opening;
    let wide = |key: &str, what: &str, width: f32| -> Result<(), GenError> {
        if f64::from(width) <= w {
            Ok(())
        } else {
            Err(err(
                key,
                format!("{what} {width} mm is wider than the {w} mm backbox"),
            ))
        }
    };
    wide("backbox.translite_opening.width", "opening", o.width)?;
    let (ob, oh) = (f64::from(o.bottom), f64::from(o.height));
    within(
        "backbox.translite_opening.bottom",
        "the translite opening",
        ob,
        ob + oh,
        h,
    )?;
    let sd = &bb.score_display;
    wide(
        "backbox.score_display.width",
        "score-display window",
        sd.width,
    )?;
    let (cv, dh) = (f64::from(sd.centre_v), f64::from(sd.height) * 0.5);
    within(
        "backbox.score_display.centre_v",
        "the score-display window",
        cv - dh,
        cv + dh,
        sph,
    )?;
    for (i, gr) in bb.grilles.iter().enumerate() {
        let r = f64::from(gr.diameter) * 0.5;
        let (u, v) = (f64::from(gr.u), f64::from(gr.v));
        within(
            &format!("backbox.grilles[{i}].u"),
            "the grille",
            u - r + hw,
            u + r + hw,
            w,
        )?;
        within(
            &format!("backbox.grilles[{i}].v"),
            "the grille",
            v - r,
            v + r,
            h,
        )?;
    }
    let bg = &bb.backglass;
    wide("backbox.backglass.width", "backglass", bg.width)?;
    let (vc, bh) = (ob + oh * 0.5, f64::from(bg.height) * 0.5);
    within(
        "backbox.backglass.height",
        "the backglass",
        vc - bh,
        vc + bh,
        h,
    )?;
    Ok(())
}

/// Panel-room checks: the depth holds the front plate and the rear panel,
/// the width the two side panels, the height the bottom panel and the top
/// (each `body.wall_thickness` thick), so no panel pokes out of the volume.
fn check_panel_room(g: &CabinetGeometry) -> Result<(), GenError> {
    let bb = &g.backbox;
    let t = f64::from(g.body.wall_thickness);
    let (w, h, d) = (
        f64::from(bb.width),
        f64::from(bb.height),
        f64::from(bb.depth),
    );
    if d < 2.0 * t {
        return Err(err(
            "backbox.depth",
            format!("depth {d} mm is less than two {t} mm panels (front plate and rear panel)"),
        ));
    }
    if w < 2.0 * t {
        return Err(err(
            "backbox.width",
            format!("width {w} mm is less than two {t} mm panels (the two side panels)"),
        ));
    }
    if h < 2.0 * t {
        return Err(err(
            "backbox.height",
            format!("height {h} mm is less than two {t} mm panels (bottom and top panels)"),
        ));
    }
    Ok(())
}

/// Profile checks (sections A and B of the backbox contract).
fn check_profiles(g: &CabinetGeometry) -> Result<(), GenError> {
    let f = Face::new(g);
    if g.backbox.front_profile == BackboxFrontProfile::Angled {
        let run = f.h * libm::tan(rad(FRONT_LEAN_DEG));
        if run > f.d - 2.0 * f.t {
            return Err(err(
                "backbox.front_profile",
                format!(
                    "an angled front leans back {run} mm, more than the depth less two panels ({} mm)",
                    f.d - 2.0 * f.t
                ),
            ));
        }
    }
    match g.backbox.top_profile {
        BackboxTopProfile::Flat => {}
        BackboxTopProfile::Angled => {
            let rear = f.z_top(0.0, f.rear()) - f.zb;
            if rear <= 2.0 * f.t {
                return Err(err(
                    "backbox.top_profile",
                    format!("an angled top falls to {rear} mm at the rear, no room for the panels"),
                ));
            }
        }
        BackboxTopProfile::Arched => {
            let o = &g.backbox.translite_opening;
            let crown = f.htop(f64::from(o.width) * 0.5);
            let top = f64::from(o.bottom) + f64::from(o.height);
            if crown < top {
                return Err(err(
                    "backbox.top_profile",
                    format!(
                        "the arched top is {crown} mm high over the opening's sides, below its top edge at {top} mm"
                    ),
                ));
            }
            let side = f.htop(f.w * 0.5);
            let need = max_f64(f64::from(g.backbox.speaker_panel_height), 2.0 * f.t);
            if side <= need {
                return Err(err(
                    "backbox.top_profile",
                    format!("the arched top falls to {side} mm at the sides, not above {need} mm"),
                ));
            }
        }
    }
    Ok(())
}

/// Blade checks (section E of the backbox contract): `mount.inset`,
/// `mount.height`, `mount.pitch_deg`, then the blade height.
fn check_blades(g: &CabinetGeometry) -> Result<(), GenError> {
    let mt = &g.mount;
    let y1 = f64::from(g.body.length) - f64::from(g.body.wall_thickness);
    let inset = f64::from(mt.inset);
    if !(inset.is_finite() && inset >= 0.0 && inset < y1) {
        return Err(err(
            "mount.inset",
            format!(
                "must be finite, >= 0 and less than {y1} mm (inside the rear wall), got {inset}"
            ),
        ));
    }
    positive("mount.height", mt.height)?;
    let p = f64::from(mt.pitch_deg);
    if !(p.is_finite() && p.abs() < MAX_PITCH_DEG) {
        return Err(err(
            "mount.pitch_deg",
            format!("must be finite and within ±{MAX_PITCH_DEG} degrees, got {p}"),
        ));
    }
    let edges = blade_edges(g);
    for y in [inset, y1] {
        let (lo, hi) = edges(y);
        if hi - lo <= MIN_BLADE_HEIGHT || !(hi - lo).is_finite() {
            return Err(err(
                "mount.height",
                format!(
                    "the playfield plane at y = {y} mm leaves {} mm of side wall under the glass channel",
                    hi - lo
                ),
            ));
        }
    }
    Ok(())
}

/// Every backbox and blade check, in the contract's order (section G):
/// values, fit, profiles, blades. The error names the `cabinet.toml` key.
pub(crate) fn check(g: &CabinetGeometry) -> Result<(), GenError> {
    check_values(g)?;
    check_fit(g)?;
    check_panel_room(g)?;
    check_profiles(g)?;
    check_blades(g)
}
