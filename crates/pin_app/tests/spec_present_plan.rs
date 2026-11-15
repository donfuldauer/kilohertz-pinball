//! PR #21 round 3 (CodeRabbit thread 4577325408): on the table path the
//! render thread never acquires or presents a blocking-mode backglass /
//! score surface. Pure, GPU-free and display-free checks of the public
//! presentation plan (`plan_surface`, `slot_decision`, `table_set_accepted`).

use pin_app::orchestrator::{
    SlotDecision, SurfacePlan, is_blocking_mode, plan_surface, slot_decision, table_set_accepted,
};
use pin_render::gpu::{SurfaceOptions, wgpu};
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
fn a_blocking_secondary_surface_is_never_drawn() {
    let mut checked = 0usize;
    for role in ROLES {
        for modes in mode_sets() {
            for formats in FORMAT_SETS {
                for (vrr, force) in [(false, false), (false, true), (true, false), (true, true)] {
                    let plan = plan_surface(role, formats, &modes, opts(vrr, force));
                    let d = slot_decision(role, plan, eight_bit(formats));
                    let ctx = format!("{role:?} {modes:?} {formats:?} vrr {vrr} force {force}");
                    checked += 1;
                    if role == SurfaceRole::Playfield {
                        match eight_bit(formats) {
                            Some(f) => assert!(
                                matches!(d, SlotDecision::Draw(m, g)
                                    if g == f && matches!(m, M::Fifo | M::FifoRelaxed)),
                                "{ctx}: {d:?}"
                            ),
                            None => assert_eq!(d, SlotDecision::Reject, "{ctx}"),
                        }
                        continue;
                    }
                    if eight_bit(formats).is_none() {
                        assert_eq!(d, SlotDecision::Skip, "{ctx}");
                        continue;
                    }
                    // A secondary surface is drawn exactly when it offers a
                    // non-blocking mode the policy uses (Mailbox / Immediate).
                    let usable_non_blocking =
                        modes.contains(&M::Mailbox) || modes.contains(&M::Immediate);
                    match d {
                        SlotDecision::Draw(m, _) => {
                            assert!(!is_blocking_mode(m), "{ctx}: drawn with {m:?}");
                            assert!(usable_non_blocking, "{ctx}");
                        }
                        SlotDecision::Defer(m) => {
                            assert!(is_blocking_mode(m), "{ctx}: deferred {m:?}");
                            assert!(!usable_non_blocking, "{ctx}");
                        }
                        other => panic!("{ctx}: {other:?}"),
                    }
                    // Fifo-only and FifoRelaxed-only windows, forced or not.
                    if modes == [M::Fifo] || modes == [M::FifoRelaxed] {
                        assert!(matches!(d, SlotDecision::Defer(_)), "{ctx}");
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
            assert_eq!(score, SurfacePlan::Deferred(M::Fifo));
            assert!(table_set_accepted(&[
                (SurfaceRole::Playfield, pf),
                (SurfaceRole::Score, score),
            ]));
            assert_eq!(
                slot_decision(SurfaceRole::Score, score, Some(formats[0])),
                SlotDecision::Defer(M::Fifo)
            );
            assert_eq!(
                slot_decision(SurfaceRole::Playfield, pf, Some(formats[0])),
                SlotDecision::Draw(M::Fifo, formats[0])
            );
        }
    }
    // A deferred backglass next to a deferred score window: still the table.
    assert!(table_set_accepted(&[
        (SurfaceRole::Playfield, SurfacePlan::Direct(M::Fifo)),
        (
            SurfaceRole::Backglass,
            SurfacePlan::Deferred(M::FifoRelaxed)
        ),
        (SurfaceRole::Score, SurfacePlan::Deferred(M::Fifo)),
    ]));
    // An unsupported playfield still rejects the set.
    assert!(!table_set_accepted(&[
        (SurfaceRole::Playfield, SurfacePlan::Unsupported),
        (SurfaceRole::Score, SurfacePlan::Direct(M::Mailbox)),
    ]));
}
