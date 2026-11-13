//! Subphase 4.3 spec tests (Independent Tester, black box): the geometry in
//! `playfield.glb` and the collision declarations agree with the layout and
//! with each other within 0.1 mm (criterion 2, deliverable 2).
//!
//! Collision geometry as observable from outside: glb nodes tagged
//! `collider: true` (per `collision_layer`) and the generated guide splines of
//! `table.toml` (pin_physics builds a zero-thickness ribbon over each).

#[path = "fixtures/spec_4_3/support.rs"]
mod support;

use pin_core::schema::layout::LayoutManifest;
use pin_core::schema::table::TableManifest;
use proptest::prelude::*;
use std::collections::BTreeSet;
use support::*;

/// How far a wall / lane rail's faces may sit from the authored line (the
/// brief does not fix a wall thickness), mm.
const NEAR: f64 = 6.0;

struct Scene {
    layout: LayoutManifest,
    base: TableManifest,
    loaded: Loaded,
    meshes: Vec<WMesh>,
}

fn scene_for(layout: LayoutManifest, tag: &str) -> Result<Scene, String> {
    let base = base_table();
    let generated = match pin_tablegen::layout::generate(&layout, &base) {
        Ok(g) => g,
        Err(e) => return Err(format!("valid layout refused: {e}")),
    };
    let loaded = check_valid_bundle(&generated, &base, tag)?;
    let meshes = world_meshes(&loaded.scene, &layout, &generated.table);
    Ok(Scene {
        layout,
        base,
        loaded,
        meshes,
    })
}

fn geo() -> Scene {
    scene_for(geo_layout(), "geometry").unwrap_or_else(|e| panic!("{e}"))
}

// ------------------------------------------------------------------ checks

fn check_floor(s: &Scene) -> Result<(), String> {
    check_surface(
        &s.meshes,
        0,
        0.0,
        &playfield_outline(&s.layout.playfield),
        "playfield floor",
    )
    .map(|_| ())
}

/// Posts are cylinders of the layout radius: the collider's outer shell lies
/// on the circle within 0.1 mm (vertices and facet mid-points), and no mesh
/// (visual or collider) sticks out of it.
fn check_posts(s: &Scene) -> Result<(), String> {
    for (i, post) in s.layout.posts.iter().enumerate() {
        let c = [f64::from(post.position[0]), f64::from(post.position[1])];
        let r = f64::from(post.radius);
        let near = |p: P3| dist2(plan(p), c) <= r + 3.0;
        let mut shell_angles: Vec<f64> = Vec::new();
        let mut z_hi = f64::NEG_INFINITY;
        let mut z_lo = f64::INFINITY;
        for m in &s.meshes {
            // A ramp or deck passing overhead on another layer is not the post.
            if m.extras.collision_layer.is_some_and(|l| l != 0) {
                continue;
            }
            for t in m.tris.iter().filter(|t| is_vertical(t)) {
                if !t.iter().all(|p| near(*p)) || t.iter().all(|p| p[2] > 13.5) {
                    continue;
                }
                for k in 0..3 {
                    let (a, b) = (t[k], t[(k + 1) % 3]);
                    let (ra, rb) = (dist2(plan(a), c), dist2(plan(b), c));
                    if ra > r + TOL {
                        return Err(format!(
                            "posts[{i}]: mesh {:?} reaches {ra:.3} mm from the centre, radius {r}",
                            m.name
                        ));
                    }
                    if !m.is_collider_on(0) {
                        continue;
                    }
                    if ra >= r - TOL {
                        shell_angles.push((a[1] - c[1]).atan2(a[0] - c[0]));
                        z_hi = z_hi.max(a[2]);
                        z_lo = z_lo.min(a[2]);
                    }
                    // A shell facet (both ends on the circle) may not cut the
                    // cylinder by more than the tolerance.
                    if ra >= r - TOL && rb >= r - TOL {
                        let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
                        let rm = dist2(mid, c);
                        if rm < r - TOL {
                            return Err(format!(
                                "posts[{i}]: collider facet mid-point at {rm:.3} mm, cylinder \
                                 radius {r} (off by more than 0.1 mm)"
                            ));
                        }
                    }
                }
            }
        }
        if shell_angles.len() < 3 {
            return Err(format!(
                "posts[{i}]: no layer-0 collider cylinder of radius {r} at {c:?}"
            ));
        }
        shell_angles.sort_by(f64::total_cmp);
        let mut gap: f64 =
            shell_angles[0] + std::f64::consts::TAU - shell_angles[shell_angles.len() - 1];
        for w in shell_angles.windows(2) {
            gap = gap.max(w[1] - w[0]);
        }
        if r * (1.0 - (gap / 2.0).cos()) > TOL || gap >= std::f64::consts::PI {
            return Err(format!(
                "posts[{i}]: the collider shell leaves an angular gap of {:.1} degrees",
                gap.to_degrees()
            ));
        }
        // Tall enough for a ball's equator (13.5 mm), standing on the floor.
        if z_lo > TOL || z_hi < 13.5 {
            return Err(format!(
                "posts[{i}]: collider spans z = {z_lo:.2}..{z_hi:.2} mm"
            ));
        }
    }
    Ok(())
}

/// Every stretch of every wall has collision geometry and a visual mesh on
/// it, reaching from the floor to `wall_height`.
fn check_walls(s: &Scene) -> Result<(), String> {
    let collision = collision_on(&s.meshes, &s.loaded.table, &s.base, 0);
    let visual = vertical_faces(&s.meshes, |_| true);
    let h = f64::from(s.layout.playfield.wall_height);
    for (i, wall) in s.layout.walls.iter().enumerate() {
        let line = to_f64(&wall.points);
        for p in polyline_samples(&line, wall.closed, 4.0) {
            let d = collision.dist(p);
            if d > NEAR {
                return Err(format!(
                    "walls[{i}]: no collision geometry within {NEAR} mm of ({:.1}, {:.1}) \
                     (nearest {d:.2} mm)",
                    p[0], p[1]
                ));
            }
            let d = visual
                .iter()
                .map(|t| d_pt_tri(p, t))
                .fold(f64::INFINITY, f64::min);
            if d > NEAR {
                return Err(format!(
                    "walls[{i}]: no visual wall face within {NEAR} mm of ({:.1}, {:.1})",
                    p[0], p[1]
                ));
            }
        }
        // Height of the wall's faces (visual and, when meshed, collider).
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for m in &s.meshes {
            for t in m.tris.iter().filter(|t| is_vertical(t)) {
                // Only faces within the wall's own height band (a deck edge or
                // the side of a floor slab is not this wall).
                if t.iter().all(|p| {
                    d_pt_polyline(plan(*p), &line, wall.closed) <= NEAR
                        && p[2] >= -TOL
                        && p[2] <= h + TOL
                }) {
                    for p in t {
                        lo = lo.min(p[2]);
                        hi = hi.max(p[2]);
                    }
                }
            }
        }
        if (hi - h).abs() > TOL || lo.abs() > TOL {
            return Err(format!(
                "walls[{i}]: faces span z = {lo:.3}..{hi:.3} mm, expected 0..{h}"
            ));
        }
    }
    Ok(())
}

/// A lane is a channel of the given width: collision rails at +-width/2 from
/// the centre line and nothing in between.
fn check_lanes(s: &Scene) -> Result<(), String> {
    let collision = collision_on(&s.meshes, &s.loaded.table, &s.base, 0);
    let visual = vertical_faces(&s.meshes, |_| true);
    for (i, lane) in s.layout.lanes.iter().enumerate() {
        let line = to_f64(&lane.points);
        let half = f64::from(lane.width) / 2.0;
        let n_seg = line.len() - 1;
        for (k, w) in line.windows(2).enumerate() {
            let len = dist2(w[0], w[1]);
            let dir = [(w[1][0] - w[0][0]) / len, (w[1][1] - w[0][1]) / len];
            let normal = [-dir[1], dir[0]];
            // Stay clear of interior corners (mitre shape is not specified)
            // and of the lane's open ends.
            let trim = 10.0_f64.min(len / 4.0);
            let steps = ((len - 2.0 * trim) / 4.0).ceil().max(1.0) as usize;
            for j in 0..=steps {
                let t = trim + (len - 2.0 * trim) * j as f64 / steps as f64;
                let c = [w[0][0] + dir[0] * t, w[0][1] + dir[1] * t];
                for side in [-1.0, 1.0] {
                    let p = [
                        c[0] + side * half * normal[0],
                        c[1] + side * half * normal[1],
                    ];
                    let d = collision.dist(p);
                    if d > NEAR {
                        return Err(format!(
                            "lanes[{i}]: no collision rail within {NEAR} mm of ({:.1}, {:.1}), \
                             {half} mm off the centre line (nearest {d:.2} mm)",
                            p[0], p[1]
                        ));
                    }
                    let d = visual
                        .iter()
                        .map(|t| d_pt_tri(p, t))
                        .fold(f64::INFINITY, f64::min);
                    if d > NEAR {
                        return Err(format!(
                            "lanes[{i}]: no visual rail within {NEAR} mm of ({:.1}, {:.1})",
                            p[0], p[1]
                        ));
                    }
                }
                let d = collision.dist(c);
                if d < half - NEAR {
                    return Err(format!(
                        "lanes[{i}] segment {k}/{n_seg}: collision geometry {d:.2} mm from the \
                         centre line at ({:.1}, {:.1}); the lane is {} mm wide",
                        c[0], c[1], lane.width
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Criterion 2 for guides, both directions: a generated guide runs along a
/// visual wall face within 0.1 mm, and every wall face near a wall or lane
/// has collision geometry within 0.1 mm.
fn check_guides_match_visual(s: &Scene) -> Result<(), String> {
    let guides = guide_curves(&s.loaded.table, &s.base, 0);
    let visual = vertical_faces(&s.meshes, |_| true);
    for (g, guide) in guides.iter().enumerate() {
        for (k, p) in guide.iter().enumerate().step_by(6) {
            let d = visual
                .iter()
                .map(|t| d_pt_tri(*p, t))
                .fold(f64::INFINITY, f64::min);
            if d > TOL {
                return Err(format!(
                    "generated guide #{g}: curve point {k} at ({:.3}, {:.3}) is {d:.3} mm from \
                     the nearest visual wall face (collision and visual disagree by > 0.1 mm)",
                    p[0], p[1]
                ));
            }
        }
    }
    let collision = collision_on(&s.meshes, &s.loaded.table, &s.base, 0);
    let wall_h = f64::from(s.layout.playfield.wall_height);
    let mut lines: Vec<(String, Vec<P2>, bool, f64)> = Vec::new();
    for (i, w) in s.layout.walls.iter().enumerate() {
        lines.push((format!("walls[{i}]"), to_f64(&w.points), w.closed, NEAR));
    }
    for (i, l) in s.layout.lanes.iter().enumerate() {
        lines.push((
            format!("lanes[{i}]"),
            to_f64(&l.points),
            false,
            f64::from(l.width) / 2.0 + NEAR,
        ));
    }
    for m in &s.meshes {
        // Faces on other layers (deck edges) are not layer-0 walls.
        if m.extras.collision_layer.is_some_and(|l| l != 0) || m.extras.lamp.is_some() {
            continue;
        }
        for t in m.tris.iter().filter(|t| is_vertical(t)) {
            for (what, line, closed, reach) in &lines {
                if !t.iter().all(|p| {
                    d_pt_polyline(plan(*p), line, *closed) <= *reach
                        && p[2] >= -TOL
                        && p[2] <= wall_h + TOL
                }) {
                    continue;
                }
                for p in t {
                    let d = collision.dist(plan(*p));
                    if d > TOL {
                        return Err(format!(
                            "{what}: visual face vertex ({:.3}, {:.3}, {:.3}) of mesh {:?} is \
                             {d:.3} mm from the collision geometry (> 0.1 mm)",
                            p[0], p[1], p[2], m.name
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Inserts: lamp-bound shapes slightly above the floor, at their position.
fn check_inserts(s: &Scene) -> Result<(), String> {
    for (i, insert) in s.layout.inserts.iter().enumerate() {
        let c = [f64::from(insert.position[0]), f64::from(insert.position[1])];
        let size = f64::from(insert.size);
        let verts: Vec<P3> = s
            .meshes
            .iter()
            .filter(|m| m.extras.lamp.as_deref() == Some(insert.lamp.as_str()))
            .flat_map(|m| m.vertices())
            .collect();
        if verts.len() < 3 {
            return Err(format!("inserts[{i}]: no lamp-bound geometry"));
        }
        let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for p in &verts {
            if dist2(plan(*p), c) > size + TOL {
                return Err(format!(
                    "inserts[{i}]: vertex {p:?} is farther than its size ({size}) from {c:?}"
                ));
            }
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        if c[0] < lo[0] - TOL || c[0] > hi[0] + TOL || c[1] < lo[1] - TOL || c[1] > hi[1] + TOL {
            return Err(format!("inserts[{i}]: shape does not contain its position"));
        }
        if (hi[0] - lo[0]).max(hi[1] - lo[1]) < size / 4.0 {
            return Err(format!(
                "inserts[{i}]: shape is far smaller than size {size}"
            ));
        }
        if lo[2] < -1.0e-3 || hi[2] <= 1.0e-4 || hi[2] > 2.0 {
            return Err(format!(
                "inserts[{i}]: spans z = {:.4}..{:.4} mm, expected slightly above the floor",
                lo[2], hi[2]
            ));
        }
    }
    Ok(())
}

fn check_all(s: &Scene) -> Result<(), String> {
    check_floor(s)?;
    check_posts(s)?;
    check_walls(s)?;
    check_lanes(s)?;
    check_guides_match_visual(s)?;
    check_inserts(s)
}

// ------------------------------------------------------------------- tests

#[test]
fn floor_is_one_gapless_triangle_mesh() {
    check_floor(&geo()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn deck_is_one_surface_on_its_layer_at_its_height() {
    let s = geo();
    for (i, deck) in s.layout.decks.iter().enumerate() {
        let outline = to_f64(&deck.outline);
        let index = check_surface(
            &s.meshes,
            deck.layer,
            f64::from(deck.z),
            &outline,
            &format!("decks[{i}]"),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        // The deck surface stays inside its outline.
        let mesh = &s.meshes[index];
        for t in mesh.tris.iter().filter(|t| is_horizontal(t)) {
            for p in t {
                assert!(
                    pt_in_poly(plan(*p), &outline)
                        || d_pt_polyline(plan(*p), &outline, true) <= TOL,
                    "decks[{i}]: surface vertex {p:?} outside the outline"
                );
            }
        }
        // And is not also the layer-0 floor.
        assert!(!mesh.is_collider_on(0));
    }
}

#[test]
fn posts_are_cylinders_of_the_layout_radius() {
    check_posts(&geo()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn walls_have_collision_and_visual_geometry_to_wall_height() {
    check_walls(&geo()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn lanes_are_channels_of_the_given_width() {
    check_lanes(&geo()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn guides_and_visual_walls_agree_within_a_tenth_of_a_millimetre() {
    check_guides_match_visual(&geo()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn inserts_sit_slightly_above_the_floor_at_their_position() {
    check_inserts(&geo()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn appendix_a3_geometry_agrees() {
    let s = scene_for(a3_layout(), "a3geo").unwrap_or_else(|e| panic!("{e}"));
    check_all(&s).unwrap_or_else(|e| panic!("{e}"));
}

/// No collision geometry and no wall faces away from every layout element
/// (nothing invented, nothing misplaced).
#[test]
fn no_stray_geometry() {
    let s = geo();
    let l = &s.layout;
    let near_element = |p: P2| -> bool {
        l.walls
            .iter()
            .any(|w| d_pt_polyline(p, &to_f64(&w.points), w.closed) <= NEAR)
            || l.lanes.iter().any(|ln| {
                d_pt_polyline(p, &to_f64(&ln.points), false)
                    <= f64::from(ln.width) / 2.0 + NEAR + 10.0
            })
            || l.posts.iter().any(|po| {
                dist2(p, [f64::from(po.position[0]), f64::from(po.position[1])])
                    <= f64::from(po.radius) + TOL
            })
            || l.decks
                .iter()
                .any(|d| d_pt_polyline(p, &to_f64(&d.outline), true) <= NEAR)
            || l.inserts.iter().any(|ins| {
                dist2(p, [f64::from(ins.position[0]), f64::from(ins.position[1])])
                    <= f64::from(ins.size) + TOL
            })
            || d_pt_polyline(p, &playfield_outline(&l.playfield), true) <= NEAR
    };
    for m in &s.meshes {
        for t in m.tris.iter().filter(|t| is_vertical(t)) {
            for p in t {
                assert!(
                    near_element(plan(*p)),
                    "mesh {:?} has a wall face at {p:?}, away from every layout element",
                    m.name
                );
            }
        }
    }
    for g in guide_curves(&s.loaded.table, &s.base, 0) {
        for p in g {
            assert!(near_element(p), "a generated guide passes {p:?}");
        }
    }
    // Generated splines are on a layer the layout uses.
    let layers: BTreeSet<u8> = std::iter::once(0)
        .chain(l.decks.iter().map(|d| d.layer))
        .collect();
    for id in generated_spline_ids(&s.loaded.table.manifest, &s.base) {
        let spline = s
            .loaded
            .table
            .manifest
            .splines
            .iter()
            .find(|x| x.id == id)
            .unwrap();
        assert!(
            layers.contains(&spline.layer),
            "spline {id:?} on layer {}",
            spline.layer
        );
    }
}

/// Flat-colour PBR materials per Material (deliverable 2).
#[test]
fn materials_are_flat_colours_per_material() {
    let s = geo();
    let scene = &s.loaded.scene;
    assert!(!scene.materials.is_empty());
    assert!(scene.images.is_empty(), "flat colours need no images");
    for (i, m) in scene.materials.iter().enumerate() {
        assert!(
            m.base_color_image.is_none()
                && m.metallic_roughness_image.is_none()
                && m.normal_image.is_none()
                && m.emissive_image.is_none(),
            "material {i} ({:?}) is textured",
            m.name
        );
        assert!(m.base_color.iter().all(|c| (0.0..=1.0).contains(c)));
        assert!((0.0..=1.0).contains(&m.metallic) && (0.0..=1.0).contains(&m.roughness));
    }
    for m in &scene.meshes {
        assert!(
            m.material.is_some_and(|i| i < scene.materials.len()),
            "mesh {:?} has no material",
            m.name
        );
    }
    // Materials of the faces along each wall: wood, steel and plastic differ.
    let wall_materials = |i: usize| -> BTreeSet<usize> {
        let w = &s.layout.walls[i];
        let line = to_f64(&w.points);
        s.meshes
            .iter()
            .filter(|m| {
                m.tris.iter().any(|t| {
                    is_vertical(t)
                        && t.iter()
                            .all(|p| d_pt_polyline(plan(*p), &line, w.closed) <= NEAR)
                })
            })
            .filter_map(|m| m.material)
            .collect()
    };
    let (wood, steel, plastic) = (wall_materials(0), wall_materials(1), wall_materials(2));
    assert!(!wood.is_empty() && !steel.is_empty() && !plastic.is_empty());
    assert!(wood.is_disjoint(&steel), "wood {wood:?} steel {steel:?}");
    assert!(wood.is_disjoint(&plastic));
    assert!(steel.is_disjoint(&plastic));
    // Posts: rubber = true gets the rubber material, others do not.
    let post_materials = |i: usize| -> BTreeSet<usize> {
        let po = &s.layout.posts[i];
        let c = [f64::from(po.position[0]), f64::from(po.position[1])];
        let r = f64::from(po.radius);
        s.meshes
            .iter()
            .filter(|m| {
                m.tris.iter().any(|t| {
                    is_vertical(t)
                        && t.iter().all(|p| {
                            let d = dist2(plan(*p), c);
                            d <= r + TOL && d >= r - TOL
                        })
                })
            })
            .filter_map(|m| m.material)
            .collect()
    };
    let (rubber_a, plain, rubber_b) = (post_materials(0), post_materials(1), post_materials(2));
    assert!(!rubber_a.is_empty() && !plain.is_empty());
    assert_eq!(
        rubber_a, rubber_b,
        "both rubber posts share the rubber material"
    );
    assert!(
        rubber_a.is_disjoint(&plain),
        "rubber and plain posts share a material"
    );
    // The acrylic deck is none of wood / steel.
    let deck = &s.layout.decks[0];
    let index = check_surface(
        &s.meshes,
        deck.layer,
        f64::from(deck.z),
        &to_f64(&deck.outline),
        "decks[0]",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let deck_material = s.meshes[index].material.unwrap();
    assert!(!wood.contains(&deck_material) && !steel.contains(&deck_material));
}

/// Collider tags (deliverable 2): every collider node names its layer, and
/// the collision of each layout element is on the right layer.
#[test]
fn collider_nodes_are_tagged_with_their_layer() {
    let s = geo();
    let mut layers = BTreeSet::new();
    for m in &s.meshes {
        if m.extras.collider {
            layers.insert(m.extras.collision_layer.expect("collider without a layer"));
        }
    }
    assert!(layers.contains(&0));
    assert!(layers.contains(&1), "the deck's collider is on layer 1");
    assert!(layers.iter().all(|l| *l <= 7));
    // Nothing collides on a layer the layout does not use.
    assert!(layers.iter().all(|l| *l == 0 || *l == 1), "{layers:?}");
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, failure_persistence: None, ..ProptestConfig::default() })]

    /// Criterion 2 across random valid layouts.
    #[test]
    fn random_layouts_agree_with_their_collision_geometry(layout in valid_layout()) {
        let s = scene_for(layout, "randgeo").map_err(TestCaseError::fail)?;
        check_all(&s).map_err(TestCaseError::fail)?;
    }
}
