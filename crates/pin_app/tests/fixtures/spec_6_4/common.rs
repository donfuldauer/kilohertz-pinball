//! Shared helpers of the Subphase 6.4 spec tests (Independent Tester) for
//! the "Feature Lab" showcase table.
//!
//! The bundle is obtained only through the table's own build script,
//! `tables/feature_lab/build.sh <out_dir>` (TABLE CONTRACT C1), run once per
//! test binary into a scratch folder under `CARGO_TARGET_TMPDIR`. Everything
//! else is black-box: structure through `pin_assets::load_table`, behaviour
//! through `pin_app::orchestrator::load_validated` and the public
//! `pin_sim::Simulation` test API, on simulated time only (D8).

#![allow(dead_code)]

use pin_app::orchestrator::load_validated;
use pin_assets::{LoadedTable, SwitchSi, Vfs, load_table};
use pin_core::input::{CabinetInput, InputEvent};
use pin_core::schema::table::{FlipperShapeDef, SwitchKind};
use pin_core::sim::{SimStep, TICK_NS};
use pin_core::snapshot::BallState;
use pin_sim::{BallHandle, SimConfig, Simulation};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// Ball profiles of the table (contract C3).
pub const STEEL: &str = "standard_steel";
pub const LOW_REM: &str = "low_rem_steel";
pub const GLO: &str = "uv_globall";
pub const XL: &str = "jumbo_xl_resin";

/// Mode ids in selector order (contract C6).
pub const MODES: [&str; 5] = [
    "banana_shots",
    "backbox_play",
    "xl_ball",
    "magnet_compare",
    "glo_ball",
];

/// The backbox flipper and VUK ids (contract C2, C5).
pub const BACKBOX_FLIPPER: &str = "tablegen:backbox.flippers[0]";
pub const BACKBOX_VUK: &str = "tablegen:backbox.vuk";

/// Runs `f` on a thread with a 64 MiB stack (script backends recurse).
pub fn big_stack<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    std::thread::Builder::new()
        .name("spec-6.4-sim".into())
        .stack_size(64 << 20)
        .spawn(f)
        .expect("spawn")
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e))
}

/// The worktree root (`tables/feature_lab` lives under it).
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("worktree root")
}

/// A fresh scratch directory under the test target dir.
pub fn scratch(tag: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "spec_6_4_{tag}_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clear scratch dir");
    }
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// Builds the bundle with `sh tables/feature_lab/build.sh <out>` from the
/// worktree root (contract C1). The caller's `CARGO_*` environment is passed
/// through unchanged.
///
/// # Errors
/// The script's status and output when it fails.
pub fn build_bundle(out: &Path) -> Result<(), String> {
    let root = repo_root();
    let script = root.join("tables/feature_lab/build.sh");
    if !script.is_file() {
        return Err(format!("contract C1: {} must exist", script.display()));
    }
    let output = Command::new("sh")
        .arg("tables/feature_lab/build.sh")
        .arg(out)
        .current_dir(&root)
        .output()
        .map_err(|e| format!("run sh tables/feature_lab/build.sh: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "build.sh failed ({}):\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    if !out.join("table.toml").is_file() {
        return Err(format!("build.sh wrote no {}/table.toml", out.display()));
    }
    Ok(())
}

/// The Feature Lab bundle, built once per test binary (a failed build is
/// reported by every test that needs the bundle, without rebuilding).
pub fn bundle() -> &'static Path {
    static DIR: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    let r = DIR.get_or_init(|| {
        let out = scratch("feature_lab_bundle");
        build_bundle(&out).map(|()| out)
    });
    match r {
        Ok(p) => p,
        Err(e) => panic!("{e}"),
    }
}

/// `pin_assets::load_table` on the bundle.
pub fn table() -> &'static LoadedTable {
    static TABLE: OnceLock<LoadedTable> = OnceLock::new();
    TABLE.get_or_init(|| {
        let vfs = Vfs::open(bundle()).expect("open the Feature Lab bundle");
        load_table(&vfs).unwrap_or_else(|e| panic!("load_table on the bundle: {e}"))
    })
}

/// `load_validated` on the bundle (default config).
pub fn load() -> Simulation {
    load_validated(bundle(), SimConfig::default())
        .unwrap_or_else(|e| panic!("load_validated on the bundle: {e}"))
}

/// `load_validated` on the bundle with `cfg`.
pub fn load_with(cfg: SimConfig) -> Simulation {
    load_validated(bundle(), cfg).unwrap_or_else(|e| panic!("load_validated on the bundle: {e}"))
}

/// `load_validated` on any bundle path.
pub fn load_from(path: &Path) -> Simulation {
    load_validated(path, SimConfig::default())
        .unwrap_or_else(|e| panic!("load_validated on {}: {e}", path.display()))
}

/// Index of persist key `name` in `[persist] keys` (contract C7).
pub fn key(name: &str) -> u16 {
    let keys = &table()
        .manifest
        .persist
        .as_ref()
        .expect("[persist] section")
        .keys;
    let i = keys
        .iter()
        .position(|k| k == name)
        .unwrap_or_else(|| panic!("persist key {name:?} in [persist] keys {keys:?}"));
    u16::try_from(i).expect("key index fits u16")
}

/// The persistent value `name`.
pub fn pv(sim: &Simulation, name: &str) -> i64 {
    sim.persist(key(name))
}

/// Radius of a built-in profile, m.
pub fn radius_m(profile: &str) -> f32 {
    pin_core::builtin_profile(profile)
        .expect("built-in profile")
        .radius_m()
}

/// Built-in profile id of `name` (index in `BUILTIN_PROFILE_NAMES`).
pub fn profile_index(name: &str) -> u16 {
    let i = pin_core::profiles::BUILTIN_PROFILE_NAMES
        .iter()
        .position(|&n| n == name)
        .expect("built-in profile name");
    u16::try_from(i).expect("fits")
}

/// Closing edges of `name`.
pub fn hits(sim: &Simulation, name: &str) -> u32 {
    sim.switch_hits(name)
        .unwrap_or_else(|| panic!("switch {name:?} is known to the sim"))
}

/// Script and ball health (contract C9 and no ball lost or non-finite).
pub fn assert_healthy(sim: &Simulation, what: &str) {
    assert!(
        !sim.script_faulted(),
        "{what}: script faulted: {:?}",
        sim.script_error()
    );
    assert_eq!(
        sim.script_queue_overflows(),
        0,
        "{what}: script queue overflow"
    );
    assert_eq!(sim.counters().balls_escaped, 0, "{what}: a ball escaped");
    for h in sim.ball_handles() {
        let b = sim.ball(h).expect("live handle");
        assert!(finite_ball(&b), "{what}: non-finite ball {b:?}");
    }
}

/// Whether every number of `b` is finite.
pub fn finite_ball(b: &BallState) -> bool {
    b.position_m
        .iter()
        .chain(&b.velocity_m_s)
        .chain(&b.angular_velocity_rad_s)
        .chain(&b.orientation)
        .all(|v| v.is_finite())
}

/// Forces switch `name` closed for `hold` ticks, opens it and hands it back
/// to its input / the physics.
pub fn pulse(sim: &mut Simulation, name: &str, hold: u64) {
    sim.set_switch(name, true).expect("known switch");
    sim.advance(hold);
    sim.set_switch(name, false).expect("known switch");
    sim.advance(5);
    sim.release_switch(name).expect("known switch");
}

/// One tick with cabinet input `input` pressed or released through the real
/// input path (`SimStep::step_1ms`).
pub fn cabinet(sim: &mut Simulation, input: CabinetInput, pressed: bool) {
    let mut out = *sim.snapshot();
    let t = sim.tick() + 1;
    let ev = InputEvent::button(t * TICK_NS - TICK_NS / 2, input, pressed);
    sim.step_1ms(t, &[ev], &mut out);
}

/// Presses cabinet input `input` for `hold` ticks and releases it.
pub fn press(sim: &mut Simulation, input: CabinetInput, hold: u64) {
    cabinet(sim, input, true);
    sim.advance(hold);
    cabinet(sim, input, false);
}

/// Steps until a ball is on the table (≤ `max` ticks).
pub fn wait_for_ball(sim: &mut Simulation, max: u64) -> bool {
    for _ in 0..max {
        if sim.balls_in_play() > 0 {
            return true;
        }
        sim.advance(1);
    }
    sim.balls_in_play() > 0
}

/// Steps until `cond` holds (checked after each tick); false after `max`.
pub fn run_until(
    sim: &mut Simulation,
    max: u64,
    mut cond: impl FnMut(&Simulation) -> bool,
) -> bool {
    for _ in 0..max {
        sim.advance(1);
        if cond(sim) {
            return true;
        }
    }
    false
}

/// Removes every ball (not a drain: the game goes on).
pub fn clear_table(sim: &mut Simulation) {
    let handles: Vec<BallHandle> = sim.ball_handles().iter().copied().collect();
    for h in handles {
        assert!(sim.remove_ball(h));
    }
}

/// Presses `start` (cabinet switch) and waits up to 10 s for the served
/// ball (contract C8).
pub fn start_game(sim: &mut Simulation) {
    sim.advance(200);
    pulse(sim, "start", 50);
    assert!(wait_for_ball(sim, 10_000), "start serves a ball");
    assert!(
        run_until(sim, 3_000, |s| pv(s, "game_state") == 1),
        "game_state is 1 after start (got {})",
        pv(sim, "game_state")
    );
}

/// Every declared switch (`[[switches]]`), by name.
pub fn switch(name: &str) -> &'static SwitchSi {
    table()
        .switches
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("switch {name:?} declared"))
}

/// The main drain: the first trough-kind switch with a position on layer 0.
pub fn drain_switch() -> &'static SwitchSi {
    table()
        .switches
        .iter()
        .find(|s| s.kind == SwitchKind::Trough && s.position_m.is_some() && s.layer == 0)
        .expect("a trough switch with a position on layer 0")
}

/// Drains every ball through the trough switch zone by state injection
/// (allowed for drains, brief S3 a): each ball is moved onto the switch.
pub fn drain_all(sim: &mut Simulation) {
    let trough = drain_switch();
    let p = trough.position_m.expect("position");
    let handles: Vec<BallHandle> = sim.ball_handles().iter().copied().collect();
    for h in handles {
        let Some(mut b) = sim.ball(h) else { continue };
        let r = pin_core::profiles::BUILTIN_PROFILE_NAMES
            .get(usize::from(b.profile.0))
            .map_or(0.0135, |n| radius_m(n));
        b.position_m = [p[0], p[1], p[2] + r];
        b.velocity_m_s = [0.0; 3];
        b.angular_velocity_rad_s = [0.0; 3];
        b.layer = trough.layer;
        assert!(sim.set_ball_state(h, &b), "move the ball onto the trough");
        let gone = run_until(sim, 200, |s| s.ball(h).is_none());
        assert!(gone, "the ball drained through {}", trough.name);
    }
}

/// Unit plan vector of `deg` (0° = +x, 90° = up the table).
pub fn dir2(deg: f32) -> [f32; 2] {
    let a = deg.to_radians();
    [a.cos(), a.sin()]
}

pub fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn add3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn scale3(a: [f32; 3], k: f32) -> [f32; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}

pub fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn len3(a: [f32; 3]) -> f32 {
    dot3(a, a).sqrt()
}

pub fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn norm3(a: [f32; 3]) -> [f32; 3] {
    let l = len3(a).max(1e-12);
    scale3(a, 1.0 / l)
}

/// Plan (x, y) distance, m.
pub fn plan_dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Even-odd point-in-polygon test (plan view).
pub fn in_polygon(poly: &[[f32; 2]], p: [f32; 2]) -> bool {
    let mut inside = false;
    let n = poly.len();
    let mut j = n.wrapping_sub(1);
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0];
            if p[0] < x {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// Distance from `p` to segment `a`-`b` (plan view).
pub fn seg_dist(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p[0] - a[0] - t * dx).hypot(p[1] - a[1] - t * dy)
}

/// Distance from `p` to the polygon's boundary (plan view).
pub fn boundary_dist(poly: &[[f32; 2]], p: [f32; 2]) -> f32 {
    let n = poly.len();
    (0..n)
        .map(|i| seg_dist(p, poly[i], poly[(i + 1) % n]))
        .fold(f32::INFINITY, f32::min)
}

/// The `jumbo_xl_resin` restrict area, mm (contract C3).
pub fn xl_area_mm() -> Vec<[f32; 2]> {
    table()
        .manifest
        .balls
        .restrict
        .iter()
        .find(|r| r.profile == XL)
        .expect("[balls] restrict entry for jumbo_xl_resin")
        .area
        .clone()
}

/// The bat's half thickness (mm) at fraction `s` of its length.
pub fn bat_radius_mm(shape: &FlipperShapeDef, s: f32) -> f32 {
    match *shape {
        FlipperShapeDef::StraightCapsule {
            base_radius,
            tip_radius,
            ..
        } => base_radius + (tip_radius - base_radius) * s,
        FlipperShapeDef::CurvedBanana { tube_radius, .. } => tube_radius,
    }
}

/// The bat's length, mm.
pub fn bat_length_mm(shape: &FlipperShapeDef) -> f32 {
    match *shape {
        FlipperShapeDef::StraightCapsule { length, .. }
        | FlipperShapeDef::CurvedBanana { length, .. } => length,
    }
}

/// "Up" in the table frame (against gravity): the table frame is pitched
/// by `pitch_rad`, z up from the playfield, y up the table.
pub fn up_vector() -> [f32; 3] {
    let p = table().pitch_rad;
    [0.0, p.sin(), p.cos()]
}

/// A ball centre resting on the up-side face of flipper `name` at its
/// current pose, at fraction `s` of the bat, with `gap_mm` of air between
/// the ball and the bat face, the ball touching the mounting plane.
///
/// * Straight bat: `s` is a fraction of the length along the pivot's bat
///   direction and the ball sits beside the tapered capsule.
/// * `CurvedBanana`: `s` is a fraction of the centreline arc's length and
///   the ball sits on the up-side face along that face's local normal (see
///   [`banana_frame`]); the surface clearance is checked to be `gap_mm`.
pub fn on_bat(sim: &Simulation, name: &str, s: f32, gap_mm: f32, profile: &str) -> [f32; 3] {
    let def = table()
        .manifest
        .flippers
        .iter()
        .find(|f| f.id == name)
        .unwrap_or_else(|| panic!("flipper {name}"));
    if matches!(def.shape, FlipperShapeDef::CurvedBanana { .. }) {
        return on_banana(sim, name, s, gap_mm, profile);
    }
    let id = sim.ids().flipper(name).expect("flipper id");
    let pivot = sim.flipper_set().pivot_m(id).expect("pivot").to_array();
    let d = sim
        .flipper_set()
        .bat_direction(id)
        .expect("bat dir")
        .to_array();
    let n = norm3(def.surface_normal);
    let mut side = norm3(cross3(n, d));
    if dot3(side, up_vector()) < 0.0 {
        side = scale3(side, -1.0);
    }
    let r = radius_m(profile);
    let len = bat_length_mm(&def.shape) * 1e-3;
    let rb = bat_radius_mm(&def.shape, s) * 1e-3;
    let along = scale3(d, s * len);
    let off = scale3(side, rb + r + gap_mm * 1e-3);
    add3(add3(add3(pivot, along), off), scale3(n, r))
}

/// The world-frame centreline of a `CurvedBanana` flipper at its current
/// pose (contract of `FlipperShape::CurvedBanana`: the pivot is the start
/// of a circular arc that leaves it along the bat direction and curls
/// toward its concave side; `length` is the chord to the tip-cap centre;
/// the concave side is the swing's leading face for `arc_sweep_deg >= 0`,
/// the trailing one otherwise).
#[derive(Clone, Copy, Debug)]
pub struct BananaFrame {
    /// Pivot, m.
    pub pivot: [f32; 3],
    /// Bat direction at the pivot (arc tangent there).
    pub x: [f32; 3],
    /// Unit vector from the pivot toward the centre of curvature.
    pub to_centre: [f32; 3],
    /// Mounting normal (rotation axis).
    pub n: [f32; 3],
    /// Centreline radius, m.
    pub arc_radius: f32,
    /// Arc angle `2·asin(length / (2·arc_radius))`, rad.
    pub phi: f32,
    /// Tube radius, m.
    pub tube: f32,
}

impl BananaFrame {
    /// Centreline point at fraction `s` of the arc length.
    pub fn point(&self, s: f32) -> [f32; 3] {
        let a = s.clamp(0.0, 1.0) * self.phi;
        add3(
            add3(self.pivot, scale3(self.x, self.arc_radius * a.sin())),
            scale3(self.to_centre, self.arc_radius * (1.0 - a.cos())),
        )
    }

    /// Unit normal of the concave face at fraction `s` (toward the centre
    /// of curvature).
    pub fn concave_normal(&self, s: f32) -> [f32; 3] {
        let a = s.clamp(0.0, 1.0) * self.phi;
        norm3(add3(
            scale3(self.x, -a.sin()),
            scale3(self.to_centre, a.cos()),
        ))
    }

    /// Plan distance (in the mounting plane) from `p` to the tube's side
    /// surface, m, by dense sampling of the centreline arc (positive
    /// outside the bat).
    pub fn side_clearance(&self, p: [f32; 3]) -> f32 {
        const N: u32 = 4_000;
        let mut best = f32::INFINITY;
        for i in 0..=N {
            let c = self.point(i as f32 / N as f32);
            let v = sub3(p, c);
            let in_plane = sub3(v, scale3(self.n, dot3(v, self.n)));
            best = best.min(len3(in_plane));
        }
        best - self.tube
    }
}

/// The [`BananaFrame`] of `CurvedBanana` flipper `name` at its current pose.
pub fn banana_frame(sim: &Simulation, name: &str) -> BananaFrame {
    let def = table()
        .manifest
        .flippers
        .iter()
        .find(|f| f.id == name)
        .unwrap_or_else(|| panic!("flipper {name}"));
    let FlipperShapeDef::CurvedBanana {
        length,
        arc_radius,
        arc_sweep_deg,
        tube_radius,
    } = def.shape
    else {
        panic!("flipper {name} is a CurvedBanana");
    };
    let id = sim.ids().flipper(name).expect("flipper id");
    let pivot = sim.flipper_set().pivot_m(id).expect("pivot").to_array();
    let x = norm3(
        sim.flipper_set()
            .bat_direction(id)
            .expect("bat dir")
            .to_array(),
    );
    let n = norm3(def.surface_normal);
    // Bat-frame +y = axis × x: the direction a bat point moves for a
    // positive (counter-clockwise) swing. The concave side is the leading
    // face (sign of the sweep) for arc_sweep >= 0, else the trailing face.
    let y = norm3(cross3(n, x));
    let leading = if def.sweep_angle_deg < 0.0 { -1.0 } else { 1.0 };
    let concave = if arc_sweep_deg < 0.0 {
        -leading
    } else {
        leading
    };
    let arc_radius = arc_radius * 1e-3;
    let length = length * 1e-3;
    BananaFrame {
        pivot,
        x,
        to_centre: scale3(y, concave),
        n,
        arc_radius,
        phi: 2.0 * (0.5 * length / arc_radius).min(1.0).asin(),
        tube: tube_radius * 1e-3,
    }
}

/// A ball centre on the up-side face of `CurvedBanana` flipper `name` at
/// fraction `s` of its centreline arc, `gap_mm` clear of the tube along the
/// face's local normal, the ball touching the mounting plane. Asserts that
/// the ball's surface clearance to the curved bat is `gap_mm` (±0.05 mm).
pub fn on_banana(sim: &Simulation, name: &str, s: f32, gap_mm: f32, profile: &str) -> [f32; 3] {
    let f = banana_frame(sim, name);
    let mut normal = f.concave_normal(s);
    if dot3(normal, up_vector()) < 0.0 {
        normal = scale3(normal, -1.0);
    }
    let r = radius_m(profile);
    let off = scale3(normal, f.tube + r + gap_mm * 1e-3);
    let centre = add3(add3(f.point(s), off), scale3(f.n, r));
    let clearance_mm = (f.side_clearance(centre) - r) * 1e3;
    assert!(
        (clearance_mm - gap_mm).abs() <= 0.05,
        "{name} offset {s}: ball placed {clearance_mm:.3} mm clear of the curved bat (want {gap_mm} mm)"
    );
    centre
}

/// `tables/feature_lab/README.md` (the brief allows reading it).
pub fn readme_text() -> String {
    let p = repo_root().join("tables/feature_lab/README.md");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// One row of the README's mode table "mode | selector index | start
/// condition | shot switches | scene" (cells with backticks stripped).
#[derive(Clone, Debug)]
pub struct ModeRow {
    pub mode: String,
    pub index: String,
    pub condition: String,
    pub shot_switches: String,
    pub scene: String,
}

fn cells(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|')
        .map(|c| c.trim().replace('`', "").trim().to_owned())
        .collect()
}

/// The README's mode table: the first Markdown table whose header names
/// "mode" and "selector" columns. Columns are found by header name.
pub fn readme_mode_rows() -> Vec<ModeRow> {
    let text = readme_text();
    let lines: Vec<&str> = text.lines().collect();
    for (i, l) in lines.iter().enumerate() {
        if !l.trim_start().starts_with('|') {
            continue;
        }
        let head: Vec<String> = cells(l).iter().map(|c| c.to_ascii_lowercase()).collect();
        let col = |pred: &dyn Fn(&str) -> bool| head.iter().position(|h| pred(h));
        let (Some(m), Some(sel)) = (
            col(&|h| h == "mode" || h.starts_with("mode")),
            col(&|h| h.contains("selector")),
        ) else {
            continue;
        };
        let cond = col(&|h| h.contains("start") || h.contains("condition"));
        let shot = col(&|h| h.contains("shot") || h.contains("switch"));
        let scene = col(&|h| h.contains("scene"));
        let mut rows = Vec::new();
        for r in lines.iter().skip(i + 1) {
            if !r.trim_start().starts_with('|') {
                break;
            }
            let c = cells(r);
            if c.iter()
                .all(|x| x.chars().all(|ch| matches!(ch, '-' | ':' | ' ')))
            {
                continue; // the separator row
            }
            let get = |k: Option<usize>| k.and_then(|k| c.get(k)).cloned().unwrap_or_default();
            rows.push(ModeRow {
                mode: get(Some(m)),
                index: get(Some(sel)),
                condition: get(cond),
                shot_switches: get(shot),
                scene: get(scene),
            });
        }
        return rows;
    }
    panic!("README.md has no mode table with \"mode\" and \"selector\" columns");
}

/// The README row of `mode`.
pub fn readme_row(mode: &str) -> ModeRow {
    readme_mode_rows()
        .into_iter()
        .find(|r| r.mode == mode)
        .unwrap_or_else(|| panic!("README mode table has a row for {mode}"))
}

/// Declared switch names mentioned in `text`, in order of appearance
/// (identifier tokens matched exactly against `[[switches]]` ids).
pub fn switches_in(text: &str) -> Vec<String> {
    let names: Vec<&str> = table().switches.iter().map(|s| s.name.as_str()).collect();
    let mut out = Vec::new();
    for tok in text
        .split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '.' | '[' | ']')))
    {
        let tok = tok.trim_matches(|c: char| c == '.' || c == ':');
        if names.contains(&tok) && !out.iter().any(|o: &String| o == tok) {
            out.push(tok.to_owned());
        }
    }
    out
}

/// Copies folder `from` into `to` recursively.
pub fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create copy dir");
    let mut entries: Vec<_> = std::fs::read_dir(from)
        .expect("read bundle dir")
        .map(|e| e.expect("dir entry").path())
        .collect();
    entries.sort();
    for p in entries {
        let dst = to.join(p.file_name().expect("file name"));
        if p.is_dir() {
            copy_dir(&p, &dst);
        } else {
            std::fs::copy(&p, &dst).unwrap_or_else(|e| panic!("copy {}: {e}", p.display()));
        }
    }
}

/// A copy of the bundle whose `table.toml` is `edit`ed as a TOML table.
pub fn modified_bundle(tag: &str, edit: impl FnOnce(&mut toml::Table)) -> PathBuf {
    let dir = scratch(tag);
    copy_dir(bundle(), &dir);
    let text = std::fs::read_to_string(dir.join("table.toml")).expect("table.toml");
    let mut t: toml::Table = text.parse().expect("table.toml is TOML");
    edit(&mut t);
    std::fs::write(
        dir.join("table.toml"),
        toml::to_string(&t).expect("serialise"),
    )
    .expect("write table.toml");
    dir
}
