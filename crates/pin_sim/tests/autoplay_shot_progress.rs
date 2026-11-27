//! SPEC §4 Subphase 2.9 (Gate A2), work item #138 / KP-057: black-box tests
//! of the autoplay fixes.
//!
//! * `#138 acceptance 1`: `[[shots]]` progress is kept per ball. A sequence
//!   started on one ball and finished on the next is not credited; one
//!   finished on a single ball is, as often as it is finished.
//! * `#138 acceptance 2`: a positionless mech switch hit is credited to the
//!   ball nearest that mech's position, so an EndlessBall finding names the
//!   switch its own ball hit.
//! * `#138 deliverable 3`: the bot models a flipper whose plane is not the
//!   table plane in that flipper's own plane, and ordinary flippers keep
//!   their behaviour.
//!
//! Every table edit is made on a scratch copy of a fixture bundle
//! (`tests/fixtures/autoplay/`) before the first tick. Simulated time only
//! (R6), fixed seeds only (R3).

#[path = "fixtures/autoplay/support.rs"]
mod support;

use pin_core::snapshot::BallState;
use pin_sim::autoplay::{
    Bot, BotConfig, BotStats, Finding, FindingKind, FlipperModel, GameMonitor, GameOutcome,
    NudgeConfig, SkillLevel, SkillParams, TableInfo,
};
use pin_sim::{BallHandle, SimConfig, Simulation};
use std::path::{Path, PathBuf};
use support::*;

/// Ball radius of `standard_steel`, m: the centre height of a ball resting
/// on the playfield.
const BALL_Z: f32 = 0.0135;

/// A stuck limit no test reaches, so held balls are never reported stuck
/// (a ball reported stuck gets no EndlessBall).
const NEVER_STUCK: u64 = 100_000_000;

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create scratch bundle");
    let mut entries: Vec<_> = std::fs::read_dir(from)
        .expect("read fixture")
        .map(|e| e.expect("dir entry").path())
        .collect();
    entries.sort();
    for p in entries {
        let dst = to.join(p.file_name().expect("file name"));
        if p.is_dir() {
            copy_dir(&p, &dst);
        } else {
            std::fs::copy(&p, &dst).expect("copy fixture file");
        }
    }
}

/// A scratch copy of fixture `name` whose `table.toml` text is passed
/// through `edit`; returns the bundle folder.
fn edited_fixture(name: &str, tag: &str, edit: impl FnOnce(&str) -> String) -> PathBuf {
    let dir = scratch(&format!("kp057_{tag}")).join(name);
    copy_dir(&fixture(name), &dir);
    let toml = dir.join("table.toml");
    let text = std::fs::read_to_string(&toml).expect("read table.toml");
    let new = edit(&text);
    assert_ne!(new, text, "the edit of {name}/table.toml changed nothing");
    std::fs::write(&toml, new).expect("write table.toml");
    dir
}

fn load(bundle: &Path) -> (Simulation, TableInfo) {
    let sim = Simulation::load(bundle, SimConfig::default())
        .unwrap_or_else(|e| panic!("{} does not load: {e:?}", bundle.display()));
    let info = TableInfo::load(bundle)
        .unwrap_or_else(|e| panic!("TableInfo of {} does not load: {e:?}", bundle.display()));
    (sim, info)
}

fn at_rest(p: [f32; 3]) -> BallState {
    BallState {
        position_m: p,
        ..BallState::default()
    }
}

fn shot_index(info: &TableInfo, name: &str) -> usize {
    info.shots
        .iter()
        .position(|s| s.name == name)
        .unwrap_or_else(|| panic!("shot {name} not declared"))
}

// ---------------------------------------------------------------------------
// Requirement 1: shot progress per ball (#138 acceptance 1)
// ---------------------------------------------------------------------------

/// The three-switch shot added to the scratch copy of `clean`: s1, s2, s3.
const DROP_BANK: [&str; 3] = ["top_rollover", "bumper_ring", "right_eos"];
/// A switch in neither shot of the scratch table.
const UNRELATED: &str = "left_eos";
/// Where a ball is held while switches are driven: open playfield, clear of
/// every switch zone, the bumper and the posts.
const PARK: [f32; 3] = [0.130, 0.500, BALL_Z];
/// The centre of the `trough` switch zone in the drain basin.
const TROUGH: [f32; 3] = [0.257, 0.040, BALL_Z];

/// `clean` plus the shot `drop_bank` (sequence [`DROP_BANK`], a 10 min
/// window, longer than every test here).
fn drop_bank_table(tag: &str) -> PathBuf {
    edited_fixture("clean", &format!("drop_bank_{tag}"), |t| {
        format!(
            "{t}\n[[shots]]\nid = \"drop_bank\"\n\
             sequence = [\"{}\", \"{}\", \"{}\"]\nwindow_ms = 600000\n",
            DROP_BANK[0], DROP_BANK[1], DROP_BANK[2]
        )
    })
}

/// One game on the `drop_bank` table, watched by a [`GameMonitor`] that
/// observes after every tick. Every ball on the table is held at rest at
/// [`PARK`] before each tick, except while the rig drains it.
struct ShotRig {
    sim: Simulation,
    info: &'static TableInfo,
    mon: GameMonitor<'static>,
    hold: bool,
}

impl ShotRig {
    /// A rig on its own scratch copy (`tag` names it; one per test, as
    /// tests run in parallel in one process).
    fn new(tag: &str) -> ShotRig {
        let (sim, info) = load(&drop_bank_table(tag));
        // The monitor borrows the table info for its whole life; leaking
        // one small TableInfo per test keeps the rig a plain struct.
        let info: &'static TableInfo = Box::leak(Box::new(info));
        let mut mon = GameMonitor::new(info, &sim, 0, NEVER_STUCK);
        mon.observe(&sim);
        ShotRig {
            sim,
            info,
            mon,
            hold: true,
        }
    }

    fn step(&mut self) {
        if self.hold {
            for h in self.sim.ball_handles() {
                assert!(self.sim.set_ball_state(h, &at_rest(PARK)));
            }
        }
        self.sim.advance(1);
        self.mon.observe(&self.sim);
    }

    fn steps(&mut self, n: u32) {
        for _ in 0..n {
            self.step();
        }
    }

    /// One closing edge of `name`: forced open (an EOS switch is closed
    /// with its bat at rest), then closed for 2 ticks, then open again; it
    /// stays forced so the physics cannot add edges of its own.
    fn pulse(&mut self, name: &str) {
        let before = self.sim.switch_hits(name).expect("declared switch");
        self.sim.set_switch(name, false).unwrap();
        self.steps(2);
        self.sim.set_switch(name, true).unwrap();
        self.steps(2);
        self.sim.set_switch(name, false).unwrap();
        self.steps(2);
        assert_eq!(self.sim.switch_hits(name), Some(before + 1), "{name}");
    }

    /// The only ball on the table.
    fn ball(&self) -> BallHandle {
        let h = self.sim.ball_handles();
        assert_eq!(h.len(), 1, "exactly one ball on the table");
        h[0]
    }

    /// Steps until a ball is on the table (the script serves it).
    fn wait_for_ball(&mut self) -> BallHandle {
        for _ in 0..200 {
            if !self.sim.ball_handles().is_empty() {
                return self.ball();
            }
            self.step();
        }
        panic!("no ball served within 200 ticks");
    }

    /// Presses `start`: the rules start a three-ball game and serve ball 1.
    fn start(&mut self) -> BallHandle {
        self.sim.set_switch("start", true).unwrap();
        self.steps(2);
        self.sim.set_switch("start", false).unwrap();
        self.wait_for_ball()
    }

    /// Lets the ball on the table drain normally: it is put at rest in the
    /// trough zone and the simulation removes it as drained (not escaped).
    /// The rules then serve the next ball, which this returns.
    fn drain_and_serve(&mut self) -> BallHandle {
        let a = self.ball();
        let drained = self.sim.counters().balls_drained;
        self.hold = false;
        assert!(self.sim.set_ball_state(a, &at_rest(TROUGH)));
        let mut n = 0;
        while self.sim.counters().balls_drained == drained {
            n += 1;
            assert!(n < 300, "the ball did not drain");
            self.step();
        }
        assert_eq!(
            self.sim.counters().balls_escaped,
            0,
            "a drain, not an escape"
        );
        self.hold = true;
        let b = self.wait_for_ball();
        assert_ne!(a, b, "a new ball was served");
        b
    }

    fn finish(self) -> (GameOutcome, &'static TableInfo) {
        (self.mon.finish(&self.sim), self.info)
    }
}

fn shots(out: &GameOutcome, info: &TableInfo) -> (u64, u64) {
    (
        out.shot_counts[shot_index(info, "drop_lane")],
        out.shot_counts[shot_index(info, "drop_bank")],
    )
}

/// SPEC §4 Subphase 2.9, `#138 acceptance 1` (1a): s1 and s2 of `drop_bank`
/// on ball A, A drains normally, ball B is served, s3 on ball B: the shot
/// is not credited.
#[test]
fn shot_started_on_one_ball_and_finished_after_a_drain_is_not_credited() {
    let mut rig = ShotRig::new("1a");
    rig.start();
    rig.steps(20);
    rig.pulse(DROP_BANK[0]);
    rig.pulse(DROP_BANK[1]);
    rig.steps(20);
    rig.drain_and_serve();
    rig.steps(20);
    rig.pulse(DROP_BANK[2]);
    rig.steps(20);
    let (out, info) = rig.finish();
    assert_eq!(out.counters.balls_escaped, 0);
    let (_, bank) = shots(&out, info);
    assert_eq!(
        bank, 0,
        "drop 1 and 2 on one ball and drop 3 on the next must not credit drop_bank"
    );
}

/// SPEC §4 Subphase 2.9, `#138 acceptance 1` (1b): s1, s2, s3 all on one
/// ball are credited once.
#[test]
fn shot_completed_on_one_ball_is_credited_once() {
    let mut rig = ShotRig::new("1b");
    rig.start();
    rig.steps(20);
    for s in DROP_BANK {
        rig.pulse(s);
        rig.steps(10);
    }
    rig.steps(20);
    let (out, info) = rig.finish();
    let (lane, bank) = shots(&out, info);
    assert_eq!(bank, 1, "all three drops on one ball credit drop_bank");
    // drop_lane (top_rollover, bumper_ring) is inside the same run.
    assert_eq!(lane, 1);
}

/// SPEC §4 Subphase 2.9, `#138 acceptance 1` (1c): the sequence completed
/// twice in a row on one ball counts twice (in-ball credit unchanged; a
/// completion resets the progress).
#[test]
fn shot_completed_twice_on_one_ball_is_credited_twice() {
    let mut rig = ShotRig::new("1c");
    rig.start();
    rig.steps(20);
    for _ in 0..2 {
        for s in DROP_BANK {
            rig.pulse(s);
            rig.steps(10);
        }
    }
    let (out, info) = rig.finish();
    let (_, bank) = shots(&out, info);
    assert_eq!(bank, 2);
}

/// SPEC §4 Subphase 2.9, `#138 acceptance 1` (1d): s1 and s2 on ball A; A
/// leaves the table and ball B is served between two observations (no
/// drain counted, B possibly in A's solver slot); s3 on B: the serve clears
/// the progress, so the shot is not credited.
#[test]
fn a_new_ball_clears_shot_progress_even_without_a_drain() {
    let mut rig = ShotRig::new("1d");
    let a = rig.start();
    rig.steps(20);
    rig.pulse(DROP_BANK[0]);
    rig.pulse(DROP_BANK[1]);
    rig.steps(20);
    let drained = rig.sim.counters().balls_drained;
    assert!(rig.sim.remove_ball(a));
    let b = rig
        .sim
        .spawn_ball("standard_steel", PARK, [0.0; 3])
        .unwrap();
    assert_ne!(a, b);
    rig.step();
    rig.steps(20);
    rig.pulse(DROP_BANK[2]);
    rig.steps(20);
    assert_eq!(rig.sim.counters().balls_drained, drained, "no drain");
    let (out, info) = rig.finish();
    let (_, bank) = shots(&out, info);
    assert_eq!(bank, 0, "the serve of ball B clears ball A's progress");
}

/// SPEC §4 Subphase 2.9, `#138 acceptance 1` (1e, no regression): the
/// fixture's own shot `drop_lane` (`top_rollover`, `bumper_ring`) with an
/// unrelated switch between its two switches, on one ball, is credited
/// once, as before the change; after a drain and a serve the same sequence
/// on the next ball is credited again.
#[test]
fn existing_two_switch_shot_with_an_unrelated_switch_between_is_credited_once() {
    let mut rig = ShotRig::new("1e");
    rig.start();
    rig.steps(20);
    rig.pulse("top_rollover");
    rig.steps(10);
    rig.pulse(UNRELATED);
    rig.steps(10);
    rig.pulse("bumper_ring");
    rig.steps(20);
    rig.drain_and_serve();
    rig.steps(20);
    rig.pulse("top_rollover");
    rig.pulse("bumper_ring");
    rig.steps(20);
    let (out, info) = rig.finish();
    let (lane, bank) = shots(&out, info);
    assert_eq!(lane, 2, "once per ball");
    assert_eq!(bank, 0, "right_eos never closed");
}

// ---------------------------------------------------------------------------
// Requirement 2: EndlessBall subject (#138 acceptance 2)
// ---------------------------------------------------------------------------

/// Endless-ball age limit used here, ticks (the API's
/// `with_endless_ticks`; the default is 300 s).
const ENDLESS: u64 = 3_000;
/// Ticks after ball A at which ball B appears.
const B_DELAY: u64 = 500;
/// Ticks run in all: both balls pass the limit.
const RUN: u64 = ENDLESS + B_DELAY + 1_000;

/// A two-ball endless scenario. Ball A is held at `a_pos` next to the mech
/// whose positionless switch `x` closes every `x_period` ticks; ball B
/// appears `B_DELAY` ticks later at `b_pos`, on the positioned switch `y`,
/// which closes every `y_period` ticks. Both are held at rest every tick,
/// so neither drains nor moves. Returns the EndlessBall findings of A and
/// B and their solver slots.
struct Endless<'a> {
    bundle: &'a Path,
    x: &'a str,
    a_pos: [f32; 3],
    y: &'a str,
    b_pos: [f32; 3],
    x_period: u64,
    y_period: u64,
}

impl Endless<'_> {
    fn run(&self) -> ((Finding, u8), (Finding, u8)) {
        let (mut sim, info) = load(self.bundle);
        let mech_switch = |name: &str| {
            let s = info
                .switches
                .iter()
                .find(|s| s.name == name)
                .unwrap_or_else(|| panic!("switch {name}"));
            (
                s.position_m,
                info.mechs.iter().any(|m| m.switch == Some(s.id)),
            )
        };
        // The scenario's premise, from the table data.
        assert_eq!(
            mech_switch(self.x),
            (None, true),
            "x: positionless mech switch"
        );
        assert!(mech_switch(self.y).0.is_some(), "y: positioned switch");

        let mut mon = GameMonitor::new(&info, &sim, 0, NEVER_STUCK).with_endless_ticks(ENDLESS);
        let a = sim
            .spawn_ball("standard_steel", self.a_pos, [0.0; 3])
            .unwrap();
        mon.observe(&sim);
        let mut b = None;
        for t in 0..RUN {
            if t == B_DELAY {
                b = Some(
                    sim.spawn_ball("standard_steel", self.b_pos, [0.0; 3])
                        .unwrap(),
                );
            }
            assert!(sim.set_ball_state(a, &at_rest(self.a_pos)));
            if let Some(b) = b {
                assert!(sim.set_ball_state(b, &at_rest(self.b_pos)));
            }
            if t % self.x_period == 0 {
                sim.set_switch(self.x, true).unwrap();
            } else if t % self.x_period == self.x_period / 2 {
                sim.set_switch(self.x, false).unwrap();
            }
            if b.is_some() {
                if t % self.y_period == 0 {
                    sim.set_switch(self.y, true).unwrap();
                } else if t % self.y_period == self.y_period / 2 {
                    sim.set_switch(self.y, false).unwrap();
                }
            }
            sim.advance(1);
            mon.observe(&sim);
        }
        let b = b.unwrap();
        assert_eq!(sim.ball_handles().len(), 2, "both balls still on the table");
        assert!(
            sim.switch_hits(self.x).unwrap() > sim.switch_hits(self.y).unwrap(),
            "x closes more often than y"
        );
        mon.end_at_cap(&sim);
        let mut endless: Vec<Finding> = mon
            .findings()
            .iter()
            .filter(|f| f.kind == FindingKind::EndlessBall)
            .cloned()
            .collect();
        assert_eq!(endless.len(), 2, "one EndlessBall per ball: {endless:?}");
        // A reached the limit B_DELAY ticks before B; a finding keeps the
        // tick it was raised at.
        endless.sort_by_key(|f| f.tick);
        assert!(endless[0].tick < endless[1].tick, "{endless:?}");
        let fb = endless.pop().unwrap();
        let fa = endless.pop().unwrap();
        ((fa, a.slot()), (fb, b.slot()))
    }
}

/// Whether `detail` names ball slot `slot` (as `slot <n>`).
fn names_slot(detail: &str, slot: u8) -> bool {
    let key = format!("slot {slot}");
    detail.match_indices(&key).any(|(i, _)| {
        !detail[i + key.len()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
    })
}

fn check_subjects(s: &Endless<'_>) {
    let ((fa, sa), (fb, sb)) = s.run();
    assert_ne!(sa, sb);
    // Ball B hit only y: its finding names y and never x.
    assert!(
        fb.subject.contains(s.y),
        "ball B's EndlessBall must name {}: {fb:?}",
        s.y
    );
    assert!(
        !fb.subject.contains(s.x),
        "ball B's EndlessBall must not name {} (only ball A hit it): {fb:?}",
        s.x
    );
    assert!(!fb.detail.contains(s.x), "ball B never hit {}: {fb:?}", s.x);
    assert!(
        names_slot(&fb.detail, sb),
        "the detail names B's own slot {sb}: {fb:?}"
    );
    // Mirror: ball A hit only x.
    assert!(
        fa.subject.contains(s.x),
        "ball A's EndlessBall must name {}: {fa:?}",
        s.x
    );
    assert!(!fa.subject.contains(s.y), "{fa:?}");
    assert!(
        names_slot(&fa.detail, sa),
        "the detail names A's own slot {sa}: {fa:?}"
    );
}

/// SPEC §4 Subphase 2.9, `#138 acceptance 2`: on `clean`, the bumper's
/// positionless ring switch closes next to ball A, while ball B sits on
/// `top_rollover` and closes it less often. Each ball's EndlessBall names
/// the switch that ball hit: B's names `top_rollover`, never
/// `bumper_ring`; A's names `bumper_ring`.
#[test]
fn endless_ball_subject_with_a_positionless_bumper_switch() {
    let bundle = fixture("clean");
    check_subjects(&Endless {
        bundle: &bundle,
        x: "bumper_ring",
        // Bumper at (237, 700) mm: A 60 mm below its centre.
        a_pos: [0.237, 0.640, BALL_Z],
        y: "top_rollover",
        b_pos: [0.257, 0.880, BALL_Z],
        x_period: 10,
        y_period: 25,
    });
}

/// SPEC §4 Subphase 2.9, `#138 acceptance 2`: the same on the grey box with
/// a drop target's positionless switch (`drop_1_sw`, drop target at
/// (200, 470) mm) next to ball A and ball B on the positioned
/// `shooter_lane` opto, at two hit-rate ratios.
#[test]
fn endless_ball_subject_with_a_positionless_drop_target_switch() {
    let bundle = fixture("greybox_452eedf");
    for (x_period, y_period) in [(10, 25), (6, 40)] {
        check_subjects(&Endless {
            bundle: &bundle,
            x: "drop_1_sw",
            a_pos: [0.150, 0.470, BALL_Z],
            y: "shooter_lane",
            b_pos: [0.492, 0.300, BALL_Z],
            x_period,
            y_period,
        });
    }
}

// ---------------------------------------------------------------------------
// Requirement 3: the bot and a flipper in a vertical plane (#138 deliverable 3)
// ---------------------------------------------------------------------------

/// Pivot of the vertical `lower_left` flipper in the scratch table, m.
const V_PIVOT: [f32; 3] = [0.165, 0.105, 0.020];

/// `clean` with `lower_left` turned into a flipper standing in the vertical
/// x–z plane through its pivot: normal −y (angle 0 along +x, +90° straight
/// up off the playfield, `pin_physics::flipper::plane_axes`), pivot 20 mm
/// above the playfield, at rest 10° below +x and sweeping 50° upward. The
/// bat at rest clears the playfield; button, coil, EOS and length are
/// unchanged.
fn vertical_flipper_table(tag: &str) -> PathBuf {
    edited_fixture("clean", &format!("vertical_{tag}"), |t| {
        let from = "base_position = [165.0, 105.0, 0.0]\n\
                    surface_normal = [0.0, 0.0, 1.0]\n\
                    rest_angle_deg = -30.0\n";
        let to = "base_position = [165.0, 105.0, 20.0]\n\
                  surface_normal = [0.0, -1.0, 0.0]\n\
                  rest_angle_deg = -10.0\n";
        assert_eq!(t.matches(from).count(), 1, "lower_left entry as expected");
        t.replacen(from, to, 1)
    })
}

/// A bot that never misses, never presses `start` and never nudges, so the
/// only flipper presses are flip and cradle decisions.
fn quiet_bot(seed: u64, info: &TableInfo) -> Bot {
    Bot::new(seed, SkillLevel::Expert)
        .with_skill_params(SkillParams {
            reaction_jitter_ms: 3,
            miss_rate: 0.0,
        })
        .with_table(info)
        .with_config(BotConfig {
            max_starts: 0,
            start_delay_ms: (600_000, 600_000),
            nudge: NudgeConfig {
                enabled: false,
                ..NudgeConfig::default()
            },
            ..BotConfig::default()
        })
}

/// What a bot run did to the vertical flipper (`lower_left`, button
/// `left_flipper`, EOS `left_eos`).
struct Run {
    stats: BotStats,
    /// Ticks after which the `left_flipper` button was held by the bot.
    left_pressed: u32,
    /// Closing edges of `left_eos` (closed at rest, it opens at the end of
    /// the stroke and closes again on the way back: a full press).
    eos_closes: u32,
}

/// Runs `bot.tick; sim.advance(1); mon.observe` for `ticks` ticks, holding
/// `held` (if any) at rest before every tick.
fn play(
    sim: &mut Simulation,
    info: &TableInfo,
    bot: &mut Bot,
    held: Option<(BallHandle, [f32; 3])>,
    ticks: u64,
) -> Run {
    let eos0 = sim.switch_hits("left_eos").unwrap();
    let mut mon = GameMonitor::new(info, sim, 0, NEVER_STUCK);
    mon.observe(sim);
    let mut left_pressed = 0;
    for _ in 0..ticks {
        if let Some((h, p)) = held {
            assert!(sim.set_ball_state(h, &at_rest(p)));
        }
        bot.tick(sim);
        if sim.switch_closed("left_flipper") == Some(true) {
            left_pressed += 1;
        }
        sim.advance(1);
        mon.observe(sim);
    }
    Run {
        stats: bot.stats(),
        left_pressed,
        eos_closes: sim.switch_hits("left_eos").unwrap() - eos0,
    }
}

/// SPEC §4 Subphase 2.9, `#138 deliverable 3` (3a): a ball dropped from
/// rest above the vertical flipper falls (under gravity, in the flipper's
/// own plane) into the bat's swept sector; the bot, given the table, makes
/// a flip or cradle decision and presses the button, so the EOS switch
/// closes. Swept over drop points and seeds.
///
/// The ball starts 4 mm on the −y side of the bat's plane (well within bat
/// radius plus ball radius) and table-plane gravity carries it further to
/// −y, so in a table-plane (plan view) model it never enters the sector the
/// bat would sweep in plan (+x turning toward +y): only a model in the
/// flipper's own plane predicts it.
#[test]
fn bot_flips_a_flipper_standing_in_a_vertical_plane() {
    let bundle = vertical_flipper_table("3a");
    // (dx, dz) from the pivot in the flipper plane, m: above the 40° end of
    // the sweep, inside the 0.1 m reach.
    let drops = [(0.030, 0.050), (0.045, 0.045), (0.060, 0.040)];
    for (i, (dx, dz)) in drops.into_iter().enumerate() {
        for seed in [1u64, 2, 3] {
            let (mut sim, info) = load(&bundle);
            let lower_left = info
                .flippers
                .iter()
                .find(|f| f.name == "lower_left")
                .unwrap();
            let pivot = sim.flipper_set().pivot_m(lower_left.id).unwrap();
            assert!(
                (pivot.to_array().iter().zip(V_PIVOT)).all(|(a, b)| (a - b).abs() < 1e-5),
                "{pivot:?}"
            );
            let p = [V_PIVOT[0] + dx, V_PIVOT[1] - 0.004, V_PIVOT[2] + dz];
            sim.spawn_ball("standard_steel", p, [0.0; 3]).unwrap();
            let mut bot = quiet_bot(seed, &info);
            let r = play(&mut sim, &info, &mut bot, None, 3_000);
            let s = r.stats;
            assert!(
                s.predictions + s.cradles > 0,
                "drop {i} seed {seed}: no flip decision at all: {s:?}"
            );
            assert_eq!(s.misses, 0);
            // The stats count every flipper; the vertical one's own button
            // and EOS show the decision was for it.
            assert!(
                r.left_pressed > 0,
                "drop {i} seed {seed}: the bot never pressed left_flipper: {s:?}"
            );
            assert!(
                r.eos_closes > 0,
                "drop {i} seed {seed}: left_eos never closed from a bot press: {s:?}"
            );
        }
    }
}

/// SPEC §4 Subphase 2.9, `#138 deliverable 3` (3b): a ball resting far from
/// the vertical flipper in the table plane makes no flip decision and the
/// flipper is never pressed.
#[test]
fn bot_ignores_a_ball_far_from_the_vertical_flipper() {
    let bundle = vertical_flipper_table("3b");
    for p in [
        [0.400, 0.600, BALL_Z],
        [0.100, 0.850, BALL_Z],
        [0.257, 0.450, BALL_Z],
    ] {
        let (mut sim, info) = load(&bundle);
        let h = sim.spawn_ball("standard_steel", p, [0.0; 3]).unwrap();
        let mut bot = quiet_bot(7, &info);
        let r = play(&mut sim, &info, &mut bot, Some((h, p)), 3_000);
        let s = r.stats;
        assert_eq!(s.predictions, 0, "{p:?}: {s:?}");
        assert_eq!(s.cradles, 0, "{p:?}: {s:?}");
        assert_eq!(s.flips_on_prediction, 0, "{p:?}: {s:?}");
        assert_eq!(r.left_pressed, 0, "{p:?}: the vertical flipper was pressed");
        assert_eq!(r.eos_closes, 0, "{p:?}: the vertical flipper moved");
    }
}

fn clean_run(seed: u64) -> (BotStats, Vec<FlipperModel>) {
    let (mut sim, info) = load(&fixture("clean"));
    let mut bot = Bot::new(seed, SkillLevel::Standard).with_table(&info);
    for _ in 0..20_000 {
        bot.tick(&mut sim);
        sim.advance(1);
    }
    (bot.stats(), bot.flipper_models())
}

/// SPEC §4 Subphase 2.9, `#138 deliverable 3` (3c, regression): on the
/// unmodified `clean` table (both flippers in the table plane) the bot is
/// deterministic for a seed, plays (makes flip decisions), and models each
/// flipper with its declared geometry (76 mm bat, ±50° sweep).
#[test]
fn bot_on_ordinary_flippers_is_unchanged_and_deterministic() {
    let info = TableInfo::load(&fixture("clean")).unwrap();
    for seed in [3u64, 11] {
        let (s1, m1) = clean_run(seed);
        let (s2, m2) = clean_run(seed);
        assert_eq!(s1, s2, "seed {seed}: same stats on a second run");
        assert_eq!(m1.len(), m2.len());
        assert!(s1.starts >= 1 && s1.predictions > 0, "seed {seed}: {s1:?}");
        assert_eq!(m1.len(), info.flippers.len(), "one model per flipper");
        for m in &m1 {
            assert!(m.length_m.is_finite() && m.sweep_rad.is_finite(), "{m:?}");
            assert!((m.length_m - 0.076).abs() < 1e-6, "{m:?}");
            assert!(
                (m.sweep_rad.abs() - 50f32.to_radians()).abs() < 1e-5,
                "{m:?}"
            );
        }
    }
}
