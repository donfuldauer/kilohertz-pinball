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
//!   drifting towards the player.

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

/// A layer-4 standard ball released at `p` (mm) with velocity `v` (m/s):
/// whether it came back to layer 0, whether a main trough switch drained
/// it, and the escape count.
fn drop_layer_4(dir: &Path, p: [f32; 3], v: [f32; 3]) -> (bool, bool, u32) {
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
    for _ in 0..20_000 {
        s.advance(1);
        match s.ball(h) {
            Some(b) => on_0 |= b.layer == 0,
            None => break,
        }
    }
    let troughs = s.switch_hits("trough_1").unwrap() + s.switch_hits("trough_2").unwrap();
    (on_0, troughs >= 1, s.counters().balls_escaped)
}

#[test]
fn a_ball_dropping_out_of_the_entry_hole_towards_the_back_returns_to_the_playfield() {
    on_big_stack(|| {
        // The reported drop, then a sweep across the hole (x 67..137) and
        // over drifts towards the back (+y) and towards the player (-y).
        let (on_0, drained, escaped) =
            drop_layer_4(bundle(), [102.0, 918.0, 85.0], [0.0, 0.4, -1.0]);
        assert!(
            on_0 && drained && escaped == 0,
            "reported drop: on_0 {on_0}, drained {drained}, escaped {escaped}"
        );
        for x in [80.0f32, 102.0, 125.0] {
            // Up to 0.6 m/s either way (the entry-channel test's bound in
            // feature_lab_physics.rs): from 85 mm with no fall speed a
            // 0.6 m/s drift reaches y ~978 mm when z passes 25 mm.
            for vy in [-0.6f32, -0.3, 0.0, 0.2, 0.4, 0.5, 0.6] {
                for vz in [0.0f32, -1.0] {
                    let (on_0, drained, escaped) =
                        drop_layer_4(bundle(), [x, 918.0, 85.0], [0.0, vy, vz]);
                    assert!(
                        on_0 && drained && escaped == 0,
                        "x {x} vy {vy} vz {vz}: on_0 {on_0}, drained {drained}, escaped {escaped}"
                    );
                }
            }
        }
    });
}

/// The extended zone stays below the generated entry (0 -> 4) and drain
/// (4 -> 0) zones, so it never overlaps them, and inside the playfield.
#[test]
fn bb_catch_stays_clear_of_the_generated_transitions() {
    let text = std::fs::read_to_string(bundle().join("table.toml")).unwrap();
    let t = TableManifest::from_toml_str(&text).unwrap();
    let catch = t
        .layer_transitions
        .iter()
        .find(|z| z.id == "bb_catch")
        .unwrap();
    assert_eq!((catch.from_layer, catch.to_layer), (4, 0));
    let (min, max) = (catch.zone.min, catch.zone.max);
    for id in ["tablegen:backbox.entry", "tablegen:backbox.drain"] {
        let (zmin, _) = zone(&t, id);
        assert!(
            max[2] < zmin[2],
            "{id}: bb_catch top {} under {}",
            max[2],
            zmin[2]
        );
    }
    // Inside the cabinet (514 x 1070 mm), clear of the orbit top (y 1040).
    assert!(
        min[0] >= 0.0 && max[0] <= 514.0 && min[1] >= 0.0,
        "{min:?} {max:?}"
    );
    assert!(max[1] < 1000.0, "{max:?}");
}
