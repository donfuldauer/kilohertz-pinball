//! Mock-display tests for the topology planner (1-, 2- and 3-screen setups).

use super::*;
use proptest::prelude::*;

fn display(index: usize, x: i32, y: i32, w: u32, h: u32) -> DisplayInfo {
    DisplayInfo {
        index,
        bounds: Rect::new(x, y, w, h),
        refresh_hz: 60.0,
        content_scale: 1.0,
    }
}

/// Landscape 1080p, a portrait 4K playfield, a 1280×1024 backglass and a
/// 1280×320 score panel, side by side.
fn cabinet_displays() -> Vec<DisplayInfo> {
    vec![
        display(0, 0, 0, 2160, 3840),
        display(1, 2160, 0, 1280, 1024),
        display(2, 3440, 0, 1280, 320),
    ]
}

fn pip(corner: Corner, width_fraction: f32) -> PipPlacement {
    PipPlacement {
        corner,
        width_fraction,
    }
}

fn single(display: usize, backglass: PipPlacement, score: PipPlacement) -> TopologyMode {
    TopologyMode::SingleMonitor {
        display,
        backglass,
        score,
    }
}

#[test]
fn single_monitor_one_screen() {
    let displays = [display(0, 0, 0, 1920, 1080)];
    let plan = plan_topology(
        &displays,
        &single(
            0,
            pip(Corner::TopRight, 0.25),
            pip(Corner::BottomRight, 0.25),
        ),
    )
    .unwrap();
    assert_eq!(plan.windows.len(), 1);
    let w = &plan.windows[0];
    assert_eq!(w.display, 0);
    assert_eq!(w.bounds, Rect::new(0, 0, 1920, 1080));
    assert!(w.borderless_fullscreen);
    assert_eq!(w.primary_role(), Some(SurfaceRole::Playfield));
    assert_eq!(
        w.viewports.as_slice(),
        &[
            (SurfaceRole::Playfield, Rect::new(0, 0, 1920, 1080)),
            (SurfaceRole::Backglass, Rect::new(1440, 0, 480, 360)),
            (SurfaceRole::Score, Rect::new(1440, 960, 480, 120)),
        ]
    );
    assert_eq!(
        plan.find(SurfaceRole::Score),
        Some((0, Rect::new(1440, 960, 480, 120)))
    );
}

#[test]
fn single_monitor_on_second_screen_uses_local_coordinates() {
    let displays = [
        display(0, 0, 0, 1920, 1080),
        display(1, -1080, -200, 1080, 1920),
    ];
    let plan = plan_topology(
        &displays,
        &single(1, pip(Corner::TopLeft, 0.5), pip(Corner::BottomLeft, 1.0)),
    )
    .unwrap();
    let w = &plan.windows[0];
    assert_eq!(w.display, 1);
    assert_eq!(w.bounds, Rect::new(-1080, -200, 1080, 1920));
    assert_eq!(
        w.viewport(SurfaceRole::Playfield),
        Some(Rect::new(0, 0, 1080, 1920))
    );
    assert_eq!(
        w.viewport(SurfaceRole::Backglass),
        Some(Rect::new(0, 0, 540, 405))
    );
    assert_eq!(
        w.viewport(SurfaceRole::Score),
        Some(Rect::new(0, 1650, 1080, 270))
    );
}

#[test]
fn all_corners() {
    let (ww, wh) = (1000, 800);
    let p = |c| pip_rect(SurfaceRole::Backglass, &pip(c, 0.3), ww, wh).unwrap();
    assert_eq!(p(Corner::TopLeft), Rect::new(0, 0, 300, 225));
    assert_eq!(p(Corner::TopRight), Rect::new(700, 0, 300, 225));
    assert_eq!(p(Corner::BottomLeft), Rect::new(0, 575, 300, 225));
    assert_eq!(p(Corner::BottomRight), Rect::new(700, 575, 300, 225));
    // Rounding down: 0.333 × 1000 = 333 → 333 × 3 / 4 = 249.75 → 249.
    let r = pip_rect(SurfaceRole::Backglass, &pip(Corner::TopLeft, 0.333), ww, wh).unwrap();
    assert_eq!((r.w, r.h), (333, 249));
    let r = pip_rect(SurfaceRole::Score, &pip(Corner::TopLeft, 0.333), ww, wh).unwrap();
    assert_eq!((r.w, r.h), (333, 83));
}

#[test]
fn invalid_fractions() {
    for f in [
        0.0,
        -0.1,
        1.0001,
        2.0,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
    ] {
        let displays = [display(0, 0, 0, 1920, 1080)];
        assert_eq!(
            plan_topology(
                &displays,
                &single(0, pip(Corner::TopLeft, f), pip(Corner::BottomRight, 0.25))
            ),
            Err(TopologyError::InvalidPip(SurfaceRole::Backglass)),
            "backglass fraction {f}"
        );
        assert_eq!(
            plan_topology(
                &displays,
                &single(0, pip(Corner::TopLeft, 0.25), pip(Corner::BottomRight, f))
            ),
            Err(TopologyError::InvalidPip(SurfaceRole::Score)),
            "score fraction {f}"
        );
    }
}

#[test]
fn pip_must_fit_and_be_non_empty() {
    // Backglass at full width on a landscape window is 1440 tall > 1080.
    assert_eq!(
        pip_rect(
            SurfaceRole::Backglass,
            &pip(Corner::TopLeft, 1.0),
            1920,
            1080
        ),
        Err(TopologyError::InvalidPip(SurfaceRole::Backglass))
    );
    // Rounds to zero height.
    assert_eq!(
        pip_rect(SurfaceRole::Score, &pip(Corner::TopLeft, 0.001), 1920, 1080),
        Err(TopologyError::InvalidPip(SurfaceRole::Score))
    );
    assert_eq!(
        pip_rect(
            SurfaceRole::Playfield,
            &pip(Corner::TopLeft, 0.5),
            1920,
            1080
        ),
        Err(TopologyError::InvalidPip(SurfaceRole::Playfield))
    );
}

#[test]
fn overlapping_pips_rejected_touching_allowed() {
    let displays = [display(0, 0, 0, 1000, 1000)];
    // Both top-left: overlap.
    assert_eq!(
        plan_topology(
            &displays,
            &single(0, pip(Corner::TopLeft, 0.4), pip(Corner::TopLeft, 0.4))
        ),
        Err(TopologyError::InvalidPip(SurfaceRole::Score))
    );
    // Backglass 600×450 top-left, score 400×100 top-right: touch at x = 600.
    let plan = plan_topology(
        &displays,
        &single(0, pip(Corner::TopLeft, 0.6), pip(Corner::TopRight, 0.4)),
    )
    .unwrap();
    let w = &plan.windows[0];
    assert_eq!(
        w.viewport(SurfaceRole::Backglass),
        Some(Rect::new(0, 0, 600, 450))
    );
    assert_eq!(
        w.viewport(SurfaceRole::Score),
        Some(Rect::new(600, 0, 400, 100))
    );
    // 0.61 + 0.4 overlaps by 10 px.
    assert_eq!(
        plan_topology(
            &displays,
            &single(0, pip(Corner::TopLeft, 0.61), pip(Corner::TopRight, 0.4))
        ),
        Err(TopologyError::InvalidPip(SurfaceRole::Score))
    );
}

#[test]
fn cabinet_three_screens() {
    let displays = cabinet_displays();
    let plan = plan_topology(
        &displays,
        &TopologyMode::Cabinet {
            playfield: 0,
            backglass: Some(1),
            score: Some(2),
        },
    )
    .unwrap();
    assert_eq!(plan.windows.len(), 3);
    let roles = [
        SurfaceRole::Playfield,
        SurfaceRole::Backglass,
        SurfaceRole::Score,
    ];
    for (i, (w, role)) in plan.windows.iter().zip(roles).enumerate() {
        assert_eq!(w.display, i);
        assert_eq!(w.bounds, displays[i].bounds);
        assert!(w.borderless_fullscreen);
        assert_eq!(
            w.viewports.as_slice(),
            &[(
                role,
                Rect::new(0, 0, displays[i].bounds.w, displays[i].bounds.h)
            )]
        );
        assert_eq!(plan.find(role).map(|(i, _)| i), Some(i));
    }
}

#[test]
fn cabinet_two_screens_score_as_pip() {
    let displays = cabinet_displays();
    let plan = plan_topology(
        &displays,
        &TopologyMode::Cabinet {
            playfield: 0,
            backglass: Some(1),
            score: None,
        },
    )
    .unwrap();
    assert_eq!(plan.windows.len(), 2);
    let pf = &plan.windows[0];
    assert_eq!(
        pf.viewports.as_slice(),
        &[
            (SurfaceRole::Playfield, Rect::new(0, 0, 2160, 3840)),
            (SurfaceRole::Score, Rect::new(1620, 3705, 540, 135)),
        ]
    );
    assert_eq!(plan.windows[1].display, 1);
    assert_eq!(plan.windows[1].primary_role(), Some(SurfaceRole::Backglass));
}

#[test]
fn cabinet_two_screens_backglass_as_pip() {
    let displays = cabinet_displays();
    let plan = plan_topology(
        &displays,
        &TopologyMode::Cabinet {
            playfield: 0,
            backglass: None,
            score: Some(2),
        },
    )
    .unwrap();
    assert_eq!(plan.windows.len(), 2);
    assert_eq!(
        plan.windows[0].viewport(SurfaceRole::Backglass),
        Some(Rect::new(1620, 0, 540, 405))
    );
    assert_eq!(plan.windows[0].viewport(SurfaceRole::Score), None);
    assert_eq!(plan.windows[1].display, 2);
    assert_eq!(plan.windows[1].primary_role(), Some(SurfaceRole::Score));
}

#[test]
fn cabinet_one_screen_matches_default_single_monitor() {
    let displays = [display(0, 0, 0, 1920, 1080)];
    let cab = plan_topology(
        &displays,
        &TopologyMode::Cabinet {
            playfield: 0,
            backglass: None,
            score: None,
        },
    )
    .unwrap();
    let one = plan_topology(
        &displays,
        &single(0, DEFAULT_BACKGLASS_PIP, DEFAULT_SCORE_PIP),
    )
    .unwrap();
    assert_eq!(cab, one);
}

#[test]
fn errors() {
    let displays = cabinet_displays();
    let cab = |playfield, backglass, score| TopologyMode::Cabinet {
        playfield,
        backglass,
        score,
    };
    assert_eq!(
        plan_topology(&[], &cab(0, None, None)),
        Err(TopologyError::NoDisplays)
    );
    assert_eq!(
        plan_topology(&[], &single(0, DEFAULT_BACKGLASS_PIP, DEFAULT_SCORE_PIP)),
        Err(TopologyError::NoDisplays)
    );
    assert_eq!(
        plan_topology(
            &displays,
            &single(3, DEFAULT_BACKGLASS_PIP, DEFAULT_SCORE_PIP)
        ),
        Err(TopologyError::DisplayOutOfRange(3))
    );
    assert_eq!(
        plan_topology(&displays, &cab(5, None, None)),
        Err(TopologyError::DisplayOutOfRange(5))
    );
    assert_eq!(
        plan_topology(&displays, &cab(0, Some(9), None)),
        Err(TopologyError::DisplayOutOfRange(9))
    );
    assert_eq!(
        plan_topology(&displays, &cab(0, None, Some(7))),
        Err(TopologyError::DisplayOutOfRange(7))
    );
    assert_eq!(
        plan_topology(&displays, &cab(1, Some(1), None)),
        Err(TopologyError::DuplicateDisplay(1))
    );
    assert_eq!(
        plan_topology(&displays, &cab(0, None, Some(0))),
        Err(TopologyError::DuplicateDisplay(0))
    );
    assert_eq!(
        plan_topology(&displays, &cab(0, Some(2), Some(2))),
        Err(TopologyError::DuplicateDisplay(2))
    );
    // Default PiPs that do not fit a tiny playfield display.
    let tiny = [display(0, 0, 0, 3, 3)];
    assert_eq!(
        plan_topology(&tiny, &cab(0, None, None)),
        Err(TopologyError::InvalidPip(SurfaceRole::Backglass))
    );
    for e in [
        TopologyError::NoDisplays,
        TopologyError::DisplayOutOfRange(1),
        TopologyError::DuplicateDisplay(2),
        TopologyError::InvalidPip(SurfaceRole::Score),
    ] {
        assert!(!e.to_string().is_empty());
    }
}

#[test]
fn display_lookup_uses_index_field() {
    // Displays listed out of order: numbers refer to `DisplayInfo::index`.
    let displays = [display(2, 100, 0, 800, 600), display(0, 0, 0, 1920, 1080)];
    let plan = plan_topology(
        &displays,
        &TopologyMode::Cabinet {
            playfield: 0,
            backglass: Some(2),
            score: None,
        },
    )
    .unwrap();
    assert_eq!(plan.windows[0].bounds, Rect::new(0, 0, 1920, 1080));
    assert_eq!(plan.windows[1].bounds, Rect::new(100, 0, 800, 600));
}

#[test]
fn rect_helpers() {
    let a = Rect::new(i32::MAX - 1, 0, u32::MAX, 10);
    assert_eq!(a.right(), i64::from(i32::MAX) - 1 + i64::from(u32::MAX));
    let b = Rect::new(0, 0, 10, 10);
    assert!(b.overlaps(&Rect::new(9, 9, 5, 5)));
    assert!(!b.overlaps(&Rect::new(10, 0, 5, 5)));
    assert!(!b.overlaps(&Rect::new(0, 0, 0, 0)));
    assert!(b.contains(&Rect::new(0, 0, 10, 10)));
    assert!(!b.contains(&Rect::new(-1, 0, 5, 5)));
    assert!(!b.contains(&Rect::new(5, 5, 6, 1)));
}

fn corner() -> impl Strategy<Value = Corner> {
    prop_oneof![
        Just(Corner::TopLeft),
        Just(Corner::TopRight),
        Just(Corner::BottomLeft),
        Just(Corner::BottomRight)
    ]
}

proptest! {
    /// Every successful PiP is inside the window, anchored to its corner, has
    /// the floor-rounded width and the role's aspect.
    #[test]
    fn pip_rect_invariants(
        role in prop_oneof![Just(SurfaceRole::Backglass), Just(SurfaceRole::Score)],
        c in corner(),
        frac in 0.0f32..=1.0,
        ww in 1u32..10_000,
        wh in 1u32..10_000,
    ) {
        match pip_rect(role, &pip(c, frac), ww, wh) {
            Ok(r) => {
                prop_assert!(Rect::new(0, 0, ww, wh).contains(&r));
                prop_assert!(r.w > 0 && r.h > 0);
                prop_assert_eq!(r.w, (f64::from(frac) * f64::from(ww)).floor() as u32);
                let (aw, ah) = role.pip_aspect().unwrap();
                prop_assert_eq!(r.h, r.w * ah / aw);
                let left = matches!(c, Corner::TopLeft | Corner::BottomLeft);
                let top = matches!(c, Corner::TopLeft | Corner::TopRight);
                prop_assert_eq!(if left { r.x as i64 } else { r.right() }, if left { 0 } else { i64::from(ww) });
                prop_assert_eq!(if top { r.y as i64 } else { r.bottom() }, if top { 0 } else { i64::from(wh) });
            }
            Err(e) => {
                prop_assert_eq!(e, TopologyError::InvalidPip(role));
                let w = (f64::from(frac) * f64::from(ww)).floor() as u32;
                let (aw, ah) = role.pip_aspect().unwrap();
                let h = w * ah / aw;
                prop_assert!(frac <= 0.0 || w == 0 || h == 0 || h > wh);
            }
        }
    }

    /// Successful single-monitor plans: one window, PiPs inside, no overlap.
    #[test]
    fn single_monitor_invariants(
        x in -5000i32..5000, y in -5000i32..5000,
        ww in 1u32..8000, wh in 1u32..8000,
        bc in corner(), bf in 0.01f32..=1.0,
        sc in corner(), sf in 0.01f32..=1.0,
    ) {
        let displays = [display(0, x, y, ww, wh)];
        if let Ok(plan) = plan_topology(&displays, &single(0, pip(bc, bf), pip(sc, sf))) {
            prop_assert_eq!(plan.windows.len(), 1);
            let w = &plan.windows[0];
            prop_assert_eq!(w.bounds, displays[0].bounds);
            let full = Rect::new(0, 0, ww, wh);
            prop_assert_eq!(w.viewport(SurfaceRole::Playfield), Some(full));
            let bg = w.viewport(SurfaceRole::Backglass).unwrap();
            let s = w.viewport(SurfaceRole::Score).unwrap();
            prop_assert!(full.contains(&bg) && full.contains(&s));
            prop_assert!(!bg.overlaps(&s));
        }
    }

    /// Cabinet plans with distinct, existing displays always succeed on
    /// reasonably sized displays and give each role exactly one viewport.
    #[test]
    fn cabinet_invariants(bg in prop::option::of(1usize..3), sc in prop::option::of(1usize..3)) {
        let displays = cabinet_displays();
        let mode = TopologyMode::Cabinet { playfield: 0, backglass: bg, score: sc };
        let result = plan_topology(&displays, &mode);
        if bg.is_some() && bg == sc {
            prop_assert_eq!(result, Err(TopologyError::DuplicateDisplay(bg.unwrap())));
        } else {
            let plan = result.unwrap();
            prop_assert_eq!(plan.windows.len(), 1 + usize::from(bg.is_some()) + usize::from(sc.is_some()));
            for role in [SurfaceRole::Playfield, SurfaceRole::Backglass, SurfaceRole::Score] {
                let n: usize = plan.windows.iter().map(|w| w.viewports.iter().filter(|(r, _)| *r == role).count()).sum();
                prop_assert_eq!(n, 1);
            }
            for w in &plan.windows {
                prop_assert!(w.borderless_fullscreen);
                prop_assert_eq!(w.viewports[0].1, Rect::new(0, 0, w.bounds.w, w.bounds.h));
            }
        }
    }
}
