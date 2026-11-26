//! The cabinet's parts as millimetre meshes in the cabinet frame with
//! `z` above the floor (see the module docs of [`super`]). Every dimension
//! comes from the [`CabinetGeometry`]; the few design values that the
//! schema does not carry (button, plunger, bracket and foot sizes) are the
//! named constants below (docs/DECISIONS.md "#110 KP-029 — Cabinet model
//! design values").

use super::mesh::{Axis, MeshMm, P3};
use pin_core::fmath::{max_f64, min_f64};
use pin_core::schema::cabinet::{
    BarProfile, BoltStyle, BracketStyle, CabinetGeometry, CrossSectionShape, FootShape, Leg,
    LegSurface, RailProfile,
};

/// Segments of every round section (leg, thread, foot, button, plunger).
pub(crate) const ROUND_SEGMENTS: usize = 16;
/// Segments of an elliptical (rounded) profile top.
const ARC_SEGMENTS: usize = 8;
/// Rib (groove) depth as a fraction of the section size.
const RIB_DEPTH_FRACTION: f64 = 0.04;
/// Bracket height as a multiple of the leg's top section size.
const BRACKET_HEIGHT_FACTOR: f64 = 1.5;
/// Corner plate thickness and reach past the leg, mm.
const CORNER_PLATE_THICKNESS: f64 = 3.0;
const CORNER_PLATE_REACH: f64 = 40.0;
/// Flush plate thickness and overhang each side of the leg, mm.
const FLUSH_PLATE_THICKNESS: f64 = 4.0;
const FLUSH_PLATE_OVERHANG: f64 = 20.0;
/// Coin door plate: how far it stands proud of its centre's `y`, mm.
pub(crate) const COIN_DOOR_PROUD: f64 = 4.0;
/// Button bezel (base) diameter and height, cap diameter and height, mm.
pub(crate) const BUTTON_BEZEL_DIAMETER: f64 = 40.0;
pub(crate) const BUTTON_BEZEL_HEIGHT: f64 = 4.0;
/// Button cap diameter, mm.
pub(crate) const BUTTON_CAP_DIAMETER: f64 = 30.0;
/// Button cap height, mm.
pub(crate) const BUTTON_CAP_HEIGHT: f64 = 12.0;
/// Button press travel, mm.
pub(crate) const BUTTON_TRAVEL_MM: f64 = 4.0;
/// Plunger housing outer diameter, flange diameter and thickness, and the
/// length it runs into the body, mm.
pub(crate) const PLUNGER_HOUSING_DIAMETER: f64 = 30.0;
const PLUNGER_FLANGE_DIAMETER: f64 = 50.0;
const PLUNGER_FLANGE_THICKNESS: f64 = 6.0;
pub(crate) const PLUNGER_HOUSING_LENGTH: f64 = 120.0;
/// Plunger rod diameter, length inside the body, length outside, knob, mm.
const PLUNGER_ROD_DIAMETER: f64 = 9.5;
const PLUNGER_ROD_INSIDE: f64 = 100.0;
const PLUNGER_ROD_OUTSIDE: f64 = 60.0;
const PLUNGER_KNOB_DIAMETER: f64 = 32.0;
const PLUNGER_KNOB_LENGTH: f64 = 40.0;
/// Plunger pull travel, mm.
pub(crate) const PLUNGER_TRAVEL_MM: f64 = 80.0;

/// A built part.
pub(crate) struct Part {
    /// Geometry, mm (local to `translation` when it is set).
    pub mesh: MeshMm,
    /// Pivot of a moving node, mm (cabinet frame, z above the floor).
    pub translation: Option<P3>,
    /// Press (or pull) axis of a moving node.
    pub travel: Option<(Axis, f64)>,
}

impl Part {
    /// A static part (identity transform, no travel).
    pub(crate) fn fixed(mesh: MeshMm) -> Part {
        Part {
            mesh,
            translation: None,
            travel: None,
        }
    }
}

/// Body-derived lines, mm, `f64`.
struct Frame<'a> {
    g: &'a CabinetGeometry,
    w: f64,
    l: f64,
    t: f64,
}

impl Frame<'_> {
    /// Body bottom above the floor at `y`.
    fn bottom(&self, y: f64) -> f64 {
        self.g.body_bottom_height_mm(y)
    }

    /// Side wall top above the floor at `y`.
    fn wall_top(&self, y: f64) -> f64 {
        let fh = f64::from(self.g.body.front_height);
        let rh = f64::from(self.g.body.rear_height);
        self.g.floor_height_mm(y, fh + (rh - fh) * y / self.l)
    }

    /// Glass channel: (bottom, top) above the floor at `y`, one channel
    /// width below the wall top, clamped to the wall.
    fn channel(&self, y: f64) -> (f64, f64) {
        let cw = f64::from(self.g.body.glass_channel.width);
        let (lo, hi) = (self.bottom(y), self.wall_top(y));
        let clamp = |z: f64| max_f64(lo, min_f64(hi, z));
        (clamp(hi - 2.0 * cw), clamp(hi - cw))
    }

    /// A body-fixed point `[x, y, z above the body bottom]` above the floor.
    fn point(&self, p: [f32; 3]) -> P3 {
        let y = f64::from(p[1]);
        [
            f64::from(p[0]),
            y,
            self.g.floor_height_mm(y, f64::from(p[2])),
        ]
    }
}

/// A box with x in `xa..xb`, y in `y0..y1` and z from `lo(y)` to `hi(y)`.
pub(crate) fn slab(
    m: &mut MeshMm,
    xa: f64,
    xb: f64,
    y0: f64,
    y1: f64,
    lo: &dyn Fn(f64) -> f64,
    hi: &dyn Fn(f64) -> f64,
) {
    let (xa, xb) = (min_f64(xa, xb), max_f64(xa, xb));
    // Rings across y, counter-clockwise about +y: (x, z) clockwise.
    let ring = |y: f64| -> [P3; 4] {
        [
            [xa, y, lo(y)],
            [xa, y, hi(y)],
            [xb, y, hi(y)],
            [xb, y, lo(y)],
        ]
    };
    m.hexa(ring(y0), ring(y1));
}

/// The five body panels: bottom, side_left, side_right, front, rear.
pub(crate) fn body(g: &CabinetGeometry) -> [Part; 5] {
    let f = frame(g);
    let (hw, l, t) = (f.w * 0.5, f.l, f.t);
    let bot = |y: f64| f.bottom(y);
    let mut bottom = MeshMm::default();
    slab(&mut bottom, -hw + t, hw - t, t, l - t, &bot, &|y| {
        f.bottom(y) + t
    });
    let depth = min_f64(f64::from(g.body.glass_channel.depth), t);
    let side = |outer: f64, inner: f64| -> MeshMm {
        let mut m = MeshMm::default();
        let notch = inner - (inner - outer).signum() * depth;
        let ch_lo = |y: f64| f.channel(y).0;
        let ch_hi = |y: f64| f.channel(y).1;
        slab(&mut m, outer, inner, 0.0, l, &bot, &ch_lo);
        slab(&mut m, outer, notch, 0.0, l, &ch_lo, &ch_hi);
        slab(&mut m, outer, inner, 0.0, l, &ch_hi, &|y| f.wall_top(y));
        m
    };
    let side_left = side(-hw, -hw + t);
    let side_right = side(hw, hw - t);
    let front_top = g.floor_height_mm(0.0, f64::from(g.body.front_height));
    let rear_top = g.floor_height_mm(l, f64::from(g.body.rear_height));
    let mut front = MeshMm::default();
    slab(&mut front, -hw + t, hw - t, 0.0, t, &bot, &|_| front_top);
    let mut rear = MeshMm::default();
    slab(&mut rear, -hw + t, hw - t, l - t, l, &bot, &|_| rear_top);
    [bottom, side_left, side_right, front, rear].map(Part::fixed)
}

fn frame(g: &CabinetGeometry) -> Frame<'_> {
    Frame {
        g,
        w: f64::from(g.body.width),
        l: f64::from(g.body.length),
        t: f64::from(g.body.wall_thickness),
    }
}

/// An elliptical cap over `[a0, a1]` from `base` up to `top` (at most half
/// the span high), as profile points from `a1` back to `a0`.
fn rounded_top(a0: f64, a1: f64, base: f64, top: f64) -> Vec<[f64; 2]> {
    let ra = (a1 - a0) * 0.5;
    let rb = max_f64(0.0, min_f64(ra, top - base));
    let (ca, cb) = (a0 + ra, top - rb);
    (0..=ARC_SEGMENTS)
        .map(|k| {
            let th = core::f64::consts::PI * k as f64 / ARC_SEGMENTS as f64;
            [ca + ra * libm::cos(th), cb + rb * libm::sin(th)]
        })
        .collect()
}

/// The left and right side rails along the side wall tops.
pub(crate) fn rails(g: &CabinetGeometry) -> [Part; 2] {
    let f = frame(g);
    let sr = &g.body.side_rail;
    let (rw, rh) = (f64::from(sr.width), f64::from(sr.height));
    let rail = |outer: f64, inward: f64| -> MeshMm {
        let inner = outer + inward * rw;
        let mut profile = vec![[outer, 0.0], [inner, 0.0]];
        match sr.profile {
            RailProfile::Flat => profile.extend([[inner, rh], [outer, rh]]),
            RailProfile::Angled => profile.extend([[inner, rh * 0.5], [outer, rh]]),
            RailProfile::Rounded => {
                let (a0, a1) = (min_f64(outer, inner), max_f64(outer, inner));
                let mut arc = rounded_top(a0, a1, 0.0, rh);
                if inward < 0.0 {
                    // Inner is the low x: walk the arc from the inner side.
                    arc.reverse();
                }
                profile.extend(arc);
            }
        }
        let mut m = MeshMm::default();
        m.extrude_y(&profile, 0.0, f.l, |y| f.wall_top(y));
        m
    };
    let hw = f.w * 0.5;
    [rail(-hw, 1.0), rail(hw, -1.0)].map(Part::fixed)
}

/// The lockdown bar: across the interior at the front, its top at
/// [`CabinetGeometry::lockdown_bar_top_height_mm`], its underside one glass
/// channel width below the front wall top (over the glass's front edge).
pub(crate) fn lockdown_bar(g: &CabinetGeometry) -> Part {
    let f = frame(g);
    let lb = &g.body.lockdown_bar;
    let top = g.lockdown_bar_top_height_mm();
    let base = g.floor_height_mm(0.0, f64::from(g.body.front_height))
        - f64::from(g.body.glass_channel.width);
    let d = f64::from(lb.depth);
    let mut profile = vec![[0.0, base], [d, base]];
    match lb.profile {
        BarProfile::Flat => profile.extend([[d, top], [0.0, top]]),
        BarProfile::Rounded => profile.extend(rounded_top(0.0, d, base, top)),
    }
    let mut m = MeshMm::default();
    let hw = f.w * 0.5 - f.t;
    m.extrude_x(&profile, -hw, hw);
    Part::fixed(m)
}

/// The playfield glass: a sheet parallel to the side wall tops, its
/// underside on the glass channel's floor, from the front wall's inner face
/// back `glass_panel.length` measured along the slope.
pub(crate) fn glass(g: &CabinetGeometry) -> Part {
    let f = frame(g);
    let gp = &g.glass_panel;
    let slope = (f.wall_top(f.l) - f.wall_top(0.0)) / f.l;
    let run = f64::from(gp.length) / libm::sqrt(1.0 + slope * slope);
    let (y0, y1) = (f.t, f.t + run);
    let hw = f64::from(gp.width) * 0.5;
    let thk = f64::from(gp.thickness);
    let mut m = MeshMm::default();
    let lo = |y: f64| f.channel(y).0;
    slab(&mut m, -hw, hw, y0, y1, &lo, &|y| f.channel(y).0 + thk);
    Part::fixed(m)
}

/// Centreline `(x, y)` of a leg, mm, as `pin_core` places it, with the
/// outward signs `(sx, sy)` (-1 left / front, +1 right / rear).
pub(crate) fn leg_xy(g: &CabinetGeometry, name: &str, leg: &Leg) -> (f64, f64, f64, f64) {
    let right = name.ends_with("right");
    let rear = name.starts_with("rear");
    let hw = f64::from(g.body.width) * 0.5;
    let ix = f64::from(leg.corner_inset[0]);
    let iy = f64::from(leg.corner_inset[1]);
    let x = if right { hw - ix } else { ix - hw };
    let y = if rear {
        f64::from(g.body.length) - iy
    } else {
        iy
    };
    (
        x,
        y,
        if right { 1.0 } else { -1.0 },
        if rear { 1.0 } else { -1.0 },
    )
}

/// Cross-section ring offsets of a leg of section `size` (side or
/// diameter), counter-clockwise about `+z`; ribbed legs have lengthwise
/// grooves cut into each face (the outer size is unchanged).
fn section_ring(shape: CrossSectionShape, surface: LegSurface, size: f64) -> Vec<[f64; 2]> {
    let ribbed = surface == LegSurface::Ribbed;
    let g = size * RIB_DEPTH_FRACTION;
    match shape {
        CrossSectionShape::Round => {
            let n = if ribbed {
                2 * ROUND_SEGMENTS
            } else {
                ROUND_SEGMENTS
            };
            (0..n)
                .map(|k| {
                    let r = if ribbed && k % 2 == 1 {
                        size * 0.5 - g
                    } else {
                        size * 0.5
                    };
                    let t = 2.0 * core::f64::consts::PI * k as f64 / n as f64;
                    [r * libm::cos(t), r * libm::sin(t)]
                })
                .collect()
        }
        CrossSectionShape::Square | CrossSectionShape::Tapered => {
            let h = size * 0.5;
            let corners = [[-h, -h], [h, -h], [h, h], [-h, h]];
            if !ribbed {
                return corners.to_vec();
            }
            let mut out = Vec::with_capacity(20);
            for i in 0..4 {
                let a = corners[i];
                let b = corners[(i + 1) % 4];
                let e = [b[0] - a[0], b[1] - a[1]];
                // Inward normal of a counter-clockwise edge: rotate +90 degrees.
                let n = [-e[1] / size, e[0] / size];
                let at = |s: f64, d: f64| [a[0] + e[0] * s + n[0] * d, a[1] + e[1] * s + n[1] * d];
                // Groove walls slope toward the middle, so the ring stays
                // star-shaped about the centre (the caps fan from it).
                out.extend([
                    at(0.0, 0.0),
                    at(1.0 / 3.0, 0.0),
                    at(0.4, g),
                    at(0.6, g),
                    at(2.0 / 3.0, 0.0),
                ]);
            }
            out
        }
    }
}

/// Leg shaft: z from `leveler.extension` to `leveler.extension + length`.
pub(crate) fn leg_shaft(g: &CabinetGeometry, name: &str, leg: &Leg) -> Part {
    let (x, y, _, _) = leg_xy(g, name, leg);
    let s = &leg.section;
    let top = f64::from(s.top);
    let bottom = if s.shape == CrossSectionShape::Tapered {
        f64::from(s.bottom)
    } else {
        top
    };
    let z0 = f64::from(leg.leveler.extension);
    let z1 = z0 + f64::from(leg.length);
    let ring = |size: f64, z: f64| -> Vec<P3> {
        section_ring(s.shape, leg.surface, size)
            .iter()
            .map(|o| [x + o[0], y + o[1], z])
            .collect()
    };
    let mut m = MeshMm::default();
    m.loft(&[ring(bottom, z0), ring(top, z1)]);
    Part::fixed(m)
}

/// One bolt head on a face at `at`, pointing along `out`.
fn bolt(m: &mut MeshMm, at: P3, out: Axis, style: BoltStyle) {
    match style {
        BoltStyle::Hex => m.lathe(at, out, &[(0.0, 8.0), (6.0, 8.0)], 6),
        BoltStyle::Carriage => m.lathe(
            at,
            out,
            &[(0.0, 10.0), (2.0, 10.0), (4.0, 8.5), (5.5, 5.0)],
            ROUND_SEGMENTS,
        ),
        BoltStyle::ButtonHead => m.lathe(
            at,
            out,
            &[(0.0, 7.0), (2.5, 6.0), (3.5, 3.5)],
            ROUND_SEGMENTS,
        ),
    }
}

/// Leg bracket at the leg's top: a corner plate (an L of two plates on the
/// leg's inner faces, two bolts through the leg's outer face) or a flush
/// plate (one plate on the leg's outer face with four bolts).
pub(crate) fn bracket(g: &CabinetGeometry, name: &str, leg: &Leg) -> Part {
    let (x, y, sx, sy) = leg_xy(g, name, leg);
    let a = f64::from(leg.section.top) * 0.5;
    let z1 = f64::from(leg.leveler.extension) + f64::from(leg.length);
    let hb = BRACKET_HEIGHT_FACTOR * f64::from(leg.section.top);
    let z0 = z1 - hb;
    let out = if sx > 0.0 { Axis::PosX } else { Axis::NegX };
    let mut m = MeshMm::default();
    let span = |p: f64, q: f64| (min_f64(p, q), max_f64(p, q));
    match leg.bracket {
        BracketStyle::CornerPlate => {
            let th = CORNER_PLATE_THICKNESS;
            let reach = CORNER_PLATE_REACH;
            let (xa, xb) = span(x - sx * a, x - sx * (a + th));
            let (ya, yb) = span(y + sy * a, y - sy * (a + reach));
            m.aabb([xa, ya, z0], [xb, yb, z1]);
            let (ya, yb) = span(y - sy * a, y - sy * (a + th));
            let (xa, xb) = span(x + sx * a, x - sx * (a + reach));
            m.aabb([xa, ya, z0], [xb, yb, z1]);
            for frac in [1.0 / 3.0, 2.0 / 3.0] {
                bolt(&mut m, [x + sx * a, y, z1 - hb * frac], out, leg.bolt);
            }
        }
        BracketStyle::FlushPlate => {
            let th = FLUSH_PLATE_THICKNESS;
            let (xa, xb) = span(x + sx * a, x + sx * (a + th));
            let half = a + FLUSH_PLATE_OVERHANG;
            m.aabb([xa, y - half, z0], [xb, y + half, z1]);
            for dy in [
                -(a + FLUSH_PLATE_OVERHANG * 0.5),
                a + FLUSH_PLATE_OVERHANG * 0.5,
            ] {
                for frac in [0.25, 0.75] {
                    bolt(
                        &mut m,
                        [x + sx * (a + th), y + dy, z1 - hb * frac],
                        out,
                        leg.bolt,
                    );
                }
            }
        }
    }
    Part::fixed(m)
}

/// Leveler: the thread from the floor up into the leg (its visible part is
/// the extension below the shaft) and the foot, whose lowest point is on
/// the floor plane `z = 0`.
pub(crate) fn leveler(g: &CabinetGeometry, name: &str, leg: &Leg) -> Part {
    let (x, y, _, _) = leg_xy(g, name, leg);
    let lv = &leg.leveler;
    let rt = f64::from(lv.thread_diameter) * 0.5;
    let top = max_f64(f64::from(lv.thread_length), f64::from(lv.extension));
    let r = f64::from(lv.foot_diameter) * 0.5;
    let at = [x, y, 0.0];
    let mut m = MeshMm::default();
    m.lathe(at, Axis::PosZ, &[(0.0, rt), (top, rt)], ROUND_SEGMENTS);
    let foot: &[(f64, f64)] = match lv.foot_shape {
        FootShape::Flat => &[(0.0, r), (6.0, r)],
        FootShape::Domed => &[
            (0.0, 0.6 * r),
            (2.0, 0.9 * r),
            (5.0, r),
            (8.0, r),
            (11.0, 0.7 * r),
            (13.0, 0.35 * r),
        ],
        FootShape::Swivel => &[
            (0.0, r),
            (5.0, r),
            (5.0, 0.35 * r),
            (10.0, 0.45 * r),
            (14.0, 0.35 * r),
        ],
    };
    m.lathe(at, Axis::PosZ, foot, ROUND_SEGMENTS);
    Part::fixed(m)
}

/// Coin door: a plate `width x height` centred on `coin_door.centre`,
/// standing [`COIN_DOOR_PROUD`] mm proud of the front face.
pub(crate) fn coin_door(g: &CabinetGeometry) -> Part {
    let f = frame(g);
    let cd = &g.controls.coin_door;
    let c = f.point(cd.centre);
    let (hw, hh) = (f64::from(cd.width) * 0.5, f64::from(cd.height) * 0.5);
    let mut m = MeshMm::default();
    m.aabb(
        [c[0] - hw, c[1] - COIN_DOOR_PROUD, c[2] - hh],
        [c[0] + hw, c[1], c[2] + hh],
    );
    Part::fixed(m)
}

/// A button: bezel and cap protruding against its press axis `travel`,
/// local to its pivot (the control position above the floor).
pub(crate) fn button(g: &CabinetGeometry, pos: [f32; 3], travel: Axis) -> Part {
    let f = frame(g);
    let mut m = MeshMm::default();
    let rb = BUTTON_BEZEL_DIAMETER * 0.5;
    let rc = BUTTON_CAP_DIAMETER * 0.5;
    let h = BUTTON_BEZEL_HEIGHT;
    m.lathe(
        [0.0; 3],
        travel.opposite(),
        &[(0.0, rb), (h, rb), (h, rc), (h + BUTTON_CAP_HEIGHT, rc)],
        ROUND_SEGMENTS,
    );
    Part {
        mesh: m,
        translation: Some(f.point(pos)),
        travel: Some((travel, BUTTON_TRAVEL_MM)),
    }
}

/// Plunger housing (static): a tube along `+y` from a flange at the front
/// face into the body, centred on `controls.plunger`.
pub(crate) fn plunger_housing(g: &CabinetGeometry) -> Part {
    let f = frame(g);
    let at = f.point(g.controls.plunger);
    let (rf, rh) = (
        PLUNGER_FLANGE_DIAMETER * 0.5,
        PLUNGER_HOUSING_DIAMETER * 0.5,
    );
    let mut m = MeshMm::default();
    m.lathe(
        at,
        Axis::PosY,
        &[
            (-PLUNGER_FLANGE_THICKNESS, rf),
            (0.0, rf),
            (0.0, rh),
            (PLUNGER_HOUSING_LENGTH, rh),
        ],
        ROUND_SEGMENTS,
    );
    Part::fixed(m)
}

/// Plunger rod and knob (moving): local to the plunger position, the rod
/// runs back into the housing and out toward the player (`-y`) to the knob.
pub(crate) fn plunger_rod(g: &CabinetGeometry) -> Part {
    let f = frame(g);
    let rr = PLUNGER_ROD_DIAMETER * 0.5;
    let rk = PLUNGER_KNOB_DIAMETER * 0.5;
    let out = PLUNGER_ROD_OUTSIDE;
    let k = PLUNGER_KNOB_LENGTH;
    let mut m = MeshMm::default();
    m.lathe(
        [0.0; 3],
        Axis::NegY,
        &[
            (-PLUNGER_ROD_INSIDE, rr),
            (out, rr),
            (out, 0.75 * rk),
            (out + 0.125 * k, rk),
            (out + 0.75 * k, rk),
            (out + k, 0.6 * rk),
        ],
        ROUND_SEGMENTS,
    );
    Part {
        mesh: m,
        translation: Some(f.point(g.controls.plunger)),
        travel: Some((Axis::NegY, PLUNGER_TRAVEL_MM)),
    }
}
