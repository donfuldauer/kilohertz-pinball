//! Independent spec tests for `pin_render::gpu::choose_present_mode` (Subphase 1.4,
//! verification criterion 5, PLAN §2.2 / D19). Pure policy: no adapter needed.
//!
//! The oracle below is written from the brief's table:
//!   Playfield: Direct(FifoRelaxed) if vrr && offered contains it, else Direct(Fifo).
//!   Backglass/Score: Direct(Mailbox) if offered, else Direct(Immediate) if offered,
//!   else PresenterThread(Fifo). An empty `offered` list is treated as [Fifo].

use pin_render::gpu::{PresentChoice, choose_present_mode};
use pin_render::topology::SurfaceRole;
use wgpu::PresentMode;

const MODES: [PresentMode; 6] = [
    PresentMode::Fifo,
    PresentMode::FifoRelaxed,
    PresentMode::Mailbox,
    PresentMode::Immediate,
    PresentMode::AutoVsync,
    PresentMode::AutoNoVsync,
];

const ROLES: [SurfaceRole; 3] = [
    SurfaceRole::Playfield,
    SurfaceRole::Backglass,
    SurfaceRole::Score,
];

/// (is_presenter_thread, mode)
type Flat = (bool, PresentMode);

fn flat(c: PresentChoice) -> Flat {
    match c {
        PresentChoice::Direct(m) => (false, m),
        PresentChoice::PresenterThread(m) => (true, m),
    }
}

fn oracle(role: SurfaceRole, offered: &[PresentMode], vrr: bool) -> Flat {
    match role {
        SurfaceRole::Playfield => {
            if vrr && offered.contains(&PresentMode::FifoRelaxed) {
                (false, PresentMode::FifoRelaxed)
            } else {
                (false, PresentMode::Fifo)
            }
        }
        SurfaceRole::Backglass | SurfaceRole::Score => {
            if offered.contains(&PresentMode::Mailbox) {
                (false, PresentMode::Mailbox)
            } else if offered.contains(&PresentMode::Immediate) {
                (false, PresentMode::Immediate)
            } else {
                (true, PresentMode::Fifo)
            }
        }
    }
}

fn subset(mask: u32) -> Vec<PresentMode> {
    MODES
        .iter()
        .enumerate()
        .filter(|(i, _)| mask & (1 << i) != 0)
        .map(|(_, m)| *m)
        .collect()
}

#[test]
fn every_subset_role_and_vrr_matches_the_table() {
    let mut checked = 0;
    for mask in 0..64u32 {
        let offered = subset(mask);
        for role in ROLES {
            for vrr in [false, true] {
                let got = flat(choose_present_mode(role, &offered, vrr));
                let want = oracle(role, &offered, vrr);
                assert_eq!(
                    got, want,
                    "role {role:?}, vrr {vrr}, offered {offered:?} (presenter_thread, mode)"
                );
                checked += 1;
            }
        }
    }
    assert_eq!(checked, 64 * 3 * 2);
}

#[test]
fn the_order_and_duplicates_of_offered_modes_do_not_matter() {
    for mask in 0..64u32 {
        let offered = subset(mask);
        let mut reversed = offered.clone();
        reversed.reverse();
        let mut rotated = offered.clone();
        if !rotated.is_empty() {
            rotated.rotate_left(1);
        }
        let mut doubled = offered.clone();
        doubled.extend(reversed.iter().copied());
        for role in ROLES {
            for vrr in [false, true] {
                let want = oracle(role, &offered, vrr);
                for variant in [&reversed, &rotated, &doubled] {
                    assert_eq!(
                        flat(choose_present_mode(role, variant, vrr)),
                        want,
                        "role {role:?}, vrr {vrr}, offered {variant:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn empty_offered_list_is_treated_as_fifo_only() {
    for vrr in [false, true] {
        assert!(matches!(
            choose_present_mode(SurfaceRole::Playfield, &[], vrr),
            PresentChoice::Direct(PresentMode::Fifo)
        ));
        for role in [SurfaceRole::Backglass, SurfaceRole::Score] {
            assert!(matches!(
                choose_present_mode(role, &[], vrr),
                PresentChoice::PresenterThread(PresentMode::Fifo)
            ));
            // Same answer as the explicit list.
            assert_eq!(
                flat(choose_present_mode(role, &[], vrr)),
                flat(choose_present_mode(role, &[PresentMode::Fifo], vrr))
            );
        }
    }
}

#[test]
fn named_cases_from_the_policy() {
    use PresentMode::*;
    // Lavapipe under Xvfb reports this list (DECISIONS 0.1 Q4).
    let lavapipe = [Immediate, Mailbox, Fifo, FifoRelaxed];
    assert!(matches!(
        choose_present_mode(SurfaceRole::Playfield, &lavapipe, false),
        PresentChoice::Direct(Fifo)
    ));
    assert!(matches!(
        choose_present_mode(SurfaceRole::Playfield, &lavapipe, true),
        PresentChoice::Direct(FifoRelaxed)
    ));
    assert!(matches!(
        choose_present_mode(SurfaceRole::Backglass, &lavapipe, false),
        PresentChoice::Direct(Mailbox)
    ));
    assert!(matches!(
        choose_present_mode(SurfaceRole::Score, &lavapipe, true),
        PresentChoice::Direct(Mailbox)
    ));

    // VRR requested but FifoRelaxed not offered: plain Fifo.
    assert!(matches!(
        choose_present_mode(SurfaceRole::Playfield, &[Fifo, Mailbox, Immediate], true),
        PresentChoice::Direct(Fifo)
    ));
    // The playfield never tears and never gets a presenter thread.
    assert!(matches!(
        choose_present_mode(SurfaceRole::Playfield, &[Immediate, Mailbox], false),
        PresentChoice::Direct(Fifo)
    ));
    // Secondary without Mailbox: Immediate.
    assert!(matches!(
        choose_present_mode(SurfaceRole::Score, &[Fifo, FifoRelaxed, Immediate], false),
        PresentChoice::Direct(Immediate)
    ));
    // Secondary with only blocking modes: its own presenter thread on Fifo, so it cannot
    // stall the playfield. FifoRelaxed and the Auto modes do not count as non-blocking.
    for offered in [
        &[Fifo][..],
        &[Fifo, FifoRelaxed][..],
        &[AutoVsync, AutoNoVsync][..],
        &[Fifo, FifoRelaxed, AutoVsync, AutoNoVsync][..],
    ] {
        for role in [SurfaceRole::Backglass, SurfaceRole::Score] {
            for vrr in [false, true] {
                assert!(
                    matches!(
                        choose_present_mode(role, offered, vrr),
                        PresentChoice::PresenterThread(Fifo)
                    ),
                    "{role:?} {offered:?} vrr {vrr}"
                );
            }
        }
    }
}

#[test]
fn vrr_only_affects_the_playfield() {
    for mask in 0..64u32 {
        let offered = subset(mask);
        for role in [SurfaceRole::Backglass, SurfaceRole::Score] {
            assert_eq!(
                flat(choose_present_mode(role, &offered, false)),
                flat(choose_present_mode(role, &offered, true))
            );
        }
        assert_eq!(
            flat(choose_present_mode(SurfaceRole::Backglass, &offered, false)),
            flat(choose_present_mode(SurfaceRole::Score, &offered, false)),
            "backglass and score follow the same rule"
        );
    }
}
