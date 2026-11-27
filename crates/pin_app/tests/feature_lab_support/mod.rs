//! Shared helpers of the Feature Lab (6.4) tests: the bundle (built once
//! per test binary through `pin_tablegen::backbox::build_from`, the
//! implementation `tables/feature_lab/build.sh` runs as `pin_tablegen
//! build`), scratch directories, a big-stack thread, persist-key lookup and
//! small state-injection helpers.
#![allow(dead_code)]

/// The Feature Lab build: the `pin_tablegen` library items the tests use
/// (one implementation; `tables/feature_lab/build.sh` runs the same code
/// through the `pin_tablegen build` command).
pub mod build {
    #[allow(unused_imports)]
    pub use pin_tablegen::backbox::{
        VUK_LATERAL_M_S, VUK_RISE_M_S, build_from, parse_hex, vuk_eject,
    };
    use std::path::{Path, PathBuf};

    /// `tables/feature_lab` (the sources).
    #[must_use]
    pub fn feature_lab_src() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tables/feature_lab")
    }

    /// Builds the bundle from `tables/feature_lab` into `out_dir`.
    ///
    /// # Errors
    /// As [`build_from`].
    pub fn build_feature_lab_bundle(out_dir: &Path) -> Result<Vec<String>, String> {
        build_from(&feature_lab_src(), out_dir)
    }
}

#[allow(unused_imports)]
pub use build::{build_feature_lab_bundle, feature_lab_src};

use pin_assets::IdKind;
use pin_sim::{BallHandle, SimConfig, Simulation};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// Standard ball radius, m.
pub const R: f32 = 0.0135;

/// A fresh scratch directory under the target dir (tests never share one).
pub fn scratch(tag: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let p = base.join(format!(
        "pin_app_{tag}_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// The Feature Lab bundle, built once per test binary.
pub fn bundle() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let d = scratch("feature_lab_bundle");
        build_feature_lab_bundle(&d).unwrap_or_else(|e| panic!("build: {e}"));
        d
    })
}

/// A copy of the bundle in a fresh scratch directory (for edited copies).
pub fn bundle_copy(tag: &str) -> PathBuf {
    let to = scratch(tag);
    copy_dir(bundle(), &to);
    to
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    let mut entries: Vec<_> = std::fs::read_dir(from)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for p in entries {
        let dest = to.join(p.file_name().unwrap());
        if p.is_dir() {
            copy_dir(&p, &dest);
        } else {
            std::fs::copy(&p, &dest).unwrap();
        }
    }
}

/// Runs `f` on a thread with an 8 MiB stack.
pub fn on_big_stack<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(f)
        .expect("spawn")
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e))
}

/// Loads the bundle (plain `Simulation::load`).
pub fn load() -> Simulation {
    Simulation::load(bundle(), SimConfig::default()).expect("load")
}

/// Loads `dir` with `seed`.
pub fn load_seeded(dir: &Path, seed: u64) -> Simulation {
    Simulation::load(
        dir,
        SimConfig {
            seed,
            ..SimConfig::default()
        },
    )
    .expect("load")
}

/// The persist key index of `name`.
pub fn key(s: &Simulation, name: &str) -> u16 {
    s.ids()
        .get(IdKind::PersistKey, name)
        .unwrap_or_else(|| panic!("persist key {name}"))
}

/// The persist value of `name`.
pub fn persist(s: &Simulation, name: &str) -> i64 {
    s.persist(key(s, name))
}

/// The five mode ids, in selector order.
pub const MODES: [&str; 5] = [
    "banana_shots",
    "backbox_play",
    "xl_ball",
    "magnet_compare",
    "glo_ball",
];

/// Holds switch `name` for `ticks` and releases it.
pub fn press(s: &mut Simulation, name: &str, ticks: u64) {
    s.set_switch(name, true).unwrap();
    s.advance(ticks);
    s.release_switch(name).unwrap();
}

/// A table point (mm) at ball-centre height of a standard ball, m.
pub fn mm(x: f32, y: f32) -> [f32; 3] {
    [x * 1.0e-3, y * 1.0e-3, R]
}

/// No script fault, no queue overflow, nothing dropped, failed or escaped.
pub fn healthy(s: &Simulation) {
    assert!(!s.script_faulted(), "script fault: {:?}", s.script_error());
    assert_eq!(s.script_queue_overflows(), 0, "script queue overflow");
    let c = s.counters();
    assert_eq!(c.edges_dropped, 0, "{c:?}");
    assert_eq!(c.spawns_failed, 0, "{c:?}");
    assert_eq!(c.balls_escaped, 0, "{c:?}");
}

/// Advances `ticks` one at a time, collecting the names of the scenes the
/// rules showed.
pub fn advance_scenes(s: &mut Simulation, ticks: u64) -> Vec<String> {
    let mut out = Vec::new();
    for _ in 0..ticks {
        s.advance(1);
        for id in s.display_scenes() {
            if let Some(n) = s.ids().name(IdKind::Scene, id.0) {
                out.push(n.to_owned());
            }
        }
    }
    out
}

/// Every ball handle in play.
pub fn balls(s: &Simulation) -> Vec<BallHandle> {
    s.ball_handles().to_vec()
}

/// Removes every ball in play (test injection).
pub fn clear_balls(s: &mut Simulation) {
    for h in balls(s) {
        s.remove_ball(h);
    }
}

/// Starts a game and waits for the first ball on the plunger.
pub fn start_game(s: &mut Simulation) {
    press(s, "start", 5);
    s.advance(1500);
    assert_eq!(persist(s, "game_state"), 1, "game state");
    assert_eq!(persist(s, "ball"), 1);
    assert_eq!(s.balls_in_play(), 1);
}

/// The plan-view area (mm) of the `jumbo_xl_resin` restriction.
pub fn xl_area(t: &pin_core::schema::table::TableManifest) -> Vec<[f32; 2]> {
    t.balls
        .restrict
        .iter()
        .find(|r| r.profile == "jumbo_xl_resin")
        .expect("XL restriction")
        .area
        .clone()
}

/// Whether `p` is inside the polygon `poly` by the even-odd rule (a ray to
/// +x crosses its edges an odd number of times).
pub fn inside_even_odd(poly: &[[f32; 2]], p: [f32; 2]) -> bool {
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + n - 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
            if p[0] < x {
                inside = !inside;
            }
        }
    }
    inside
}

/// Presses `action` until the selector shows mode index `i`.
pub fn select_mode(s: &mut Simulation, i: i64) {
    for _ in 0..5 {
        if persist(s, "mode_sel") == i {
            return;
        }
        press(s, "action", 5);
        s.advance(5);
    }
    assert_eq!(persist(s, "mode_sel"), i, "selector");
}

// --- the closed pocket beside the orbit (feature_lab_a2_catch, feature_lab_cr1)

/// The orbit lane's centre line and width (layout.toml `[[lanes]]`), mm.
pub const ORBIT: [[f32; 2]; 8] = [
    [85.0, 615.0],
    [55.0, 650.0],
    [38.0, 700.0],
    [36.0, 780.0],
    [45.0, 880.0],
    [90.0, 1000.0],
    [170.0, 1040.0],
    [260.0, 1046.0],
];
/// Half the orbit lane's width, mm.
pub const ORBIT_HALF_WIDTH: f32 = 20.0;

/// From this height up the orbit's outer rail is within 15.7 mm of the
/// cabinet wall: the strip between them is a closed pocket, mm.
pub const POCKET_Y_MIN: f32 = 744.0;

/// Whether a layer-0 ball centre `p` (mm) is in the closed pocket: up the
/// table from `POCKET_Y_MIN`, outside the orbit lane on its cabinet side
/// (left of the centre line, beyond the lane's half width).
///
/// Use it on a static zone footprint, on the point where a ball comes back
/// to layer 0 and on a layer-0 ball's final rest point. Not on every tick
/// of a ball that runs the orbit: past the lane's end (260, 1046) it is
/// also true for points that are not in the pocket, e.g. (279.7, 1050.4).
pub fn in_pocket(p: [f32; 2]) -> bool {
    let (d, side) = orbit_offset(p);
    p[1] >= POCKET_Y_MIN && d > ORBIT_HALF_WIDTH && side > 0.0
}

/// Distance (mm) of `p` from the orbit's centre line, and its side
/// (positive on the left of the lane's travel direction: the cabinet side
/// of its up-going left leg).
pub fn orbit_offset(p: [f32; 2]) -> (f32, f32) {
    let (mut best_d, mut best_side) = (f32::INFINITY, 0.0f32);
    for w in ORBIT.windows(2) {
        let (a, b) = (w[0], w[1]);
        let ab = [b[0] - a[0], b[1] - a[1]];
        let ap = [p[0] - a[0], p[1] - a[1]];
        let t = ((ap[0] * ab[0] + ap[1] * ab[1]) / (ab[0] * ab[0] + ab[1] * ab[1])).clamp(0.0, 1.0);
        let d = (ap[0] - ab[0] * t).hypot(ap[1] - ab[1] * t);
        if d < best_d {
            best_d = d;
            best_side = ab[0] * ap[1] - ab[1] * ap[0];
        }
    }
    (best_d, best_side)
}

/// Whether `p` (m) is outside the simulation's escape bounds (the box the
/// EscapedBall check uses).
pub fn outside(bounds: ([f32; 3], [f32; 3]), p: [f32; 3]) -> bool {
    (0..3).any(|k| !(p[k] >= bounds.0[k] && p[k] <= bounds.1[k]))
}

/// Whether the closed boxes `a` and `b` (`(min, max)`, any unit) share a
/// point. Layer-transition bounds are inclusive, so boxes that only touch
/// on a face share points and count as overlapping.
pub fn boxes_touch_or_overlap(a: ([f32; 3], [f32; 3]), b: ([f32; 3], [f32; 3])) -> bool {
    (0..3).all(|k| a.0[k] <= b.1[k] && b.0[k] <= a.1[k])
}
