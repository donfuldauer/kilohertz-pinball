//! Subphase 5.5 (fix 1b): the optional polygon `outline` and the
//! `[[backbox.walls]]` polylines of `backbox_layout.toml`. Valid outlines
//! (convex, concave, either winding, 1024 points) generate deterministic
//! geometry whose edge panels cover the polygon's boundary except the
//! entry and drain slots; walls become layer-4 panels on their segments;
//! every invalid outline or wall is an error naming `backbox.outline`,
//! `backbox.outline[i]` or `backbox.walls[i]`; the box default is
//! unchanged; random input never panics.

#[path = "backbox_common.rs"]
mod common;

use glam::Vec3;
use pin_physics::world::StaticShape;
use pin_tablegen::backbox::{
    BackboxCollider, GeneratedBackbox, MAX_OUTLINE_POINTS, generate_backbox, parse_backbox_layout,
    parse_backbox_layout_bytes,
};
use pin_tablegen::layout::GenError;
use proptest::prelude::*;
use std::fmt::Write as _;

/// Panel thickness the generator uses, mm (`PANEL_MM`).
const T: f32 = 5.0;
const DEPTH: f32 = 35.0;

const RECT: &str = "[[-200.0, 0.0], [200.0, 0.0], [200.0, 300.0], [-200.0, 300.0]]";
const RECT_CW: &str = "[[-200.0, 0.0], [-200.0, 300.0], [200.0, 300.0], [200.0, 0.0]]";
/// Concave: the top right quarter is cut away.
const L_SHAPE: &str =
    "[[-200.0, 0.0], [200.0, 0.0], [200.0, 120.0], [0.0, 120.0], [0.0, 300.0], [-200.0, 300.0]]";

fn pts_of(outline: &str) -> Vec<[f32; 2]> {
    let v: toml::Value = toml::from_str(&format!("a = {outline}")).unwrap();
    v["a"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            let a = p.as_array().unwrap();
            let f = |x: &toml::Value| {
                x.as_float()
                    .unwrap_or_else(|| x.as_integer().unwrap() as f64)
            };
            [f(&a[0]) as f32, f(&a[1]) as f32]
        })
        .collect()
}

/// A `backbox_layout.toml` with the given outline (or none), size, holes
/// and extra `[backbox]` text (walls, flippers).
fn text(outline: Option<&str>, w: f32, h: f32, entry: f32, drain: f32, extra: &str) -> String {
    let mut s = format!(
        "schema_version = 1\n\n[backbox]\norigin = [0.0, 1150.0, 250.0]\nwidth = {w:?}\n\
         height = {h:?}\ndepth = {DEPTH:?}\n"
    );
    if let Some(o) = outline {
        let _ = writeln!(s, "outline = {o}");
    }
    let _ = write!(
        s,
        "\n[backbox.entry]\nvuk = [120.0, 1000.0]\nat = {entry:?}\ncoil = \"bb_vuk_coil\"\n\
         switch = \"bb_vuk_switch\"\n\n[backbox.drain]\nat = {drain:?}\n\n{extra}"
    );
    s
}

fn gen_text(t: &str) -> Result<GeneratedBackbox, GenError> {
    let file = parse_backbox_layout(t)?;
    generate_backbox(&file.backbox, &common::base())
}

fn gen_ok(t: &str) -> GeneratedBackbox {
    gen_text(t).unwrap_or_else(|e| panic!("{e}\n{t}"))
}

fn expect_err(t: &str, element: &str, msg_has: &str) {
    let e = match gen_text(t) {
        Ok(_) => panic!("accepted:\n{t}"),
        Err(e) => e,
    };
    assert_eq!(e.element, element, "{e}\n{t}");
    assert!(e.message.contains(msg_has), "{e:?} lacks {msg_has:?}");
}

const WALLS: &str = "[[backbox.walls]]\npoints = [[-160.0, 40.0], [-160.0, 260.0]]\n\n\
                     [[backbox.walls]]\npoints = [[100.0, 60.0], [160.0, 120.0], [160.0, 200.0]]\n";

/// Plane coordinates (mm) of the four in-plane corners of a panel at its
/// mid-depth (local `x` = ±half, local `z` = ±half), and a check that the
/// panel spans glass to back.
fn corners(g: &GeneratedBackbox, c: &BackboxCollider) -> [[f32; 2]; 4] {
    let StaticShape::Cuboid {
        center,
        half_extents: h,
        rotation,
    } = c.shape
    else {
        panic!("{} is not a cuboid", c.name)
    };
    let inv = g.frame.rotation.inverse();
    let cl = inv * (center - g.frame.origin_m);
    let rl = inv * rotation;
    assert!(
        (rl * Vec3::Y - Vec3::Y).length() < 1e-5,
        "{}: not standing glass to back",
        c.name
    );
    assert!((cl.y * 1e3 - 0.5 * DEPTH).abs() < 1e-2, "{}", c.name);
    assert!((h.y * 1e3 - 0.5 * DEPTH).abs() < 1e-2, "{}", c.name);
    [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].map(|(sx, sz)| {
        let p = cl + rl * Vec3::new(sx * h.x, 0.0, sz * h.z);
        [p.x * 1e3, p.z * 1e3]
    })
}

fn dist_to_segment(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
    let t = (((p[0] - a[0]) * ex + (p[1] - a[1]) * ey) / (ex * ex + ey * ey)).clamp(0.0, 1.0);
    let (dx, dy) = (p[0] - (a[0] + t * ex), p[1] - (a[1] + t * ey));
    (dx * dx + dy * dy).sqrt()
}

fn inside(poly: &[[f32; 2]], p: [f32; 2]) -> bool {
    let n = poly.len();
    let mut c = false;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if p[0] < x {
                c = !c;
            }
        }
    }
    c
}

fn signed_area(poly: &[[f32; 2]]) -> f32 {
    let n = poly.len();
    (0..n)
        .map(|i| {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f32>()
        * 0.5
}

/// The outline edge panels: each has two corners on its own edge (the
/// inner face) and two 5 mm outside the polygon; sampled along every edge,
/// the boundary is covered by inner faces except over a hole on a
/// downward-facing edge, where nothing covers it.
fn assert_edges_cover_outline(g: &GeneratedBackbox, poly: &[[f32; 2]], holes: [(f32, f32); 2]) {
    let n = poly.len();
    let ccw = signed_area(poly) > 0.0;
    let mut faces: Vec<(usize, [f32; 2], [f32; 2])> = Vec::new();
    for c in g
        .colliders
        .iter()
        .filter(|c| c.name.starts_with("backbox.outline["))
    {
        let i: usize = c.name["backbox.outline[".len()..]
            .split(']')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(i < n, "{}", c.name);
        assert_eq!(c.layer_mask, 1 << 4, "{}", c.name);
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let k = corners(g, c);
        let on: Vec<[f32; 2]> = k
            .iter()
            .copied()
            .filter(|&p| dist_to_segment(p, a, b) < 0.02)
            .collect();
        let off: Vec<[f32; 2]> = k
            .iter()
            .copied()
            .filter(|&p| dist_to_segment(p, a, b) >= 0.02)
            .collect();
        assert_eq!(
            on.len(),
            2,
            "{}: corners {k:?} vs edge {a:?}..{b:?}",
            c.name
        );
        for p in &off {
            let d = dist_to_segment(*p, a, b);
            assert!(
                (d - T).abs() < 0.02,
                "{}: outer face {d} mm from the edge",
                c.name
            );
            assert!(
                !inside(poly, *p),
                "{}: outer face inside the polygon",
                c.name
            );
        }
        faces.push((i, on[0], on[1]));
    }
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let du = b[0] - a[0];
        let down = if ccw { du > 0.0 } else { du < 0.0 };
        for s in 0..64 {
            let f = (s as f32 + 0.5) / 64.0;
            let p = [a[0] + f * (b[0] - a[0]), a[1] + f * (b[1] - a[1])];
            if holes
                .iter()
                .any(|h| (p[0] - h.0).abs() < 0.5 || (p[0] - h.1).abs() < 0.5)
            {
                continue;
            }
            let in_hole = down && holes.iter().any(|h| p[0] > h.0 && p[0] < h.1);
            let covered = faces
                .iter()
                .any(|(j, fa, fb)| *j == i && dist_to_segment(p, *fa, *fb) < 0.02);
            assert_eq!(covered, !in_hole, "edge {i} at {p:?} (hole {in_hole})");
        }
    }
}

fn hashes(g: &GeneratedBackbox) -> (Vec<u8>, Vec<u8>, String) {
    (
        g.scene_glb.clone(),
        g.colliders_glb.clone(),
        g.table_toml.clone(),
    )
}

#[test]
fn convex_outline_with_walls_generates_deterministically() {
    let t = text(Some(RECT), 400.0, 300.0, -120.0, 150.0, WALLS);
    let (a, b) = (gen_ok(&t), gen_ok(&t));
    assert_eq!(hashes(&a), hashes(&b));
    assert_eq!(&a.colliders_glb[0..4], b"glTF");
    assert_edges_cover_outline(&a, &pts_of(RECT), [(-140.0, -100.0), (130.0, 170.0)]);
    // The box-only panels are replaced by the outline edges.
    for gone in ["backbox.walls[0]", "backbox.roof", "backbox.shelf[0]"] {
        assert!(a.colliders.iter().all(|c| c.name != gone), "{gone}");
    }
    let names: Vec<&str> = a.colliders.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names[..2], ["backbox.back", "backbox.glass"]);
    // The bottom edge is cut twice (three pieces), the others whole.
    assert!(names.contains(&"backbox.outline[0].2") && !names.contains(&"backbox.outline[0].3"));
    assert!(names.contains(&"backbox.outline[1]") && names.contains(&"backbox.outline[3]"));
}

#[test]
fn concave_outline_generates_deterministically_and_is_covered() {
    let t = text(Some(L_SHAPE), 400.0, 300.0, -120.0, 120.0, WALLS);
    let (a, b) = (gen_ok(&t), gen_ok(&t));
    assert_eq!(hashes(&a), hashes(&b));
    let poly = pts_of(L_SHAPE);
    assert_edges_cover_outline(&a, &poly, [(-140.0, -100.0), (100.0, 140.0)]);
    // The notch edge (interior below it) is not cut by the drain below it.
    assert!(a.colliders.iter().any(|c| c.name == "backbox.outline[3]"));
}

#[test]
fn clockwise_outline_puts_panels_outside_too() {
    let t = text(Some(RECT_CW), 400.0, 300.0, -120.0, 150.0, "");
    let g = gen_ok(&t);
    assert_edges_cover_outline(&g, &pts_of(RECT_CW), [(-140.0, -100.0), (130.0, 170.0)]);
}

#[test]
fn walls_become_layer_4_panels_on_their_segments() {
    for outline in [None, Some(RECT)] {
        let g = gen_ok(&text(outline, 400.0, 300.0, -120.0, 150.0, WALLS));
        let walls = [
            vec![[-160.0f32, 40.0], [-160.0, 260.0]],
            vec![[100.0, 60.0], [160.0, 120.0], [160.0, 200.0]],
        ];
        for (i, w) in walls.iter().enumerate() {
            for (j, seg) in w.windows(2).enumerate() {
                let name = format!("backbox.walls[{i}].segments[{j}]");
                let c = g
                    .colliders
                    .iter()
                    .find(|c| c.name == name)
                    .unwrap_or_else(|| panic!("{name} missing"));
                assert_eq!(c.layer_mask, 1 << 4);
                let k = corners(&g, c);
                // Centred on the segment: the mid-points of the two short
                // sides are the segment's end points, 5 mm thick.
                let mid = |p: [f32; 2], q: [f32; 2]| [0.5 * (p[0] + q[0]), 0.5 * (p[1] + q[1])];
                let ends = [mid(k[0], k[3]), mid(k[1], k[2])];
                for (e, want) in ends.iter().zip(seg) {
                    assert!(
                        (e[0] - want[0]).abs() < 0.02 && (e[1] - want[1]).abs() < 0.02,
                        "{name}: {ends:?} vs {seg:?}"
                    );
                }
                let thick = ((k[0][0] - k[3][0]).powi(2) + (k[0][1] - k[3][1]).powi(2)).sqrt();
                assert!((thick - T).abs() < 0.02, "{name}: {thick}");
            }
            let n = g
                .colliders
                .iter()
                .filter(|c| c.name.starts_with(&format!("backbox.walls[{i}].")))
                .count();
            assert_eq!(n, w.len() - 1);
        }
        // Scene and collider glb carry the walls too.
        let scene = String::from_utf8_lossy(&g.scene_glb);
        let cols = String::from_utf8_lossy(&g.colliders_glb);
        assert!(scene.contains("\"backbox.walls[1].segments[1]\""));
        assert!(cols.contains("\"tablegen:backbox.walls[1].segments[1]\""));
    }
}

#[test]
fn walls_on_a_box_do_not_change_the_box_panels() {
    let plain = gen_ok(&text(None, 400.0, 300.0, -120.0, 150.0, ""));
    let walled = gen_ok(&text(None, 400.0, 300.0, -120.0, 150.0, WALLS));
    let k = plain.colliders.len();
    assert_eq!(walled.colliders[..k], plain.colliders[..]);
    assert_eq!(walled.colliders.len(), k + 3);
}

#[test]
fn box_default_is_byte_identical_to_the_pinned_example() {
    // The pinned example (`backbox_bundle.rs` golden hashes) has neither key;
    // spelling out an empty wall list changes nothing.
    let file = parse_backbox_layout(common::LAYOUT).unwrap();
    assert_eq!(file.backbox.outline, None);
    assert!(file.backbox.walls.is_empty());
    let explicit = common::LAYOUT.replace("depth = 35.0\n", "depth = 35.0\nwalls = []\n");
    assert_ne!(explicit, common::LAYOUT);
    let a = common::generated();
    let b = gen_ok(&explicit);
    assert_eq!(hashes(&a), hashes(&b));
}

fn ngon(n: usize, cx: f64, cy: f64, rx: f64, ry: f64) -> (String, f32, f32, f32, f32) {
    let mut s = String::from("[");
    let mut pts = Vec::new();
    for i in 0..n {
        let a = i as f64 * std::f64::consts::TAU / n as f64;
        let p = [(cx + rx * a.cos()) as f32, (cy + ry * a.sin()) as f32];
        let _ = write!(s, "[{:?}, {:?}],", p[0], p[1]);
        pts.push(p);
    }
    s.push(']');
    let lo = |k: usize| pts.iter().map(|p| p[k]).fold(f32::INFINITY, f32::min);
    let hi = |k: usize| pts.iter().map(|p| p[k]).fold(f32::NEG_INFINITY, f32::max);
    (s, lo(0), hi(0), lo(1), hi(1))
}

#[test]
fn point_count_limits() {
    let (s, x0, x1, y0, y1) = ngon(MAX_OUTLINE_POINTS, 0.0, 150.0, 150.0, 140.0);
    let g = gen_ok(&text(Some(&s), x1 - x0, y1 - y0, -60.0, 60.0, ""));
    // The VUK exit sits inside the round outline, a ball radius above its
    // bottom, and both transition zones are below the outline.
    let m = &g.mechs[0];
    let exit = (Vec3::from(m.position.unwrap()) + Vec3::from(m.exit_offset.unwrap())) * 1e-3;
    let local = g.frame.rotation.inverse() * (exit - g.frame.origin_m) * 1e3;
    let poly = pts_of(&s);
    assert!(inside(&poly, [local.x, local.z]), "{local:?}");
    let edge_d = (0..poly.len())
        .map(|i| dist_to_segment([local.x, local.z], poly[i], poly[(i + 1) % poly.len()]))
        .fold(f32::INFINITY, f32::min);
    assert!(edge_d > 13.5, "exit {edge_d} mm from the outline");
    for z in &g.layer_transitions {
        let top = g.frame.rotation.inverse() * (Vec3::from(z.zone.max) * 1e-3 - g.frame.origin_m);
        assert!(
            top.z * 1e3 < y0 + 1.0,
            "{} zone reaches into the backbox",
            z.id
        );
    }
    let (s, x0, x1, y0, y1) = ngon(MAX_OUTLINE_POINTS + 1, 0.0, 150.0, 150.0, 140.0);
    expect_err(
        &text(Some(&s), x1 - x0, y1 - y0, -60.0, 60.0, ""),
        "backbox.outline",
        "1025 points",
    );
}

#[test]
fn outline_errors_name_the_outline() {
    let r = |o: &str| text(Some(o), 400.0, 300.0, -120.0, 150.0, "");
    expect_err(
        &r("[[-200.0, 0.0], [200.0, nan], [200.0, 300.0], [-200.0, 300.0]]"),
        "backbox.outline[1]",
        "NaN",
    );
    expect_err(
        &r("[[-200.0, 0.0], [200.0, 0.0], [inf, 300.0], [-200.0, 300.0]]"),
        "backbox.outline[2]",
        "inf",
    );
    expect_err(
        &r("[[-200.0, 0.0], [200.0, 0.0], [200.0, 300.0], [-200.0, 1.0e6]]"),
        "backbox.outline[3]",
        "within",
    );
    expect_err(
        &r("[[-200.0, 0.0], [200.0, 300.0]]"),
        "backbox.outline",
        "at least 3",
    );
    expect_err(&r("[]"), "backbox.outline", "at least 3");
    expect_err(
        &r("[[10.0, 10.0], [10.0, 10.0], [10.0, 10.0], [10.0, 10.0]]"),
        "backbox.outline",
        "zero-size",
    );
    expect_err(
        &r("[[-200.0, 0.0], [0.0, 0.0], [200.0, 0.0]]"),
        "backbox.outline",
        "zero-size",
    );
    expect_err(
        &r("[[-200.0, 5.0], [200.0, 5.0], [200.0, 5.0], [-200.0, 5.0]]"),
        "backbox.outline",
        "zero-size",
    );
    // A sliver: not collinear, but almost no area.
    expect_err(
        &r("[[0.0, 0.0], [0.5, 0.0], [0.25, 0.002]]"),
        "backbox.outline",
        "zero-size outline: area",
    );
    // Bow-tie.
    expect_err(
        &r("[[-200.0, 0.0], [200.0, 300.0], [200.0, 0.0], [-200.0, 300.0]]"),
        "backbox.outline",
        "self-intersecting",
    );
    // A vertex touching a non-adjacent edge.
    expect_err(
        &r("[[-200.0, 0.0], [200.0, 0.0], [200.0, 300.0], [0.0, 0.0], [-200.0, 300.0]]"),
        "backbox.outline",
        "self-intersecting",
    );
    // An edge folding back over its neighbour.
    expect_err(
        &r("[[-200.0, 0.0], [200.0, 0.0], [200.0, 300.0], [-200.0, 300.0], [-100.0, 300.0]]"),
        "backbox.outline",
        "self-intersecting",
    );
    // A repeated point.
    expect_err(
        &r("[[-200.0, 0.0], [200.0, 0.0], [200.0, 0.0], [200.0, 300.0], [-200.0, 300.0]]"),
        "backbox.outline[2]",
        "repeats",
    );
    // Too small (but valid as a polygon).
    expect_err(
        &text(
            Some("[[0.0, 0.0], [50.0, 0.0], [50.0, 300.0], [0.0, 300.0]]"),
            400.0,
            300.0,
            10.0,
            30.0,
            "",
        ),
        "backbox.outline",
        "bounding-box width",
    );
    expect_err(
        &text(
            Some("[[0.0, 0.0], [400.0, 0.0], [400.0, 50.0], [0.0, 50.0]]"),
            400.0,
            300.0,
            100.0,
            300.0,
            "",
        ),
        "backbox.outline",
        "bounding-box height",
    );
    // width / height must match the outline's bounding box.
    expect_err(
        &text(Some(RECT), 401.0, 300.0, -120.0, 150.0, ""),
        "backbox",
        "does not match",
    );
    expect_err(
        &text(Some(RECT), 400.0, 290.0, -120.0, 150.0, ""),
        "backbox",
        "does not match",
    );
}

#[test]
fn outline_errors_come_before_box_errors() {
    // A zero-size outline with a zero width names the outline, not the box.
    expect_err(
        &text(
            Some("[[0.0, 0.0], [100.0, 0.0], [200.0, 0.0]]"),
            0.0,
            0.0,
            0.0,
            0.0,
            "",
        ),
        "backbox.outline",
        "zero-size",
    );
}

#[test]
fn wall_errors_name_the_wall() {
    let w = |walls: &str| text(Some(RECT), 400.0, 300.0, -120.0, 150.0, walls);
    let ok = "[[backbox.walls]]\npoints = [[0.0, 50.0], [0.0, 100.0]]\n";
    expect_err(
        &w("[[backbox.walls]]\npoints = [[0.0, 50.0]]\n"),
        "backbox.walls[0]",
        "at least 2",
    );
    expect_err(
        &w("[[backbox.walls]]\npoints = []\n"),
        "backbox.walls[0]",
        "at least 2",
    );
    expect_err(
        &w(&format!(
            "{ok}[[backbox.walls]]\npoints = [[40.0, 40.0], [nan, 260.0]]\n"
        )),
        "backbox.walls[1]",
        "points[1][0] NaN",
    );
    expect_err(
        &w(&format!(
            "{ok}{ok}[[backbox.walls]]\npoints = [[40.0, 40.0], [40.0, 40.0], [40.0, 40.0]]\n"
        )),
        "backbox.walls[2]",
        "zero length",
    );
    expect_err(
        &w("[[backbox.walls]]\npoints = [[40.0, -inf], [40.0, 40.0]]\n"),
        "backbox.walls[0]",
        "points[0][1] -inf",
    );
    let mut many = String::from("[[backbox.walls]]\npoints = [");
    for i in 0..=MAX_OUTLINE_POINTS {
        let _ = write!(many, "[{:?}, 100.0],", -150.0 + i as f32 * 0.25);
    }
    many.push_str("]\n");
    expect_err(&w(&many), "backbox.walls[0]", "more than 1024");
    // Walls are checked on a box layout too.
    expect_err(
        &text(
            None,
            400.0,
            300.0,
            -120.0,
            150.0,
            "[[backbox.walls]]\npoints = [[1.0, 1.0]]\n",
        ),
        "backbox.walls[0]",
        "at least 2",
    );
    // A repeated point inside an otherwise valid wall is fine (no panel).
    let g = gen_ok(&w(
        "[[backbox.walls]]\npoints = [[0.0, 50.0], [0.0, 50.0], [0.0, 100.0]]\n",
    ));
    let names: Vec<&str> = g
        .colliders
        .iter()
        .map(|c| c.name.as_str())
        .filter(|n| n.starts_with("backbox.walls"))
        .collect();
    assert_eq!(names, ["backbox.walls[0].segments[1]"]);
    // Unknown wall keys are a parse error naming the file.
    let e = parse_backbox_layout(&w(
        "[[backbox.walls]]\npoints = [[0.0, 0.0], [1.0, 1.0]]\nbogus = 1\n",
    ))
    .unwrap_err();
    assert_eq!(e.element, "backbox_layout.toml");
}

#[test]
fn holes_and_flippers_follow_the_outline() {
    // Holes must lie within the outline's u range (here -200..200).
    expect_err(
        &text(Some(RECT), 400.0, 300.0, -190.0, 150.0, ""),
        "backbox.entry",
        "reaches past the outline",
    );
    expect_err(
        &text(Some(RECT), 400.0, 300.0, -120.0, 250.0, ""),
        "backbox.drain",
        "at 250",
    );
    expect_err(
        &text(Some(RECT), 400.0, 300.0, -120.0, -130.0, ""),
        "backbox.drain",
        "overlaps",
    );
    // A shifted outline moves the allowed range: u 0..400.
    let shifted = "[[0.0, 0.0], [400.0, 0.0], [400.0, 300.0], [0.0, 300.0]]";
    gen_ok(&text(Some(shifted), 400.0, 300.0, 100.0, 300.0, ""));
    expect_err(
        &text(Some(shifted), 400.0, 300.0, -120.0, 300.0, ""),
        "backbox.entry",
        "at -120",
    );
    let flipper = |pos: &str| {
        format!(
            "[[backbox.flippers]]\nside = \"left\"\nposition = {pos}\ncoil = \"bb_left_coil\"\n\
             eos_switch = \"bb_left_eos\"\nstaging = {{ type = \"ParallelLinked\", input = \"left_flipper\" }}\n"
        )
    };
    let g = gen_ok(&text(
        Some(L_SHAPE),
        400.0,
        300.0,
        -120.0,
        120.0,
        &flipper("[-150.0, 40.0]"),
    ));
    assert_eq!(g.flippers.len(), 1);
    // In the cut-away corner of the L: inside the bounding box, outside the
    // polygon.
    expect_err(
        &text(
            Some(L_SHAPE),
            400.0,
            300.0,
            -120.0,
            120.0,
            &flipper("[100.0, 200.0]"),
        ),
        "backbox.flippers[0]",
        "outside the backbox",
    );
    // Outside the bounding box.
    expect_err(
        &text(
            Some(L_SHAPE),
            400.0,
            300.0,
            -120.0,
            120.0,
            &flipper("[-150.0, 310.0]"),
        ),
        "backbox.flippers[0]",
        "position[1]",
    );
}

fn fmt_f(v: f32) -> String {
    if v.is_nan() {
        "nan".into()
    } else if v.is_infinite() {
        if v > 0.0 { "inf".into() } else { "-inf".into() }
    } else {
        format!("{v:?}")
    }
}

fn list(pts: &[(f32, f32)]) -> String {
    let mut s = String::from("[");
    for (x, y) in pts {
        let _ = write!(s, "[{}, {}],", fmt_f(*x), fmt_f(*y));
    }
    s.push(']');
    s
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// Random finite outlines and walls never panic parse or generate; an
    /// accepted one generates the same bytes twice, with finite colliders
    /// on layer 4 only.
    #[test]
    fn random_outlines_and_walls_never_panic(
        outline in proptest::collection::vec((-400.0f32..400.0, -50.0f32..450.0), 0..24),
        walls in proptest::collection::vec(
            proptest::collection::vec((-300.0f32..300.0, -50.0f32..400.0), 0..6),
            0..4,
        ),
        w in 50.0f32..900.0,
        h in 50.0f32..900.0,
    ) {
        let mut extra = String::new();
        for wall in &walls {
            let _ = writeln!(extra, "[[backbox.walls]]\npoints = {}\n", list(wall));
        }
        let t = text(Some(&list(&outline)), w, h, -20.0, 40.0, &extra);
        let file = parse_backbox_layout(&t);
        prop_assert!(file.is_ok(), "{:?}", file.err());
        match gen_text(&t) {
            Ok(a) => {
                let b = gen_text(&t).unwrap();
                prop_assert!(hashes(&a) == hashes(&b));
                for c in &a.colliders {
                    prop_assert_eq!(c.layer_mask, 1u8 << 4);
                    if let StaticShape::Cuboid { center, half_extents, rotation } = &c.shape {
                        prop_assert!(center.is_finite() && half_extents.is_finite() && rotation.is_finite());
                        prop_assert!(half_extents.min_element() > 0.0);
                    }
                }
            }
            Err(e) => prop_assert!(!e.element.is_empty() && !e.message.is_empty()),
        }
    }

    /// Rectangles of random size and position, with walls, always generate
    /// and keep the boundary covered.
    #[test]
    fn random_rectangles_generate_and_are_covered(
        x0 in -300.0f32..0.0,
        y0 in -100.0f32..100.0,
        w in 200.0f32..600.0,
        h in 120.0f32..600.0,
        cw in any::<bool>(),
    ) {
        let (x1, y1) = (x0 + w, y0 + h);
        let mut pts = vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
        if cw {
            pts.reverse();
        }
        let o = list(&pts);
        let poly = pts_of(&o);
        let bw = poly.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max)
            - poly.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min);
        let bh = poly.iter().map(|p| p[1]).fold(f32::NEG_INFINITY, f32::max)
            - poly.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
        let (ea, da) = (x0 + 0.25 * w, x0 + 0.75 * w);
        let wall = format!("[[backbox.walls]]\npoints = {}\n", list(&[(x0 + 0.5 * w, y0 + 0.3 * h), (x0 + 0.5 * w, y0 + 0.7 * h)]));
        let g = gen_text(&text(Some(&o), bw, bh, ea, da, &wall));
        prop_assert!(g.is_ok(), "{:?}", g.err());
        assert_edges_cover_outline(&g.unwrap(), &poly, [(ea - 20.0, ea + 20.0), (da - 20.0, da + 20.0)]);
    }

    /// Random bytes never panic the parser (nor the generator after it).
    #[test]
    fn random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        if let Ok(f) = parse_backbox_layout_bytes(&bytes) {
            let _ = generate_backbox(&f.backbox, &common::base());
        }
    }

    /// Any numbers (NaN, inf, huge) in an outline: an error or a valid
    /// generation, never a panic.
    #[test]
    fn any_numbers_never_panic(
        pts in proptest::collection::vec(
            (prop_oneof![Just(f32::NAN), Just(f32::INFINITY), Just(f32::NEG_INFINITY), -1.0e7f32..1.0e7],
             prop_oneof![Just(f32::NAN), -1.0e7f32..1.0e7]),
            0..12,
        ),
    ) {
        let t = text(Some(&list(&pts)), 400.0, 300.0, -20.0, 40.0, "");
        let _ = gen_text(&t);
    }
}
