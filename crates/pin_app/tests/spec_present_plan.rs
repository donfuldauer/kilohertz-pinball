//! PR #21 rounds 3 and 4 (CodeRabbit threads 4577325408, 4577423193): on
//! the table path the render thread never acquires or presents a
//! blocking-mode backglass / score surface itself; such a surface (or any
//! backglass / score surface with `force_presenter_thread`) is retained and
//! presented by its own presenter thread instead of being dropped (D19).
//! Pure, GPU-free and display-free checks of the public presentation plan
//! (`plan_surface`, `slot_decision`, `table_set_accepted`).

use pin_app::orchestrator::{
    SlotDecision, SurfacePlan, is_blocking_mode, plan_surface, slot_decision, table_set_accepted,
};
use pin_render::gpu::{SurfaceOptions, choose_present_mode, wgpu};
use pin_render::topology::SurfaceRole;
use wgpu::PresentMode as M;
use wgpu::TextureFormat as F;

const ROLES: [SurfaceRole; 3] = [
    SurfaceRole::Playfield,
    SurfaceRole::Backglass,
    SurfaceRole::Score,
];

const MODES: [M; 6] = [
    M::Fifo,
    M::FifoRelaxed,
    M::Mailbox,
    M::Immediate,
    M::AutoVsync,
    M::AutoNoVsync,
];

const FORMAT_SETS: [&[F]; 5] = [
    &[F::Bgra8UnormSrgb],
    &[F::Rgba8Unorm, F::Rgba16Float],
    &[F::Bgra8Unorm],
    &[F::Rgba16Float, F::Rgb10a2Unorm],
    &[],
];

fn opts(vrr: bool, force: bool) -> SurfaceOptions {
    SurfaceOptions {
        vrr,
        force_presenter_thread: force,
    }
}

/// The 8-bit format the presenter would pick (unorm first, then sRGB).
fn eight_bit(formats: &[F]) -> Option<F> {
    formats
        .iter()
        .copied()
        .find(|f| matches!(f, F::Bgra8Unorm | F::Rgba8Unorm))
        .or_else(|| {
            formats
                .iter()
                .copied()
                .find(|f| matches!(f, F::Bgra8UnormSrgb | F::Rgba8UnormSrgb))
        })
}

/// Every subset of [`MODES`] (64 present-mode sets, including the empty one).
fn mode_sets() -> Vec<Vec<M>> {
    (0u32..(1 << MODES.len()))
        .map(|bits| {
            MODES
                .iter()
                .enumerate()
                .filter(|(i, _)| bits & (1 << i) != 0)
                .map(|(_, m)| *m)
                .collect()
        })
        .collect()
}

#[test]
fn blocking_or_forced_secondary_surfaces_get_a_presenter_thread() {
    let mut checked = 0usize;
    for role in ROLES {
        for modes in mode_sets() {
            for formats in FORMAT_SETS {
                for (vrr, force) in [(false, false), (false, true), (true, false), (true, true)] {
                    let plan = plan_surface(role, formats, &modes, opts(vrr, force));
                    let d = slot_decision(role, plan, eight_bit(formats));
                    let ctx = format!("{role:?} {modes:?} {formats:?} vrr {vrr} force {force}");
                    let chosen = choose_present_mode(role, &modes, vrr).mode();
                    checked += 1;
                    if role == SurfaceRole::Playfield {
                        // Never a presenter thread, forced or not.
                        assert!(
                            !matches!(plan, SurfacePlan::Presenter(_)),
                            "{ctx}: {plan:?}"
                        );
                        match eight_bit(formats) {
                            Some(f) => {
                                assert_eq!(plan, SurfacePlan::Direct(chosen), "{ctx}");
                                assert!(
                                    matches!(d, SlotDecision::Draw(m, g)
                                        if g == f && matches!(m, M::Fifo | M::FifoRelaxed)),
                                    "{ctx}: {d:?}"
                                );
                            }
                            None => {
                                assert_eq!(plan, SurfacePlan::Unsupported, "{ctx}");
                                assert_eq!(d, SlotDecision::Reject, "{ctx}");
                            }
                        }
                        continue;
                    }
                    let Some(f) = eight_bit(formats) else {
                        assert_eq!(plan, SurfacePlan::Unsupported, "{ctx}");
                        assert_eq!(d, SlotDecision::Skip, "{ctx}");
                        continue;
                    };
                    // A secondary surface is drawn directly exactly when it
                    // is not forced and offers a non-blocking mode the policy
                    // uses (Mailbox / Immediate); otherwise it gets a
                    // presenter thread with the mode the policy picked.
                    let usable_non_blocking =
                        modes.contains(&M::Mailbox) || modes.contains(&M::Immediate);
                    if force || is_blocking_mode(chosen) {
                        assert_eq!(plan, SurfacePlan::Presenter(chosen), "{ctx}");
                        assert_eq!(d, SlotDecision::Present(chosen, f), "{ctx}");
                    } else {
                        assert_eq!(plan, SurfacePlan::Direct(chosen), "{ctx}");
                        assert_eq!(d, SlotDecision::Draw(chosen, f), "{ctx}");
                    }
                    match d {
                        SlotDecision::Draw(m, _) => {
                            assert!(!is_blocking_mode(m), "{ctx}: drawn with {m:?}");
                            assert!(usable_non_blocking && !force, "{ctx}");
                        }
                        SlotDecision::Present(m, _) => {
                            assert!(force || is_blocking_mode(m), "{ctx}: {m:?}");
                            assert!(force || !usable_non_blocking, "{ctx}");
                        }
                        other => panic!("{ctx}: {other:?}"),
                    }
                    // Fifo-only and FifoRelaxed-only windows, forced or not:
                    // retained on a presenter thread, never dropped.
                    if modes == [M::Fifo] || modes == [M::FifoRelaxed] {
                        assert_eq!(d, SlotDecision::Present(M::Fifo, f), "{ctx}");
                    }
                    // Forced with Mailbox offered: the presenter keeps Mailbox.
                    if force && modes.contains(&M::Mailbox) {
                        assert_eq!(d, SlotDecision::Present(M::Mailbox, f), "{ctx}");
                    }
                }
            }
        }
    }
    assert_eq!(checked, 3 * 64 * FORMAT_SETS.len() * 4);
}

#[test]
fn a_fifo_only_score_window_keeps_the_table_presenter() {
    let formats = [F::Bgra8UnormSrgb];
    for playfield_modes in [&[M::Fifo, M::Mailbox][..], &[M::Fifo][..]] {
        for (vrr, force) in [(false, false), (false, true), (true, false), (true, true)] {
            let o = opts(vrr, force);
            let pf = plan_surface(SurfaceRole::Playfield, &formats, playfield_modes, o);
            let score = plan_surface(SurfaceRole::Score, &formats, &[M::Fifo], o);
            assert_eq!(pf, SurfacePlan::Direct(M::Fifo));
            assert_eq!(score, SurfacePlan::Presenter(M::Fifo));
            assert!(table_set_accepted(&[
                (SurfaceRole::Playfield, pf),
                (SurfaceRole::Score, score),
            ]));
            assert_eq!(
                slot_decision(SurfaceRole::Score, score, Some(formats[0])),
                SlotDecision::Present(M::Fifo, formats[0])
            );
            assert_eq!(
                slot_decision(SurfaceRole::Playfield, pf, Some(formats[0])),
                SlotDecision::Draw(M::Fifo, formats[0])
            );
        }
    }
    // Presenter-thread backglass and score windows next to a direct
    // playfield: still the table.
    assert!(table_set_accepted(&[
        (SurfaceRole::Playfield, SurfacePlan::Direct(M::Fifo)),
        (
            SurfaceRole::Backglass,
            SurfacePlan::Presenter(M::FifoRelaxed)
        ),
        (SurfaceRole::Score, SurfacePlan::Presenter(M::Fifo)),
    ]));
    // Presenter-thread surfaces alone are not drawable: nothing paces.
    assert!(!table_set_accepted(&[
        (SurfaceRole::Backglass, SurfacePlan::Presenter(M::Fifo)),
        (SurfaceRole::Score, SurfacePlan::Presenter(M::Mailbox)),
    ]));
    assert!(!table_set_accepted(&[(
        SurfaceRole::Score,
        SurfacePlan::Presenter(M::Fifo)
    )]));
    // An unsupported playfield still rejects the set, whatever else.
    assert!(!table_set_accepted(&[
        (SurfaceRole::Playfield, SurfacePlan::Unsupported),
        (SurfaceRole::Score, SurfacePlan::Direct(M::Mailbox)),
    ]));
    assert!(!table_set_accepted(&[
        (SurfaceRole::Playfield, SurfacePlan::Unsupported),
        (SurfaceRole::Score, SurfacePlan::Presenter(M::Fifo)),
    ]));
    // Unsupported secondary surfaces are skipped, not rejecting.
    assert!(table_set_accepted(&[
        (SurfaceRole::Playfield, SurfacePlan::Direct(M::Fifo)),
        (SurfaceRole::Score, SurfacePlan::Unsupported),
    ]));
    assert!(!table_set_accepted(&[]));
}

/// `table_set_accepted` over every combination of up to three surfaces:
/// accepted exactly when some surface is `Direct` and no playfield is
/// `Unsupported`.
#[test]
fn table_set_acceptance_sweep() {
    let plans = [
        SurfacePlan::Direct(M::Fifo),
        SurfacePlan::Direct(M::Mailbox),
        SurfacePlan::Presenter(M::Fifo),
        SurfacePlan::Presenter(M::Mailbox),
        SurfacePlan::Unsupported,
    ];
    let items: Vec<(SurfaceRole, SurfacePlan)> = ROLES
        .iter()
        .flat_map(|r| plans.iter().map(move |p| (*r, *p)))
        .collect();
    let mut sets: Vec<Vec<(SurfaceRole, SurfacePlan)>> = vec![vec![]];
    for a in &items {
        sets.push(vec![*a]);
        for b in &items {
            sets.push(vec![*a, *b]);
            for c in &items {
                sets.push(vec![*a, *b, *c]);
            }
        }
    }
    for set in &sets {
        let direct = set.iter().any(|(_, p)| matches!(p, SurfacePlan::Direct(_)));
        let bad_pf = set
            .iter()
            .any(|(r, p)| *r == SurfaceRole::Playfield && *p == SurfacePlan::Unsupported);
        assert_eq!(table_set_accepted(set), direct && !bad_pf, "{set:?}");
    }
}
