//! Shared helpers of the Subphase 6.1 spec tests (Independent Tester): the
//! "Apex Circuit" table built once per test binary, fresh simulations of it,
//! and the black-box drivers (cabinet inputs, forced switch pulses, drains,
//! persist reads) the `spec_6_1_*` targets share.
//!
//! The bundle is built with the documented one-command build
//! (`tables/apex_circuit/build.sh <out_dir>`, README "Build"). When the
//! script is missing or fails, the failure is recorded (the
//! `build_script_builds_the_bundle` test of `spec_6_1_structure` reports it)
//! and the bundle is built by [`build_fallback`], which mirrors the 6.0
//! helper (`tests/support/mod.rs`) on the `tables/apex_circuit` sources.

#![allow(dead_code)]

use pin_app::orchestrator::{ValidatedTable, load_checked, validate_table};
use pin_assetgen::{TableMeta, generate_kit};
use pin_assets::{LoadedTable, SwitchSi};
use pin_core::input::{CabinetInput, InputEvent};
use pin_core::schema::LayoutManifest;
use pin_core::schema::table::{SwitchKind, TableManifest};
use pin_core::sim::{SimStep, TICK_NS};
use pin_sim::{BallHandle, SimConfig, Simulation};
use pin_tablegen::layout::parse_layout;
use pin_tablegen::{generate, write_bundle};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Profile of the steel balls.
pub const STEEL: &str = "standard_steel";
/// Profile of the Powerball.
pub const POWERBALL: &str = "ceramic_powerball";
/// The table's id.
pub const TABLE_ID: &str = "apex_circuit";
/// Diameter of the steel ball the Zipper criterion is about, m.
pub const BALL_DIAMETER_M: f32 = 0.027;

/// `tables/apex_circuit` (the sources; only README.md and build.sh are
/// read by name here, the rest is handed to the build).
pub fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tables/apex_circuit")
}

/// Runs `f` on a thread with a generous stack (script backends recurse).
pub fn big_stack<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    std::thread::Builder::new()
        .name("spec-6.1-sim".into())
        .stack_size(64 << 20)
        .spawn(f)
        .expect("spawn")
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e))
}

/// Scratch directory under the test target dir, unique to this process.
pub fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("spec_6_1_{name}_{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clear scratch dir");
    }
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// Runs `tables/apex_circuit/build.sh <out>` (the README's one-command
/// build). `Err` carries the reason (missing script, exit status, output).
pub fn build_with_script(out: &Path) -> Result<(), String> {
    let script = src_dir().join("build.sh");
    if !script.is_file() {
        return Err(format!("{} does not exist", script.display()));
    }
    let o = std::process::Command::new("sh")
        .arg(&script)
        .arg(out)
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .output()
        .map_err(|e| format!("run {}: {e}", script.display()))?;
    if o.status.success() {
        Ok(())
    } else {
        Err(format!(
            "build.sh exited {}: stdout={} stderr={}",
            o.status,
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        ))
    }
}

fn parse_hex(c: &str) -> [u8; 3] {
    let h = c.trim_start_matches('#');
    let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).expect("palette colour");
    [b(0), b(2), b(4)]
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create dir");
    for entry in std::fs::read_dir(from).expect("read dir") {
        let entry = entry.expect("dir entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy file");
        }
    }
}

/// The fallback build (the brief's item 1 fallback): what
/// `tests/support/mod.rs` does for the grey-box, applied to
/// `tables/apex_circuit/{layout.toml, table.toml, rules.lua, backglass.toml,
/// glass.toml, display/}`.
pub fn build_fallback(out: &Path) {
    let src = src_dir();
    let layout_text = std::fs::read_to_string(src.join("layout.toml")).expect("layout.toml");
    let table_text = std::fs::read_to_string(src.join("table.toml")).expect("table.toml");
    let layout = parse_layout(&layout_text).expect("layout.toml parses");
    let base = TableManifest::from_toml_str(&table_text).expect("table.toml parses");
    let generated = generate(&layout, &base).unwrap_or_else(|e| panic!("pin_tablegen: {e}"));
    write_bundle(&generated, out).expect("write bundle");
    for f in ["rules.lua", "backglass.toml", "layout.toml", "glass.toml"] {
        std::fs::copy(src.join(f), out.join(f)).unwrap_or_else(|e| panic!("copy {f}: {e}"));
    }
    copy_dir(&src.join("display"), &out.join("display"));
    let meta = TableMeta {
        title: base.meta.name.clone(),
        palette: base.meta.palette.iter().map(|c| parse_hex(c)).collect(),
        seed: 0,
    };
    generate_kit(&meta, out).expect("placeholder kit");
}

/// Outcome of the documented build script in this process.
static SCRIPT_RESULT: OnceLock<Result<(), String>> = OnceLock::new();

/// The Apex Circuit bundle, built once per test binary.
pub fn bundle() -> &'static Path {
    static BUNDLE: OnceLock<PathBuf> = OnceLock::new();
    BUNDLE.get_or_init(|| {
        let out = scratch("apex_bundle");
        let r = build_with_script(&out);
        if let Err(e) = &r {
            eprintln!("spec 6.1 FINDING: documented build failed, using the fallback build: {e}");
            std::fs::remove_dir_all(&out).ok();
            std::fs::create_dir_all(&out).expect("recreate bundle dir");
            build_fallback(&out);
        }
        let _ = SCRIPT_RESULT.set(r);
        out
    })
}

/// Whether `build.sh` built the shared bundle (`Err` = the reason the
/// fallback was used).
pub fn script_result() -> &'static Result<(), String> {
    bundle();
    SCRIPT_RESULT.get().expect("set by bundle()")
}

/// `validate_table` on the bundle (manifests, glTF, clearance), once.
pub fn validated() -> &'static ValidatedTable {
    static V: OnceLock<ValidatedTable> = OnceLock::new();
    V.get_or_init(|| {
        validate_table(bundle()).unwrap_or_else(|e| panic!("apex_circuit validates: {e}"))
    })
}

/// The loaded table (SI).
pub fn table() -> &'static LoadedTable {
    &validated().table
}

/// The bundle's `layout.toml` (copied into the bundle by the build).
pub fn layout() -> &'static LayoutManifest {
    static L: OnceLock<LayoutManifest> = OnceLock::new();
    L.get_or_init(|| {
        let text =
            std::fs::read_to_string(bundle().join("layout.toml")).expect("bundle has layout.toml");
        parse_layout(&text).expect("bundle layout.toml parses")
    })
}

/// A fresh simulation of the validated bundle snapshot.
pub fn fresh() -> Simulation {
    load_checked(validated(), SimConfig::default()).expect("apex_circuit loads")
}

/// Radius of a built-in profile, m.
pub fn radius_of(profile: &str) -> f32 {
    pin_core::builtin_profile(profile)
        .expect("built-in profile")
        .radius_m()
}

// ---------------------------------------------------------------------------
// Health
// ---------------------------------------------------------------------------

/// No script fault, no queue overflow, no escaped and no non-finite ball.
pub fn assert_healthy(sim: &Simulation, what: &str) {
    assert!(
        !sim.script_faulted(),
        "{what}: script faulted: {:?}",
        sim.script_error()
    );
    assert_eq!(sim.script_fault_count(), 0, "{what}: script faults");
    assert_eq!(
        sim.script_queue_overflows(),
        0,
        "{what}: script queue overflow"
    );
    assert_eq!(sim.counters().balls_escaped, 0, "{what}: a ball escaped");
    for h in sim.ball_handles() {
        let b = sim.ball(h).expect("live handle");
        let finite = b.position_m.iter().all(|v| v.is_finite())
            && b.velocity_m_s.iter().all(|v| v.is_finite())
            && b.angular_velocity_rad_s.iter().all(|v| v.is_finite());
        assert!(finite, "{what}: non-finite ball {b:?}");
    }
}

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

/// One tick with cabinet input `input` pressed or released, through the
/// real input path (`SimStep::step_1ms`).
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

/// Closes switch `name` for `hold` ticks, opens it and hands it back to the
/// physics. A forced pulse is a rule event only, never evidence of a
/// physics contact.
pub fn pulse(sim: &mut Simulation, name: &str, hold: u64) {
    sim.set_switch(name, true).expect("known switch");
    sim.advance(hold);
    sim.set_switch(name, false).expect("known switch");
    sim.advance(5);
    sim.release_switch(name).expect("known switch");
}

/// Closing edges of switch `name` since load.
pub fn hits(sim: &Simulation, name: &str) -> u32 {
    sim.switch_hits(name)
        .unwrap_or_else(|| panic!("unknown switch {name}"))
}

/// Player 1's score (0 before any report).
pub fn score(sim: &Simulation) -> i64 {
    sim.score(0).unwrap_or(0)
}

/// Index of persist key `name` in the loaded manifest's `[persist]` order.
pub fn persist_index(name: &str) -> u16 {
    let keys = &table()
        .manifest
        .persist
        .as_ref()
        .expect("[persist] section")
        .keys;
    let i = keys
        .iter()
        .position(|k| k == name)
        .unwrap_or_else(|| panic!("[persist] key {name} declared"));
    u16::try_from(i).expect("persist index fits u16")
}

/// The persistent value `name`.
pub fn persist(sim: &Simulation, name: &str) -> i64 {
    sim.persist(persist_index(name))
}

/// Steps until a ball is on the table; `false` after `max` ticks.
pub fn wait_for_ball(sim: &mut Simulation, max: u64) -> bool {
    for _ in 0..max {
        if sim.balls_in_play() > 0 {
            return true;
        }
        sim.advance(1);
    }
    sim.balls_in_play() > 0
}

/// Removes every ball (no drain: the game goes on).
pub fn clear_table(sim: &mut Simulation) {
    let handles: Vec<BallHandle> = sim.ball_handles().iter().copied().collect();
    for h in handles {
        assert!(sim.remove_ball(h));
    }
}

/// The trough switches, the positioned ones first.
pub fn trough_switches() -> Vec<&'static SwitchSi> {
    let mut v: Vec<&SwitchSi> = table()
        .switches
        .iter()
        .filter(|s| s.kind == SwitchKind::Trough)
        .collect();
    v.sort_by_key(|s| s.position_m.is_none());
    v
}

/// Steps until ball `h` is gone (≤ `max` ticks); whether it went.
fn gone_within(sim: &mut Simulation, h: BallHandle, max: u64) -> bool {
    for _ in 0..max {
        if sim.ball(h).is_none() {
            return true;
        }
        sim.advance(1);
    }
    sim.ball(h).is_none()
}

/// Drains every ball on the table through the first trough switch: moved
/// onto it (when it has a position) or removed with the switch pulsed. A
/// ball a mechanism holds (the scoop keeps its ball where it is) is taken
/// off and a steel ball is set down on the trough in its place, so the
/// drain is still a physical one.
pub fn drain_all(sim: &mut Simulation) {
    let trough = trough_switches()[0];
    let handles: Vec<BallHandle> = sim.ball_handles().iter().copied().collect();
    for h in handles {
        let Some(was) = sim.ball(h) else { continue };
        match trough.position_m {
            Some(p) => {
                let on_trough = [p[0], p[1], p[2] + radius_of(STEEL)];
                let mut b = was;
                b.position_m = on_trough;
                b.velocity_m_s = [0.0; 3];
                b.angular_velocity_rad_s = [0.0; 3];
                b.layer = trough.layer;
                assert!(sim.set_ball_state(h, &b), "move the ball onto the trough");
                if gone_within(sim, h, 200) {
                    continue;
                }
                assert!(sim.remove_ball(h), "take the held ball off");
                let n = sim
                    .spawn_ball_on_layer(STEEL, trough.layer, on_trough, [0.0; 3])
                    .expect("spawn on the trough");
                assert!(
                    gone_within(sim, n, 200),
                    "a ball set down on {} drains (held ball was {was:?})",
                    trough.name
                );
            }
            None => {
                assert!(sim.remove_ball(h));
                pulse(sim, &trough.name, 5);
            }
        }
    }
}

/// Presses `start` (real input path) and waits ≤ 10 s for the served ball.
pub fn start_game(sim: &mut Simulation) {
    press(sim, CabinetInput::Start, 50);
    assert!(wait_for_ball(sim, 10_000), "start serves a ball");
}

/// The profile name of the ball `h`.
pub fn profile_name(sim: &Simulation, profile: pin_core::ProfileId) -> String {
    sim.ids()
        .name(pin_assets::validate::IdKind::Profile, profile.0)
        .unwrap_or("?")
        .to_owned()
}

/// Profile names of the free balls in the last snapshot. Captive balls
/// appear as ordinary balls entries (SPEC Appendix A "Dynamic mesh slot
/// convention"), published after the free balls, so the free balls are
/// exactly `balls()[..balls_in_play()]`.
pub fn snapshot_profiles(sim: &Simulation) -> Vec<String> {
    let snap = sim.snapshot();
    let free = sim.balls_in_play().min(snap.balls().len());
    snap.balls()[..free]
        .iter()
        .map(|b| profile_name(sim, b.profile))
        .collect()
}

/// Whether the source of the README contains `needle` (case-insensitive).
pub fn readme() -> String {
    std::fs::read_to_string(src_dir().join("README.md")).unwrap_or_default()
}

/// The top-lane rollovers, left to right: the `rollover` switches named
/// `lane_<n>` (README: "three skill-shot top lanes (`lane_1..3`)"); when
/// there are fewer than 3 of them, the `rollover` switches in the top 35 %
/// of the playfield. (Decision log 2026-11-18-6.1-tester "6.1-T-1 — top-lane
/// identification".)
pub fn top_lanes() -> Vec<String> {
    let length = layout().playfield.length * 1.0e-3;
    let pick = |named: bool| -> Vec<String> {
        let mut v: Vec<(f32, String)> = table()
            .switches
            .iter()
            .filter(|s| s.kind == SwitchKind::Rollover)
            .filter_map(|s| {
                let p = s.position_m.unwrap_or([0.0; 3]);
                let ok = if named {
                    s.name
                        .strip_prefix("lane_")
                        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
                } else {
                    s.position_m.is_some() && p[1] >= 0.65 * length
                };
                ok.then(|| (p[0], s.name.clone()))
            })
            .collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        v.into_iter().map(|(_, n)| n).collect()
    };
    let named = pick(true);
    if named.len() >= 3 { named } else { pick(false) }
}

/// Ends the ball in play through the trough: drains every ball on the
/// table ([`drain_all`]); when the table is empty (the test took the ball
/// off), a steel ball is set down on the trough switch so the drain is a
/// physical one.
pub fn drain_ball(sim: &mut Simulation) {
    if sim.balls_in_play() == 0 {
        let trough = trough_switches()[0];
        let p = trough.position_m.expect("a positioned trough switch");
        sim.spawn_ball_on_layer(
            STEEL,
            trough.layer,
            [p[0], p[1], p[2] + radius_of(STEEL)],
            [0.0; 3],
        )
        .expect("spawn on the trough");
    }
    drain_all(sim);
}

/// Steps until every ball has stayed slower than 1 cm/s for 100
/// consecutive ticks (≤ `max` ticks): a served ball comes to rest on the
/// plunger tip.
pub fn settle(sim: &mut Simulation, max: u64) {
    let mut still = 0;
    for _ in 0..max {
        let moving = sim.ball_handles().iter().any(|&h| {
            sim.ball(h).is_some_and(|b| {
                let v = b.velocity_m_s;
                (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt() > 0.01
            })
        });
        still = if moving { 0 } else { still + 1 };
        if still >= 100 {
            return;
        }
        sim.advance(1);
    }
    panic!("settle: balls still moving after {max} ticks");
}

/// Plan-view distance, m.
pub fn plan_dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// The magnet's Powerball detector: (the eddy switch, the opto nearest to
/// it). Decision log "6.1-T-2 — the magnet's opto".
pub fn magnet_sensors() -> (&'static SwitchSi, &'static SwitchSi) {
    let t = table();
    let eddy = t
        .switches
        .iter()
        .find(|s| s.kind == SwitchKind::Eddy)
        .expect("an eddy switch");
    let pe = eddy.position_m.expect("eddy position");
    let opto = t
        .switches
        .iter()
        .filter(|s| s.kind == SwitchKind::Opto && s.position_m.is_some())
        .min_by(|a, b| {
            plan_dist(a.position_m.unwrap(), pe).total_cmp(&plan_dist(b.position_m.unwrap(), pe))
        })
        .expect("an opto switch");
    (eddy, opto)
}
