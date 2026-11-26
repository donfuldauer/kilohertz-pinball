//! Helpers of the KP-023 (#103) independent tests: bundles built from the
//! replay fixture with an explicit `[[mechs]]` list, a scripted rules file,
//! and quaternion read-outs. Own copies; nothing is shared with
//! `sim_support`.
#![allow(dead_code)]

use pin_core::snapshot::{MeshTransform, NEWTON_SWING_RANGE_RAD};
use pin_sim::Simulation;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// Ball radius of the standard steel profile, m.
pub const R: f32 = 0.0135;

/// The in-repo replay fixture bundle (a folder).
pub fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/replay_table")
}

/// Runs `f` on a thread with a 16 MiB stack.
pub fn on_sim_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(f)
        .expect("spawn sim thread")
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e))
}

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A scratch bundle directory removed when dropped.
#[derive(Debug)]
pub struct Bundle(PathBuf);

impl Bundle {
    /// The directory.
    pub fn path(&self) -> PathBuf {
        self.0.clone()
    }
}

impl Drop for Bundle {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Switches and coils every test table declares (unused ones are inert).
pub const COMMON: &str = r#"
[[switches]]
id = "sling_sw"
number = 20
kind = "leaf"
layer = 0

[[switches]]
id = "kick_sw"
number = 21
kind = "leaf"
layer = 0

[[switches]]
id = "drop_sw"
number = 22
kind = "target"
layer = 0

[[switches]]
id = "drop2_sw"
number = 23
kind = "target"
layer = 0

[[switches]]
id = "spin_sw"
number = 24
kind = "leaf"
layer = 0

[[switches]]
id = "vuk_sw"
number = 25
kind = "leaf"
layer = 0

[[switches]]
id = "plunger_sw"
number = 26
kind = "leaf"
layer = 0

[[switches]]
id = "gate_sw"
number = 27
kind = "leaf"
layer = 0

[[switches]]
id = "newton_sw"
number = 28
kind = "target"
layer = 0

[[switches]]
id = "ring2_sw"
number = 29
kind = "ring"
layer = 0

[[coils]]
id = "sling_coil"
number = 20
kind = "sling"

[[coils]]
id = "kick_coil"
number = 21
kind = "kicker"

[[coils]]
id = "drop_coil"
number = 22
kind = "drop_reset"

[[coils]]
id = "drop2_coil"
number = 23
kind = "drop_reset"

[[coils]]
id = "vuk_coil"
number = 24
kind = "vuk"

[[coils]]
id = "plunger_coil"
number = 25
kind = "autoplunger"

[[coils]]
id = "gate_coil"
number = 26
kind = "kicker"

[[coils]]
id = "ring2_coil"
number = 27
kind = "bumper"
"#;

/// Mech snippets (table.toml text). Positions in mm, angles in degrees.
pub const SLING: &str = r#"
[[mechs]]
id = "k_sling"
kind = "sling"
coil = "sling_coil"
switch = "sling_sw"
position = [100.0, 290.0, 0.0]
angle_deg = 35.0
"#;
/// Kickback mech.
pub const KICKBACK: &str = r#"
[[mechs]]
id = "k_kick"
kind = "kickback"
coil = "kick_coil"
switch = "kick_sw"
position = [480.0, 200.0, 0.0]
angle_deg = 120.0
"#;
/// Drop target with an explicit 40 mm height.
pub const DROP_40: &str = r#"
[[mechs]]
id = "k_drop"
kind = "drop_target"
coil = "drop_coil"
switch = "drop_sw"
position = [200.0, 470.0, 0.0]
angle_deg = 270.0
height = 40.0
"#;
/// Drop target with the default height (32 mm).
pub const DROP_DEFAULT: &str = r#"
[[mechs]]
id = "k_drop2"
kind = "drop_target"
coil = "drop2_coil"
switch = "drop2_sw"
position = [330.0, 470.0, 0.0]
angle_deg = 270.0
"#;
/// Spinner pointing along +y.
pub const SPINNER: &str = r#"
[[mechs]]
id = "k_spin"
kind = "spinner"
switch = "spin_sw"
position = [400.0, 640.0, 0.0]
angle_deg = 90.0
"#;
/// Vertical up-kicker.
pub const VUK: &str = r#"
[[mechs]]
id = "k_vuk"
kind = "vuk"
coil = "vuk_coil"
switch = "vuk_sw"
position = [110.0, 800.0, 0.0]
angle_deg = 270.0
layer = 0
exit_layer = 0
exit_offset = [0.0, 60.0, 20.0]
"#;
/// Plunger.
pub const PLUNGER: &str = r#"
[[mechs]]
id = "k_plunger"
kind = "plunger"
coil = "plunger_coil"
switch = "plunger_sw"
position = [492.0, 50.0, 0.0]
angle_deg = 90.0
"#;
/// Diverter bound to the fixture glb's `posts[1]`.
pub const DIVERTER: &str = r#"
[[mechs]]
id = "posts[1]"
kind = "diverter"
position = [374.0, 760.0, 0.0]
"#;
/// Gate bound to the fixture glb's `posts[0]`.
pub const GATE: &str = r#"
[[mechs]]
id = "posts[0]"
kind = "gate"
coil = "gate_coil"
switch = "gate_sw"
position = [140.0, 760.0, 0.0]
"#;
/// Newton ball.
pub const NEWTON: &str = r#"
[[mechs]]
id = "k_newton"
kind = "newton_ball"
switch = "newton_sw"
position = [257.0, 300.0, 0.0]
angle_deg = 90.0
length = 50.0
"#;
/// Bumper at the left of the table.
pub const BUMPER: &str = r#"
[[mechs]]
id = "k_bumper"
kind = "bumper"
coil = "ring2_coil"
switch = "ring2_sw"
position = [150.0, 700.0, 0.0]
"#;

/// Every kind once (the second drop target has the default height), in
/// slot order.
pub fn all_kinds() -> Vec<&'static str> {
    vec![
        SLING,
        KICKBACK,
        DROP_40,
        DROP_DEFAULT,
        SPINNER,
        VUK,
        PLUNGER,
        DIVERTER,
        GATE,
        NEWTON,
        BUMPER,
    ]
}

/// Mech count of [`all_kinds`].
pub const ALL_KINDS_COUNT: usize = 11;

/// Rules of the all-kinds table: coils fire in a 400-tick cycle so every
/// mech moves.
pub const ALL_KINDS_RULES: &str = r#"
function on_init() end
function on_tick()
  local m = pinball.tick() % 400
  if m == 10 then
    pinball.pulse_coil(pinball.coil.sling_coil, 25)
    pinball.pulse_coil(pinball.coil.kick_coil, 25)
    pinball.pulse_coil(pinball.coil.ring2_coil, 20)
  elseif m == 50 then
    pinball.pulse_coil(pinball.coil.drop_coil, 150)
    pinball.pulse_coil(pinball.coil.drop2_coil, 150)
  elseif m == 100 then
    pinball.hold_coil(pinball.coil.gate_coil, true)
    pinball.set_diverter(pinball.diverter["posts[1]"], true)
  elseif m == 200 then
    pinball.pulse_coil(pinball.coil.plunger_coil, 100)
  elseif m == 300 then
    pinball.hold_coil(pinball.coil.gate_coil, false)
    pinball.set_diverter(pinball.diverter["posts[1]"], false)
  end
end
"#;

/// Rules that do nothing.
pub const QUIET: &str = "function on_init() end\nfunction on_tick() end\n";

/// A bundle: the fixture with `table.toml` = the fixture's table text up to
/// its own `[[mechs]]` block, then [`COMMON`] and `mechs`, and `rules.lua`
/// replaced by `rules`.
pub fn bundle(tag: &str, mechs: &[&str], rules: &str) -> Bundle {
    let base = std::fs::read_to_string(fixture().join("table.toml")).expect("table.toml");
    let cut = base.find("[[mechs]]").expect("fixture has a mech block");
    let mut table = base[..cut].to_string();
    table.push_str(COMMON);
    for m in mechs {
        table.push_str(m);
    }
    bundle_from_table(tag, &table, rules)
}

/// A bundle from the complete `table.toml` text.
pub fn bundle_from_table(tag: &str, table: &str, rules: &str) -> Bundle {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("pin_sim_kp023_{tag}_{}_{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    for entry in std::fs::read_dir(fixture()).expect("fixture dir") {
        let entry = entry.expect("entry");
        std::fs::copy(entry.path(), dir.join(entry.file_name())).expect("copy");
    }
    std::fs::write(dir.join("table.toml"), table).expect("write table");
    std::fs::write(dir.join("rules.lua"), rules).expect("write rules");
    Bundle(dir)
}

/// The published slots of the last tick.
pub fn slots(sim: &Simulation) -> Vec<MeshTransform> {
    sim.snapshot().dynamic_meshes().to_vec()
}

/// Whether `t` is exactly the identity transform.
pub fn is_identity(t: &MeshTransform) -> bool {
    t.position_m == [0.0; 3] && t.orientation == [0.0, 0.0, 0.0, 1.0]
}

/// Length of a quaternion.
pub fn qlen(q: [f32; 4]) -> f64 {
    q.iter()
        .map(|c| f64::from(*c) * f64::from(*c))
        .sum::<f64>()
        .sqrt()
}

/// Whether every component of `t` is finite.
pub fn finite(t: &MeshTransform) -> bool {
    t.position_m.iter().all(|c| c.is_finite()) && t.orientation.iter().all(|c| c.is_finite())
}

/// Rotation angle of a quaternion (non-negative, in `[0, pi]`; the sign of
/// `w` is normalised first) and its unit axis (zero for the identity).
pub fn angle_axis(q: [f32; 4]) -> (f64, [f64; 3]) {
    let s = if q[3] < 0.0 { -1.0 } else { 1.0 };
    let v = [
        s * f64::from(q[0]),
        s * f64::from(q[1]),
        s * f64::from(q[2]),
    ];
    let w = s * f64::from(q[3]);
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    let angle = 2.0 * libm::atan2(n, w);
    if n < 1e-12 {
        (angle, [0.0; 3])
    } else {
        (angle, [v[0] / n, v[1] / n, v[2] / n])
    }
}

/// Rotation angle of `q`, rad.
pub fn angle_of(q: [f32; 4]) -> f64 {
    angle_axis(q).0
}

/// Wraps an angle difference into `[-pi, pi]`.
pub fn wrap(d: f64) -> f64 {
    let tau = std::f64::consts::TAU;
    let mut x = d % tau;
    if x > std::f64::consts::PI {
        x -= tau;
    } else if x < -std::f64::consts::PI {
        x += tau;
    }
    x
}

/// Unit vector `(cos a, sin a, 0)` of an angle in degrees.
pub fn dir(angle_deg: f64) -> [f64; 3] {
    let a = angle_deg.to_radians();
    [libm::cos(a), libm::sin(a), 0.0]
}

/// `NEWTON_SWING_RANGE_RAD` as f64 (re-export for the tests).
pub fn newton_range() -> f64 {
    f64::from(NEWTON_SWING_RANGE_RAD)
}

/// Balls launched at the all-kinds table's mechs at the start of each
/// 400-tick cycle: `(position, velocity)`.
pub const LAUNCHES: [([f32; 3], [f32; 3]); 5] = [
    ([0.200, 0.43, R], [0.0, 1.5, 0.0]),
    ([0.330, 0.43, R], [0.0, 1.5, 0.0]),
    ([0.400, 0.60, R], [0.0, 2.0, 0.0]),
    ([0.257, 0.24, R], [0.0, 1.5, 0.0]),
    ([0.150, 0.64, R], [0.0, 1.5, 0.0]),
];

/// Spawns this cycle's balls (ignores a full table).
pub fn spawn_launches(sim: &mut Simulation) {
    for (p, v) in LAUNCHES {
        let _ = sim.spawn_ball("standard_steel", p, v);
    }
}

/// Larger of two f64 (plain comparison; no NaNs reach it).
pub fn fmax(a: f64, b: f64) -> f64 {
    if b > a { b } else { a }
}

/// Larger of two f32.
pub fn fmax32(a: f32, b: f32) -> f32 {
    if b > a { b } else { a }
}
