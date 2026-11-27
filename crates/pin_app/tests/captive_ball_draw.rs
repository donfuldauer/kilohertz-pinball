//! #163 gate round 11 (R12 analytic, GPU-free): every captive Newton ball of
//! the in-repo tables is published as an ordinary `FrameStateSnapshot::balls`
//! entry (SPEC Appendix A "Dynamic mesh slot convention": "Captive balls are
//! published as ordinary `balls` entries and take no extra slots";
//! REQ-VIS-002), after the free balls, so every consumer sees it once: the
//! playfield view pushes one ball `DrawItem` and one `MotionBall` per
//! `snap.balls()` entry, and the debug overlay ([`snapshot_lines`]) draws one
//! circle per entry.
//!
//! The view's ball loop (`TableView::encode_playfield`) is private and
//! GPU-bound; this test drives its public inputs: the snapshot, the
//! `default_ball_radius` it uses, `MotionBall::from_state` and
//! `snapshot_lines`.

mod support;

mod sky_deck_support;

use pin_app::orchestrator::default_ball_radius;
use pin_core::schema::table::{MechKind, TableManifest};
use pin_render::debug::{CIRCLE_SEGMENTS, DebugLines, snapshot_lines};
use pin_render::post::MotionBall;
use pin_sim::{SimConfig, Simulation};
use std::path::Path;

/// Newton mechs declared by the bundle's `table.toml`.
fn newton_count(bundle: &Path) -> usize {
    let text = std::fs::read_to_string(bundle.join("table.toml")).expect("table.toml");
    let m = TableManifest::from_toml_str(&text).expect("table.toml parses");
    m.mechs
        .iter()
        .filter(|x| x.kind == MechKind::NewtonBall)
        .count()
}

/// Loads `bundle`, runs it in attract mode (no free ball), and checks that
/// the snapshot carries exactly one entry per captive ball and that the
/// view and overlay inputs built from it count one ball each.
fn check(bundle: &'static Path) {
    let captives = newton_count(bundle);
    assert!(
        captives >= 1,
        "{bundle:?}: the table declares a newton_ball"
    );
    support::on_big_stack(move || {
        let mut sim = Simulation::load(bundle, SimConfig::default()).expect("load");
        let mut lines = DebugLines::new(4096);
        for _ in 0..500 {
            sim.advance(1);
            let tick = sim.tick();
            let snap = sim.snapshot();
            let free = sim.balls_in_play();
            assert_eq!(
                usize::from(snap.ball_count),
                free + captives,
                "tick {tick}: balls_in_play {free} + captives {captives}"
            );
            // The view: one DrawItem and one MotionBall per entry.
            let motion: Vec<MotionBall> = snap
                .balls()
                .iter()
                .map(|b| MotionBall::from_state(b, default_ball_radius(b.profile)))
                .collect();
            assert_eq!(motion.len(), free + captives);
            for (b, m) in snap.balls()[free..].iter().zip(&motion[free..]) {
                assert!(
                    b.position_m.iter().all(|c| c.is_finite())
                        && b.velocity_m_s.iter().all(|c| c.is_finite()),
                    "tick {tick}: captive {b:?}"
                );
                assert!(m.radius > 0.0 && m.radius.is_finite());
            }
            // The debug overlay: one circle per entry.
            lines.clear();
            snapshot_lines(snap, &default_ball_radius, &mut lines);
            assert_eq!(lines.len(), (free + captives) * CIRCLE_SEGMENTS);
        }
    });
}

/// Grey box (one newton_ball mech).
#[test]
fn spec_appendix_a_greybox_publishes_its_captive_ball_once() {
    check(support::greybox_bundle());
}

/// Sky Deck (one newton_ball mech).
#[test]
fn spec_appendix_a_sky_deck_publishes_its_captive_ball_once() {
    check(sky_deck_support::sky_deck_bundle());
}
