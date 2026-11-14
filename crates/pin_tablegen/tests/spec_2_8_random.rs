//! Subphase 2.8 spec tests (Independent Tester, black box): the clearance
//! validator never panics on generated or random tables, and its report
//! keeps the contract's invariants (property tests).
//!
//! Random bundles come from the merged 4.3 generator fed with random
//! layouts (decks, lanes, walls, posts) and random table sections (ball
//! profiles and restrictions incl. malformed areas, a second flipper, a
//! wireform, a VUK, hand-written lane guides, layer transitions). A second
//! property feeds the validator scenes that do not belong to the table and
//! scenes with perturbed geometry.

#[path = "fixtures/spec_2_8/support.rs"]
mod support;

use pin_assets::GltfScene;
use pin_tablegen::clearance::{ClearanceReport, Feature, validate_clearance};
use proptest::prelude::*;
use std::collections::BTreeSet;
use support::*;

const PROFILES: [&str; 7] = pin_core::profiles::BUILTIN_PROFILE_NAMES;

#[derive(Clone, Debug)]
struct Restrict {
    profile: usize,
    layers: Vec<u8>,
    area: Vec<[f32; 2]>,
}

#[derive(Clone, Debug)]
struct Case {
    profiles: Vec<usize>,
    restrict: Vec<Restrict>,
    dispenser: Vec<usize>,
    decks: Vec<(u8, f32, [f32; 4])>,
    lanes: Vec<(f32, [f32; 4])>,
    walls: Vec<[f32; 4]>,
    posts: Vec<([f32; 2], f32)>,
    right_flipper: Option<(f32, f32, f32, u8)>,
    wireform: Option<(f32, u8, f32)>,
    vuk: Option<([f32; 2], u8, u8, [f32; 3])>,
    hand_lane: Option<(f32, u8)>,
    transition: Option<(u8, u8, [f32; 2])>,
}

fn r1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

fn point() -> impl Strategy<Value = [f32; 2]> {
    (-50.0f32..564.0, -50.0f32..1120.0).prop_map(|(x, y)| [r1(x), r1(y)])
}

fn area() -> impl Strategy<Value = Vec<[f32; 2]>> {
    prop_oneof![
        3 => Just(Vec::new()),
        // Rectangles.
        4 => (0.0f32..400.0, 0.0f32..900.0, 1.0f32..500.0, 1.0f32..1000.0).prop_map(|(x, y, w, h)| {
            let (x, y, w, h) = (r1(x), r1(y), r1(w), r1(h));
            vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h]]
        }),
        // Any polygon, self-intersecting ones included.
        3 => prop::collection::vec(point(), 3..8),
        // Malformed: too few points, repeated points, collinear points.
        1 => prop::collection::vec(point(), 1..3),
        1 => point().prop_map(|p| vec![p, p, p, p]),
        1 => (point(), 1.0f32..100.0).prop_map(|(p, d)| vec![p, [p[0] + d, p[1]], [p[0] + 2.0 * d, p[1]]]),
    ]
}

fn restrict() -> impl Strategy<Value = Restrict> {
    (0usize..7, prop::collection::vec(0u8..8, 0..4), area()).prop_map(|(profile, layers, area)| {
        Restrict {
            profile,
            layers,
            area,
        }
    })
}

fn rect() -> impl Strategy<Value = [f32; 4]> {
    (
        10.0f32..400.0,
        10.0f32..900.0,
        20.0f32..300.0,
        20.0f32..400.0,
    )
        .prop_map(|(x, y, w, h)| {
            [
                r1(x),
                r1(y),
                r1((x + w).min(504.0)),
                r1((y + h).min(1060.0)),
            ]
        })
}

fn case() -> impl Strategy<Value = Case> {
    let balls = (
        prop::collection::vec(0usize..7, 1..5),
        prop::collection::vec(restrict(), 0..4),
        prop::collection::vec(0usize..7, 1..4),
    );
    let layout = (
        prop::collection::vec((1u8..4, 5.0f32..160.0, rect()), 0..3),
        prop::collection::vec((16.0f32..110.0, rect()), 0..3),
        prop::collection::vec(rect(), 0..4),
        prop::collection::vec((point(), 2.0f32..15.0), 0..4),
    );
    let table = (
        prop::option::of((250.0f32..500.0, 60.0f32..200.0, 150.0f32..260.0, 0u8..2)),
        prop::option::of((3.0f32..100.0, 0u8..4, 0.0f32..150.0)),
        prop::option::of((
            point(),
            0u8..3,
            0u8..3,
            prop::array::uniform3(-120.0f32..200.0),
        )),
        prop::option::of((10.0f32..120.0, 0u8..3)),
        prop::option::of((0u8..4, 0u8..4, point())),
    );
    (balls, layout, table).prop_map(
        |(
            (profiles, restrict, dispenser),
            (decks, lanes, walls, posts),
            (right_flipper, wireform, vuk, hand_lane, transition),
        )| Case {
            profiles,
            restrict,
            dispenser,
            decks,
            lanes,
            walls,
            posts,
            right_flipper,
            wireform,
            vuk,
            hand_lane,
            transition,
        },
    )
}

fn names(ix: &[usize]) -> String {
    ix.iter()
        .map(|i| format!("{:?}", PROFILES[*i]))
        .collect::<Vec<_>>()
        .join(", ")
}

impl Case {
    fn balls(&self, with_restrictions: bool) -> String {
        let mut s = format!("profiles = [{}]", names(&self.profiles));
        if with_restrictions && !self.restrict.is_empty() {
            let entries: Vec<String> = self
                .restrict
                .iter()
                .map(|r| {
                    let area = if r.area.is_empty() {
                        String::new()
                    } else {
                        format!(
                            ", area = [{}]",
                            r.area
                                .iter()
                                .map(|p| format!("[{:?}, {:?}]", p[0], p[1]))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    };
                    format!(
                        "{{ profile = {:?}, layers = {:?}{area} }}",
                        PROFILES[r.profile], r.layers
                    )
                })
                .collect();
            s.push_str(&format!("\nrestrict = [ {} ]", entries.join(", ")));
        }
        s
    }

    fn layout(&self) -> String {
        let mut s = String::new();
        for (layer, z, r) in &self.decks {
            s.push_str(&format!(
                "\n[[decks]]\nlayer = {layer}\nz = {:?}\noutline = [[{:?}, {:?}], [{:?}, {:?}], [{:?}, {:?}], [{:?}, {:?}]]\nmaterial = \"acrylic\"\n",
                r1(*z), r[0], r[1], r[2], r[1], r[2], r[3], r[0], r[3]
            ));
        }
        for (width, r) in &self.lanes {
            s.push_str(&format!(
                "\n[[lanes]]\nkind = \"orbit\"\npoints = [[{:?}, {:?}], [{:?}, {:?}]]\nwidth = {:?}\n",
                r[0], r[1], r[2], r[3], r1(*width)
            ));
        }
        for r in &self.walls {
            s.push_str(&format!(
                "\n[[walls]]\npoints = [[{:?}, {:?}], [{:?}, {:?}]]\nclosed = false\nmaterial = \"steel\"\n",
                r[0], r[1], r[2], r[3]
            ));
        }
        for (p, radius) in &self.posts {
            s.push_str(&format!(
                "\n[[posts]]\nposition = [{:?}, {:?}]\nradius = {:?}\nrubber = true\n",
                p[0],
                p[1],
                r1(*radius)
            ));
        }
        layout_toml(&s)
    }

    fn table(&self, with_restrictions: bool) -> String {
        let mut s = String::new();
        if let Some((x, y, rest, layer)) = self.right_flipper {
            s.push_str(&format!(
                r#"
[[flippers]]
id = "lower_right"
coil = "right_flipper_coil"
eos_switch = "lower_right_eos"
collision_layer = {layer}
base_position = [{:?}, {:?}, 0.0]
surface_normal = [0.0, 0.0, 1.0]
rest_angle_deg = {:?}
sweep_angle_deg = -50.0
eos_angle_fraction = 0.92
shape = {{ type = "StraightCapsule", length = 76.0, base_radius = 11.0, tip_radius = 6.5 }}
staging = {{ type = "DedicatedButton", input = "right_flipper" }}
"#,
                r1(x),
                r1(y),
                r1(rest)
            ));
        }
        if let Some((gauge, layer, z)) = self.wireform {
            let z = r1(z);
            s.push_str(&format!(
                "\n[[splines]]\nid = \"wf\"\nkind = \"wireform\"\nlayer = {layer}\ngauge = {:?}\npoints = [[60.0, 900.0, {z:?}], [40.0, 600.0, {:?}], [150.0, 260.0, {z:?}]]\nroll_deg = [0.0, 8.0, 0.0]\n",
                r1(gauge),
                z + 15.0
            ));
        }
        if let Some((p, layer, exit_layer, off)) = self.vuk {
            s.push_str(&format!(
                "\n[[mechs]]\nid = \"scoop\"\nkind = \"vuk\"\ncoil = \"scoop_coil\"\nswitch = \"scoop_switch\"\nposition = [{:?}, {:?}, 0.0]\nlayer = {layer}\nexit_layer = {exit_layer}\nexit_offset = [{:?}, {:?}, {:?}]\n",
                p[0], p[1], r1(off[0]), r1(off[1]), r1(off[2])
            ));
        }
        if let Some((gap, layer)) = self.hand_lane {
            let x = 200.0 + r1(gap);
            s.push_str(&format!(
                "\n[[splines]]\nid = \"feed.left\"\nkind = \"guide\"\nlayer = {layer}\npoints = [[200.0, 400.0, 0.0], [200.0, 480.0, 0.0], [200.0, 560.0, 0.0]]\n\n[[splines]]\nid = \"feed.right\"\nkind = \"guide\"\nlayer = {layer}\npoints = [[{x:?}, 400.0, 0.0], [{x:?}, 480.0, 0.0], [{x:?}, 560.0, 0.0]]\n"
            ));
        }
        if let Some((from, to, p)) = self.transition {
            s.push_str(&format!(
                "\n[[layer_transitions]]\nid = \"hop\"\nfrom_layer = {from}\nto_layer = {to}\nzone = {{ min = [{:?}, {:?}, 0.0], max = [{:?}, {:?}, 40.0] }}\n",
                p[0], p[1], p[0] + 50.0, p[1] + 40.0
            ));
        }
        let text = table_toml(&self.balls(with_restrictions), &s);
        // The base table's dispenser holds two standard balls; give it the
        // case's contents.
        let from = "contents = [\"standard_steel\", \"standard_steel\"]";
        assert!(text.contains(from));
        text.replace(from, &format!("contents = [{}]", names(&self.dispenser)))
    }

    fn declared(&self, with_restrictions: bool) -> BTreeSet<&'static str> {
        let mut out: BTreeSet<&'static str> = self
            .profiles
            .iter()
            .chain(&self.dispenser)
            .map(|i| PROFILES[*i])
            .collect();
        if with_restrictions {
            out.extend(self.restrict.iter().map(|r| PROFILES[r.profile]));
        }
        out
    }
}

/// The contract's invariants of a report.
fn check_report(
    report: &ClearanceReport,
    loaded: &Loaded,
    declared: &BTreeSet<&'static str>,
) -> Result<(), TestCaseError> {
    for e in report.errors.iter().chain(&report.warnings) {
        check_error_shape(e, Some(loaded));
        prop_assert!(
            declared.contains(e.profile.as_str()),
            "finding for a profile the table does not declare: {e:?}"
        );
        prop_assert!(!e.message.trim().is_empty());
        // A length finding carries finite, non-negative numbers.
        if e.feature != Feature::Restriction {
            prop_assert!(e.required_mm.is_finite() && e.required_mm > 0.0, "{e:?}");
            prop_assert!(!e.actual_mm.is_nan(), "{e:?}");
        }
        // Key paths point at something that exists.
        let m = &loaded.table.manifest;
        let index = |prefix: &str| -> Option<usize> {
            let rest = e.path.strip_prefix(prefix)?.strip_prefix('[')?;
            rest[..rest.find(']')?].parse().ok()
        };
        if e.file == "table.toml" {
            if let Some(i) = index("splines") {
                prop_assert!(i < m.splines.len(), "{e:?}");
            } else if let Some(i) = index("flippers") {
                prop_assert!(i < m.flippers.len(), "{e:?}");
            } else if let Some(i) = index("mechs") {
                prop_assert!(i < m.mechs.len(), "{e:?}");
            } else if let Some(i) = index("layer_transitions") {
                prop_assert!(i < m.layer_transitions.len(), "{e:?}");
            } else if let Some(i) = index("balls.restrict") {
                prop_assert!(i < m.balls.restrict.len(), "{e:?}");
            } else {
                prop_assert!(false, "unexpected key path {:?}", e.path);
            }
        } else {
            let i = index("meshes");
            prop_assert!(i.is_some_and(|i| i < loaded.scene.meshes.len()), "{e:?}");
        }
    }
    assert_sorted(&report.errors, "errors");
    assert_sorted(&report.warnings, "warnings");
    prop_assert_eq!(report.is_ok(), report.errors.is_empty());
    let (ne, nw) = (report.errors.len(), report.warnings.len());
    match report.clone().into_result() {
        Ok(w) => prop_assert!(ne == 0 && w.len() == nw),
        Err(e) => prop_assert!(ne > 0 && e.len() == ne),
    }
    Ok(())
}

fn keys(report: &ClearanceReport) -> BTreeSet<(String, String, String)> {
    report
        .errors
        .iter()
        .filter(|e| e.feature != Feature::Restriction)
        .map(|e| (e.file.clone(), e.path.clone(), e.profile.clone()))
        .collect()
}

fn text(report: &ClearanceReport) -> String {
    format!("{:?}\n{:?}", report.errors, report.warnings)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 192, max_global_rejects: 8192, .. ProptestConfig::default() })]

    /// Random generated tables: no panic, the report invariants hold, the
    /// result is deterministic, and a restriction only ever removes
    /// findings.
    #[test]
    fn validator_never_panics_on_generated_tables(c in case()) {
        let dir = match try_bundle("random", &c.layout(), &c.table(true)) {
            Ok(d) => d,
            // The generator or the schema refused the random input.
            Err(_) => return Err(TestCaseError::reject("fixture not generated")),
        };
        let loaded = try_load(&dir);
        // The bundle entry point must not panic either, loadable or not.
        if let Ok(vfs) = pin_assets::Vfs::open(&dir) {
            let via = pin_tablegen::clearance::validate_bundle(&vfs);
            prop_assert_eq!(via.is_ok(), loaded.is_ok(), "validate_bundle and the loader disagree");
            if let (Ok(r), Ok(l)) = (&via, &loaded) {
                prop_assert_eq!(text(r), text(&validate_clearance(&l.table, &l.scene)));
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
        let Ok(loaded) = loaded else {
            return Err(TestCaseError::reject("the 4.1 loader refuses the bundle"));
        };
        let report = validate_clearance(&loaded.table, &loaded.scene);
        check_report(&report, &loaded, &c.declared(true))?;
        prop_assert_eq!(text(&report), text(&validate_clearance(&loaded.table, &loaded.scene)));

        // The same table without its restrictions: every non-restriction
        // error of the restricted table is still there for profiles that
        // are declared either way.
        if !c.restrict.is_empty() {
            let Ok(dir) = try_bundle("random-open", &c.layout(), &c.table(false)) else {
                return Ok(());
            };
            let open = try_load(&dir);
            let _ = std::fs::remove_dir_all(&dir);
            if let Ok(open) = open {
                let open_report = validate_clearance(&open.table, &open.scene);
                check_report(&open_report, &open, &c.declared(false))?;
                let open_keys = keys(&open_report);
                let still = c.declared(false);
                for k in keys(&report) {
                    if still.contains(k.2.as_str()) {
                        prop_assert!(
                            open_keys.contains(&k),
                            "restricting profiles added the finding {:?}\nrestricted:\n{}unrestricted:\n{}",
                            k, describe(&report), describe(&open_report)
                        );
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------ foreign and perturbed scenes

#[derive(Clone, Debug)]
enum Tweak {
    DropMesh(usize),
    DuplicateMesh(usize),
    ToggleCollider(usize),
    SetLayer(usize, Option<u8>),
    Scale(usize, f32),
    Offset(usize, [f32; 3]),
    Collapse(usize),
    Flatten(usize, usize),
    Jitter(usize, u64),
    DropTriangles(usize, usize),
    Transform(usize, [f32; 3], f32),
    Clear,
}

fn tweak() -> impl Strategy<Value = Tweak> {
    prop_oneof![
        any::<usize>().prop_map(Tweak::DropMesh),
        any::<usize>().prop_map(Tweak::DuplicateMesh),
        any::<usize>().prop_map(Tweak::ToggleCollider),
        (any::<usize>(), prop::option::of(any::<u8>())).prop_map(|(i, l)| Tweak::SetLayer(i, l)),
        (
            any::<usize>(),
            prop_oneof![Just(0.0f32), 0.01f32..3.0, -2.0f32..-0.1]
        )
            .prop_map(|(i, s)| Tweak::Scale(i, s)),
        (any::<usize>(), prop::array::uniform3(-0.3f32..0.3))
            .prop_map(|(i, o)| Tweak::Offset(i, o)),
        any::<usize>().prop_map(Tweak::Collapse),
        (any::<usize>(), 0usize..3).prop_map(|(i, a)| Tweak::Flatten(i, a)),
        (any::<usize>(), any::<u64>()).prop_map(|(i, s)| Tweak::Jitter(i, s)),
        (any::<usize>(), 0usize..8).prop_map(|(i, n)| Tweak::DropTriangles(i, n)),
        (
            any::<usize>(),
            prop::array::uniform3(-0.5f32..0.5),
            0.2f32..2.0
        )
            .prop_map(|(i, t, s)| Tweak::Transform(i, t, s)),
        Just(Tweak::Clear),
    ]
}

fn apply(scene: &mut GltfScene, t: &Tweak) {
    let n = scene.meshes.len();
    if n == 0 {
        return;
    }
    match t {
        Tweak::DropMesh(i) => {
            scene.meshes.remove(i % n);
        }
        Tweak::DuplicateMesh(i) => {
            let m = scene.meshes[i % n].clone();
            scene.meshes.push(m);
        }
        Tweak::ToggleCollider(i) => {
            let e = &mut scene.meshes[i % n].extras;
            e.collider = !e.collider;
        }
        Tweak::SetLayer(i, l) => scene.meshes[i % n].extras.collision_layer = *l,
        Tweak::Scale(i, s) => {
            for p in &mut scene.meshes[i % n].positions {
                for c in p.iter_mut() {
                    *c *= s;
                }
            }
        }
        Tweak::Offset(i, o) => {
            for p in &mut scene.meshes[i % n].positions {
                for (c, d) in p.iter_mut().zip(o) {
                    *c += d;
                }
            }
        }
        Tweak::Collapse(i) => {
            let m = &mut scene.meshes[i % n];
            if let Some(first) = m.positions.first().copied() {
                for p in &mut m.positions {
                    *p = first;
                }
            }
        }
        Tweak::Flatten(i, axis) => {
            for p in &mut scene.meshes[i % n].positions {
                p[*axis] = 0.0;
            }
        }
        Tweak::Jitter(i, seed) => {
            // A small deterministic LCG: ±20 mm per coordinate.
            let mut s = *seed | 1;
            for p in &mut scene.meshes[i % n].positions {
                for c in p.iter_mut() {
                    s = s
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    *c += ((s >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.04;
                }
            }
        }
        Tweak::DropTriangles(i, k) => {
            let m = &mut scene.meshes[i % n];
            let keep = m.indices.len().saturating_sub(3 * k);
            m.indices.truncate(keep - keep % 3);
        }
        Tweak::Transform(i, t, s) => {
            scene.meshes[i % n].transform = [
                [*s, 0.0, 0.0, 0.0],
                [0.0, *s, 0.0, 0.0],
                [0.0, 0.0, *s, 0.0],
                [t[0], t[1], t[2], 1.0],
            ];
        }
        Tweak::Clear => scene.meshes.clear(),
    }
}

/// Reports must stay well-formed whatever the scene; only the scene-bound
/// parts of the shape check are skipped (mesh indices are checked against
/// the tweaked scene).
fn check_loose(report: &ClearanceReport, scene: &GltfScene) -> Result<(), TestCaseError> {
    for e in report.errors.iter().chain(&report.warnings) {
        check_error_shape(e, None);
        if let Some(rest) = e.path.strip_prefix("meshes[") {
            let i: usize = rest.trim_end_matches(']').parse().unwrap_or(usize::MAX);
            prop_assert!(i < scene.meshes.len(), "{e:?}");
            prop_assert_eq!(&e.name, &scene.meshes[i].name);
        }
    }
    assert_sorted(&report.errors, "errors");
    prop_assert_eq!(report.is_ok(), report.errors.is_empty());
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, max_global_rejects: 8192, .. ProptestConfig::default() })]

    /// A table checked against a scene generated from another layout, and
    /// against scenes with perturbed geometry: no panic, well-formed report.
    #[test]
    fn validator_never_panics_on_foreign_or_perturbed_scenes(
        a in case(),
        b in case(),
        tweaks in prop::collection::vec(tweak(), 0..6),
    ) {
        let Ok(dir_a) = try_bundle("foreign-a", &a.layout(), &a.table(true)) else {
            return Err(TestCaseError::reject("fixture a"));
        };
        let la = try_load(&dir_a);
        let _ = std::fs::remove_dir_all(&dir_a);
        let Ok(la) = la else { return Err(TestCaseError::reject("load a")); };
        let Ok(dir_b) = try_bundle("foreign-b", &b.layout(), &a.table(true)) else {
            return Err(TestCaseError::reject("fixture b"));
        };
        let lb = try_load(&dir_b);
        let _ = std::fs::remove_dir_all(&dir_b);
        let Ok(lb) = lb else { return Err(TestCaseError::reject("load b")); };
        // Table a, scene b.
        let r = validate_clearance(&la.table, &lb.scene);
        check_loose(&r, &lb.scene)?;
        // Table a, its own scene perturbed step by step.
        let mut scene = la.scene.clone();
        for t in &tweaks {
            apply(&mut scene, t);
            let r = validate_clearance(&la.table, &scene);
            check_loose(&r, &scene)?;
        }
    }
}

/// The empty scene and a table with every feature: nothing to collide
/// with, no panic.
#[test]
fn validator_handles_an_empty_scene() {
    let dir = bundle(
        "empty-scene",
        &layout_toml(""),
        &table_toml(&balls(&[STD, XL, MINI]), ""),
    );
    let mut l = load(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    l.scene.meshes.clear();
    let r = validate_clearance(&l.table, &l.scene);
    assert_eq!(r.is_ok(), r.errors.is_empty());
    for m in 0..3 {
        // Meshes without triangles, or with triangles but all collapsed.
        let dir = bundle(
            "empty-scene-2",
            &layout_toml(""),
            &table_toml(&balls(&[STD, XL]), ""),
        );
        let mut l = load(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        for mesh in &mut l.scene.meshes {
            match m {
                0 => mesh.indices.clear(),
                1 => mesh.positions.iter_mut().for_each(|p| *p = [0.0; 3]),
                _ => mesh.extras.collision_layer = None,
            }
        }
        let _ = validate_clearance(&l.table, &l.scene);
    }
}

/// Guards the properties above against being vacuous: of 300 deterministic
/// random cases most must build and load, and every feature kind must be
/// both violated and satisfied somewhere.
#[test]
fn random_cases_exercise_every_feature() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;
    let mut runner = TestRunner::deterministic();
    let strategy = case();
    let (mut built, mut loaded_n, mut ok, mut warned) = (0, 0, 0, 0);
    let mut by_feature = std::collections::BTreeMap::<&'static str, usize>::new();
    let mut first_fail = None;
    for _ in 0..300 {
        let c = strategy.new_tree(&mut runner).unwrap().current();
        let dir = match try_bundle("coverage", &c.layout(), &c.table(true)) {
            Ok(d) => d,
            Err(e) => {
                first_fail.get_or_insert(e);
                continue;
            }
        };
        built += 1;
        let l = try_load(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        let l = match l {
            Ok(l) => l,
            Err(e) => {
                first_fail.get_or_insert(e);
                continue;
            }
        };
        loaded_n += 1;
        let r = validate_clearance(&l.table, &l.scene);
        ok += usize::from(r.is_ok());
        warned += usize::from(!r.warnings.is_empty());
        let kinds: BTreeSet<&'static str> = r.errors.iter().map(|e| e.feature.label()).collect();
        for k in kinds {
            *by_feature.entry(k).or_default() += 1;
        }
    }
    println!(
        "random cases: 300 drawn, {built} generated, {loaded_n} loaded, {ok} accepted, {warned} with warnings; rejected by feature: {by_feature:?}; first refusal: {first_fail:?}"
    );
    assert!(
        loaded_n >= 150,
        "only {loaded_n}/300 random cases load: {first_fail:?}"
    );
    assert!(ok >= 5 && ok < loaded_n, "accepted {ok} of {loaded_n}");
    for f in [
        "lane",
        "flipper gap",
        "wireform",
        "scoop",
        "vuk exit",
        "deck headroom",
        "restriction",
    ] {
        assert!(
            by_feature.get(f).copied().unwrap_or(0) > 0,
            "no random case violates {f:?}: {by_feature:?}"
        );
    }
}
