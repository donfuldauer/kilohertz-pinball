//! Subphase 6.1 spec tests (Independent Tester), S2: physics-contact switch
//! events (brief item 5).
//!
//! No switch is forced in this file. For every switch-bearing element a
//! ball is injected (`spawn_ball_on_layer`) on a trajectory through the
//! element, the simulation steps simulated ticks up to a bound, and the test
//! requires the element's switch hit count (`switch_hits`) to rise, with no
//! ball escaped or non-finite and no script fault. Several approach
//! trajectories are tried per element (a parameter sweep); the first one
//! whose physics closes the switch is the evidence. The scoop must eject
//! the ball back into play; the Zipper must close the lower flippers' tip
//! gap by twice the declared slide so that a 27 mm ball no longer passes.

#[path = "fixtures/spec_6_1/common.rs"]
mod common;

use common::*;
use pin_core::schema::table::{FlipperShapeDef, MechKind, SwitchKind};
use pin_sim::{SimConfig, Simulation};
use std::path::PathBuf;
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Geometry helpers
// ---------------------------------------------------------------------------

fn dir(angle_deg: f32) -> [f32; 3] {
    let a = angle_deg.to_radians();
    [a.cos(), a.sin(), 0.0]
}

fn add(p: [f32; 3], u: [f32; 3], d: f32) -> [f32; 3] {
    [p[0] + u[0] * d, p[1] + u[1] * d, p[2] + u[2] * d]
}

fn scale(u: [f32; 3], k: f32) -> [f32; 3] {
    [u[0] * k, u[1] * k, u[2] * k]
}

fn dist2d(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// One injected trajectory: start (ball centre, m) and velocity (m/s).
#[derive(Copy, Clone, Debug)]
struct Approach {
    start: [f32; 3],
    vel: [f32; 3],
}

/// Where a switch is: its declared position, else its mech's position,
/// else the centroid of the glTF node bound to it (extras `switch`).
fn switch_site(name: &str) -> Option<([f32; 3], u8)> {
    let t = table();
    let s = t.switches.iter().find(|s| s.name == name)?;
    if let Some(p) = s.position_m {
        return Some((p, s.layer));
    }
    if let Some(m) = t
        .manifest
        .mechs
        .iter()
        .find(|m| m.switch.as_deref() == Some(name))
    {
        let si = t.mechs.iter().find(|x| x.name == m.id)?;
        return si.position_m.map(|p| (p, m.layer.unwrap_or(s.layer)));
    }
    let mesh = validated()
        .scene
        .meshes
        .iter()
        .find(|m| m.extras.switch.as_deref() == Some(name))?;
    let n = mesh.positions.len().max(1) as f32;
    let mut c = [0.0f32; 3];
    for p in &mesh.positions {
        for i in 0..3 {
            c[i] += p[i] / n;
        }
    }
    let zmin = mesh.positions.iter().map(|p| p[2]).fold(f32::MAX, f32::min);
    c[2] = zmin;
    Some((c, mesh.extras.collision_layer.unwrap_or(s.layer)))
}

/// Trajectories through a mech of `kind` at `p` (surface point) facing
/// `angle_deg`.
fn mech_approaches(kind: MechKind, p: [f32; 3], angle_deg: f32) -> Vec<Approach> {
    let r = radius_of(STEEL);
    let c = [p[0], p[1], p[2] + r];
    let u = dir(angle_deg);
    let mut v = Vec::new();
    let mut through = |d: [f32; 3], dist: f32, speed: f32| {
        v.push(Approach {
            start: add(c, d, -dist),
            vel: scale(d, speed),
        });
    };
    match kind {
        MechKind::Sling | MechKind::DropTarget => {
            for dist in [0.04, 0.06] {
                for speed in [1.0, 1.5] {
                    through(scale(u, -1.0), dist, speed);
                }
            }
        }
        MechKind::Bumper => {
            for a in [90.0, 270.0, 0.0, 180.0, 45.0, 135.0, 225.0, 315.0] {
                through(dir(a), 0.065, 1.0);
            }
        }
        MechKind::Spinner => {
            for a in [angle_deg, angle_deg + 180.0, 90.0, 270.0] {
                for speed in [1.5, 3.0] {
                    through(dir(a), 0.06, speed);
                }
            }
        }
        MechKind::Vuk => {
            for a in [270.0, 90.0, 0.0, 180.0] {
                through(dir(a), 0.04, 0.3);
            }
            v.push(Approach {
                start: c,
                vel: [0.0; 3],
            });
        }
        _ => {
            for a in [90.0, 270.0, 0.0, 180.0] {
                through(dir(a), 0.05, 1.0);
            }
        }
    }
    v
}

/// Trajectories over a ball sensor (rollover, opto, eddy, trough, ramp
/// switch) at `p`: rolling through it from four sides, then set down on it.
fn sensor_approaches(p: [f32; 3]) -> Vec<Approach> {
    let r = radius_of(STEEL);
    let c = [p[0], p[1], p[2] + r];
    let mut v = Vec::new();
    for a in [90.0f32, 270.0, 0.0, 180.0] {
        for (dist, speed) in [(0.05, 1.0), (0.03, 0.5)] {
            let d = dir(a);
            v.push(Approach {
                start: add(c, d, -dist),
                vel: scale(d, speed),
            });
        }
    }
    v.push(Approach {
        start: c,
        vel: [0.0; 3],
    });
    v
}

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------

/// A game in progress (cabinet `start`), table cleared.
fn game() -> Simulation {
    let mut sim = fresh();
    sim.advance(200);
    start_game(&mut sim);
    clear_table(&mut sim);
    sim.advance(50);
    sim
}

/// A successful physics hit: the injected ball.
#[derive(Debug)]
struct Hit {
    handle: pin_sim::BallHandle,
}

/// Tries `approaches` in order on `sim` (one ball each, table cleared)
/// until the physics closes `switch`; `max` ticks per try. Asserts health
/// after every try.
fn drive(
    sim: &mut Simulation,
    switch: &str,
    layer: u8,
    approaches: &[Approach],
    max: u64,
    profile: &str,
) -> Result<Hit, String> {
    let mut tried = Vec::new();
    let pf = &layout().playfield;
    let (w, l) = (pf.width * 1e-3, pf.length * 1e-3);
    let r = radius_of(profile);
    for a in approaches {
        // Starts outside the cabinet's playfield rectangle are not
        // trajectories through the table.
        let inside = (r..=w - r).contains(&a.start[0]) && (r..=l - r).contains(&a.start[1]);
        if !inside {
            tried.push(format!("{a:?}: starts outside the playfield"));
            continue;
        }
        clear_table(sim);
        sim.advance(20);
        let before = hits(sim, switch);
        let drained = sim.counters().balls_drained;
        let h = match sim.spawn_ball_on_layer(profile, layer, a.start, a.vel) {
            Ok(h) => h,
            Err(e) => {
                tried.push(format!("{a:?}: spawn {e}"));
                continue;
            }
        };
        for _ in 0..max {
            sim.advance(1);
            if hits(sim, switch) > before {
                assert_healthy(sim, switch);
                return Ok(Hit { handle: h });
            }
            if sim.ball(h).is_none() {
                break;
            }
        }
        assert_healthy(sim, switch);
        tried.push(format!(
            "{a:?}: no hit (ball {}, drained {})",
            if sim.ball(h).is_some() {
                "alive"
            } else {
                "gone"
            },
            sim.counters().balls_drained - drained
        ));
    }
    Err(format!(
        "{switch}: no physics-generated closing edge; tried:\n  {}",
        tried.join("\n  ")
    ))
}

/// Drives switch `name` by physics contact on `sim` and asserts the hit
/// count rose (and, unless it is a trough, that the ball was not lost).
fn expect_contact(sim: &mut Simulation, name: &str) -> Hit {
    let t = table();
    let (p, layer) = switch_site(name).unwrap_or_else(|| panic!("{name}: no position"));
    let approaches = match t
        .manifest
        .mechs
        .iter()
        .find(|m| m.switch.as_deref() == Some(name))
    {
        Some(m) => {
            let mp = t
                .mechs
                .iter()
                .find(|x| x.name == m.id)
                .and_then(|x| x.position_m)
                .unwrap_or(p);
            mech_approaches(m.kind, mp, m.angle_deg.unwrap_or(90.0))
        }
        None => sensor_approaches(p),
    };
    let before = hits(sim, name);
    let escaped = sim.counters().balls_escaped;
    let hit = drive(sim, name, layer, &approaches, 1_000, STEEL).unwrap_or_else(|e| panic!("{e}"));
    assert!(hits(sim, name) > before, "{name}: switch_hits increased");
    assert_eq!(sim.counters().balls_escaped, escaped, "{name}: ball lost");
    let kind = t.switches.iter().find(|s| s.name == name).unwrap().kind;
    if kind != SwitchKind::Trough {
        assert!(
            sim.ball(hit.handle).is_some(),
            "{name}: the ball is still in play at the hit"
        );
    }
    hit
}

fn mech_switches(kind: MechKind) -> Vec<String> {
    table()
        .manifest
        .mechs
        .iter()
        .filter(|m| m.kind == kind)
        .filter_map(|m| m.switch.clone())
        .collect()
}

fn contact_all(names: &[String], what: &str) {
    assert!(!names.is_empty(), "{what}: no switches");
    let names = names.to_vec();
    big_stack(move || {
        let mut sim = game();
        for n in &names {
            expect_contact(&mut sim, n);
        }
        clear_table(&mut sim);
        sim.advance(500);
        assert_healthy(&sim, "after the contact runs");
    });
}

// ---------------------------------------------------------------------------
// Per-mechanism contact tests
// ---------------------------------------------------------------------------

#[test]
fn physics_contact_bumpers() {
    let s = mech_switches(MechKind::Bumper);
    assert_eq!(s.len(), 3, "3 bumper switches");
    contact_all(&s, "bumpers");
}

#[test]
fn physics_contact_slings() {
    let s = mech_switches(MechKind::Sling);
    assert_eq!(s.len(), 2, "2 sling switches");
    contact_all(&s, "slings");
}

#[test]
fn physics_contact_spinner() {
    contact_all(&mech_switches(MechKind::Spinner), "spinner");
}

#[test]
fn physics_contact_drop_targets() {
    let s = mech_switches(MechKind::DropTarget);
    assert_eq!(s.len(), 3, "3 drop-target switches");
    contact_all(&s, "drop targets");
}

#[test]
fn physics_contact_magnet_eddy_and_opto() {
    let (e, o) = magnet_sensors();
    let eddy = vec![e.name.clone()];
    let opto = vec![o.name.clone()];
    let mut both = eddy.clone();
    both.extend(opto.clone());
    contact_all(&both, "magnet sensors");
    // Physically: the eddy sensor does not see the ceramic Powerball, the
    // opto does (DECISIONS D12; the Powerball Mania detector).
    let (e, o) = (eddy[0].clone(), opto[0].clone());
    big_stack(move || {
        let mut sim = game();
        let (pe, le) = switch_site(&e).unwrap();
        let eb = hits(&sim, &e);
        let r = drive(&mut sim, &e, le, &sensor_approaches(pe), 300, POWERBALL);
        assert!(r.is_err(), "the eddy sensor closed for the ceramic ball");
        assert_eq!(hits(&sim, &e), eb);
        let (po, lo) = switch_site(&o).unwrap();
        drive(&mut sim, &o, lo, &sensor_approaches(po), 1_000, POWERBALL)
            .unwrap_or_else(|err| panic!("opto with the Powerball: {err}"));
        assert_healthy(&sim, "powerball over the magnet sensors");
    });
}

#[test]
fn physics_contact_ramp_entry_and_made() {
    let shot = table()
        .manifest
        .shots
        .iter()
        .find(|s| s.id == "cross_ramp")
        .expect("cross_ramp shot");
    assert!(
        shot.sequence.len() >= 2,
        "cross_ramp has an entry and a made switch: {:?}",
        shot.sequence
    );
    contact_all(&shot.sequence, "cross_ramp");
}

#[test]
fn physics_contact_top_lanes() {
    let lanes = top_lanes();
    assert!(lanes.len() >= 3, "top lanes {lanes:?}");
    contact_all(&lanes, "top lanes");
}

#[test]
fn physics_contact_trough() {
    let names: Vec<String> = trough_switches()
        .iter()
        .filter(|s| switch_site(&s.name).is_some())
        .map(|s| s.name.clone())
        .collect();
    assert!(!names.is_empty(), "a positioned trough switch");
    big_stack(move || {
        let mut sim = game();
        for n in &names {
            let drained = sim.counters().balls_drained;
            expect_contact(&mut sim, n);
            sim.advance(10);
            assert!(
                sim.counters().balls_drained > drained,
                "{n}: the ball drained"
            );
        }
        assert_healthy(&sim, "trough");
    });
}

/// The scoop holds the ball and ejects it back into play.
#[test]
fn physics_contact_scoop_ejects_back_into_play() {
    big_stack(|| {
        let t = table();
        let (index, def) = t
            .manifest
            .mechs
            .iter()
            .enumerate()
            .find(|(_, m)| m.kind == MechKind::Vuk)
            .expect("vuk");
        let sw = def.switch.clone().expect("scoop switch");
        let p = t.mechs[index].position_m.expect("scoop position");
        let mut sim = game();
        let hit = expect_contact(&mut sim, &sw);
        let h = hit.handle;
        // Held, then ejected: the ball leaves the scoop (no longer held,
        // > 60 mm away or on another layer) and is still in play.
        let layer0 = def.layer.unwrap_or(0);
        let mut ejected = false;
        for _ in 0..8_000 {
            sim.advance(1);
            let Some(b) = sim.ball(h) else { break };
            let away = dist2d(b.position_m, p) > 0.060 || b.layer != layer0;
            if away && sim.mech_set().vuk_ball(index).is_none() {
                ejected = true;
                break;
            }
        }
        assert!(
            ejected,
            "the scoop ejected the ball back into play within 8 s (ball {:?})",
            sim.ball(h)
        );
        assert_healthy(&sim, "scoop");
    });
}

/// Every switch-bearing mech and every positioned non-EOS, non-cabinet
/// switch closes by physics contact on one game.
#[test]
fn every_playfield_switch_closes_by_physics_contact() {
    big_stack(|| {
        let t = table();
        let cab: Vec<u16> = pin_core::input::CabinetInput::ALL
            .iter()
            .map(|&i| t.manifest.cabinet_switches.number(i))
            .collect();
        let eos: Vec<&str> = t
            .manifest
            .flippers
            .iter()
            .map(|f| f.eos_switch.as_str())
            .collect();
        let names: Vec<String> = t
            .switches
            .iter()
            .filter(|s| s.kind != SwitchKind::Eos && !eos.contains(&s.name.as_str()))
            .filter(|s| !cab.contains(&s.id.0))
            .filter(|s| switch_site(&s.name).is_some())
            .map(|s| s.name.clone())
            .collect();
        let mech_named: Vec<String> = t
            .manifest
            .mechs
            .iter()
            .filter_map(|m| m.switch.clone())
            .collect();
        for m in &mech_named {
            assert!(names.contains(m), "mech switch {m} is located");
        }
        let mut sim = game();
        for n in &names {
            expect_contact(&mut sim, n);
        }
        assert_healthy(&sim, "all switches");
    });
}

// ---------------------------------------------------------------------------
// Zipper (PLAN 2.3 criterion on this table)
// ---------------------------------------------------------------------------

/// A scratch copy of the bundle whose rules hold both lower flippers'
/// zipper `engaged` from the first tick (fixture `zipper_rules.lua`).
fn zipper_bundle(engaged: bool) -> PathBuf {
    static ON: OnceLock<PathBuf> = OnceLock::new();
    static OFF: OnceLock<PathBuf> = OnceLock::new();
    let cell = if engaged { &ON } else { &OFF };
    cell.get_or_init(|| {
        let dir = scratch(if engaged { "zipper_on" } else { "zipper_off" });
        copy_tree(bundle(), &dir);
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/spec_6_1/zipper_rules.lua");
        let mut text = std::fs::read_to_string(src).expect("fixture rules");
        if !engaged {
            text = text.replace("local ZIPPER = true", "local ZIPPER = false");
        }
        std::fs::write(dir.join("rules.lua"), text).expect("write rules");
        dir
    })
    .clone()
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let target = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_tree(&e.path(), &target);
        } else {
            std::fs::copy(e.path(), &target).unwrap();
        }
    }
}

/// (base radius, tip radius) of a lower flipper, m.
fn radii(id: &str) -> (f32, f32) {
    let f = table()
        .manifest
        .flippers
        .iter()
        .find(|f| f.id == id)
        .unwrap();
    match f.shape {
        FlipperShapeDef::StraightCapsule {
            base_radius,
            tip_radius,
            ..
        } => (base_radius * 1e-3, tip_radius * 1e-3),
        FlipperShapeDef::CurvedBanana { tube_radius, .. } => {
            (tube_radius * 1e-3, tube_radius * 1e-3)
        }
    }
}

fn length_m(id: &str) -> f32 {
    let f = table()
        .manifest
        .flippers
        .iter()
        .find(|f| f.id == id)
        .unwrap();
    match f.shape {
        FlipperShapeDef::StraightCapsule { length, .. }
        | FlipperShapeDef::CurvedBanana { length, .. } => length * 1e-3,
    }
}

/// Bat segment (pivot, tip) of flipper `id` now, m.
fn bat(sim: &Simulation, id: &str) -> ([f32; 3], [f32; 3]) {
    let fid = sim.ids().flipper(id).expect("flipper id");
    let p = sim.flipper_set().pivot_m(fid).expect("pivot");
    let d = sim.flipper_set().bat_direction(fid).expect("direction");
    let l = length_m(id);
    (
        [p.x, p.y, p.z],
        [p.x + d.x * l, p.y + d.y * l, p.z + d.z * l],
    )
}

/// Smallest distance between the two lower bats' surfaces (tapered
/// capsules, sampled at 0.25 mm), m.
fn tip_gap(sim: &Simulation) -> f32 {
    let (la, lb) = bat(sim, "left_flipper");
    let (ra, rb) = bat(sim, "right_flipper");
    let (lr0, lr1) = radii("left_flipper");
    let (rr0, rr1) = radii("right_flipper");
    let n = 320;
    let mut best = f32::MAX;
    for i in 0..=n {
        let s = i as f32 / n as f32;
        let p = add(la, [lb[0] - la[0], lb[1] - la[1], lb[2] - la[2]], s);
        let pr = lr0 + (lr1 - lr0) * s;
        for j in 0..=n {
            let t = j as f32 / n as f32;
            let q = add(ra, [rb[0] - ra[0], rb[1] - ra[1], rb[2] - ra[2]], t);
            let qr = rr0 + (rr1 - rr0) * t;
            best = best.min(dist2d(p, q) - pr - qr);
        }
    }
    best
}

fn declared_slide_m(id: &str) -> f32 {
    let f = table()
        .manifest
        .flippers
        .iter()
        .find(|f| f.id == id)
        .unwrap();
    let a = f.zipper_slide_axis.expect("zipper axis");
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt() * 1e-3
}

/// Drops a 27 mm steel ball down the middle of the lower flippers' gap;
/// returns whether it reached the trough (drained) within 4 s.
fn drop_through_gap(sim: &mut Simulation) -> bool {
    let (_, lt) = bat(sim, "left_flipper");
    let (_, rt) = bat(sim, "right_flipper");
    let mid = [(lt[0] + rt[0]) / 2.0, (lt[1] + rt[1]) / 2.0, lt[2]];
    let r = radius_of(STEEL);
    let drained = sim.counters().balls_drained;
    let h = sim
        .spawn_ball(STEEL, [mid[0], mid[1] + 0.06, r], [0.0, -0.5, 0.0])
        .expect("spawn above the gap");
    for _ in 0..4_000 {
        sim.advance(1);
        if sim.ball(h).is_none() {
            break;
        }
    }
    assert_eq!(sim.counters().balls_escaped, 0, "ball escaped");
    sim.counters().balls_drained > drained
}

/// Open and engaged simulations (fixture rules), settled for 1 s.
fn zipper_sims() -> (Simulation, Simulation) {
    let mut open =
        Simulation::load(&zipper_bundle(false), SimConfig::default()).expect("load open");
    open.advance(1_000);
    let mut shut =
        Simulation::load(&zipper_bundle(true), SimConfig::default()).expect("load engaged");
    shut.advance(1_000);
    (open, shut)
}

/// The PLAN 2.3 Zipper numbers on this table: 48 mm open tip gap, 12 mm
/// inward slide each (so 24 mm closed).
#[test]
fn zipper_declared_numbers_are_48mm_gap_and_12mm_slide() {
    big_stack(|| {
        let (open, _) = zipper_sims();
        let g_open = tip_gap(&open);
        let slides = [
            declared_slide_m("left_flipper"),
            declared_slide_m("right_flipper"),
        ];
        assert!(
            (g_open - 0.048).abs() < 0.001 && slides.iter().all(|s| (s - 0.012).abs() < 1e-6),
            "open tip gap {:.2} mm (brief: 48 mm), slides {:.1} / {:.1} mm (brief: 12 mm)",
            g_open * 1e3,
            slides[0] * 1e3,
            slides[1] * 1e3
        );
    });
}

/// Engaging both lower zippers moves each by its declared slide, closes
/// the tip gap by twice the slide, and a 27 mm ball dropped down the gap
/// no longer reaches the trough (it does with the zipper open).
#[test]
fn zipper_closes_the_gap_by_twice_the_slide_and_holds_a_27mm_ball() {
    big_stack(|| {
        let slide = declared_slide_m("left_flipper");
        assert!(
            (slide - declared_slide_m("right_flipper")).abs() < 1e-6,
            "equal slides"
        );
        let (mut open, mut shut) = zipper_sims();
        let g_open = tip_gap(&open);
        for (i, f) in shut.snapshot().flippers().iter().enumerate() {
            let name = &table().manifest.flippers[i].id;
            if name == "left_flipper" || name == "right_flipper" {
                assert!(
                    (f.zipper_offset_m - slide).abs() < 1e-4,
                    "{name}: zipper offset {} m, declared slide {slide} m",
                    f.zipper_offset_m
                );
            }
        }
        for f in open.snapshot().flippers() {
            assert_eq!(f.zipper_offset_m, 0.0, "open: zipper home");
        }
        let g_shut = tip_gap(&shut);
        println!(
            "tip gap open {:.2} mm, engaged {:.2} mm, slide {:.2} mm",
            g_open * 1e3,
            g_shut * 1e3,
            slide * 1e3
        );
        assert!(
            ((g_open - g_shut) - 2.0 * slide).abs() < 0.0005,
            "gap {:.2} -> {:.2} mm, expected a fall of {:.2} mm",
            g_open * 1e3,
            g_shut * 1e3,
            2.0 * slide * 1e3
        );
        assert!(g_shut < BALL_DIAMETER_M, "closed gap {g_shut} m < 27 mm");
        assert!(
            drop_through_gap(&mut open),
            "control: the ball passes the open gap and drains"
        );
        assert!(
            !drop_through_gap(&mut shut),
            "a 27 mm ball dropped through the closed gap reached the trough"
        );
        assert_healthy(&open, "zipper open");
        assert_healthy(&shut, "zipper engaged");
    });
}
