//! Work item w7pre-backbox-colliders (DECISIONS.md "w7pre-backbox-colliders
//! — backbox colliders in the runtime world"): a bundle's
//! `backbox_colliders.glb` (written by `pin_tablegen::backbox`) is loaded
//! into the simulation's static world after the playfield colliders and the
//! glass, on the backbox layer; a bundle without it loads exactly as before.
//!
//! Requirement 2 (no change without the file): the grey-box and the replay
//! fixture have exactly the playfield glb's collider count plus the glass.
//! Requirement 3 (the generated backbox works headless): the colliders are
//! in the world on layer 4 with the generator's materials and order, a
//! layer-4 ball is contained by the box and comes to rest on a panel or bat,
//! a layer-0 ball falls through it, and the generated VUK ejects a ball
//! into the box where it reaches a backbox flipper.

mod sim_support;

use glam::Vec3;
use pin_assets::{BACKBOX_COLLIDERS_FILE, Vfs, load_gltf};
use pin_core::fmath::min_f32;
use pin_core::schema::table::{FlipperShapeDef, MechKind, TableManifest};
use pin_physics::world::layers;
use pin_sim::load::{BOUNDS_MARGIN_M, GLASS_THICKNESS_M, backbox_colliders};
use pin_sim::{SimConfig, Simulation};
use pin_tablegen::backbox::{
    BACKBOX_PREFIX, BackboxFrame, GeneratedBackbox, generate_backbox, parse_backbox_layout,
    write_backbox,
};
use sim_support::*;
use std::path::{Path, PathBuf};

const R: f32 = 0.0135;

/// A backbox over the replay fixture's playfield (514 x 1070 mm): two
/// flippers, a VUK at a free spot of the main deck feeding the entry hole,
/// and a drain.
const LAYOUT: &str = r#"
schema_version = 1

[backbox]
origin = [257.0, 1150.0, 250.0]
width = 400.0
height = 300.0
depth = 35.0

[[backbox.flippers]]
side = "left"
position = [-70.0, 50.0]
coil = "bb_left_coil"
eos_switch = "bb_left_eos"
staging = { type = "DedicatedButton", input = "left_upper" }

[[backbox.flippers]]
side = "right"
position = [70.0, 50.0]
coil = "bb_right_coil"
eos_switch = "bb_right_eos"
staging = { type = "DedicatedButton", input = "right_upper" }

[backbox.entry]
vuk = [430.0, 980.0]
at = -150.0
coil = "bb_vuk_coil"
switch = "bb_vuk_switch"

[backbox.drain]
at = 160.0
"#;

/// The coils and switches [`LAYOUT`] names.
const EXTRA: &str = r#"
[[coils]]
id = "bb_left_coil"
number = 90
kind = "flipper"

[[coils]]
id = "bb_right_coil"
number = 91
kind = "flipper"

[[coils]]
id = "bb_vuk_coil"
number = 92
kind = "vuk"

[[switches]]
id = "bb_left_eos"
number = 180
kind = "eos"
layer = 0

[[switches]]
id = "bb_right_eos"
number = 181
kind = "eos"
layer = 0

[[switches]]
id = "bb_vuk_switch"
number = 182
kind = "opto"
layer = 0
"#;

/// No ball at init; the VUK ejects whatever it captures.
const RULES: &str = r#"
function on_init()
end

function on_switch(id, closed)
  if closed and id == pinball.sw.bb_vuk_switch then
    pinball.pulse_coil(pinball.coil.bb_vuk_coil, 20)
  end
end

function on_tick()
end
"#;

/// The fixture's `table.toml` with [`EXTRA`] appended.
fn base() -> TableManifest {
    let text = fixture_table_toml() + EXTRA;
    TableManifest::from_toml_str(&text).expect("base table.toml")
}

/// The replay fixture plus the generated backbox ([`LAYOUT`]) and
/// [`RULES`]; `edit` may change the generated manifest before it is
/// written (`table.toml`).
fn backbox_bundle(
    tag: &str,
    edit: impl FnOnce(&mut TableManifest),
) -> (ScratchDir, GeneratedBackbox) {
    let dir = with_rules(tag, RULES);
    let layout = parse_backbox_layout(LAYOUT).unwrap_or_else(|e| panic!("{e}"));
    let g = generate_backbox(&layout.backbox, &base()).unwrap_or_else(|e| panic!("{e}"));
    write_backbox(&g, &dir).expect("write backbox");
    let mut table = g.table.clone();
    edit(&mut table);
    if table != g.table {
        let text = table.to_toml_string().expect("table.toml");
        std::fs::write(dir.join("table.toml"), text).expect("write table.toml");
    }
    (dir, g)
}

/// Collider nodes of the bundle's playfield glb (what the loader adds
/// before the glass).
fn playfield_collider_count(dir: &Path) -> usize {
    let vfs = Vfs::open(dir).expect("vfs");
    let table = pin_assets::load_table(&vfs).unwrap_or_else(|e| panic!("{e}"));
    load_gltf(&vfs, &table.manifest.meta.playfield_mesh)
        .expect("playfield glb")
        .meshes
        .iter()
        .filter(|m| m.extras.collider)
        .count()
}

fn load(dir: &Path) -> Simulation {
    let path = dir.to_path_buf();
    on_sim_thread(move || Simulation::load(&path, SimConfig::default()).expect("load"))
}

/// Ball centre in the backbox frame, mm.
fn local(frame: &BackboxFrame, p: [f32; 3]) -> Vec3 {
    frame.rotation.inverse() * (Vec3::from(p) - frame.origin_m) * 1.0e3
}

fn to_table(frame: &BackboxFrame, local_mm: [f32; 3]) -> [f32; 3] {
    frame.to_table(Vec3::from(local_mm) * 1.0e-3).to_array()
}

// --- Requirement 2: no file, no change. ---------------------------------

fn parse_hex(c: &str) -> [u8; 3] {
    let h = c.trim_start_matches('#');
    let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).expect("palette colour");
    [b(0), b(2), b(4)]
}

/// The grey-box bundle: `GREYBOX_BUNDLE` when set (a `tables/greybox/
/// build.sh` output), else built here by the same steps (pin_tablegen's
/// playfield and table.toml, the copied rules, backglass, layout, glass and
/// display files, and pin_assetgen's placeholder kit).
fn greybox_bundle() -> (Option<ScratchDir>, PathBuf) {
    if let Some(p) = std::env::var_os("GREYBOX_BUNDLE") {
        return (None, PathBuf::from(p));
    }
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tables/greybox");
    let dir = scratch_dir("backbox_greybox");
    let layout_text = std::fs::read_to_string(src.join("layout.toml")).unwrap();
    let table_text = std::fs::read_to_string(src.join("table.toml")).unwrap();
    let layout = pin_tablegen::layout::parse_layout(&layout_text).unwrap();
    let base = TableManifest::from_toml_str(&table_text).unwrap();
    let generated = pin_tablegen::generate(&layout, &base).unwrap_or_else(|e| panic!("{e}"));
    pin_tablegen::write_bundle(&generated, &dir).unwrap();
    std::fs::create_dir_all(dir.join("display")).unwrap();
    let mut copied = vec![
        "rules.lua".to_owned(),
        "backglass.toml".to_owned(),
        "layout.toml".to_owned(),
        "glass.toml".to_owned(),
    ];
    let mut scenes: Vec<String> = std::fs::read_dir(src.join("display"))
        .unwrap()
        .map(|e| format!("display/{}", e.unwrap().file_name().to_string_lossy()))
        .filter(|f| f.ends_with(".toml"))
        .collect();
    scenes.sort();
    copied.extend(scenes);
    for f in &copied {
        std::fs::copy(src.join(f), dir.join(f)).unwrap_or_else(|e| panic!("copy {f}: {e}"));
    }
    let meta = pin_assetgen::TableMeta {
        title: base.meta.name.clone(),
        palette: base.meta.palette.iter().map(|c| parse_hex(c)).collect(),
        seed: 0,
    };
    pin_assetgen::generate_kit(&meta, &dir).expect("placeholder kit");
    let path = dir.to_path_buf();
    (Some(dir), path)
}

#[test]
fn greybox_world_is_the_playfield_colliders_plus_the_glass() {
    let (_guard, dir) = greybox_bundle();
    let vfs = Vfs::open(&dir).unwrap();
    assert!(!vfs.exists(BACKBOX_COLLIDERS_FILE), "the grey-box has none");
    assert!(backbox_colliders(&vfs).unwrap().is_empty());
    let want = playfield_collider_count(&dir) + 1;
    assert!(want > 1, "the grey-box playfield has colliders");
    let sim = load(&dir);
    assert_eq!(sim.static_world().collider_count(), want);
}

#[test]
fn replay_fixture_world_is_the_playfield_colliders_plus_the_glass() {
    let dir = fixture();
    let want = playfield_collider_count(&dir) + 1;
    let sim = load(&dir);
    assert_eq!(sim.static_world().collider_count(), want);
    // The glass is on every layer (the last collider): a sphere touching
    // its underside meets it on each layer.
    let (lo, hi) = sim.escape_bounds();
    let under = hi[2] - GLASS_THICKNESS_M - BOUNDS_MARGIN_M;
    let c = Vec3::new((lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, under - 0.004);
    for layer in 0..8u8 {
        let mut out = arrayvec::ArrayVec::new();
        sim.static_world()
            .sphere_contacts(c, 0.005, layers::layer_bit(layer), &mut out);
        assert!(
            out.iter().any(|k| k.collider.index() == want - 1),
            "layer {layer}: the glass"
        );
    }
}

// --- Requirement 3: the generated backbox in the world. -----------------

#[test]
fn the_file_name_is_the_generators() {
    assert_eq!(
        BACKBOX_COLLIDERS_FILE,
        pin_tablegen::backbox::BACKBOX_COLLIDERS_FILE
    );
}

#[test]
fn backbox_colliders_append_after_the_playfield_and_the_glass() {
    let (dir, g) = backbox_bundle("bb_world", |_| {});
    let vfs = Vfs::open(&dir).unwrap();
    let loaded = backbox_colliders(&vfs).expect("backbox colliders");
    // Same panels, same order, layer and material as the generator's.
    assert_eq!(loaded.len(), g.colliders.len());
    for (l, c) in loaded.iter().zip(&g.colliders) {
        assert!(
            l.name.ends_with(&c.name),
            "glb node order: {} vs {}",
            l.name,
            c.name
        );
        assert_eq!(l.layer_mask, layers::BACKBOX, "{}", l.name);
        assert_eq!(l.layer_mask, c.layer_mask, "{}", l.name);
        assert_eq!(l.material, c.material, "{}", l.name);
    }
    let playfield = playfield_collider_count(&dir);
    let sim = load(&dir);
    let world = sim.static_world();
    assert_eq!(world.collider_count(), playfield + 1 + loaded.len());

    // Every backbox panel is in the world at its index (after the glass),
    // on layer 4 only: a small sphere at a vertex of each panel touches it
    // on layer 4 and on no other layer.
    for (i, c) in loaded.iter().enumerate() {
        let pin_physics::world::StaticShape::TriMesh { vertices, .. } = &c.shape else {
            panic!("trimesh");
        };
        let index = playfield + 1 + i;
        for layer in 0..8u8 {
            let mut out = arrayvec::ArrayVec::new();
            world.sphere_contacts(vertices[0], 0.002, layers::layer_bit(layer), &mut out);
            let hit = out.iter().any(|k| k.collider.index() == index);
            assert_eq!(hit, layer == 4, "{} on layer {layer}", c.name);
        }
    }

    // The playfield glass leaves the backbox layer out; the escape box
    // holds every backbox vertex.
    let (lo, hi) = sim.escape_bounds();
    for c in &loaded {
        let pin_physics::world::StaticShape::TriMesh { vertices, .. } = &c.shape else {
            panic!("trimesh");
        };
        for v in vertices {
            for k in 0..3 {
                assert!(v[k] > lo[k] && v[k] < hi[k], "{} {v:?} escapes", c.name);
            }
        }
    }
}

#[test]
fn the_playfield_glass_stays_where_it_was_and_skips_the_backbox_layer() {
    // The backbox flippers and panels grow the escape box, not the
    // playfield bounds: the glass sits where it does on the same playfield
    // without a backbox (the fixture), and it is not on the backbox layer.
    let (dir, _g) = backbox_bundle("bb_glass", |_| {});
    let glass = playfield_collider_count(&dir);
    let sim = load(&dir);
    let reference = load(&fixture());
    let (rlo, rhi) = reference.escape_bounds();
    let under = rhi[2] - GLASS_THICKNESS_M - BOUNDS_MARGIN_M;
    let world = sim.static_world();
    let touches = |c: Vec3, mask: u8| {
        let mut out = arrayvec::ArrayVec::new();
        world.sphere_contacts(c, 0.005, mask, &mut out);
        out.iter().any(|k| k.collider.index() == glass)
    };
    let mid = Vec3::new((rlo[0] + rhi[0]) * 0.5, (rlo[1] + rhi[1]) * 0.5, 0.0);
    // Touching the reference underside from below, and 2 mm under it.
    assert!(touches(mid + Vec3::Z * (under - 0.004), layers::MAIN_DECK));
    assert!(!touches(mid + Vec3::Z * (under - 0.007), layers::MAIN_DECK));
    for layer in [1u8, 2, 3, 5, 6, 7] {
        let mask = layers::layer_bit(layer);
        assert!(
            touches(mid + Vec3::Z * (under - 0.004), mask),
            "layer {layer}"
        );
    }
    assert!(!touches(mid + Vec3::Z * (under - 0.004), layers::BACKBOX));
    // The escape box reaches above the backbox roof (origin z 250 mm +
    // 300 mm high).
    let (_, hi) = sim.escape_bounds();
    assert!(hi[2] > 0.55, "escape top {}", hi[2]);
}

/// Steps until `ticks`, returning the local positions of ball `h`.
fn track(
    sim: &mut Simulation,
    h: pin_sim::BallHandle,
    frame: &BackboxFrame,
    ticks: u64,
) -> Vec<Option<(Vec3, u8)>> {
    (0..ticks)
        .map(|_| {
            sim.advance(1);
            sim.ball(h).map(|b| (local(frame, b.position_m), b.layer))
        })
        .collect()
}

#[test]
fn a_layer_4_ball_is_contained_and_comes_to_rest_and_a_layer_0_ball_falls_through() {
    let (dir, g) = backbox_bundle("bb_contain", |_| {});
    let path = dir.to_path_buf();
    let frame = g.frame;
    on_sim_thread(move || {
        // Drop points across the box (u, v in mm; mid-depth), away from the
        // entry (u = −150) and drain (u = 160) holes.
        for &(u, v) in &[
            (-60.0f32, 150.0f32),
            (0.0, 120.0),
            (60.0, 200.0),
            (-100.0, 260.0),
            (100.0, 90.0),
        ] {
            let mut sim = Simulation::load(&path, SimConfig::default()).expect("load");
            let at = to_table(&frame, [u, 17.5, v]);
            let h = sim
                .spawn_ball_on_layer("standard_steel", 4, at, [0.0; 3])
                .expect("spawn");
            let path4 = track(&mut sim, h, &frame, 1500);
            let mut lowest = f32::INFINITY;
            for (t, s) in path4.iter().enumerate() {
                let (p, layer) = s.unwrap_or_else(|| panic!("({u},{v}) tick {t}: ball removed"));
                assert_eq!(layer, 4, "({u},{v}) tick {t}: stays on the backbox layer");
                assert!(
                    p.y > -0.5 && p.y < 35.5,
                    "({u},{v}) tick {t}: between the glass and the back: {p:?}"
                );
                assert!(
                    p.x > -200.5 && p.x < 200.5 && p.z > -0.5 && p.z < 300.5,
                    "({u},{v}) tick {t}: inside the box: {p:?}"
                );
                lowest = min_f32(lowest, p.z);
            }
            // It fell with gravity along the plane (down the backbox) and
            // came to rest on the shelf or a bat (speed ~0 at the end).
            let end = sim.ball(h).unwrap();
            let speed = Vec3::from(end.velocity_m_s).length();
            let p = local(&frame, end.position_m);
            assert!(p.z < v - 20.0, "({u},{v}): fell down the plane: {p:?}");
            assert!(speed < 0.02, "({u},{v}): at rest, speed {speed} at {p:?}");
            assert!(
                p.z >= R * 1.0e3 - 0.5,
                "({u},{v}): on or above the shelf: {p:?}"
            );

            // The same drop on layer 0 passes every backbox panel.
            let mut sim = Simulation::load(&path, SimConfig::default()).expect("load");
            let h = sim
                .spawn_ball_on_layer("standard_steel", 0, at, [0.0; 3])
                .expect("spawn");
            let path0 = track(&mut sim, h, &frame, 400);
            let below = path0.iter().flatten().any(|(p, _)| p.z < -2.0 * R * 1.0e3);
            let removed = path0.iter().any(Option::is_none);
            assert!(
                below || removed,
                "({u},{v}): layer 0 falls through the shelf: {:?}",
                path0.last()
            );
            // Unaffected by the walls: it never moves across or in depth
            // before it leaves the box.
            for (p, _) in path0.iter().flatten().take_while(|(p, _)| p.z > 0.0) {
                assert!((p.x - u).abs() < 0.5, "({u},{v}): no sideways push: {p:?}");
            }
        }
    });
}

/// Distance in the backbox plane from local point `p` (mm) to the bat
/// tapered capsule of flipper `def` at rest (its surface), mm.
fn gap_to_bat(frame: &BackboxFrame, def: &pin_core::schema::table::FlipperDef, p: Vec3) -> f32 {
    let FlipperShapeDef::StraightCapsule {
        length,
        base_radius,
        tip_radius,
    } = def.shape
    else {
        panic!("generated bats are capsules");
    };
    let pivot = local(frame, (Vec3::from(def.base_position) * 1.0e-3).to_array());
    let a = def.rest_angle_deg.to_radians();
    let dir = Vec3::new(libm::cosf(a), 0.0, libm::sinf(a));
    let d = Vec3::new(p.x - pivot.x, 0.0, p.z - pivot.z);
    let s = d.dot(dir).clamp(0.0, length);
    let radius = base_radius + (tip_radius - base_radius) * s / length;
    (d - dir * s).length() - radius
}

#[test]
fn the_vuk_ejects_into_the_box_and_the_ball_reaches_a_flipper() {
    // The generated VUK, aimed across the backbox (+x, towards the left
    // flipper) with a kick swept over a range: every eject reaches the bat.
    for kick in [0.45f32, 0.5, 0.55, 0.6] {
        let (dir, g) = backbox_bundle(&format!("bb_vuk_{}", (kick * 100.0) as u32), |t| {
            let vuk = t
                .mechs
                .iter_mut()
                .find(|m| m.id == format!("{BACKBOX_PREFIX}vuk"))
                .expect("generated VUK");
            assert_eq!(vuk.kind, MechKind::Vuk);
            vuk.angle_deg = Some(0.0);
            vuk.kick_speed_m_s = Some(kick);
        });
        let left = g
            .flippers
            .iter()
            .find(|f| f.id == format!("{BACKBOX_PREFIX}flippers[0]"))
            .expect("left backbox flipper")
            .clone();
        let vuk = g.mechs[0].position.expect("VUK position");
        let frame = g.frame;
        let path = dir.to_path_buf();
        on_sim_thread(move || {
            let mut sim = Simulation::load(&path, SimConfig::default()).expect("load");
            let hold = [vuk[0] * 1.0e-3, vuk[1] * 1.0e-3, R];
            let h = sim
                .spawn_ball("standard_steel", hold, [0.0; 3])
                .expect("spawn");
            let mut ejected = None;
            let mut best = f32::INFINITY;
            for t in 0..1500u32 {
                sim.advance(1);
                let b = sim
                    .ball(h)
                    .unwrap_or_else(|| panic!("kick {kick} tick {t}: removed"));
                let p = local(&frame, b.position_m);
                if b.layer == 4 {
                    ejected.get_or_insert(t);
                    assert!(
                        p.y > -0.5 && p.y < 35.5 && p.x > -200.5 && p.x < 200.5,
                        "kick {kick} tick {t}: inside the box: {p:?}"
                    );
                    best = min_f32(best, gap_to_bat(&frame, &left, p));
                }
            }
            assert!(
                sim.switch_hits("bb_vuk_switch").unwrap() >= 1,
                "kick {kick}: the VUK captured the ball"
            );
            let t = ejected.unwrap_or_else(|| panic!("kick {kick}: never ejected to layer 4"));
            println!("kick {kick}: ejected at tick {t}, closest to the bat {best:.2} mm");
            assert!(
                best <= R * 1.0e3 + 0.5,
                "kick {kick}: the ball reaches the left bat (gap {best} mm)"
            );
        });
        drop(dir);
    }
}

// --- CodeRabbit r4606779403: a backbox layer is the backbox's alone. ---

fn load_err(dir: &Path) -> String {
    let path = dir.to_path_buf();
    on_sim_thread(
        move || match Simulation::load(&path, SimConfig::default()) {
            Ok(_) => panic!("load unexpectedly succeeded"),
            Err(e) => e.to_string(),
        },
    )
}

/// Rewrites the playfield glb's JSON chunk with `edit`.
fn edit_playfield_json(dir: &Path, edit: impl FnOnce(&mut serde_json::Value)) {
    let path = dir.join("playfield.glb");
    let d = std::fs::read(&path).expect("playfield.glb");
    let n = u32::from_le_bytes(d[12..16].try_into().unwrap()) as usize;
    let mut json: serde_json::Value = serde_json::from_slice(&d[20..20 + n]).expect("json");
    edit(&mut json);
    let mut text = serde_json::to_vec(&json).unwrap();
    while !text.len().is_multiple_of(4) {
        text.push(b' ');
    }
    let rest = &d[20 + n..];
    let mut out = Vec::new();
    out.extend_from_slice(&d[0..8]);
    out.extend_from_slice(&((12 + 8 + text.len() + rest.len()) as u32).to_le_bytes());
    out.extend_from_slice(&(text.len() as u32).to_le_bytes());
    out.extend_from_slice(&d[16..20]);
    out.extend_from_slice(&text);
    out.extend_from_slice(rest);
    std::fs::write(path, out).expect("write playfield.glb");
}

#[test]
fn the_unmodified_generated_backbox_still_loads_with_its_flippers_and_vuk() {
    let (dir, g) = backbox_bundle("bb_overlap_ok", |_| {});
    assert!(g.table.flippers.iter().any(|f| f.collision_layer == 4));
    assert!(g.table.mechs.iter().any(|m| m.kind == MechKind::Vuk));
    let _ = load(&dir);
}

#[test]
fn a_playfield_collider_on_the_backbox_layer_is_a_load_error() {
    let (dir, _g) = backbox_bundle("bb_overlap_a", |_| {});
    edit_playfield_json(&dir, |j| {
        let node = j["nodes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|n| n["name"] == "walls[0]")
            .expect("walls[0]");
        node["extras"]["collision_layer"] = 4.into();
    });
    let m = load_err(&dir);
    assert!(m.contains("backbox layer 4"), "{m}");
    assert!(m.contains("playfield collider \"walls[0]\""), "{m}");
}

#[test]
fn a_guide_or_wireform_on_the_backbox_layer_is_a_load_error() {
    use pin_core::schema::table::{SplineDef, SplineKind};
    for (kind, what) in [
        (SplineKind::Guide, "guide"),
        (SplineKind::Wireform, "wireform"),
    ] {
        let (dir, _g) = backbox_bundle(&format!("bb_overlap_b_{what}"), |t| {
            t.splines.push(SplineDef {
                id: "bad_spline".to_owned(),
                kind,
                layer: 4,
                gauge: Some(30.0),
                points: vec![
                    [100.0, 200.0, 5.0],
                    [150.0, 300.0, 5.0],
                    [200.0, 400.0, 5.0],
                    [250.0, 500.0, 5.0],
                ],
                roll_deg: None,
            });
        });
        let m = load_err(&dir);
        assert!(m.contains("backbox layer 4"), "{what}: {m}");
        assert!(m.contains(&format!("{what} \"bad_spline\"")), "{what}: {m}");
    }
}

#[test]
fn a_flipper_on_the_backbox_layer_far_from_the_backbox_is_a_load_error() {
    let (dir, _g) = backbox_bundle("bb_overlap_c", |t| {
        let mut f = t.flippers[0].clone();
        assert_ne!(f.collision_layer, 4);
        f.id = "pf_flip".to_owned();
        f.collision_layer = 4;
        f.base_position = [100.0, 100.0, 0.0];
        t.flippers.push(f);
    });
    let m = load_err(&dir);
    assert!(m.contains("backbox layer 4"), "{m}");
    assert!(
        m.contains("flipper \"pf_flip\" whose base is outside the backbox geometry"),
        "{m}"
    );
}
