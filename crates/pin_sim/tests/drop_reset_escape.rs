//! a1-glass, test (ii): balls over the grey-box drop target while its reset
//! coil lifts it must not leave the table through the top.
//!
//! The Gate A1 soaks lost 6 balls (EscapedBall) to the drop-target reset:
//! a ball rolling onto the rising target's edge is launched upward (a
//! correct curb-edge contact, e = 0.5, μ = 0.2) above the old escape ceiling
//! (geometry bounds + 50 mm). With the playfield glass the ball bounces back
//! off the glass.
//!
//! The sweep (the Investigator's probe as a test): ball speeds 0–3.5 m/s ×
//! 8 directions × arrival phases 0–64 ticks after the rise starts ×
//! perpendicular offsets −10 / 0 / +10 mm, each trial on a fresh
//! `Simulation` of the grey-box bundle (built from `tables/greybox` into the
//! target dir by `tables/greybox/build.sh`) and of the same bundle with its
//! glass lifted out of reach (one unused dispenser 3 m up; see
//! `glass_lid.rs`), which behaves as the table without a glass. Per trial:
//! * no ball passes the glass (no escape through the top); before
//!   a1-glass 105 of the 432 trials at 0° and 45° lost the ball, most of
//!   them through the top;
//! * the glass adds no loss to a trial that never reaches it: a trial
//!   that loses its ball with the glass lost it before a1-glass too
//!   (escaped, or its centre rose above the old ceiling, which is the glass
//!   underside), or its top reached the glass height.
//!
//! Some balls still leave sideways (over the 28 mm perimeter walls) or drop
//! through the floor as layer-2 balls at the ramp entry (main's grey box;
//! fixed by a1/fix). Neither is the glass's to stop (DECISIONS "a1-glass —
//! playfield glass"); the test prints their count.

use pin_core::snapshot::BallState;
use pin_sim::load::{BOUNDS_MARGIN_M, GLASS_THICKNESS_M};
use pin_sim::{SimConfig, Simulation};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Standard steel ball radius, m.
const R: f32 = 0.0135;
/// Grey-box `drop_1` position (table.toml `[[mechs]] drop_1`), m.
const CX: f32 = 0.200;
const CY: f32 = 0.470;
/// Rise time of a reset drop target in ticks: it starts 2 mm below the
/// playfield (`DROP_HIDE`) and rises its 32 mm height at the 0.5 m/s
/// `[mechs.drop_target] kick_speed`: 34 mm / 0.5 m/s = 68 ms.
const RISE_TICKS: u64 = 68;
/// Ticks between placing a moving ball and its arrival over the target
/// centre (the probe's lead).
const LEAD: i64 = 25;
/// Ticks each trial follows the ball after it is placed.
const FOLLOW: i64 = 600;

const SPEEDS: [f32; 8] = [0.0, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5];
const PHASES: [i64; 9] = [0, 8, 16, 24, 32, 40, 48, 56, 64];
const OFFSETS_MM: [f32; 3] = [-10.0, 0.0, 10.0];
/// Height of the baseline's extra dispenser exit, mm.
const RAISE_MM: f32 = 3000.0;
/// How far a ball may press into the glass in a contact, m.
const CONTACT_TOL_M: f32 = 1.0e-4;

/// The grey-box bundle and the raised-glass baseline, built once per test
/// binary into the target dir.
fn bundles() -> &'static (PathBuf, PathBuf) {
    static DIRS: OnceLock<(PathBuf, PathBuf)> = OnceLock::new();
    DIRS.get_or_init(|| {
        let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("a1_glass_drop_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let lid = base.join("greybox");
        build_greybox(&lid);
        let raised = base.join("greybox_raised");
        copy_dir(&lid, &raised);
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

/// Runs `f` on a thread with a big stack (the Lua rules and the solver).
fn on_big_stack<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(f)
        .expect("spawn")
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e))
}

/// A started game with `drop_1` knocked down by a face hit; returns the sim
/// and the tick at which the target's switch closed.
fn dropped(bundle: &Path) -> (Simulation, u64) {
    let mut s = Simulation::load(bundle, SimConfig::default()).expect("load grey-box");
    s.set_switch("start", true).unwrap();
    s.advance(5);
    s.release_switch("start").unwrap();
    s.advance(1500);
    let h = s
        .spawn_ball("standard_steel", [CX, CY - 0.03, R], [0.0, 1.5, 0.0])
        .unwrap();
    for _ in 0..200 {
        s.advance(1);
        if s.switch_closed("drop_1_sw") == Some(true) {
            let t = s.tick();
            s.remove_ball(h);
            return (s, t);
        }
    }
    panic!("drop_1 did not drop");
}

/// Tick at which the reset rise starts (calibrated once: the switch opens
/// when the target is fully up, `RISE_TICKS` after the rise starts).
fn rise_start() -> u64 {
    static T: OnceLock<u64> = OnceLock::new();
    *T.get_or_init(|| {
        let (mut s, _) = dropped(&bundles().0);
        for _ in 0..3000 {
            s.advance(1);
            if s.switch_closed("drop_1_sw") == Some(false) {
                return s.tick() - RISE_TICKS;
            }
        }
        panic!("drop_1 never reset");
    })
}

/// A ball rolling without slipping on the playfield.
fn rolling(p: [f32; 3], v: [f32; 3]) -> BallState {
    BallState {
        position_m: p,
        velocity_m_s: v,
        angular_velocity_rad_s: [-v[1] / R, v[0] / R, 0.0],
        ..BallState::default()
    }
}

#[derive(Debug, Clone, Copy)]
struct Trial {
    speed: f32,
    angle_deg: u32,
    phase: i64,
    offset_mm: f32,
}

/// What one trial saw.
#[derive(Debug, Clone, Copy)]
struct Outcome {
    /// Balls the simulation removed as escaped.
    escaped: u32,
    /// The ball's highest centre z (m), including the point it was heading
    /// for (`p + v·dt`) when it was removed.
    peak_z: f32,
    /// The ball's last observed position (m) and layer.
    last: Option<([f32; 3], u8)>,
    /// Underside of the bundle's glass, m.
    underside: f32,
}

/// Runs one trial on `bundle`.
fn run(bundle: &Path, t: Trial, t_rise: u64) -> Outcome {
    let (mut s, _) = dropped(bundle);
    let underside = s.escape_bounds().1[2] - GLASS_THICKNESS_M - BOUNDS_MARGIN_M;
    let a = (t.angle_deg as f32).to_radians();
    let d = [a.cos(), a.sin()];
    let dist = t.speed * LEAD as f32 * 1.0e-3;
    let off = t.offset_mm * 1.0e-3;
    let p = [
        CX - d[0] * dist - d[1] * off,
        CY - d[1] * dist + d[0] * off,
        R,
    ];
    let v = [d[0] * t.speed, d[1] * t.speed, 0.0];
    let at = u64::try_from(t_rise as i64 + t.phase - LEAD).expect("tick");
    assert!(s.tick() < at, "the target dropped too late for {t:?}");
    s.advance(at - s.tick());
    let h = s.spawn_ball("standard_steel", p, [0.0; 3]).unwrap();
    assert!(s.set_ball_state(h, &rolling(p, v)));
    let escaped0 = s.counters().balls_escaped;
    let mut peak_z = R;
    let mut last = None;
    let mut last_vz = 0.0f32;
    for _ in 0..FOLLOW {
        s.advance(1);
        match s.ball(h) {
            Some(b) => {
                peak_z = peak_z.max(b.position_m[2]);
                last = Some((b.position_m, b.layer));
                last_vz = b.velocity_m_s[2];
            }
            None => {
                if let Some((p, _)) = last {
                    peak_z = peak_z.max(p[2] + last_vz * 1.0e-3);
                }
                break;
            }
        }
    }
    Outcome {
        escaped: s.counters().balls_escaped - escaped0,
        peak_z,
        last,
        underside,
    }
}

/// The sweep for the directions `angles` (degrees).
fn sweep(angles: &[u32]) {
    let angles = angles.to_vec();
    on_big_stack(move || {
        let (lid, raised) = bundles().clone();
        let t_rise = rise_start();
        let mut trials = 0;
        let mut peak = 0.0f32;
        let (mut lost_before, mut lost_with) = (0, Vec::new());
        for &angle_deg in &angles {
            for speed in SPEEDS {
                for phase in PHASES {
                    for offset_mm in OFFSETS_MM {
                        let t = Trial {
                            speed,
                            angle_deg,
                            phase,
                            offset_mm,
                        };
                        let with = run(&lid, t, t_rise);
                        let base = run(&raised, t, t_rise);
                        let underside = with.underside;
                        assert!(base.underside > RAISE_MM * 1.0e-3);
                        trials += 1;
                        peak = peak.max(with.peak_z);
                        assert!(
                            with.peak_z + R <= underside + CONTACT_TOL_M,
                            "{t:?}: ball centre reached {} m, glass underside {underside} m",
                            with.peak_z
                        );
                        let before = base.escaped > 0 || base.peak_z > underside;
                        let touches = base.peak_z + R >= underside;
                        lost_before += u32::from(before);
                        if with.escaped > 0 {
                            lost_with.push((t, with.last));
                            assert!(
                                before || touches,
                                "{t:?}: the ball is lost with the glass (last seen at {:?}) \
                                 but never reached it",
                                with.last
                            );
                        }
                    }
                }
            }
        }
        println!(
            "directions {angles:?}: {trials} trials, highest ball centre {:.1} mm; balls lost \
             before a1-glass {lost_before}, with the glass {} (sideways or through the floor; \
             trial, last position and layer: {lost_with:?})",
            peak * 1.0e3,
            lost_with.len()
        );
        assert!(
            (lost_with.len() as u32) < lost_before,
            "the glass saved no ball"
        );
    });
}

#[test]
fn drop_reset_launch_stays_on_the_table_0_and_45_deg() {
    sweep(&[0, 45]);
}

#[test]
fn drop_reset_launch_stays_on_the_table_90_and_135_deg() {
    sweep(&[90, 135]);
}

#[test]
fn drop_reset_launch_stays_on_the_table_180_and_225_deg() {
    sweep(&[180, 225]);
}

#[test]
fn drop_reset_launch_stays_on_the_table_270_and_315_deg() {
    sweep(&[270, 315]);
}
