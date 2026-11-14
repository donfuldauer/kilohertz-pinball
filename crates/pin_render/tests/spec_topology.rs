//! Black-box spec tests for `pin_render::topology` (Subphase 1.3 brief).
//!
//! Rules under test (from the fixed public API):
//! - `SingleMonitor` -> exactly one borderless-fullscreen window covering the
//!   chosen display; Playfield viewport = full window; Backglass and Score are
//!   PiP rectangles inside the window, in window-local coordinates.
//! - PiP width = `floor(width_fraction * window_width)`; height from a fixed
//!   aspect (Backglass 4:3, Score 4:1); anchored to its corner with margin 0.
//! - Backglass and Score PiPs must not overlap (`Err` if they would).
//! - `width_fraction` must be finite and in `(0, 1]`, else `InvalidPip(role)`.
//! - `Cabinet` -> one borderless-fullscreen window per role that has a display,
//!   each with a single full-window viewport; a role without a display becomes
//!   a PiP on the playfield window with the defaults Backglass TopRight 0.25,
//!   Score BottomRight 0.25. Displays must be distinct (`DuplicateDisplay`).
//! - `NoDisplays`, `DisplayOutOfRange(i)`.
//!
//! The tests deliberately rely only on what the brief fixes: `Rect` is the
//! only type assumed to be `Copy`/`Debug`/`PartialEq`; window and viewport
//! ordering is not assumed; PiP height may be rounded either way (within one
//! pixel of the exact aspect). Displays are always built with
//! `DisplayInfo::index == position in the slice`, so the tests hold whichever
//! of the two a plan's `display` field refers to.

use pin_render::topology::{
    Corner, DisplayInfo, PipPlacement, Rect, SurfaceRole, TopologyError, TopologyMode,
    TopologyPlan, WindowPlan, plan_topology,
};
use proptest::prelude::*;

// ---------------------------------------------------------------------------
// Helpers (test-side mirrors of the non-`Copy` API enums)
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Role {
    Playfield,
    Backglass,
    Score,
}

impl Role {
    fn of(role: &SurfaceRole) -> Role {
        match role {
            SurfaceRole::Playfield => Role::Playfield,
            SurfaceRole::Backglass => Role::Backglass,
            SurfaceRole::Score => Role::Score,
        }
    }

    /// Aspect as (width units, height units).
    fn aspect(self) -> (u64, u64) {
        match self {
            Role::Backglass => (4, 3),
            Role::Score => (4, 1),
            Role::Playfield => panic!("playfield has no PiP aspect"),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Anchor {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

const ANCHORS: [Anchor; 4] = [
    Anchor::TopLeft,
    Anchor::TopRight,
    Anchor::BottomLeft,
    Anchor::BottomRight,
];

impl Anchor {
    fn corner(self) -> Corner {
        match self {
            Anchor::TopLeft => Corner::TopLeft,
            Anchor::TopRight => Corner::TopRight,
            Anchor::BottomLeft => Corner::BottomLeft,
            Anchor::BottomRight => Corner::BottomRight,
        }
    }

    fn is_left(self) -> bool {
        matches!(self, Anchor::TopLeft | Anchor::BottomLeft)
    }

    fn is_top(self) -> bool {
        matches!(self, Anchor::TopLeft | Anchor::TopRight)
    }

    /// Where a `w x h` PiP anchored here sits in a `win_w x win_h` window.
    fn place(self, win_w: u32, win_h: u32, w: u32, h: u32) -> (i64, i64) {
        let x = if self.is_left() {
            0
        } else {
            win_w as i64 - w as i64
        };
        let y = if self.is_top() {
            0
        } else {
            win_h as i64 - h as i64
        };
        (x, y)
    }
}

fn rect(x: i32, y: i32, w: u32, h: u32) -> Rect {
    Rect { x, y, w, h }
}

fn display(index: usize, bounds: Rect) -> DisplayInfo {
    DisplayInfo {
        index,
        bounds,
        refresh_hz: 60.0,
        content_scale: 1.0,
    }
}

fn pip(anchor: Anchor, width_fraction: f32) -> PipPlacement {
    PipPlacement {
        corner: anchor.corner(),
        width_fraction,
    }
}

fn single(display: usize, bg: (Anchor, f32), score: (Anchor, f32)) -> TopologyMode {
    TopologyMode::SingleMonitor {
        display,
        backglass: pip(bg.0, bg.1),
        score: pip(score.0, score.1),
    }
}

fn cabinet(playfield: usize, backglass: Option<usize>, score: Option<usize>) -> TopologyMode {
    TopologyMode::Cabinet {
        playfield,
        backglass,
        score,
    }
}

/// One 1920x1080 display at the origin.
fn one_fhd() -> Vec<DisplayInfo> {
    vec![display(0, rect(0, 0, 1920, 1080))]
}

/// A typical three-screen cabinet: 4K playfield, FHD backglass to its right,
/// a 4:1 score display to its left (negative desktop coordinates).
fn cabinet_displays() -> Vec<DisplayInfo> {
    vec![
        display(0, rect(0, 0, 3840, 2160)),
        display(1, rect(3840, 0, 1920, 1080)),
        display(2, rect(-1280, 200, 1280, 320)),
    ]
}

fn ok(result: Result<TopologyPlan, TopologyError>) -> TopologyPlan {
    match result {
        Ok(plan) => plan,
        Err(e) => panic!("expected a plan, got error {}", describe(&e)),
    }
}

fn err(result: Result<TopologyPlan, TopologyError>) -> TopologyError {
    match result {
        Ok(_) => panic!("expected an error, got a plan"),
        Err(e) => e,
    }
}

fn describe(e: &TopologyError) -> String {
    match e {
        TopologyError::NoDisplays => "NoDisplays".to_string(),
        TopologyError::DisplayOutOfRange(i) => format!("DisplayOutOfRange({i})"),
        TopologyError::DuplicateDisplay(i) => format!("DuplicateDisplay({i})"),
        TopologyError::InvalidPip(r) => format!("InvalidPip({:?})", Role::of(r)),
    }
}

fn full(win: &WindowPlan) -> Rect {
    rect(0, 0, win.bounds.w, win.bounds.h)
}

/// The viewport for `role` in `win`; panics if absent or present twice.
fn viewport(win: &WindowPlan, role: Role) -> Rect {
    let found: Vec<Rect> = win
        .viewports
        .iter()
        .filter(|(r, _)| Role::of(r) == role)
        .map(|(_, rc)| *rc)
        .collect();
    assert_eq!(
        found.len(),
        1,
        "window on display {} must have exactly one {role:?} viewport",
        win.display
    );
    found[0]
}

fn roles(win: &WindowPlan) -> Vec<Role> {
    let mut v: Vec<Role> = win.viewports.iter().map(|(r, _)| Role::of(r)).collect();
    v.sort();
    v
}

/// The single window whose viewports include `role`.
fn window_with(plan: &TopologyPlan, role: Role) -> &WindowPlan {
    let found: Vec<&WindowPlan> = plan
        .windows
        .iter()
        .filter(|w| w.viewports.iter().any(|(r, _)| Role::of(r) == role))
        .collect();
    assert_eq!(found.len(), 1, "exactly one window must show {role:?}");
    found[0]
}

fn overlaps(a: Rect, b: Rect) -> bool {
    let (ax0, ay0) = (a.x as i64, a.y as i64);
    let (ax1, ay1) = (ax0 + a.w as i64, ay0 + a.h as i64);
    let (bx0, by0) = (b.x as i64, b.y as i64);
    let (bx1, by1) = (bx0 + b.w as i64, by0 + b.h as i64);
    ax0 < bx1 && bx0 < ax1 && ay0 < by1 && by0 < ay1
}

fn contained(inner: Rect, win_w: u32, win_h: u32) -> bool {
    inner.x >= 0
        && inner.y >= 0
        && inner.x as i64 + inner.w as i64 <= win_w as i64
        && inner.y as i64 + inner.h as i64 <= win_h as i64
}

/// `h` is within one pixel of the exact aspect height for width `w`.
fn aspect_holds(role: Role, w: u32, h: u32) -> bool {
    let (aw, ah) = role.aspect();
    // |h - w*ah/aw| < 1  <=>  |h*aw - w*ah| < aw
    (h as i64 * aw as i64 - w as i64 * ah as i64).unsigned_abs() < aw
}

/// Full PiP contract for one viewport: exact width, aspect, anchoring,
/// non-degenerate, contained.
fn assert_pip(vp: Rect, role: Role, anchor: Anchor, win_w: u32, win_h: u32, expect_w: u32) {
    assert_eq!(vp.w, expect_w, "{role:?} PiP width in {win_w}x{win_h}");
    assert!(vp.h > 0, "{role:?} PiP must have a height");
    assert!(
        aspect_holds(role, vp.w, vp.h),
        "{role:?} PiP {}x{} violates its {:?} aspect",
        vp.w,
        vp.h,
        role.aspect()
    );
    let (x, y) = anchor.place(win_w, win_h, vp.w, vp.h);
    assert_eq!(
        (vp.x as i64, vp.y as i64),
        (x, y),
        "{role:?} PiP not flush with {anchor:?}: {vp:?}"
    );
    assert!(
        contained(vp, win_w, win_h),
        "{role:?} PiP {vp:?} leaves the {win_w}x{win_h} window"
    );
}

/// Invariants every successful plan must satisfy, whatever the mode.
fn assert_plan_invariants(plan: &TopologyPlan, displays: &[DisplayInfo]) {
    assert!(!plan.windows.is_empty() && plan.windows.len() <= 3);
    let mut seen_displays = Vec::new();
    let mut seen_roles = Vec::new();
    for win in plan.windows.iter() {
        assert!(
            win.borderless_fullscreen,
            "every planned window is borderless fullscreen"
        );
        assert!(win.display < displays.len());
        assert_eq!(
            win.bounds, displays[win.display].bounds,
            "window covers its display"
        );
        assert!(
            !seen_displays.contains(&win.display),
            "two windows on display {}",
            win.display
        );
        seen_displays.push(win.display);
        assert!(!win.viewports.is_empty(), "a window with nothing to show");
        for (role, vp) in win.viewports.iter() {
            let role = Role::of(role);
            assert!(!seen_roles.contains(&role), "{role:?} is shown twice");
            seen_roles.push(role);
            assert!(
                vp.w > 0 && vp.h > 0,
                "{role:?} viewport is degenerate: {vp:?}"
            );
            assert!(
                contained(*vp, win.bounds.w, win.bounds.h),
                "{role:?} viewport {vp:?} is outside its {}x{} window (window-local coordinates)",
                win.bounds.w,
                win.bounds.h
            );
        }
    }
    seen_roles.sort();
    assert_eq!(
        seen_roles,
        vec![Role::Playfield, Role::Backglass, Role::Score],
        "all three surfaces are shown exactly once"
    );
    let pf_win = window_with(plan, Role::Playfield);
    assert_eq!(
        viewport(pf_win, Role::Playfield),
        full(pf_win),
        "playfield fills its window"
    );
    // PiPs sharing a window never overlap each other.
    for win in plan.windows.iter() {
        let r = roles(win);
        if r.contains(&Role::Backglass) && r.contains(&Role::Score) {
            let (b, s) = (viewport(win, Role::Backglass), viewport(win, Role::Score));
            assert!(!overlaps(b, s), "Backglass {b:?} overlaps Score {s:?}");
        }
    }
}

type WindowSummary = (usize, Rect, bool, Vec<(Role, Rect)>);

/// Order-independent, comparable summary of a plan.
fn summary(plan: &TopologyPlan) -> Vec<WindowSummary> {
    let mut out: Vec<_> = plan
        .windows
        .iter()
        .map(|w| {
            let mut vps: Vec<(Role, Rect)> = w
                .viewports
                .iter()
                .map(|(r, rc)| (Role::of(r), *rc))
                .collect();
            vps.sort_by_key(|(r, _)| *r);
            (w.display, w.bounds, w.borderless_fullscreen, vps)
        })
        .collect();
    out.sort_by_key(|(d, ..)| *d);
    out
}

// ---------------------------------------------------------------------------
// Single-monitor mode (1 screen)
// ---------------------------------------------------------------------------

#[test]
fn single_monitor_fhd_default_like_layout() {
    let displays = one_fhd();
    let plan = ok(plan_topology(
        &displays,
        &single(0, (Anchor::TopRight, 0.25), (Anchor::BottomRight, 0.25)),
    ));
    assert_eq!(plan.windows.len(), 1);
    let win = &plan.windows[0];
    assert_eq!(win.display, 0);
    assert_eq!(win.bounds, rect(0, 0, 1920, 1080));
    assert!(win.borderless_fullscreen);
    assert_eq!(win.viewports.len(), 3);
    assert_eq!(viewport(win, Role::Playfield), rect(0, 0, 1920, 1080));
    // 0.25 * 1920 = 480; 480 at 4:3 = 360; 480 at 4:1 = 120.
    assert_eq!(viewport(win, Role::Backglass), rect(1440, 0, 480, 360));
    assert_eq!(viewport(win, Role::Score), rect(1440, 960, 480, 120));
    assert_plan_invariants(&plan, &displays);
}

#[test]
fn single_monitor_backglass_in_every_corner() {
    let displays = one_fhd();
    // Score sits in a different corner each time, small enough never to clash.
    let cases = [
        (Anchor::TopLeft, rect(0, 0, 480, 360), Anchor::BottomRight),
        (
            Anchor::TopRight,
            rect(1440, 0, 480, 360),
            Anchor::BottomLeft,
        ),
        (Anchor::BottomLeft, rect(0, 720, 480, 360), Anchor::TopRight),
        (
            Anchor::BottomRight,
            rect(1440, 720, 480, 360),
            Anchor::TopLeft,
        ),
    ];
    for (anchor, want, score_anchor) in cases {
        let plan = ok(plan_topology(
            &displays,
            &single(0, (anchor, 0.25), (score_anchor, 0.125)),
        ));
        let win = &plan.windows[0];
        assert_eq!(
            viewport(win, Role::Backglass),
            want,
            "backglass at {anchor:?}"
        );
        assert_pip(
            viewport(win, Role::Score),
            Role::Score,
            score_anchor,
            1920,
            1080,
            240,
        );
        assert_plan_invariants(&plan, &displays);
    }
}

#[test]
fn single_monitor_score_in_every_corner() {
    let displays = one_fhd();
    let cases = [
        (Anchor::TopLeft, rect(0, 0, 960, 240), Anchor::BottomRight),
        (Anchor::TopRight, rect(960, 0, 960, 240), Anchor::BottomLeft),
        (Anchor::BottomLeft, rect(0, 840, 960, 240), Anchor::TopRight),
        (
            Anchor::BottomRight,
            rect(960, 840, 960, 240),
            Anchor::TopLeft,
        ),
    ];
    for (anchor, want, bg_anchor) in cases {
        let plan = ok(plan_topology(
            &displays,
            &single(0, (bg_anchor, 0.25), (anchor, 0.5)),
        ));
        let win = &plan.windows[0];
        assert_eq!(viewport(win, Role::Score), want, "score at {anchor:?}");
        assert_pip(
            viewport(win, Role::Backglass),
            Role::Backglass,
            bg_anchor,
            1920,
            1080,
            480,
        );
        assert_plan_invariants(&plan, &displays);
    }
}

#[test]
fn pip_aspects_are_4_3_and_4_1() {
    let displays = vec![display(0, rect(0, 0, 1600, 1200))];
    let plan = ok(plan_topology(
        &displays,
        &single(0, (Anchor::TopLeft, 0.5), (Anchor::BottomRight, 0.5)),
    ));
    let win = &plan.windows[0];
    let b = viewport(win, Role::Backglass);
    let s = viewport(win, Role::Score);
    assert_eq!((b.w, b.h), (800, 600));
    assert_eq!(b.w * 3, b.h * 4);
    assert_eq!((s.w, s.h), (800, 200));
    assert_eq!(s.w, s.h * 4);
}

#[test]
fn pip_width_is_rounded_down() {
    // 0.5 * 1001 = 500.5 -> 500 (exact in binary floating point, so the
    // expectation does not depend on f32 vs f64 arithmetic).
    let displays = vec![display(0, rect(0, 0, 1001, 1000))];
    let plan = ok(plan_topology(
        &displays,
        &single(0, (Anchor::TopRight, 0.5), (Anchor::BottomRight, 0.5)),
    ));
    let win = &plan.windows[0];
    let b = viewport(win, Role::Backglass);
    let s = viewport(win, Role::Score);
    assert_eq!(b, rect(501, 0, 500, 375));
    assert_eq!(s, rect(501, 875, 500, 125));

    // 0.75 * 1003 = 752.25 -> 752; 0.125 * 1003 = 125.375 -> 125.
    let displays = vec![display(0, rect(0, 0, 1003, 2000))];
    let plan = ok(plan_topology(
        &displays,
        &single(0, (Anchor::BottomLeft, 0.75), (Anchor::TopRight, 0.125)),
    ));
    let win = &plan.windows[0];
    assert_eq!(
        viewport(win, Role::Backglass),
        rect(0, 2000 - 564, 752, 564)
    );
    assert_pip(
        viewport(win, Role::Score),
        Role::Score,
        Anchor::TopRight,
        1003,
        2000,
        125,
    );
}

#[test]
fn width_fraction_of_exactly_one_is_valid() {
    // Score across the full width at the bottom, small backglass at the top.
    let displays = one_fhd();
    let plan = ok(plan_topology(
        &displays,
        &single(0, (Anchor::TopLeft, 0.25), (Anchor::BottomLeft, 1.0)),
    ));
    let win = &plan.windows[0];
    assert_eq!(viewport(win, Role::Score), rect(0, 600, 1920, 480));
    assert_eq!(viewport(win, Role::Backglass), rect(0, 0, 480, 360));

    // Backglass across the full width of a portrait display.
    let displays = vec![display(0, rect(0, 0, 1080, 1920))];
    let plan = ok(plan_topology(
        &displays,
        &single(0, (Anchor::TopRight, 1.0), (Anchor::BottomRight, 1.0)),
    ));
    let win = &plan.windows[0];
    assert_eq!(viewport(win, Role::Backglass), rect(0, 0, 1080, 810));
    assert_eq!(viewport(win, Role::Score), rect(0, 1650, 1080, 270));
    assert_plan_invariants(&plan, &displays);
}

#[test]
fn single_monitor_viewports_are_window_local_not_desktop_coordinates() {
    // The chosen display sits at a far-away, partly negative desktop position.
    let displays = vec![
        display(0, rect(0, 0, 1920, 1080)),
        display(1, rect(-2560, -300, 2560, 1440)),
        display(2, rect(1920, 0, 1280, 1024)),
    ];
    let plan = ok(plan_topology(
        &displays,
        &single(1, (Anchor::TopRight, 0.25), (Anchor::BottomLeft, 0.25)),
    ));
    assert_eq!(plan.windows.len(), 1);
    let win = &plan.windows[0];
    assert_eq!(win.display, 1);
    assert_eq!(win.bounds, rect(-2560, -300, 2560, 1440));
    assert!(win.borderless_fullscreen);
    assert_eq!(viewport(win, Role::Playfield), rect(0, 0, 2560, 1440));
    assert_eq!(viewport(win, Role::Backglass), rect(1920, 0, 640, 480));
    assert_eq!(viewport(win, Role::Score), rect(0, 1280, 640, 160));
    assert_plan_invariants(&plan, &displays);
}

#[test]
fn single_monitor_uses_only_the_selected_display() {
    let displays = cabinet_displays();
    for chosen in 0..2 {
        let plan = ok(plan_topology(
            &displays,
            &single(chosen, (Anchor::TopLeft, 0.25), (Anchor::BottomRight, 0.25)),
        ));
        assert_eq!(
            plan.windows.len(),
            1,
            "single-monitor mode never opens a second window"
        );
        assert_eq!(plan.windows[0].display, chosen);
        assert_eq!(plan.windows[0].bounds, displays[chosen].bounds);
        assert_eq!(
            roles(&plan.windows[0]),
            vec![Role::Playfield, Role::Backglass, Role::Score]
        );
    }
}

#[test]
fn pips_that_only_touch_do_not_count_as_overlapping() {
    let displays = one_fhd();
    // Side by side along the top edge: [0, 960) and [960, 1920).
    let plan = ok(plan_topology(
        &displays,
        &single(0, (Anchor::TopLeft, 0.5), (Anchor::TopRight, 0.5)),
    ));
    let win = &plan.windows[0];
    assert_eq!(viewport(win, Role::Backglass), rect(0, 0, 960, 720));
    assert_eq!(viewport(win, Role::Score), rect(960, 0, 960, 240));

    // Stacked full-width in a 1600x1600 window: backglass 1600x1200 on top,
    // score 1600x400 below, sharing one edge.
    let displays = vec![display(0, rect(0, 0, 1600, 1600))];
    let plan = ok(plan_topology(
        &displays,
        &single(0, (Anchor::TopRight, 1.0), (Anchor::BottomRight, 1.0)),
    ));
    let win = &plan.windows[0];
    assert_eq!(viewport(win, Role::Backglass), rect(0, 0, 1600, 1200));
    assert_eq!(viewport(win, Role::Score), rect(0, 1200, 1600, 400));
}

#[test]
fn same_corner_pips_are_rejected() {
    // Same corner always overlaps (both rectangles contain the corner pixel),
    // so every corner must be rejected.
    let displays = one_fhd();
    for anchor in ANCHORS {
        let e = err(plan_topology(
            &displays,
            &single(0, (anchor, 0.25), (anchor, 0.25)),
        ));
        assert!(
            matches!(e, TopologyError::InvalidPip(_)),
            "both PiPs in {anchor:?} must be InvalidPip, got {}",
            describe(&e)
        );
    }
}

#[test]
fn overlapping_pips_in_different_corners_are_rejected() {
    let displays = one_fhd();
    // Horizontal clash along the top: 0.75 + 0.5 > 1 and both start at y = 0.
    let e = err(plan_topology(
        &displays,
        &single(0, (Anchor::TopLeft, 0.5), (Anchor::TopRight, 0.75)),
    ));
    assert!(
        matches!(e, TopologyError::InvalidPip(_)),
        "got {}",
        describe(&e)
    );
    // Vertical clash in the right column: 720 + 480 > 1080.
    let e = err(plan_topology(
        &displays,
        &single(0, (Anchor::TopRight, 0.5), (Anchor::BottomRight, 1.0)),
    ));
    assert!(
        matches!(e, TopologyError::InvalidPip(_)),
        "got {}",
        describe(&e)
    );
    // Diagonal clash: 1152x864 from the top-left meets 1920x480 from the bottom-right.
    let e = err(plan_topology(
        &displays,
        &single(0, (Anchor::TopLeft, 0.6), (Anchor::BottomRight, 1.0)),
    ));
    assert!(
        matches!(e, TopologyError::InvalidPip(_)),
        "got {}",
        describe(&e)
    );
}

#[test]
fn a_pip_that_cannot_fit_inside_the_window_is_never_planned() {
    // A full-width 4:3 backglass on a 16:9 window would be 1440 px tall in a
    // 1080 px window. PiPs must be inside the window, so this cannot succeed.
    let displays = one_fhd();
    let e = err(plan_topology(
        &displays,
        &single(0, (Anchor::TopLeft, 1.0), (Anchor::BottomRight, 0.125)),
    ));
    assert!(
        matches!(e, TopologyError::InvalidPip(_)),
        "got {}",
        describe(&e)
    );

    // A full-width 4:1 score strip on an 8:1 bar display: 3840/4 = 960 > 480.
    let displays = vec![display(0, rect(0, 0, 3840, 480))];
    let e = err(plan_topology(
        &displays,
        &single(0, (Anchor::TopLeft, 0.0625), (Anchor::BottomRight, 1.0)),
    ));
    assert!(
        matches!(e, TopologyError::InvalidPip(_)),
        "got {}",
        describe(&e)
    );
}

#[test]
fn refresh_rate_and_content_scale_do_not_change_the_geometry() {
    let base = one_fhd();
    let exotic = vec![DisplayInfo {
        index: 0,
        bounds: rect(0, 0, 1920, 1080),
        refresh_hz: 239.76,
        content_scale: 2.5,
    }];
    let mode = || single(0, (Anchor::TopRight, 0.25), (Anchor::BottomLeft, 0.375));
    assert_eq!(
        summary(&ok(plan_topology(&base, &mode()))),
        summary(&ok(plan_topology(&exotic, &mode())))
    );
    assert_eq!(
        summary(&ok(plan_topology(&base, &cabinet(0, None, None)))),
        summary(&ok(plan_topology(&exotic, &cabinet(0, None, None))))
    );
}

#[test]
fn planning_is_deterministic() {
    let displays = cabinet_displays();
    for mode in [
        || single(0, (Anchor::TopRight, 0.25), (Anchor::BottomLeft, 0.25)),
        || cabinet(0, Some(1), Some(2)),
        || cabinet(1, None, Some(0)),
    ] {
        let a = summary(&ok(plan_topology(&displays, &mode())));
        let b = summary(&ok(plan_topology(&displays, &mode())));
        assert_eq!(a, b);
    }
}

// ---------------------------------------------------------------------------
// Cabinet mode (1, 2 and 3 screens)
// ---------------------------------------------------------------------------

#[test]
fn cabinet_three_screens_one_full_window_per_role() {
    let displays = cabinet_displays();
    let plan = ok(plan_topology(&displays, &cabinet(0, Some(1), Some(2))));
    assert_eq!(plan.windows.len(), 3);
    for (role, disp) in [
        (Role::Playfield, 0usize),
        (Role::Backglass, 1),
        (Role::Score, 2),
    ] {
        let win = window_with(&plan, role);
        assert_eq!(win.display, disp, "{role:?} window display");
        assert_eq!(win.bounds, displays[disp].bounds, "{role:?} window bounds");
        assert!(win.borderless_fullscreen);
        assert_eq!(
            roles(win),
            vec![role],
            "{role:?} window shows only its own surface"
        );
        assert_eq!(
            viewport(win, role),
            full(win),
            "{role:?} viewport is the full window"
        );
    }
    // Spot-check the absolute numbers, including the negative-origin display.
    assert_eq!(
        window_with(&plan, Role::Score).bounds,
        rect(-1280, 200, 1280, 320)
    );
    assert_eq!(
        viewport(window_with(&plan, Role::Score), Role::Score),
        rect(0, 0, 1280, 320)
    );
    assert_eq!(
        viewport(window_with(&plan, Role::Backglass), Role::Backglass),
        rect(0, 0, 1920, 1080)
    );
    assert_eq!(
        viewport(window_with(&plan, Role::Playfield), Role::Playfield),
        rect(0, 0, 3840, 2160)
    );
    assert_plan_invariants(&plan, &displays);
}

#[test]
fn cabinet_three_screens_every_role_assignment() {
    let displays = cabinet_displays();
    let perms = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    for [p, b, s] in perms {
        let plan = ok(plan_topology(&displays, &cabinet(p, Some(b), Some(s))));
        assert_eq!(plan.windows.len(), 3);
        for (role, disp) in [(Role::Playfield, p), (Role::Backglass, b), (Role::Score, s)] {
            let win = window_with(&plan, role);
            assert_eq!(win.display, disp);
            assert_eq!(win.bounds, displays[disp].bounds);
            assert_eq!(roles(win), vec![role]);
            assert_eq!(viewport(win, role), full(win));
        }
        assert_plan_invariants(&plan, &displays);
    }
}

#[test]
fn cabinet_two_screens_backglass_window_score_as_default_pip() {
    let displays = vec![
        display(0, rect(0, 0, 1920, 1080)),
        display(1, rect(1920, 0, 1600, 1200)),
    ];
    let plan = ok(plan_topology(&displays, &cabinet(0, Some(1), None)));
    assert_eq!(plan.windows.len(), 2);

    let bg = window_with(&plan, Role::Backglass);
    assert_eq!(bg.display, 1);
    assert_eq!(bg.bounds, rect(1920, 0, 1600, 1200));
    assert!(bg.borderless_fullscreen);
    assert_eq!(roles(bg), vec![Role::Backglass]);
    assert_eq!(viewport(bg, Role::Backglass), rect(0, 0, 1600, 1200));

    let pf = window_with(&plan, Role::Playfield);
    assert_eq!(pf.display, 0);
    assert_eq!(pf.bounds, rect(0, 0, 1920, 1080));
    assert!(pf.borderless_fullscreen);
    assert_eq!(roles(pf), vec![Role::Playfield, Role::Score]);
    assert_eq!(viewport(pf, Role::Playfield), rect(0, 0, 1920, 1080));
    // Default Score PiP: BottomRight, 0.25 of the width, 4:1.
    assert_eq!(viewport(pf, Role::Score), rect(1440, 960, 480, 120));
    assert_plan_invariants(&plan, &displays);
}

#[test]
fn cabinet_two_screens_score_window_backglass_as_default_pip() {
    let displays = vec![
        display(0, rect(0, 0, 1920, 1080)),
        display(1, rect(0, 1080, 1920, 480)),
    ];
    let plan = ok(plan_topology(&displays, &cabinet(0, None, Some(1))));
    assert_eq!(plan.windows.len(), 2);

    let sc = window_with(&plan, Role::Score);
    assert_eq!(sc.display, 1);
    assert_eq!(sc.bounds, rect(0, 1080, 1920, 480));
    assert_eq!(roles(sc), vec![Role::Score]);
    assert_eq!(viewport(sc, Role::Score), rect(0, 0, 1920, 480));

    let pf = window_with(&plan, Role::Playfield);
    assert_eq!(pf.display, 0);
    assert_eq!(roles(pf), vec![Role::Playfield, Role::Backglass]);
    assert_eq!(viewport(pf, Role::Playfield), rect(0, 0, 1920, 1080));
    // Default Backglass PiP: TopRight, 0.25 of the width, 4:3.
    assert_eq!(viewport(pf, Role::Backglass), rect(1440, 0, 480, 360));
    assert_plan_invariants(&plan, &displays);
}

#[test]
fn cabinet_two_screens_playfield_on_the_second_display() {
    // Portrait playfield as display 1, landscape backglass as display 0.
    let displays = vec![
        display(0, rect(0, 0, 1920, 1080)),
        display(1, rect(1920, -420, 1080, 1920)),
    ];
    let plan = ok(plan_topology(&displays, &cabinet(1, Some(0), None)));
    assert_eq!(plan.windows.len(), 2);
    let pf = window_with(&plan, Role::Playfield);
    assert_eq!(pf.display, 1);
    assert_eq!(pf.bounds, rect(1920, -420, 1080, 1920));
    assert_eq!(viewport(pf, Role::Playfield), rect(0, 0, 1080, 1920));
    // 0.25 * 1080 = 270; 270 at 4:1 = 67.5 (rounding direction is not fixed).
    assert_pip(
        viewport(pf, Role::Score),
        Role::Score,
        Anchor::BottomRight,
        1080,
        1920,
        270,
    );
    let bg = window_with(&plan, Role::Backglass);
    assert_eq!(bg.display, 0);
    assert_eq!(viewport(bg, Role::Backglass), rect(0, 0, 1920, 1080));
    assert_plan_invariants(&plan, &displays);
}

#[test]
fn cabinet_one_screen_falls_back_to_both_default_pips() {
    let displays = one_fhd();
    let plan = ok(plan_topology(&displays, &cabinet(0, None, None)));
    assert_eq!(plan.windows.len(), 1);
    let win = &plan.windows[0];
    assert_eq!(win.display, 0);
    assert_eq!(win.bounds, rect(0, 0, 1920, 1080));
    assert!(win.borderless_fullscreen);
    assert_eq!(
        roles(win),
        vec![Role::Playfield, Role::Backglass, Role::Score]
    );
    assert_eq!(viewport(win, Role::Playfield), rect(0, 0, 1920, 1080));
    assert_eq!(viewport(win, Role::Backglass), rect(1440, 0, 480, 360));
    assert_eq!(viewport(win, Role::Score), rect(1440, 960, 480, 120));
    assert_plan_invariants(&plan, &displays);
}

#[test]
fn cabinet_defaults_equal_the_documented_single_monitor_placements() {
    // "Absent role -> PiP on the playfield window with defaults
    //  Backglass TopRight 0.25, Score BottomRight 0.25."
    for bounds in [
        rect(0, 0, 1920, 1080),
        rect(-100, 50, 2560, 1440),
        rect(0, 0, 1080, 1920),
    ] {
        let displays = vec![display(0, bounds)];
        let via_cabinet = ok(plan_topology(&displays, &cabinet(0, None, None)));
        let via_single = ok(plan_topology(
            &displays,
            &single(0, (Anchor::TopRight, 0.25), (Anchor::BottomRight, 0.25)),
        ));
        assert_eq!(
            summary(&via_cabinet),
            summary(&via_single),
            "bounds {bounds:?}"
        );
    }
}

#[test]
fn cabinet_ignores_displays_it_was_not_given() {
    // Three displays attached, but only the playfield is assigned: one window.
    let displays = cabinet_displays();
    let plan = ok(plan_topology(&displays, &cabinet(1, None, None)));
    assert_eq!(plan.windows.len(), 1);
    assert_eq!(plan.windows[0].display, 1);
    assert_eq!(plan.windows[0].bounds, rect(3840, 0, 1920, 1080));
    assert_eq!(
        viewport(&plan.windows[0], Role::Backglass),
        rect(1440, 0, 480, 360)
    );
    assert_eq!(
        viewport(&plan.windows[0], Role::Score),
        rect(1440, 960, 480, 120)
    );
}

// ---------------------------------------------------------------------------
// Errors: every variant
// ---------------------------------------------------------------------------

#[test]
fn no_displays_single_monitor() {
    let e = err(plan_topology(
        &[],
        &single(0, (Anchor::TopRight, 0.25), (Anchor::BottomRight, 0.25)),
    ));
    assert!(
        matches!(e, TopologyError::NoDisplays),
        "got {}",
        describe(&e)
    );
}

#[test]
fn no_displays_cabinet() {
    for mode in [cabinet(0, None, None), cabinet(0, Some(1), Some(2))] {
        let e = err(plan_topology(&[], &mode));
        assert!(
            matches!(e, TopologyError::NoDisplays),
            "got {}",
            describe(&e)
        );
    }
}

#[test]
fn display_out_of_range_single_monitor() {
    let displays = one_fhd();
    for bad in [1usize, 2, 17, usize::MAX] {
        let e = err(plan_topology(
            &displays,
            &single(bad, (Anchor::TopRight, 0.25), (Anchor::BottomRight, 0.25)),
        ));
        assert!(
            matches!(e, TopologyError::DisplayOutOfRange(i) if i == bad),
            "display {bad}: got {}",
            describe(&e)
        );
    }
}

#[test]
fn display_out_of_range_cabinet_names_the_offending_index() {
    let displays = cabinet_displays();
    let cases = [
        (cabinet(3, None, None), 3usize),
        (cabinet(3, Some(0), Some(1)), 3),
        (cabinet(0, Some(5), None), 5),
        (cabinet(0, Some(5), Some(1)), 5),
        (cabinet(0, None, Some(9)), 9),
        (cabinet(0, Some(1), Some(9)), 9),
        (cabinet(usize::MAX, Some(1), Some(2)), usize::MAX),
    ];
    for (mode, bad) in cases {
        let e = err(plan_topology(&displays, &mode));
        assert!(
            matches!(e, TopologyError::DisplayOutOfRange(i) if i == bad),
            "expected DisplayOutOfRange({bad}), got {}",
            describe(&e)
        );
    }
}

#[test]
fn duplicate_display_cabinet_names_the_shared_index() {
    let displays = cabinet_displays();
    let cases = [
        (cabinet(1, Some(1), None), 1usize),
        (cabinet(1, Some(1), Some(2)), 1),
        (cabinet(0, None, Some(0)), 0),
        (cabinet(0, Some(1), Some(0)), 0),
        (cabinet(0, Some(2), Some(2)), 2),
        (cabinet(2, Some(2), Some(2)), 2),
    ];
    for (mode, dup) in cases {
        let e = err(plan_topology(&displays, &mode));
        assert!(
            matches!(e, TopologyError::DuplicateDisplay(i) if i == dup),
            "expected DuplicateDisplay({dup}), got {}",
            describe(&e)
        );
    }
}

#[test]
fn duplicate_display_on_a_single_screen_cabinet() {
    let displays = one_fhd();
    let e = err(plan_topology(&displays, &cabinet(0, Some(0), None)));
    assert!(
        matches!(e, TopologyError::DuplicateDisplay(0)),
        "got {}",
        describe(&e)
    );
    let e = err(plan_topology(&displays, &cabinet(0, None, Some(0))));
    assert!(
        matches!(e, TopologyError::DuplicateDisplay(0)),
        "got {}",
        describe(&e)
    );
}

const BAD_FRACTIONS: [f32; 10] = [
    0.0,
    -0.0,
    -0.25,
    -1.0,
    1.000_001,
    1.5,
    f32::NAN,
    f32::INFINITY,
    f32::NEG_INFINITY,
    f32::MAX,
];

#[test]
fn invalid_backglass_fraction_is_invalid_pip_backglass() {
    let displays = one_fhd();
    for bad in BAD_FRACTIONS {
        for anchor in ANCHORS {
            // Score is valid and far away from any backglass corner clash.
            let score_anchor = if anchor == Anchor::BottomRight {
                Anchor::TopLeft
            } else {
                Anchor::BottomRight
            };
            let e = err(plan_topology(
                &displays,
                &single(0, (anchor, bad), (score_anchor, 0.125)),
            ));
            assert!(
                matches!(e, TopologyError::InvalidPip(SurfaceRole::Backglass)),
                "backglass width_fraction {bad} at {anchor:?}: got {}",
                describe(&e)
            );
        }
    }
}

#[test]
fn invalid_score_fraction_is_invalid_pip_score() {
    let displays = one_fhd();
    for bad in BAD_FRACTIONS {
        for anchor in ANCHORS {
            let bg_anchor = if anchor == Anchor::TopRight {
                Anchor::BottomLeft
            } else {
                Anchor::TopRight
            };
            let e = err(plan_topology(
                &displays,
                &single(0, (bg_anchor, 0.125), (anchor, bad)),
            ));
            assert!(
                matches!(e, TopologyError::InvalidPip(SurfaceRole::Score)),
                "score width_fraction {bad} at {anchor:?}: got {}",
                describe(&e)
            );
        }
    }
}

#[test]
fn both_fractions_invalid_reports_invalid_pip() {
    let displays = one_fhd();
    let e = err(plan_topology(
        &displays,
        &single(0, (Anchor::TopLeft, f32::NAN), (Anchor::BottomRight, 0.0)),
    ));
    assert!(
        matches!(e, TopologyError::InvalidPip(_)),
        "got {}",
        describe(&e)
    );
}

// ---------------------------------------------------------------------------
// Property tests
// ---------------------------------------------------------------------------

/// (x, y, w, h) for a display whose width is a multiple of 64, so that
/// `k/64 * w` is an exact integer in both f32 and f64.
fn display_bounds() -> impl Strategy<Value = (i32, i32, u32, u32)> {
    (
        -20_000i32..20_000,
        -20_000i32..20_000,
        4u32..=120,
        200u32..=4320,
    )
        .prop_map(|(x, y, a, h)| (x, y, a * 64, h))
}

fn display_set(max: usize) -> impl Strategy<Value = Vec<(i32, i32, u32, u32)>> {
    proptest::collection::vec(display_bounds(), 1..=max)
}

fn build(displays: &[(i32, i32, u32, u32)]) -> Vec<DisplayInfo> {
    displays
        .iter()
        .enumerate()
        .map(|(i, &(x, y, w, h))| display(i, rect(x, y, w, h)))
        .collect()
}

fn ceil_div(n: u64, d: u64) -> u64 {
    n.div_ceil(d)
}

/// What the brief allows for two PiPs in a `w x h` window, given exact widths.
#[derive(Debug, PartialEq, Eq)]
enum Expect {
    /// Fits and is disjoint under either height rounding: must succeed.
    MustSucceed,
    /// Too tall or overlapping under either height rounding: must fail.
    MustFail,
    /// Depends on how a fractional height is rounded.
    Either,
}

fn classify(win_w: u32, win_h: u32, bg: (Anchor, u32), sc: (Anchor, u32)) -> Expect {
    let heights = |role: Role, w: u32| {
        let (aw, ah) = role.aspect();
        (
            (w as u64 * ah / aw) as u32,
            ceil_div(w as u64 * ah, aw) as u32,
        )
    };
    let (b_lo, b_hi) = heights(Role::Backglass, bg.1);
    let (s_lo, s_hi) = heights(Role::Score, sc.1);
    let place = |anchor: Anchor, w: u32, h: u32| {
        let (x, y) = anchor.place(win_w, win_h, w, h);
        (x, y, w as i64, h as i64)
    };
    let clash = |a: (i64, i64, i64, i64), b: (i64, i64, i64, i64)| {
        a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
    };
    let bad = |bh: u32, sh: u32| {
        bh == 0
            || sh == 0
            || bh > win_h
            || sh > win_h
            || clash(place(bg.0, bg.1, bh), place(sc.0, sc.1, sh))
    };
    let outcomes = [
        bad(b_lo, s_lo),
        bad(b_lo, s_hi),
        bad(b_hi, s_lo),
        bad(b_hi, s_hi),
    ];
    if outcomes.iter().all(|b| !*b) {
        Expect::MustSucceed
    } else if outcomes.iter().all(|b| *b) {
        Expect::MustFail
    } else {
        Expect::Either
    }
}

fn anchor_strategy() -> impl Strategy<Value = Anchor> {
    (0usize..4).prop_map(|i| ANCHORS[i])
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    /// Single-monitor planning over random display sets and placements:
    /// success exactly when the PiPs fit and are disjoint; on success the full
    /// PiP contract (width, aspect, anchoring, containment, non-overlap) holds.
    #[test]
    fn single_monitor_random_displays_and_placements(
        raw in display_set(4),
        pick in any::<prop::sample::Index>(),
        bg_anchor in anchor_strategy(),
        sc_anchor in anchor_strategy(),
        bg_k in 1u32..=64,
        sc_k in 1u32..=64,
    ) {
        let displays = build(&raw);
        let chosen = pick.index(displays.len());
        let (_, _, win_w, win_h) = raw[chosen];
        let bg_w = win_w / 64 * bg_k;
        let sc_w = win_w / 64 * sc_k;
        let mode = single(chosen, (bg_anchor, bg_k as f32 / 64.0), (sc_anchor, sc_k as f32 / 64.0));
        let expect = classify(win_w, win_h, (bg_anchor, bg_w), (sc_anchor, sc_w));

        match plan_topology(&displays, &mode) {
            Ok(plan) => {
                prop_assert_ne!(expect, Expect::MustFail, "planned an impossible layout");
                prop_assert_eq!(plan.windows.len(), 1);
                let win = &plan.windows[0];
                prop_assert_eq!(win.display, chosen);
                prop_assert_eq!(win.bounds, displays[chosen].bounds);
                prop_assert!(win.borderless_fullscreen);
                prop_assert_eq!(win.viewports.len(), 3);
                prop_assert_eq!(viewport(win, Role::Playfield), rect(0, 0, win_w, win_h));
                let b = viewport(win, Role::Backglass);
                let s = viewport(win, Role::Score);
                assert_pip(b, Role::Backglass, bg_anchor, win_w, win_h, bg_w);
                assert_pip(s, Role::Score, sc_anchor, win_w, win_h, sc_w);
                prop_assert!(!overlaps(b, s), "Backglass {:?} overlaps Score {:?}", b, s);
                assert_plan_invariants(&plan, &displays);
            }
            Err(e) => {
                prop_assert_ne!(expect, Expect::MustSucceed, "rejected a valid layout: {}", describe(&e));
                prop_assert!(
                    matches!(e, TopologyError::InvalidPip(_)),
                    "a geometric PiP failure must be InvalidPip, got {}", describe(&e)
                );
            }
        }
    }

    /// Opposite-side PiPs whose widths sum to at most the window width and
    /// whose heights fit can never overlap, so they must always be accepted.
    #[test]
    fn disjoint_columns_always_plan(
        raw in display_bounds(),
        left_top in any::<bool>(),
        right_top in any::<bool>(),
        bg_on_left in any::<bool>(),
        bg_k in 1u32..=32,
        sc_k in 1u32..=32,
    ) {
        let (x, y, win_w, _) = raw;
        // Tall enough for a half-width 4:3 backglass.
        let win_h = win_w;
        let displays = vec![display(0, rect(x, y, win_w, win_h))];
        let left = if left_top { Anchor::TopLeft } else { Anchor::BottomLeft };
        let right = if right_top { Anchor::TopRight } else { Anchor::BottomRight };
        let (bg_anchor, sc_anchor) = if bg_on_left { (left, right) } else { (right, left) };
        let mode = single(0, (bg_anchor, bg_k as f32 / 64.0), (sc_anchor, sc_k as f32 / 64.0));
        let plan = match plan_topology(&displays, &mode) {
            Ok(p) => p,
            Err(e) => return Err(TestCaseError::fail(format!("rejected disjoint PiPs: {}", describe(&e)))),
        };
        let win = &plan.windows[0];
        assert_pip(viewport(win, Role::Backglass), Role::Backglass, bg_anchor, win_w, win_h, win_w / 64 * bg_k);
        assert_pip(viewport(win, Role::Score), Role::Score, sc_anchor, win_w, win_h, win_w / 64 * sc_k);
        assert_plan_invariants(&plan, &displays);
    }

    /// Any non-finite or out-of-(0, 1] fraction is `InvalidPip` for that role,
    /// on any display set.
    #[test]
    fn invalid_fractions_always_rejected(
        raw in display_set(3),
        pick in any::<prop::sample::Index>(),
        bad in prop_oneof![
            -1.0e6f32..=0.0,
            1.000_01f32..1.0e6,
            Just(f32::NAN),
            Just(f32::INFINITY),
            Just(f32::NEG_INFINITY),
        ],
        bad_is_backglass in any::<bool>(),
    ) {
        let displays = build(&raw);
        let chosen = pick.index(displays.len());
        // The valid PiP is tiny and diagonally opposite.
        let mode = if bad_is_backglass {
            single(chosen, (Anchor::TopLeft, bad), (Anchor::BottomRight, 1.0 / 64.0))
        } else {
            single(chosen, (Anchor::TopLeft, 1.0 / 64.0), (Anchor::BottomRight, bad))
        };
        match plan_topology(&displays, &mode) {
            Ok(_) => prop_assert!(false, "accepted width_fraction {}", bad),
            Err(e) => {
                let right_role = if bad_is_backglass {
                    matches!(e, TopologyError::InvalidPip(SurfaceRole::Backglass))
                } else {
                    matches!(e, TopologyError::InvalidPip(SurfaceRole::Score))
                };
                prop_assert!(right_role, "width_fraction {}: got {}", bad, describe(&e));
            }
        }
    }

    /// Cabinet planning over random display sets and (possibly invalid) role
    /// assignments: the right error for out-of-range and duplicate displays,
    /// and the full window/viewport contract on success.
    #[test]
    fn cabinet_random_displays_and_assignments(
        raw in display_set(5),
        playfield in 0usize..7,
        backglass in proptest::option::of(0usize..7),
        score in proptest::option::of(0usize..7),
    ) {
        let displays = build(&raw);
        let n = displays.len();
        let assigned: Vec<usize> = [Some(playfield), backglass, score].into_iter().flatten().collect();
        let out_of_range: Vec<usize> = assigned.iter().copied().filter(|&i| i >= n).collect();
        let duplicated: Vec<usize> = assigned
            .iter()
            .copied()
            .filter(|&i| assigned.iter().filter(|&&j| j == i).count() > 1)
            .collect();

        let result = plan_topology(&displays, &cabinet(playfield, backglass, score));

        if !out_of_range.is_empty() || !duplicated.is_empty() {
            let e = match result {
                Ok(_) => return Err(TestCaseError::fail("accepted an invalid cabinet assignment")),
                Err(e) => e,
            };
            let acceptable = match e {
                TopologyError::DisplayOutOfRange(i) => out_of_range.contains(&i),
                TopologyError::DuplicateDisplay(i) => duplicated.contains(&i),
                _ => false,
            };
            prop_assert!(
                acceptable,
                "assignment ({}, {:?}, {:?}) on {} displays: got {}",
                playfield, backglass, score, n, describe(&e)
            );
            return Ok(());
        }

        // Valid assignment. Roles without a display become default PiPs on the
        // playfield window (width 0.25: exact, since widths are multiples of 64).
        let (_, _, pf_w, pf_h) = raw[playfield];
        let pip_w = pf_w / 4;
        let expect = match (backglass, score) {
            (Some(_), Some(_)) => Expect::MustSucceed,
            (None, None) => classify(pf_w, pf_h, (Anchor::TopRight, pip_w), (Anchor::BottomRight, pip_w)),
            (None, Some(_)) => {
                let h_hi = ceil_div(pip_w as u64 * 3, 4) as u32;
                let h_lo = pip_w * 3 / 4;
                if h_hi <= pf_h { Expect::MustSucceed } else if h_lo > pf_h { Expect::MustFail } else { Expect::Either }
            }
            (Some(_), None) => {
                let h_hi = ceil_div(pip_w as u64, 4) as u32;
                let h_lo = pip_w / 4;
                if h_hi <= pf_h { Expect::MustSucceed } else if h_lo > pf_h { Expect::MustFail } else { Expect::Either }
            }
        };

        let plan = match result {
            Ok(plan) => {
                prop_assert_ne!(expect, Expect::MustFail, "planned an impossible default PiP layout");
                plan
            }
            Err(e) => {
                prop_assert_ne!(expect, Expect::MustSucceed, "rejected a valid cabinet: {}", describe(&e));
                prop_assert!(matches!(e, TopologyError::InvalidPip(_)), "got {}", describe(&e));
                return Ok(());
            }
        };

        prop_assert_eq!(plan.windows.len(), assigned.len(), "one window per role that has a display");
        assert_plan_invariants(&plan, &displays);

        let pf = window_with(&plan, Role::Playfield);
        prop_assert_eq!(pf.display, playfield);
        prop_assert_eq!(pf.bounds, displays[playfield].bounds);
        prop_assert_eq!(viewport(pf, Role::Playfield), rect(0, 0, pf_w, pf_h));

        for (role, assigned_display, default_anchor) in [
            (Role::Backglass, backglass, Anchor::TopRight),
            (Role::Score, score, Anchor::BottomRight),
        ] {
            let win = window_with(&plan, role);
            match assigned_display {
                Some(d) => {
                    prop_assert_eq!(win.display, d);
                    prop_assert_eq!(win.bounds, displays[d].bounds);
                    prop_assert!(win.borderless_fullscreen);
                    prop_assert_eq!(roles(win), vec![role]);
                    prop_assert_eq!(viewport(win, role), full(win));
                }
                None => {
                    prop_assert_eq!(win.display, playfield, "{:?} without a display is a PiP on the playfield", role);
                    assert_pip(viewport(win, role), role, default_anchor, pf_w, pf_h, pip_w);
                }
            }
        }
    }

    /// With no displays at all, every mode reports `NoDisplays`.
    #[test]
    fn empty_display_set_is_always_no_displays(
        idx in 0usize..4,
        backglass in proptest::option::of(0usize..4),
        score in proptest::option::of(0usize..4),
        use_cabinet in any::<bool>(),
    ) {
        let mode = if use_cabinet {
            cabinet(idx, backglass, score)
        } else {
            single(idx, (Anchor::TopRight, 0.25), (Anchor::BottomRight, 0.25))
        };
        match plan_topology(&[], &mode) {
            Ok(_) => prop_assert!(false, "planned windows with no displays"),
            Err(e) => prop_assert!(matches!(e, TopologyError::NoDisplays), "got {}", describe(&e)),
        }
    }
}
