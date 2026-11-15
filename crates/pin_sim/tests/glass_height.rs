//! a1-glass-height: `glass.toml` `height` (mm, CR-007) places the
//! playfield glass underside.
//!
//! The bundles are copies of the 2.9 `autoplay/clean` fixture (a closed
//! 514 x 1070 mm playfield with 28 mm walls) in `CARGO_TARGET_TMPDIR`, with
//! `layout.toml` / `glass.toml` edited:
//!
//! * `glass_60`: `glass.toml` `height = 60` (the glb is unchanged; walls 28 mm);
//! * `glass_10`: `height = 10`, below the 28 mm walls;
//! * `tall_walls_glass` / `tall_walls_default`: `wall_height` raised to
//!   92 mm and the glb and generated table entries regenerated with
//!   `pin_tablegen` (a nested `cargo run`, as `glass_lid.rs` runs
//!   `build.sh`); the first sets `glass.toml` `height = 112` (gap 20 mm, under the
//!   27 mm ball diameter), the second has no `glass.toml` (the glass then sits
//!   50 mm above the geometry, a gap a ball fits through).
//!
//! The sweeps launch one steel ball per run from the open playfield with an
//! upward speed (4 values), a horizontal speed (4 values) and 8 directions:
//! 128 runs per bundle, each followed for 700 ticks.

use pin_sim::load::{BOUNDS_MARGIN_M, GLASS_THICKNESS_M};
use pin_sim::{SimConfig, SimError, Simulation};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Standard steel ball radius, m.
const R: f32 = 0.0135;
/// Ball diameter, m: a wall seals against the glass only below this gap.
const BALL_DIAMETER_M: f32 = 2.0 * R;
/// Spawn point: open playfield below the bumper, m.
const SPAWN: [f32; 3] = [0.257, 0.450, R];
const TICKS: u32 = 700;
const VZ: [f32; 4] = [1.0, 2.0, 3.0, 4.0];
const VH: [f32; 4] = [0.5, 1.5, 2.5, 3.5];
const DIRECTIONS: u32 = 8;
/// How far a ball may press into the glass in a contact, m.
const CONTACT_TOL_M: f32 = 1.0e-4;
/// Glass heights and the raised wall height, mm.
const GLASS_LOW_MM: f32 = 60.0;
const GLASS_BELOW_MM: f32 = 10.0;
const GLASS_TALL_MM: f32 = 112.0;
const TALL_WALL_MM: f32 = GLASS_TALL_MM - 20.0;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/autoplay/clean")
}

fn base_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("a1_glass_height_{}", std::process::id()))
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

/// Rewrites `wall_height` (when given) in the `[playfield]` table of
/// `dir/layout.toml`, and writes `dir/glass.toml` with `height = glass_mm`
/// (or removes it when `None`).
fn edit_bundle(dir: &Path, wall_mm: Option<f32>, glass_mm: Option<f32>) {
    let path = dir.join("layout.toml");
    if let Some(w) = wall_mm {
        let text = std::fs::read_to_string(&path).expect("layout.toml");
        let mut out = String::new();
        for line in text.lines() {
            let key = line.split('=').next().unwrap_or("").trim();
            if key == "wall_height" {
                out.push_str(&format!("wall_height = {w:.1}\n"));
            } else {
                out.push_str(line);
                out.push('\n');
            }
        }
        assert!(
            out.contains(&format!("wall_height = {w:.1}")),
            "no wall_height in {}",
            path.display()
        );
        std::fs::write(&path, out).expect("write layout.toml");
    }
    let glass = dir.join("glass.toml");
    match glass_mm {
        Some(g) => std::fs::write(&glass, format!("schema_version = 1\nheight = {g:.1}\n"))
            .expect("write glass.toml"),
        None => {
            let _ = std::fs::remove_file(&glass);
        }
    }
}

/// Regenerates the glb and the generated `table.toml` entries of `dir`
/// from its `layout.toml` with the cargo that runs this test.
fn regenerate(dir: &Path) {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| env!("CARGO").into());
    let status = std::process::Command::new(cargo)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["run", "-q", "-p", "pin_tablegen", "--"])
        .arg(dir.join("layout.toml"))
        .arg("--table")
        .arg(dir.join("table.toml"))
        .arg("--out")
        .arg(dir)
        .arg("--force")
        .status()
        .expect("run pin_tablegen");
    assert!(status.success(), "pin_tablegen failed: {status}");
}

/// A copy of the fixture with `layout.toml` / `glass.toml` edited, built once per binary.
fn bundle(name: &str, glass_mm: Option<f32>) -> PathBuf {
    let dir = base_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(&fixture(), &dir);
    edit_bundle(&dir, None, glass_mm);
    dir
}

/// The fixture with 92 mm walls (regenerated): `(glass 112 mm, no glass_height)`.
fn tall_wall_bundles() -> &'static (PathBuf, PathBuf) {
    static DIRS: OnceLock<(PathBuf, PathBuf)> = OnceLock::new();
    DIRS.get_or_init(|| {
        let with = base_dir().join("tall_walls_glass");
        let _ = std::fs::remove_dir_all(&with);
        copy_dir(&fixture(), &with);
        edit_bundle(&with, Some(TALL_WALL_MM), Some(GLASS_TALL_MM));
        regenerate(&with);
        let without = base_dir().join("tall_walls_default");
        let _ = std::fs::remove_dir_all(&without);
        copy_dir(&with, &without);
        edit_bundle(&without, None, None);
        (with, without)
    })
}

/// Runs `f` on a thread with a big stack (the Lua rules and the solver).
fn on_big_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(f)
        .expect("spawn")
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e))
}

fn load(dir: &Path) -> Result<Simulation, SimError> {
    Simulation::load(dir, SimConfig::default())
}

/// Underside of the glass of `sim` (from its escape box), m.
fn glass_underside(sim: &Simulation) -> f32 {
    sim.escape_bounds().1[2] - GLASS_THICKNESS_M - BOUNDS_MARGIN_M
}

/// The geometry's max z of a bundle without `glass_height` (its glass
/// underside less the bounds margin), m.
fn geometry_max_z(sim: &Simulation) -> f32 {
    glass_underside(sim) - BOUNDS_MARGIN_M
}

#[derive(Debug, Default)]
struct Sweep {
    runs: u32,
    /// Highest live ball-centre z over all runs, m.
    peak_z: f32,
    /// Runs whose ball the simulation removed as escaped.
    escaped: u32,
    /// Escaped runs whose last live position was within one ball diameter
    /// of a side of the escape box (left sideways, not through the floor).
    escaped_sideways: u32,
}

/// The launch sweep on `dir`.
fn sweep(dir: &Path) -> Sweep {
    let mut out = Sweep::default();
    for vz in VZ {
        for vh in VH {
            for k in 0..DIRECTIONS {
                let a = 2.0 * core::f32::consts::PI * (k as f32 + 0.5) / DIRECTIONS as f32;
                let mut s = load(dir).expect("load");
                let (lo, hi) = s.escape_bounds();
                let h = s
                    .spawn_ball("standard_steel", SPAWN, [vh * a.cos(), vh * a.sin(), vz])
                    .expect("spawn");
                let mut last = SPAWN;
                for _ in 0..TICKS {
                    s.advance(1);
                    match s.ball(h) {
                        Some(b) => {
                            last = b.position_m;
                            out.peak_z = out.peak_z.max(b.position_m[2]);
                        }
                        None => break,
                    }
                }
                out.runs += 1;
                if s.counters().balls_escaped > 0 {
                    out.escaped += 1;
                    let near_side = (0..2).any(|i| {
                        last[i] - lo[i] < BALL_DIAMETER_M || hi[i] - last[i] < BALL_DIAMETER_M
                    });
                    out.escaped_sideways += u32::from(near_side);
                }
            }
        }
    }
    out
}

#[test]
fn glass_height_below_the_geometry_is_a_build_error() {
    on_big_stack(|| {
        let default = load(&fixture()).expect("load fixture");
        let max_z = geometry_max_z(&default);
        assert!(
            max_z > GLASS_BELOW_MM * 1.0e-3,
            "fixture max z {max_z} m is not above {GLASS_BELOW_MM} mm"
        );
        let dir = bundle("glass_10", Some(GLASS_BELOW_MM));
        match load(&dir) {
            Err(SimError::Build(msg)) => {
                assert!(msg.contains("glass.toml height"), "{msg}");
                assert!(msg.contains("10.000"), "{msg}");
                assert!(msg.contains("max z"), "{msg}");
            }
            Err(e) => panic!("expected a build error, got {e}"),
            Ok(_) => panic!("glass_height {GLASS_BELOW_MM} mm below max z {max_z} m loaded"),
        }
    });
}

#[test]
fn glass_height_sets_the_underside_and_the_escape_top() {
    on_big_stack(|| {
        let default = load(&fixture()).expect("load fixture");
        let (dlo, dhi) = default.escape_bounds();
        let max_z = geometry_max_z(&default);
        for g in [GLASS_LOW_MM, 250.0] {
            assert!(g * 1.0e-3 >= max_z);
            let dir = bundle(&format!("glass_{g}"), Some(g));
            let s = load(&dir).expect("load with glass_height");
            let (lo, hi) = s.escape_bounds();
            let x = g * 1.0e-3;
            assert!(
                (hi[2] - (x + GLASS_THICKNESS_M + BOUNDS_MARGIN_M)).abs() <= 1.0e-6,
                "escape top {} for glass_height {g} mm",
                hi[2]
            );
            assert!((glass_underside(&s) - x).abs() <= 1.0e-6);
            // x/y and the floor are the default box's.
            assert_eq!(lo, dlo);
            assert_eq!([hi[0], hi[1]], [dhi[0], dhi[1]]);
        }
    });
}

#[test]
fn nothing_rises_through_a_glass_set_by_glass_height() {
    on_big_stack(|| {
        let dir = bundle("glass_60_sweep", Some(GLASS_LOW_MM));
        let under = GLASS_LOW_MM * 1.0e-3;
        let s = sweep(&dir);
        println!("glass {GLASS_LOW_MM} mm: {s:?}");
        assert!(s.runs >= 100);
        assert!(
            s.peak_z <= under - R + CONTACT_TOL_M,
            "ball centre reached {} m under a glass at {under} m",
            s.peak_z
        );
        // The sweep reaches the glass (it tests something).
        assert!(s.peak_z >= under - R - 1.0e-3, "peak {} m", s.peak_z);
    });
}

#[test]
fn walls_within_a_ball_diameter_of_the_glass_lose_no_ball_sideways() {
    on_big_stack(|| {
        let (with, without) = tall_wall_bundles().clone();
        const { assert!((GLASS_TALL_MM - TALL_WALL_MM) * 1.0e-3 < BALL_DIAMETER_M) };
        let under = GLASS_TALL_MM * 1.0e-3;
        let sealed = sweep(&with);
        println!("walls {TALL_WALL_MM} mm, glass {GLASS_TALL_MM} mm: {sealed:?}");
        assert!(sealed.runs >= 100);
        assert_eq!(sealed.escaped, 0, "{sealed:?}");
        assert!(sealed.peak_z <= under - R + CONTACT_TOL_M, "{sealed:?}");
        assert!(sealed.peak_z >= under - R - 1.0e-3, "{sealed:?}");
        // Control: same walls, glass at the default 50 mm above the
        // geometry, a gap a ball fits through.
        let s = load(&without).expect("load control");
        let gap = glass_underside(&s) - TALL_WALL_MM * 1.0e-3;
        assert!(gap > BALL_DIAMETER_M, "control gap {gap} m");
        let open = sweep(&without);
        println!("walls {TALL_WALL_MM} mm, default glass: {open:?}");
        assert!(open.escaped_sideways > 0, "{open:?}");
    });
}

#[test]
fn malformed_glass_toml_is_a_build_error_naming_the_file() {
    on_big_stack(|| {
        let cases = [
            ("zero_height", "schema_version = 1\nheight = 0.0\n"),
            (
                "unknown_key",
                "schema_version = 1\nheight = 60.0\nextra = 1\n",
            ),
            ("bad_version", "schema_version = 2\nheight = 60.0\n"),
        ];
        for (name, body) in cases {
            let dir = base_dir().join(format!("glass_bad_{name}"));
            let _ = std::fs::remove_dir_all(&dir);
            copy_dir(&fixture(), &dir);
            std::fs::write(dir.join("glass.toml"), body).unwrap();
            match load(&dir) {
                Err(SimError::Build(msg)) => assert!(msg.contains("glass.toml"), "{name}: {msg}"),
                Err(e) => panic!("{name}: expected a build error, got {e}"),
                Ok(_) => panic!("{name}: malformed glass.toml loaded"),
            }
        }
    });
}
