//! a1-glass, test (i): the playfield glass keeps balls launched upward on
//! the table and changes nothing before a ball reaches it.
//!
//! A sweep on the grey-box bundle (built from `tables/greybox` into the
//! target dir by `tables/greybox/build.sh`): a steel ball spawned on the
//! playfield with an upward speed vz of 1–6 m/s (5 values), a horizontal
//! speed of 0–3 m/s (10 values) and 16 directions, followed 900 ticks.
//!
//! **No-lid baseline.** `SimConfig` has no switch to leave the glass out,
//! so the baseline is the same bundle with one extra dispenser whose exit
//! is 3 m above the playfield (`[[dispensers]] glass_lid_raise`) and no
//! `glass.toml` (the grey box ships one, DECISIONS "a1-glass-height —
//! grey-box walls to the glass"). Dispenser
//! exits feed the table bounds, so that bundle's glass sits above 3 m, out
//! of reach of every ball of the sweep (a 6 m/s launch rises < 1.9 m); the
//! dispenser is never used.
//!
//! Per run:
//! * no ball passes the glass (its centre stays `r` below the underside, so
//!   nothing escapes through the top);
//! * until the first tick at which the ball's top reaches the glass
//!   underside, the ball position (f32 bits) and `state_hash` are identical
//!   to the baseline's;
//! * the glass adds no loss to a run that never reaches it: a run whose ball
//!   escapes with the glass lost it before a1-glass (it escaped in the
//!   baseline, or its centre rose above the glass underside) or its top
//!   reached the glass height (from there on the run may differ).
//!
//! The grey box's walls are 90 mm tall under a glass at 112 mm (a 22 mm gap,
//! under one ball diameter), so they seal against the glass; earlier
//! grey boxes with lower walls lost balls sideways, which are not the
//! glass's to stop (DECISIONS "a1-glass — playfield glass"). The sweep
//! checks that the glass never adds a loss and saves some.

use pin_sim::load::{BOUNDS_MARGIN_M, GLASS_THICKNESS_M};
use pin_sim::{SimConfig, Simulation};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Standard steel ball radius, m.
const R: f32 = 0.0135;
/// Spawn point: open playfield between the drop target and the magnet, m.
const SPAWN: [f32; 3] = [0.260, 0.460, R];
/// Ticks each run is followed.
const TICKS: u32 = 900;
const VZ: [f32; 5] = [1.0, 2.25, 3.5, 4.75, 6.0];
const DIRECTIONS: u32 = 16;
/// Height of the baseline's extra dispenser exit, mm.
const RAISE_MM: f32 = 3000.0;
/// How far a ball may press into the glass in a contact, m (the solver
/// resolves contacts to within a few µm; observed: none).
const CONTACT_TOL_M: f32 = 1.0e-4;

fn horizontal_speeds() -> [f32; 10] {
    core::array::from_fn(|i| 3.0 * i as f32 / 9.0)
}

/// The grey-box bundle and the raised-glass baseline, built once per test
/// binary into the target dir.
fn bundles() -> &'static (PathBuf, PathBuf) {
    static DIRS: OnceLock<(PathBuf, PathBuf)> = OnceLock::new();
    DIRS.get_or_init(|| {
        let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("a1_glass_lid_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let lid = base.join("greybox");
        build_greybox(&lid);
        let raised = base.join("greybox_raised");
        copy_dir(&lid, &raised);
        // The baseline removes the bundle's glass.toml, so its glass follows
        // the (raised) bounds.
        let glass = raised.join("glass.toml");
        assert!(glass.is_file(), "the grey-box bundle has no glass.toml");
        std::fs::remove_file(&glass).expect("remove glass.toml");
        let toml = raised.join("table.toml");
        let mut text = std::fs::read_to_string(&toml).expect("table.toml");
        text.push_str(&format!(
            "\n[[dispensers]]\nid = \"glass_lid_raise\"\nnumber = 99\n\
             exit_position = [257.0, 535.0, {RAISE_MM:.1}]\nexit_velocity = [0.0, 0.0, 0.0]\n\
             contents = [\"standard_steel\"]\n"
        ));
        std::fs::write(&toml, text).expect("write table.toml");
        (lid, raised)
    })
}

/// Runs `tables/greybox/build.sh <out>` with the cargo that runs this test
/// first on `PATH` (pin_sim has no pin_tablegen dev-dependency).
fn build_greybox(out: &Path) {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tables/greybox/build.sh");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| env!("CARGO").into());
    let mut path = Vec::new();
    if let Some(dir) = Path::new(&cargo).parent() {
        path.push(dir.to_path_buf());
    }
    if let Some(p) = std::env::var_os("PATH") {
        path.extend(std::env::split_paths(&p));
    }
    let status = std::process::Command::new("sh")
        .arg(&script)
        .arg(out)
        .env("PATH", std::env::join_paths(path).expect("PATH"))
        .status()
        .expect("run tables/greybox/build.sh");
    assert!(status.success(), "tables/greybox/build.sh failed: {status}");
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Runs `f` on a thread with a big stack (the Lua rules and the solver).
fn on_big_stack<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(f)
        .expect("spawn")
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e))
}

/// Underside of the glass of `sim` (from its escape box), m.
fn glass_underside(sim: &Simulation) -> f32 {
    sim.escape_bounds().1[2] - GLASS_THICKNESS_M - BOUNDS_MARGIN_M
}

/// One tick of a run: the ball's position bits (None once removed) and the
/// state hash.
type Sample = (Option<[u32; 3]>, u64);

/// One run of a ball.
struct Run {
    samples: Vec<Sample>,
    /// Balls the simulation removed as escaped.
    escaped: u32,
    /// Highest centre z of the ball (m), including the point it was heading
    /// for (`p + v·dt`) when it was removed.
    peak_z: f32,
    /// Underside of this bundle's glass, m.
    underside: f32,
}

/// Spawns the ball with `v` and follows it for `ticks`.
fn run(bundle: &Path, v: [f32; 3], ticks: u32) -> Run {
    let mut s = Simulation::load(bundle, SimConfig::default()).expect("load grey-box");
    let underside = glass_underside(&s);
    let h = s.spawn_ball("standard_steel", SPAWN, v).expect("spawn");
    let mut samples = Vec::with_capacity(ticks as usize);
    let mut peak_z = SPAWN[2];
    let mut last = None;
    for _ in 0..ticks {
        s.advance(1);
        let b = s.ball(h);
        samples.push((b.map(|b| b.position_m.map(f32::to_bits)), s.state_hash()));
        match b {
            Some(b) => {
                peak_z = peak_z.max(b.position_m[2]);
                last = Some(b);
            }
            None => {
                if let Some(b) = last.take() {
                    peak_z = peak_z.max(b.position_m[2] + b.velocity_m_s[2] * 1.0e-3);
                }
            }
        }
    }
    Run {
        samples,
        escaped: s.counters().balls_escaped,
        peak_z,
        underside,
    }
}

#[derive(Debug, Default)]
struct Totals {
    runs: u32,
    /// Runs whose ball reached the glass height.
    reached: u32,
    compared_ticks: u64,
    /// Runs that lose the ball without the glass (escaped, or centre above
    /// the glass underside) and with the glass.
    lost_before: u32,
    lost_with: u32,
}

fn sweep(dirs: core::ops::Range<u32>) {
    on_big_stack(move || {
        let (lid, raised) = bundles().clone();
        let mut t = Totals::default();
        for vz in VZ {
            for vh in horizontal_speeds() {
                for k in dirs.clone() {
                    let a = 2.0 * core::f32::consts::PI * k as f32 / DIRECTIONS as f32;
                    let v = [vh * a.cos(), vh * a.sin(), vz];
                    let what = format!("vz {vz} m/s, vh {vh:.3} m/s, direction {k}/16");
                    let with = run(&lid, v, TICKS);
                    let base = run(&raised, v, TICKS);
                    let underside = with.underside;
                    assert!(
                        base.underside > RAISE_MM * 1.0e-3,
                        "baseline glass at {} m",
                        base.underside
                    );
                    t.runs += 1;
                    // Nothing passes the glass: no escape through the top.
                    assert!(
                        with.peak_z + R <= underside + CONTACT_TOL_M,
                        "{what}: ball centre reached {} m, glass underside {underside} m",
                        with.peak_z
                    );
                    // Bit-identical until the ball's top reaches the glass.
                    let n = base
                        .samples
                        .iter()
                        .position(|s| s.0.is_some_and(|p| f32::from_bits(p[2]) + R >= underside))
                        .unwrap_or(base.samples.len());
                    if n < base.samples.len() {
                        t.reached += 1;
                    }
                    for i in 0..n {
                        assert_eq!(
                            with.samples[i],
                            base.samples[i],
                            "{what}: tick {} differs from the no-lid run before the ball reaches the glass",
                            i + 1
                        );
                    }
                    t.compared_ticks += n as u64;
                    // The glass adds no loss to a run that never reaches it.
                    let lost_before = base.escaped > 0 || base.peak_z > underside;
                    let touches = base.peak_z + R >= underside;
                    t.lost_before += u32::from(lost_before);
                    t.lost_with += u32::from(with.escaped > 0);
                    assert!(
                        with.escaped == 0 || lost_before || touches,
                        "{what}: the ball is lost with the glass but never reached it"
                    );
                }
            }
        }
        println!(
            "directions {dirs:?}: {} runs, {} reached the glass, {} ticks compared; \
             balls lost without the glass {}, with the glass {}",
            t.runs, t.reached, t.compared_ticks, t.lost_before, t.lost_with
        );
        assert!(t.reached > 0, "no ball reached the glass");
        assert!(t.lost_with < t.lost_before, "the glass saved no ball");
    });
}

#[test]
fn glass_lid_sweep_directions_0_to_3() {
    sweep(0..4);
}

#[test]
fn glass_lid_sweep_directions_4_to_7() {
    sweep(4..8);
}

#[test]
fn glass_lid_sweep_directions_8_to_11() {
    sweep(8..12);
}

#[test]
fn glass_lid_sweep_directions_12_to_15() {
    sweep(12..16);
}

#[test]
fn glass_sits_at_the_glass_toml_height() {
    on_big_stack(|| {
        let (lid, _) = bundles().clone();
        let s = Simulation::load(&lid, SimConfig::default()).expect("load grey-box");
        let (lo, hi) = s.escape_bounds();
        let under = glass_underside(&s);
        // The grey-box glass.toml puts the glass underside at 112 mm, 22 mm
        // above its 90 mm walls; the escape box's floor and sides are the
        // usual 50 mm margin.
        assert!((under - 0.112).abs() < 1.0e-6, "glass underside {under} m");
        assert!(lo[2] < 0.0 && hi[2] > under + GLASS_THICKNESS_M);
        assert!(lo[0] < 0.0 && hi[0] > 0.514 && lo[1] < 0.0 && hi[1] > 1.07);
        // A ball parked just under the glass falls back; one parked above
        // its top face (inside the escape box) is not removed either way.
        let mut s = s;
        let h = s
            .spawn_ball(
                "standard_steel",
                [0.26, 0.46, under - R - 0.001],
                [0.0, 0.0, 2.0],
            )
            .unwrap();
        let mut max_z = 0.0f32;
        for _ in 0..200 {
            s.advance(1);
            if let Some(b) = s.ball(h) {
                max_z = max_z.max(b.position_m[2]);
            }
        }
        assert_eq!(s.counters().balls_escaped, 0);
        assert!(
            max_z + R <= under + 1.0e-4,
            "ball top {} above the glass {under}",
            max_z + R
        );
        assert!(s.ball(h).unwrap().position_m[2] < under - R - 0.005);
    });
}
