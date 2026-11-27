//! Helpers of the KP-032 (#113) independent tests (REQ-CAB-008): the
//! analytic gravity formula, a hand computation of the Appendix A.4
//! attitude, `cabinet.toml` texts, an open-deck bundle generated with
//! `pin_tablegen`, ball-run sampling and a least-squares fit.
#![allow(dead_code)]

use crate::sim_support::{ScratchDir, fixture, scratch_dir};
use pin_core::schema::cabinet::{CabinetManifest, playfield_attitude};
use pin_core::schema::table::TableManifest;
use pin_sim::{SimConfig, SimError, Simulation};
use std::path::Path;

/// Standard gravity of the contract, m/s².
pub const G: f64 = 9.81;
/// Radius of the built-in `standard_steel` ball, m (27 mm diameter).
pub const R: f32 = 0.0135;
/// The default playfield pitch, degrees (SPEC §7 REQ-CAB-008, A.4).
pub const DEFAULT_PITCH_DEG: f64 = 6.5;

/// Gravity contract of #113 (D-1): `9.81·(−sin φ, −cos φ·sin θ, −cos φ·cos θ)`
/// in the playfield frame, θ pitch and φ roll in radians.
pub fn gravity_formula(pitch_rad: f64, roll_rad: f64) -> [f64; 3] {
    [
        -G * libm::sin(roll_rad),
        -G * libm::cos(roll_rad) * libm::sin(pitch_rad),
        -G * libm::cos(roll_rad) * libm::cos(pitch_rad),
    ]
}

/// The simulation's gravity as f64 components.
pub fn sim_gravity(sim: &Simulation) -> [f64; 3] {
    let g = sim.solver().config().gravity;
    [f64::from(g.x), f64::from(g.y), f64::from(g.z)]
}

/// Euclidean norm.
pub fn norm(v: [f64; 3]) -> f64 {
    libm::sqrt(v[0] * v[0] + v[1] * v[1] + v[2] * v[2])
}

/// `|a − b| / |b|`.
pub fn rel_err(a: [f64; 3], b: [f64; 3]) -> f64 {
    norm([a[0] - b[0], a[1] - b[1], a[2] - b[2]]) / norm(b)
}

/// The data of one preset as written in `crates/pin_core/data/cabinet_presets.toml`
/// (transcribed by hand; [`check_preset_data`] cross-checks it against pin_core).
#[derive(Clone, Copy, Debug)]
pub struct PresetData {
    /// Preset id.
    pub id: &'static str,
    /// `body.width`, mm.
    pub body_width: f64,
    /// `body.length`, mm.
    pub body_length: f64,
    /// `legs.default.length`, mm.
    pub leg_length: f64,
    /// `legs.default.corner_inset`, `[across, along]`, mm.
    pub corner_inset: [f64; 2],
    /// `legs.default.leveler.extension`, mm.
    pub extension: f64,
    /// `legs.default.leveler.extension_min` / `extension_max`, mm.
    pub extension_range: [f64; 2],
    /// `mount.pitch_deg`, `mount.roll_deg`.
    pub mount_deg: [f64; 2],
}

/// The three presets of Appendix A.4 / E.
pub const PRESETS: [PresetData; 3] = [
    PresetData {
        id: "classic_standard",
        body_width: 560.0,
        body_length: 1315.0,
        leg_length: 724.0,
        corner_inset: [38.0, 38.0],
        extension: 0.0,
        extension_range: [0.0, 44.0],
        mount_deg: [6.5, 0.0],
    },
    PresetData {
        id: "classic_widebody",
        body_width: 630.0,
        body_length: 1315.0,
        leg_length: 724.0,
        corner_inset: [38.0, 38.0],
        extension: 0.0,
        extension_range: [0.0, 44.0],
        mount_deg: [6.5, 0.0],
    },
    PresetData {
        id: "modern_standard",
        body_width: 559.0,
        body_length: 1308.0,
        leg_length: 778.0,
        corner_inset: [38.0, 38.0],
        extension: 25.0,
        extension_range: [25.0, 69.0],
        mount_deg: [6.5, 0.0],
    },
];

/// The preset data of `id`.
pub fn preset_data(id: &str) -> PresetData {
    *PRESETS.iter().find(|p| p.id == id).expect("known preset")
}

/// Asserts the transcribed [`PresetData`] equals what pin_core resolves for
/// the preset (so the hand computation reads the real data file values).
pub fn check_preset_data(p: &PresetData) {
    let g = pin_core::schema::cabinet::CabinetGeometry::preset(p.id).expect("preset resolves");
    assert_eq!(f64::from(g.body.width), p.body_width, "{} body.width", p.id);
    assert_eq!(
        f64::from(g.body.length),
        p.body_length,
        "{} body.length",
        p.id
    );
    for (name, leg) in g.legs.all() {
        assert_eq!(
            f64::from(leg.length),
            p.leg_length,
            "{} {name} length",
            p.id
        );
        assert_eq!(
            [
                f64::from(leg.corner_inset[0]),
                f64::from(leg.corner_inset[1])
            ],
            p.corner_inset,
            "{} {name} corner_inset",
            p.id
        );
        assert_eq!(
            f64::from(leg.leveler.extension),
            p.extension,
            "{} {name}",
            p.id
        );
        assert_eq!(
            [
                f64::from(leg.leveler.extension_min),
                f64::from(leg.leveler.extension_max)
            ],
            p.extension_range,
            "{} {name} extension range",
            p.id
        );
    }
    assert_eq!(
        [f64::from(g.mount.pitch_deg), f64::from(g.mount.roll_deg)],
        p.mount_deg,
        "{} mount",
        p.id
    );
}

/// Leveler extensions of the four legs, mm: front left, front right, rear
/// left, rear right.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extensions {
    /// Front left.
    pub fl: f64,
    /// Front right.
    pub fr: f64,
    /// Rear left.
    pub rl: f64,
    /// Rear right.
    pub rr: f64,
}

impl Extensions {
    /// All four at `e`.
    pub fn all(e: f64) -> Self {
        Self {
            fl: e,
            fr: e,
            rl: e,
            rr: e,
        }
    }
}

/// Independent hand computation of the Appendix A.4 attitude for a preset
/// whose four legs share length and corner inset: returns `(pitch, roll)`
/// in radians.
///
/// Leg centrelines: x = ±(width/2 − across), front y = along, rear y =
/// length − along. Effective length = length + extension. Body pitch =
/// atan2(R − F, Sy), body roll = atan2(Rt − L, Sx); playfield = body +
/// mount.
pub fn hand_attitude(p: &PresetData, e: Extensions) -> (f64, f64) {
    let x_right = p.body_width / 2.0 - p.corner_inset[0];
    let x_left = -(p.body_width / 2.0 - p.corner_inset[0]);
    let y_front = p.corner_inset[1];
    let y_rear = p.body_length - p.corner_inset[1];
    let sx = x_right - x_left;
    let sy = y_rear - y_front;
    let len = |ext: f64| p.leg_length + ext;
    let front = (len(e.fl) + len(e.fr)) / 2.0;
    let rear = (len(e.rl) + len(e.rr)) / 2.0;
    let left = (len(e.fl) + len(e.rl)) / 2.0;
    let right = (len(e.fr) + len(e.rr)) / 2.0;
    let deg = core::f64::consts::PI / 180.0;
    let pitch = libm::atan2(rear - front, sy) + p.mount_deg[0] * deg;
    let roll = libm::atan2(right - left, sx) + p.mount_deg[1] * deg;
    (pitch, roll)
}

/// Formats an f64 extension as a TOML float.
fn toml_f(v: f64) -> String {
    format!("{v:?}")
}

/// A `cabinet.toml` with `preset` and per-leg leveler extensions.
pub fn cabinet_toml(preset: &str, e: Extensions) -> String {
    let mut s = format!("schema_version = 1\npreset = \"{preset}\"\n");
    for (leg, v) in [
        ("front_left", e.fl),
        ("front_right", e.fr),
        ("rear_left", e.rl),
        ("rear_right", e.rr),
    ] {
        s.push_str(&format!(
            "\n[legs.{leg}.leveler]\nextension = {}\n",
            toml_f(v)
        ));
    }
    s
}

/// A `cabinet.toml` with only a preset.
pub fn preset_toml(preset: &str) -> String {
    format!("schema_version = 1\npreset = \"{preset}\"\n")
}

/// The oracle attitude `(pitch, roll)` for a `cabinet.toml` text.
pub fn oracle_attitude(text: &str) -> (f64, f64) {
    let m = CabinetManifest::from_toml_str(text).expect("oracle parses cabinet.toml");
    let a = playfield_attitude(&m.resolve().expect("oracle resolves cabinet.toml"));
    (a.pitch_rad, a.roll_rad)
}

/// The oracle attitude of the default cabinet (no `cabinet.toml`).
pub fn default_attitude() -> (f64, f64) {
    let g = pin_core::schema::cabinet::CabinetGeometry::default_preset().expect("default preset");
    let a = playfield_attitude(&g);
    (a.pitch_rad, a.roll_rad)
}

/// Minimal rules: no ball is launched, nothing is driven.
pub const QUIET_RULES: &str = "function on_init()\nend\n\
function on_switch(id, closed)\nend\n\
function on_tick()\nend\n";

/// Open-deck layout: the standard 514 × 1070 mm playfield closed by a
/// rectangular wall and nothing else, so a ball has more than 1 m of flat
/// deck up the table.
pub const OPEN_LAYOUT: &str = r#"schema_version = 1

[playfield]
width = 514.0
length = 1070.0
wall_height = 28.0

[[walls]]
points = [[0.0, 0.0], [0.0, 1070.0], [514.0, 1070.0], [514.0, 0.0]]
closed = true
material = "wood"
"#;

/// Base `table.toml` of the open-deck bundle (`[physics]` added on demand).
fn open_base_table(rolling_resistance: Option<f64>) -> String {
    let physics = match rolling_resistance {
        Some(c) => format!("\n[physics]\nrolling_resistance = {}\n", toml_f(c)),
        None => String::new(),
    };
    format!(
        r#"schema_version = 1

[meta]
id = "kp032_open_deck"
name = "KP-032 Open Deck"
author = "KP-032 spec tests"
version = "1.0.0"
script_entry = "rules.lua"
playfield_mesh = "playfield.glb"
pitch_deg = 6.5
players_max = 1

[balls]
profiles = ["standard_steel"]
{physics}
[cabinet_switches]
left_flipper = 1
right_flipper = 2
left_upper = 3
right_upper = 4
action = 5
start = 6
launch = 7
tilt = 8
"#
    )
}

/// An open-deck bundle generated with `pin_tablegen` from [`OPEN_LAYOUT`],
/// with [`QUIET_RULES`], the fixture's `backglass.toml`, an optional `[physics] rolling_resistance` and an
/// optional `cabinet.toml`.
pub fn open_deck_bundle(
    tag: &str,
    rolling_resistance: Option<f64>,
    cabinet: Option<&str>,
) -> ScratchDir {
    let dir = scratch_dir(&format!("kp032_{tag}"));
    let layout = pin_tablegen::layout::parse_layout(OPEN_LAYOUT).expect("open layout parses");
    let base = TableManifest::from_toml_str(&open_base_table(rolling_resistance))
        .expect("open base table parses");
    let generated = pin_tablegen::generate(&layout, &base).unwrap_or_else(|e| panic!("{e}"));
    pin_tablegen::write_bundle(&generated, &dir).expect("write open-deck bundle");
    std::fs::write(dir.join("rules.lua"), QUIET_RULES).expect("write rules");
    std::fs::write(dir.join("layout.toml"), OPEN_LAYOUT).expect("write layout");
    std::fs::copy(fixture().join("backglass.toml"), dir.join("backglass.toml"))
        .expect("copy backglass.toml");
    if let Some(c) = cabinet {
        std::fs::write(dir.join("cabinet.toml"), c).expect("write cabinet.toml");
    }
    dir
}

/// A copy of the replay fixture with `cabinet.toml` set to `text` and the
/// fixture's rules replaced by [`QUIET_RULES`].
pub fn fixture_with_cabinet(tag: &str, text: &[u8]) -> ScratchDir {
    crate::sim_support::fixture_variant(
        &format!("kp032_{tag}"),
        &[
            ("cabinet.toml", Some(text)),
            ("rules.lua", Some(QUIET_RULES.as_bytes())),
        ],
    )
}

/// Loads `bundle` with the default config.
pub fn load(bundle: &Path) -> Result<Simulation, SimError> {
    Simulation::load(bundle, SimConfig::default())
}

/// Path of the unmodified fixture.
pub fn plain_fixture() -> std::path::PathBuf {
    fixture()
}

/// Positions of one ball run: `pos[k]` after `k` ticks (`pos[0]` is the spawn).
pub struct Run {
    /// Positions, m.
    pub pos: Vec<[f64; 3]>,
    /// `state_hash` after the last tick.
    pub hash: u64,
}

/// Spawns `standard_steel` at `start` at rest, advances `ticks` single
/// ticks and records the position after each.
pub fn run_ball(sim: &mut Simulation, start: [f32; 3], ticks: usize) -> Run {
    let h = sim
        .spawn_ball("standard_steel", start, [0.0; 3])
        .unwrap_or_else(|e| panic!("spawn: {e:?}"));
    let mut pos = Vec::with_capacity(ticks + 1);
    let p = |s: &Simulation| {
        let b = s.ball(h).expect("ball in play");
        [
            f64::from(b.position_m[0]),
            f64::from(b.position_m[1]),
            f64::from(b.position_m[2]),
        ]
    };
    pos.push(p(sim));
    for _ in 0..ticks {
        sim.advance(1);
        pos.push(p(sim));
    }
    Run {
        hash: sim.state_hash(),
        pos,
    }
}

/// Least-squares fit of `s(t) = s0 + v0·t + a/2·t²` to samples
/// `(t_k, s_k)`; returns `a`. Times are centred for conditioning.
pub fn fit_accel(samples: &[(f64, f64)]) -> f64 {
    let n = samples.len() as f64;
    let tm = samples.iter().map(|s| s.0).sum::<f64>() / n;
    let sm = samples.iter().map(|s| s.1).sum::<f64>() / n;
    // Basis 1, u, u² with u = t − tm; normal equations.
    let (mut s1, mut s2, mut s3, mut s4) = (0.0, 0.0, 0.0, 0.0);
    let (mut y0, mut y1, mut y2) = (0.0, 0.0, 0.0);
    for &(t, s) in samples {
        let u = t - tm;
        let y = s - sm;
        s1 += u;
        s2 += u * u;
        s3 += u * u * u;
        s4 += u * u * u * u;
        y0 += y;
        y1 += y * u;
        y2 += y * u * u;
    }
    let m = [[n, s1, s2], [s1, s2, s3], [s2, s3, s4]];
    let det = |m: [[f64; 3]; 3]| {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let d = det(m);
    let mut m2 = m;
    m2[0][2] = y0;
    m2[1][2] = y1;
    m2[2][2] = y2;
    let c2 = det(m2) / d;
    2.0 * c2
}
