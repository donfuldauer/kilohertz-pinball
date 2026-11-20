//! 6.4 CodeRabbit round 1 (PR #58) regressions on the Feature Lab table and
//! its build:
//!
//! * the build refuses an output directory that is the source directory
//!   (also through a symlink alias) and leaves every source untouched;
//! * the orbit and flipper-shot observations (`last_entry`, `last_flip`)
//!   do not carry over from one player's ball to the next;
//! * the Banana bonus (`banana_shots`) is paid on a complete `banana_vuk`
//!   shot (VUK lane into the backbox VUK), as README "Modes" documents, not
//!   on a bare VUK-lane hit;
//! * `bb_catch` returns a layer-4 ball that drops through the entry hole
//!   while drifting towards the back (+y) to the playfield, as it does one
//!   drifting towards the player (since PR #142 the cups around the entry
//!   zone catch some of these balls first, and one of them stays in play
//!   instead of draining: back on layer 0, outside the closed pocket, never
//!   out of the table);
//! * every 4 -> 0 zone the table defines stays clear of the generated
//!   transitions and of the VUK's exit zone.

mod feature_lab_support;

use feature_lab_support::build::build_from;
use feature_lab_support::*;
use pin_core::schema::table::TableManifest;
use pin_sim::Simulation;
use std::path::{Path, PathBuf};

// --- item 1: the build never writes into its own sources ----------------------

/// Every regular file under `dir` (relative path, bytes), sorted.
fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                let rel = p.strip_prefix(root).unwrap().to_path_buf();
                out.push((rel, std::fs::read(&p).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// A copy of the `tables/feature_lab` sources in a fresh scratch directory.
fn sources_copy(tag: &str) -> PathBuf {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let p = e.unwrap().path();
            let dest = to.join(p.file_name().unwrap());
            if p.is_dir() {
                copy(&p, &dest);
            } else {
                std::fs::copy(&p, &dest).unwrap();
            }
        }
    }
    let to = scratch(tag);
    copy(&feature_lab_src(), &to);
    to
}

#[test]
fn the_build_refuses_its_source_directory_as_output() {
    let src = sources_copy("cr1_src_eq_out");
    let before = snapshot(&src);
    assert!(before.iter().any(|(p, _)| p == Path::new("table.toml")));
    let err = build_from(&src, &src).expect_err("building into the sources must fail");
    assert!(
        err.contains("source directory"),
        "the message names the problem: {err}"
    );
    assert_eq!(snapshot(&src), before, "every source file is unchanged");
    // A distinct output directory still builds, and leaves the sources alone.
    let out = scratch("cr1_src_eq_out_ok");
    build_from(&src, &out).expect("a distinct output directory builds");
    assert!(out.join("table.toml").is_file());
    assert!(out.join("playfield.glb").is_file());
    assert_eq!(snapshot(&src), before, "the sources are still unchanged");
    std::fs::remove_dir_all(&src).unwrap();
    std::fs::remove_dir_all(&out).unwrap();
}

/// The same through a symlink alias of the source directory, and through a
/// path that is not normalised (`<src>/display/..`).
#[cfg(unix)]
#[test]
fn the_build_refuses_an_alias_of_its_source_directory() {
    let src = sources_copy("cr1_src_alias");
    let before = snapshot(&src);
    let links = scratch("cr1_src_alias_links");
    let alias = links.join("alias");
    std::os::unix::fs::symlink(&src, &alias).unwrap();
    for (from, to) in [
        (src.clone(), alias.clone()),
        (alias.clone(), src.clone()),
        (src.clone(), src.join("display").join("..")),
    ] {
        let err = build_from(&from, &to).expect_err(&format!(
            "{} -> {} must fail",
            from.display(),
            to.display()
        ));
        assert!(err.contains("source directory"), "{err}");
        assert_eq!(snapshot(&src), before, "{} unchanged", to.display());
    }
    std::fs::remove_dir_all(&links).unwrap();
    std::fs::remove_dir_all(&src).unwrap();
}

// --- item 15: scoring observations end with the player's ball -----------------

/// Score of player 1 (0 before the first report).
fn score(s: &Simulation) -> i64 {
    s.score(0).unwrap_or(0)
}

/// Closes switch `name` for 5 ticks, releases it and lets the rules see the
/// opening edge; returns the points it scored.
fn hit(s: &mut Simulation, name: &str) -> i64 {
    let before = score(s);
    press(s, name, 5);
    s.advance(3);
    score(s) - before
}

/// A game on ball 1 with the player's ball on the plunger.
fn game() -> Simulation {
    let mut s = load();
    s.advance(10);
    start_game(&mut s);
    s
}

/// Ends the player's ball (every ball removed) and waits for the next one.
fn next_ball(s: &mut Simulation) {
    let b = persist(s, "ball");
    clear_balls(s);
    s.advance(20);
    assert_eq!(persist(s, "ball"), b + 1, "the next ball is served");
    assert_eq!(s.balls_in_play(), 1);
}

#[test]
fn an_orbit_entry_does_not_carry_over_to_the_next_ball() {
    on_big_stack(|| {
        // Control: orbit made from the bottom within the same ball.
        for gap in [0u64, 500, 2000] {
            let mut s = game();
            assert_eq!(hit(&mut s, "orbit_entry"), 100);
            s.advance(gap);
            assert_eq!(hit(&mut s, "orbit_top"), 5000, "same ball, gap {gap}");
            healthy(&s);
        }
        // The reported case: orbit_entry, the ball ends, the next ball's
        // plunge reaches orbit_top within the orbit window.
        for gap in [0u64, 500, 2000] {
            let mut s = game();
            assert_eq!(hit(&mut s, "orbit_entry"), 100);
            next_ball(&mut s);
            s.advance(gap);
            assert_eq!(hit(&mut s, "orbit_top"), 500, "next ball, gap {gap}");
            // The next ball's own orbit still scores.
            assert_eq!(hit(&mut s, "orbit_entry"), 100);
            assert_eq!(hit(&mut s, "orbit_top"), 5000, "next ball's own orbit");
            healthy(&s);
        }
    });
}

#[test]
fn a_flip_does_not_carry_over_to_the_next_ball() {
    on_big_stack(|| {
        // Control: a flip, then a VUK-lane hit within 1 s scores the
        // flipper shot (100 + 5000).
        let mut s = game();
        press(&mut s, "left_flipper", 150);
        s.advance(20);
        assert_eq!(hit(&mut s, "vuk_lane"), 5100, "same ball");
        healthy(&s);
        // A flip, the ball ends at once, a VUK-lane hit on the next ball
        // within 1 s of the flip: the lane's 100 only.
        let mut s = game();
        press(&mut s, "left_flipper", 150);
        next_ball(&mut s);
        assert_eq!(hit(&mut s, "vuk_lane"), 100, "next ball");
        healthy(&s);
    });
}

// --- item 16: the Banana bonus is paid on the complete shot -------------------

#[test]
fn the_banana_bonus_is_paid_on_the_complete_banana_vuk_shot() {
    on_big_stack(|| {
        let mut s = game();
        select_mode(&mut s, 0);
        // The qualifying shot: VUK lane 100, backbox VUK 500, mode start
        // 2000; no bonus yet.
        assert_eq!(hit(&mut s, "vuk_lane"), 100);
        assert_eq!(hit(&mut s, "bb_vuk_sw"), 500 + 2000);
        assert_eq!(persist(&s, "mode:banana_shots"), 1);
        s.advance(1000);
        // A bare VUK-lane hit (no backbox VUK after it): 100.
        assert_eq!(hit(&mut s, "vuk_lane"), 100, "lane hit alone");
        s.advance(3500);
        // The next complete shot pays the bonus once (lane 100, then VUK
        // 500 + 25000); the mode does not start again.
        assert_eq!(hit(&mut s, "vuk_lane"), 100);
        assert_eq!(hit(&mut s, "bb_vuk_sw"), 500 + 25_000, "complete shot");
        assert_eq!(persist(&s, "mode:banana_shots"), 1);
        // Out of its 3 s window the backbox VUK alone is no Banana shot.
        s.advance(3500);
        assert_eq!(hit(&mut s, "bb_vuk_sw"), 500, "VUK alone");
        // The bonus ends with the ball.
        next_ball(&mut s);
        assert_eq!(hit(&mut s, "vuk_lane"), 100);
        assert_eq!(hit(&mut s, "bb_vuk_sw"), 500, "next ball, mode not running");
        healthy(&s);
    });
}

// --- item 19: bb_catch covers the +y drift of a ball dropping out ------------

fn zone(t: &TableManifest, id: &str) -> ([f32; 3], [f32; 3]) {
    let z = t
        .layer_transitions
        .iter()
        .find(|z| z.id == id)
        .unwrap_or_else(|| panic!("layer transition {id}"));
    (z.zone.min, z.zone.max)
}

/// What happened to a layer-4 standard ball released at `p` (mm) with
/// velocity `v` (m/s) over 20 s of simulated time.
#[derive(Debug)]
struct Fall {
    /// It was on layer 0 on some tick.
    on_0: bool,
    /// Centre (mm) on the first tick it was on layer 0.
    returned_at: Option<[f32; 2]>,
    /// A main trough switch closed.
    drained: bool,
    /// Still in play on layer 0 at the end.
    alive_on_0: bool,
    /// Last centre (mm).
    final_at: [f32; 2],
    escaped: u32,
}

impl Fall {
    /// Back on layer 0 outside the closed pocket, never escaped, and either
    /// drained or alive on the playfield at a rest point outside the pocket
    /// (the pocket predicate is checked at the return and final points
    /// only: it is not valid on every tick of a ball that runs the orbit).
    fn back_in_play(&self) -> bool {
        self.on_0
            && self.escaped == 0
            && !self.returned_at.is_some_and(in_pocket)
            && (self.drained || (self.alive_on_0 && !in_pocket(self.final_at)))
    }
}

fn drop_layer_4(dir: &Path, p: [f32; 3], v: [f32; 3]) -> Fall {
    let mut s = Simulation::load(dir, pin_sim::SimConfig::default()).expect("load");
    let h = s
        .spawn_ball_on_layer(
            "standard_steel",
            4,
            [p[0] * 1.0e-3, p[1] * 1.0e-3, p[2] * 1.0e-3],
            v,
        )
        .unwrap();
    let mut on_0 = false;
    let mut returned_at = None;
    let mut final_at = [0.0; 2];
    for _ in 0..20_000 {
        s.advance(1);
        let Some(b) = s.ball(h) else { break };
        final_at = [b.position_m[0] * 1.0e3, b.position_m[1] * 1.0e3];
        if b.layer == 0 {
            on_0 = true;
            returned_at.get_or_insert(final_at);
        }
    }
    let troughs = s.switch_hits("trough_1").unwrap() + s.switch_hits("trough_2").unwrap();
    Fall {
        on_0,
        returned_at,
        drained: troughs >= 1,
        alive_on_0: s.ball(h).is_some_and(|b| b.layer == 0),
        final_at,
        escaped: s.counters().balls_escaped,
    }
}

#[test]
fn a_ball_dropping_out_of_the_entry_hole_towards_the_back_returns_to_the_playfield() {
    on_big_stack(|| {
        // The reported drop, then a sweep across the hole (x 67..137) and
        // over drifts towards the back (+y) and towards the player (-y).
        let f = drop_layer_4(bundle(), [102.0, 918.0, 85.0], [0.0, 0.4, -1.0]);
        assert!(
            f.on_0 && f.drained && f.escaped == 0,
            "reported drop: {f:?}"
        );
        for x in [80.0f32, 102.0, 125.0] {
            // Up to 0.6 m/s either way (the entry-channel test's bound in
            // feature_lab_physics.rs): from 85 mm with no fall speed a
            // 0.6 m/s drift reaches y ~978 mm when z passes 25 mm. A ball
            // that a cup returns next to the entry channel may come to rest
            // on the playfield instead of draining within the 20 s (PR #142:
            // x 80, vy 0.4, vz 0 rests at the Banana).
            for vy in [-0.6f32, -0.3, 0.0, 0.2, 0.4, 0.5, 0.6] {
                for vz in [0.0f32, -1.0] {
                    let f = drop_layer_4(bundle(), [x, 918.0, 85.0], [0.0, vy, vz]);
                    assert!(f.back_in_play(), "x {x} vy {vy} vz {vz}: {f:?}");
                }
            }
        }
    });
}

/// The 4 -> 0 zones `table.toml` defines (PR #142: `bb_catch` split at
/// y 955 into `bb_catch` and `bb_catch_back`, and the five cups around the
/// generated entry zone).
const CATCHES: [&str; 8] = [
    "bb_catch",
    "bb_catch_back",
    "bb_cup_left",
    "bb_cup_front",
    "bb_cup_back",
    "bb_cup_right",
    "bb_cup_floor",
    "bb_catch_left",
];

/// The catch zones stay clear of the generated transitions and inside the
/// cabinet (514 x 1070 mm).
///
/// * `bb_catch` and `bb_catch_back` lie below the generated entry (0 -> 4)
///   and drain (4 -> 0) zones and end before the orbit top (y < 1000).
/// * Every table-defined 4 -> 0 zone (the cups around the entry zone,
///   which are beside it, not below it, included) shares no point with
///   either generated zone. Transition bounds are inclusive, so boxes that
///   only touch on a face count as overlapping.
/// * The VUK's exit zone (0 -> 4, built from the generated mech
///   `tablegen:backbox.vuk`, which the rules fire through `bb_vuk_coil`
///   after `bb_vuk_sw` closes) stays above z 96 mm, above every
///   table-defined 4 -> 0 zone and clear of them.
#[test]
fn bb_catch_stays_clear_of_the_generated_transitions() {
    let text = std::fs::read_to_string(bundle().join("table.toml")).unwrap();
    let t = TableManifest::from_toml_str(&text).unwrap();
    let generated = [
        ("tablegen:backbox.entry", zone(&t, "tablegen:backbox.entry")),
        ("tablegen:backbox.drain", zone(&t, "tablegen:backbox.drain")),
    ];
    for id in ["bb_catch", "bb_catch_back"] {
        let catch = t
            .layer_transitions
            .iter()
            .find(|z| z.id == id)
            .unwrap_or_else(|| panic!("layer transition {id}"));
        assert_eq!((catch.from_layer, catch.to_layer), (4, 0), "{id}");
        let (min, max) = (catch.zone.min, catch.zone.max);
        for (gid, (zmin, _)) in generated {
            assert!(
                max[2] < zmin[2],
                "{gid}: {id} top {} under {}",
                max[2],
                zmin[2]
            );
        }
        // Inside the cabinet (514 x 1070 mm), clear of the orbit top (y 1040).
        assert!(
            min[0] >= 0.0 && max[0] <= 514.0 && min[1] >= 0.0,
            "{id}: {min:?} {max:?}"
        );
        assert!(max[1] < 1000.0, "{id}: {max:?}");
    }
    let catches: Vec<_> = t
        .layer_transitions
        .iter()
        .filter(|z| z.from_layer == 4 && z.to_layer == 0 && !z.id.starts_with("tablegen:"))
        .collect();
    let ids: Vec<&str> = catches.iter().map(|z| z.id.as_str()).collect();
    for id in CATCHES {
        assert!(ids.contains(&id), "{id} among the 4 -> 0 zones {ids:?}");
    }
    for z in &catches {
        let b = (z.zone.min, z.zone.max);
        for (gid, g) in generated {
            assert!(
                !boxes_touch_or_overlap(b, g),
                "{} {b:?} touches or overlaps {gid} {g:?}",
                z.id
            );
        }
        assert!(
            b.0[0] >= 0.0 && b.1[0] <= 514.0 && b.0[1] >= 0.0 && b.1[1] <= 1070.0,
            "{} inside the cabinet: {b:?}",
            z.id
        );
    }
    // The VUK's exit zone, as the sim builds it (mm).
    let vuk = t
        .mechs
        .iter()
        .find(|m| m.id == "tablegen:backbox.vuk")
        .expect("tablegen:backbox.vuk");
    let (pos, off) = (vuk.position.unwrap(), vuk.exit_offset.unwrap());
    let exit = [pos[0] + off[0], pos[1] + off[1], pos[2] + off[2]];
    let s = load();
    let vuk_zones: Vec<([f32; 3], [f32; 3])> = s
        .mech_set()
        .transitions()
        .zones()
        .iter()
        .filter(|z| z.from_layer == 0 && z.to_layer == 4)
        .map(|z| {
            (
                z.min.to_array().map(|c| c * 1.0e3),
                z.max.to_array().map(|c| c * 1.0e3),
            )
        })
        .collect();
    assert_eq!(vuk_zones.len(), 1, "one VUK exit zone: {vuk_zones:?}");
    let v = vuk_zones[0];
    for k in 0..3 {
        let mid = (v.0[k] + v.1[k]) / 2.0;
        assert!(
            (mid - exit[k]).abs() < 0.01,
            "centred on the exit {exit:?}: {v:?}"
        );
    }
    assert!(v.0[2] > 96.0, "VUK exit zone floor {} above z 96", v.0[2]);
    for z in &catches {
        let b = (z.zone.min, z.zone.max);
        assert!(
            v.0[2] > b.1[2] && !boxes_touch_or_overlap(b, v),
            "VUK exit zone {v:?} above and clear of {} {b:?}",
            z.id
        );
    }
}
