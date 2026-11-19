//! Subphase 6.4 spec tests (Independent Tester), brief S3: the Feature Lab
//! rules through `load_validated` and the `Simulation` test API, on
//! simulated time (TABLE CONTRACT C6–C9).
//!
//! * (a) a full 3-ball game by injection (drains by moving the ball onto the
//!   trough switch);
//! * (b) the mode selector: +1 (mod 5) at every ball start and on every
//!   `action` press;
//! * (c) each of the five modes started by a physics-made qualifying shot
//!   (the README's shot switches), `mode:<id>` +1 exactly, points > 0;
//! * (d) extra balls never end the player's ball or hang the game;
//! * (e) the magnet comparison: both steel profiles held at the pole while
//!   the coil is on, `cmp_release_ms_standard > cmp_release_ms_low_rem`.

#[path = "fixtures/spec_6_4/common.rs"]
mod common;

use common::*;
use pin_core::input::CabinetInput;
use pin_core::schema::table::{FlipperStagingDef, MechKind, SwitchKind};
use pin_core::snapshot::BallState;
use pin_sim::{BallHandle, Simulation};

/// Steps one tick at a time, asserting the score never decreases.
struct ScoreWatch {
    last: std::cell::Cell<i64>,
}

impl ScoreWatch {
    fn new(sim: &Simulation) -> Self {
        Self {
            last: std::cell::Cell::new(sim.score(0).unwrap_or(0)),
        }
    }

    fn check(&self, sim: &Simulation, what: &str) {
        let last = self.last.get();
        let s = sim.score(0).unwrap_or(last);
        assert!(
            s >= last,
            "{what}: score decreased {last} -> {s} at tick {}",
            sim.tick()
        );
        self.last.set(s);
    }

    fn advance(&mut self, sim: &mut Simulation, ticks: u64, what: &str) {
        for _ in 0..ticks {
            sim.advance(1);
            self.check(sim, what);
        }
    }

    fn until(
        &mut self,
        sim: &mut Simulation,
        max: u64,
        what: &str,
        mut cond: impl FnMut(&Simulation) -> bool,
    ) -> bool {
        for _ in 0..max {
            sim.advance(1);
            self.check(sim, what);
            if cond(sim) {
                return true;
            }
        }
        false
    }
}

/// Switches of scoring objects (bumpers, slings, spinners, drop targets),
/// falling back to ring / leaf / target / spinner switches.
fn scoring_switches() -> Vec<String> {
    let m = &table().manifest;
    let mut v: Vec<String> = m
        .mechs
        .iter()
        .filter(|x| {
            matches!(
                x.kind,
                MechKind::Bumper | MechKind::Sling | MechKind::Spinner | MechKind::DropTarget
            )
        })
        .filter_map(|x| x.switch.clone())
        .collect();
    if v.is_empty() {
        v = m
            .switches
            .iter()
            .filter(|s| {
                matches!(
                    s.kind,
                    SwitchKind::Ring | SwitchKind::Leaf | SwitchKind::Target | SwitchKind::Spinner
                )
            })
            .map(|s| s.id.clone())
            .collect();
    }
    v
}

/// Drains every ball, then waits (≤ 10 s) until `done`; balls that appear
/// meanwhile without `done` (late extra balls) are drained too (≤ 5
/// rounds).
fn drain_until(
    sim: &mut Simulation,
    w: &mut ScoreWatch,
    what: &str,
    done: impl Fn(&Simulation) -> bool,
) {
    for _ in 0..5 {
        drain_all_with(sim, |s| w.check(s, what));
        w.check(sim, what);
        if w.until(sim, 10_000, what, |s| done(s) || s.balls_in_play() > 0) && done(sim) {
            return;
        }
        if done(sim) {
            return;
        }
    }
    panic!("{what}: condition not reached after draining");
}

// ---------------------------------------------------------------------------
// (a) full game by injection
// ---------------------------------------------------------------------------

#[test]
fn a_full_three_ball_game_by_injection() {
    big_stack(|| {
        let scorers = scoring_switches();
        assert!(!scorers.is_empty(), "the table has scoring-object switches");
        let mut sim = load();
        let mut w = ScoreWatch::new(&sim);
        w.advance(&mut sim, 2_000, "attract");
        assert_eq!(pv(&sim, "game_state"), 0, "attract before start");
        assert_eq!(pv(&sim, "ball"), 0, "no ball number in attract");
        assert_eq!(sim.balls_in_play(), 0, "no ball before start");
        let games_before = pv(&sim, "games_played");

        pulse_with(&mut sim, "start", 50, |s| w.check(s, "start pulse"));
        assert!(
            w.until(&mut sim, 10_000, "start", |s| s.balls_in_play() > 0),
            "start serves a ball"
        );
        assert!(
            w.until(&mut sim, 2_000, "start", |s| pv(s, "game_state") == 1),
            "game_state 1 after start"
        );
        let mut balls_seen = Vec::new();
        for ball in 1..=3i64 {
            assert!(
                w.until(&mut sim, 10_000, "serve", |s| s.balls_in_play() > 0
                    && pv(s, "ball") == ball),
                "ball {ball} served (ball = {}, balls {})",
                pv(&sim, "ball"),
                sim.balls_in_play()
            );
            balls_seen.push(pv(&sim, "ball"));
            assert_eq!(pv(&sim, "game_state"), 1);
            pulse_with(&mut sim, "launch", 50, |s| w.check(s, "launch pulse"));
            w.advance(&mut sim, 1_500, "launch");
            let before = sim.score(0).unwrap_or(0);
            for sw in &scorers {
                pulse_with(&mut sim, sw, 20, |s| w.check(s, "scorer pulse"));
                w.advance(&mut sim, 20, "scorer pulse");
            }
            let after = sim.score(0).unwrap_or(0);
            assert!(
                after > before,
                "ball {ball}: scoring switches score ({before} -> {after})"
            );
            let what = format!("drain ball {ball}");
            if ball < 3 {
                drain_until(&mut sim, &mut w, &what, |s| pv(s, "ball") == ball + 1);
            } else {
                drain_until(&mut sim, &mut w, &what, |s| pv(s, "game_state") == 2);
            }
            assert_healthy(&sim, &what);
        }
        assert_eq!(balls_seen, [1, 2, 3]);
        assert_eq!(pv(&sim, "game_state"), 2, "game over after ball 3");
        assert_eq!(pv(&sim, "ball"), 0, "ball is 0 outside a game");
        // No fourth ball, then back to attract.
        let back = w.until(&mut sim, 60_000, "game over", |s| {
            assert_eq!(s.balls_in_play(), 0, "no ball after the 3rd drain");
            pv(s, "game_state") == 0
        });
        assert!(back, "game_state returns to 0 (attract) within 60 s");
        let score = sim.score(0).expect("score reported");
        assert!(score > 0, "final score {score} > 0");
        assert_eq!(pv(&sim, "games_played"), games_before + 1);
        assert!(pv(&sim, "high_score") >= score, "high score updated");
        assert_healthy(&sim, "game over");
    });
}

// ---------------------------------------------------------------------------
// (b) the mode selector
// ---------------------------------------------------------------------------

/// Plays balls without `action` (each drained at once by injection) and
/// returns `mode_sel` 300 ms into each ball, `n` balls over as many games as
/// needed, plus the value before the first game.
fn selector_without_action(sim: &mut Simulation, n: usize) -> (i64, Vec<i64>) {
    sim.advance(500);
    let initial = pv(sim, "mode_sel");
    let mut seen = Vec::new();
    while seen.len() < n {
        if pv(sim, "game_state") != 1 {
            assert!(
                run_until(sim, 60_000, |s| pv(s, "game_state") == 0),
                "back to attract before the next game"
            );
            pulse(sim, "start", 50);
        }
        let ball = pv(sim, "ball");
        let ok = run_until(sim, 10_000, |s| {
            s.balls_in_play() > 0 && pv(s, "game_state") == 1 && pv(s, "ball") != 0
        });
        assert!(ok, "a ball is served (after ball {ball})");
        sim.advance(300);
        seen.push(pv(sim, "mode_sel"));
        let b = pv(sim, "ball");
        // Drain; the next ball (or the game over) follows.
        for _ in 0..5 {
            drain_all(sim);
            if run_until(sim, 10_000, |s| {
                pv(s, "ball") != b || pv(s, "game_state") != 1 || s.balls_in_play() > 0
            }) && (pv(sim, "ball") != b || pv(sim, "game_state") != 1)
            {
                break;
            }
        }
        assert_healthy(sim, "selector run");
    }
    (initial, seen)
}

#[test]
fn b_selector_advances_at_every_ball_start() {
    big_stack(|| {
        let mut sim = load();
        let (initial, seen) = selector_without_action(&mut sim, 10);
        let mut prev = initial;
        for (i, &v) in seen.iter().enumerate() {
            assert!((0..5).contains(&v), "mode_sel in 0..4: {v}");
            assert_eq!(
                v,
                (prev + 1) % 5,
                "ball {}: mode_sel {prev} -> {v} (sequence {initial} then {seen:?})",
                i + 1
            );
            prev = v;
        }
        let mut all: Vec<i64> = seen.clone();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all, [0, 1, 2, 3, 4], "10 balls visit all five modes");
    });
}

#[test]
fn b_selector_advances_on_every_action_press_and_wraps() {
    big_stack(|| {
        let mut sim = load();
        start_game(&mut sim);
        sim.advance(300);
        let ball = pv(&sim, "ball");
        let mut v = pv(&sim, "mode_sel");
        let mut wrapped = false;
        for k in 0..7 {
            pulse(&mut sim, "action", 30);
            sim.advance(60);
            let n = pv(&sim, "mode_sel");
            assert_eq!(n, (v + 1) % 5, "action press {k}: mode_sel {v} -> {n}");
            wrapped |= v == 4 && n == 0;
            v = n;
        }
        assert!(wrapped, "mode_sel wrapped 4 -> 0");
        assert_eq!(pv(&sim, "ball"), ball, "action does not change the ball");
        assert_eq!(sim.switch_hits("action"), Some(7));
        assert_healthy(&sim, "action presses");
    });
}

// ---------------------------------------------------------------------------
// (c) every mode by a physics-made qualifying shot
// ---------------------------------------------------------------------------

/// A start state for the shot ball.
#[derive(Clone, Debug)]
enum Variant {
    /// Rolled from `pos` with `vel` on `layer`.
    Roll {
        layer: u8,
        pos: [f32; 3],
        vel: [f32; 3],
    },
    /// Resting on the up-side face of `flipper` at fraction `s` of the bat,
    /// then flipped with cabinet switch `button` (its EOS switch closes).
    Flip {
        flipper: String,
        button: &'static str,
        s: f32,
    },
}

/// Position (m) and layer of switch `name`: its own zone, or the position
/// of the mech it belongs to.
fn switch_place(name: &str) -> Option<([f32; 3], u8, Option<MechKind>)> {
    let t = table();
    let s = switch(name);
    if let Some(p) = s.position_m {
        let mech = t
            .manifest
            .mechs
            .iter()
            .find(|m| m.switch.as_deref() == Some(name))
            .map(|m| m.kind);
        return Some((p, s.layer, mech));
    }
    let def = t
        .manifest
        .mechs
        .iter()
        .find(|m| m.switch.as_deref() == Some(name))?;
    let si = t.mechs.iter().find(|m| m.name == def.id)?;
    Some((si.position_m?, def.layer.unwrap_or(0), Some(def.kind)))
}

/// The cabinet switch key of a flipper's dedicated button.
fn button_key(input: CabinetInput) -> &'static str {
    match input {
        CabinetInput::LeftFlipper => "left_flipper",
        CabinetInput::RightFlipper => "right_flipper",
        CabinetInput::LeftUpper => "left_upper",
        CabinetInput::RightUpper => "right_upper",
        CabinetInput::Action => "action",
        CabinetInput::Start => "start",
        CabinetInput::Launch => "launch",
        CabinetInput::Tilt => "tilt",
    }
}

/// Start states that make the README shot of `mode` by physics:
///
/// * a shot that begins with a flipper's EOS switch: the ball rests on that
///   flipper's bat at 15 offsets (0.15..0.85 of its length) and is flipped;
/// * otherwise: rolled along the line from the first to the second shot
///   switch (or up the table) from 25 / 50 / 90 mm back, with three lateral
///   offsets and three speeds; rolled at the first switch from the second
///   switch's side (so a kicker sends it on toward the second); and, for a
///   scoop first, rolled down into it (tried first).
fn shot_variants(mode: &str) -> Vec<Variant> {
    let row = readme_row(mode);
    let names = switches_in(&row.shot_switches);
    assert!(!names.is_empty(), "{mode}: README shot switches");
    if let Some(f) = table()
        .manifest
        .flippers
        .iter()
        .find(|f| f.eos_switch == names[0])
    {
        let FlipperStagingDef::DedicatedButton { input } = f.staging else {
            panic!("{mode}: flipper {} has a dedicated button", f.id)
        };
        return (0..15)
            .map(|i| Variant::Flip {
                flipper: f.id.clone(),
                button: button_key(input),
                s: 0.15 + 0.05 * i as f32,
            })
            .collect();
    }
    let places: Vec<_> = names.iter().filter_map(|n| switch_place(n)).collect();
    assert!(
        !places.is_empty(),
        "{mode}: a shot switch with a position ({names:?})"
    );
    let r = radius_m(STEEL);
    let (p1, layer, kind) = places[0];
    let up = [0.0, 1.0, 0.0];
    let gap = places.get(1).map(|(p2, _, _)| sub3(*p2, p1));
    let d = gap.filter(|v| len3(*v) > 0.005).map_or(up, norm3);
    let lat = norm3(cross3([0.0, 0.0, 1.0], d));
    let base = [p1[0], p1[1], p1[2] + r];
    let mut through = Vec::new();
    for speed in [1.6f32, 2.4, 3.2] {
        for back in [0.025f32, 0.05, 0.09] {
            for side in [0.0f32, -0.005, 0.005] {
                through.push(Variant::Roll {
                    layer,
                    pos: add3(add3(base, scale3(d, -back)), scale3(lat, side)),
                    vel: scale3(d, speed),
                });
            }
        }
    }
    if let Some(g) = gap.map(len3).filter(|g| *g > 0.06) {
        for speed in [0.8f32, 1.4] {
            for side in [0.0f32, -0.008, 0.008] {
                through.push(Variant::Roll {
                    layer,
                    pos: add3(add3(base, scale3(d, g * 0.5)), scale3(lat, side)),
                    vel: scale3(d, -speed),
                });
            }
        }
    }
    let mut down = Vec::new();
    for side in [0.0f32, -0.003, 0.003] {
        down.push(Variant::Roll {
            layer,
            pos: add3(add3(base, scale3(up, 0.04)), [side, 0.0, 0.0]),
            vel: scale3(up, -0.3),
        });
    }
    let mut all = if kind == Some(MechKind::Vuk) {
        down.extend(through);
        down
    } else {
        through.extend(down);
        through
    };
    // Starts on the playfield only (its plan origin is the lower-left
    // corner, x and y >= 0).
    all.retain(|v| match v {
        Variant::Roll { pos, .. } => pos[0] >= r && pos[1] >= r,
        Variant::Flip { .. } => true,
    });
    all
}

/// The game's main ball: the served ball, at rest on the plunger.
struct Main {
    handle: BallHandle,
    park: BallState,
}

/// The plunger's position (the served ball rests next to it).
fn plunger_pos() -> [f32; 3] {
    let t = table();
    let def = t
        .manifest
        .mechs
        .iter()
        .find(|m| m.kind == MechKind::Plunger)
        .expect("a plunger mech");
    t.mechs
        .iter()
        .find(|m| m.name == def.id)
        .and_then(|m| m.position_m)
        .expect("plunger position")
}

/// Starts a game if none is running and returns the served ball once it
/// rests at the plunger (within 80 mm, speed < 1 cm/s for 200 ms; ≤ 30 s).
fn main_ball(sim: &mut Simulation) -> Main {
    if pv(sim, "game_state") != 1 {
        assert!(
            run_until(sim, 60_000, |s| pv(s, "game_state") != 2),
            "game over ends"
        );
        start_game(sim);
    }
    let pp = plunger_pos();
    let mut still: Option<(BallHandle, u32)> = None;
    for _ in 0..30_000 {
        sim.advance(1);
        let resting = sim.ball_handles().into_iter().find(|&h| {
            sim.ball(h)
                .is_some_and(|b| len3(b.velocity_m_s) < 0.01 && plan_dist(b.position_m, pp) < 0.08)
        });
        still = match (resting, still) {
            (Some(h), Some((c, n))) if c == h => Some((c, n + 1)),
            (Some(h), _) => Some((h, 1)),
            (None, _) => None,
        };
        if let Some((h, n)) = still
            && n >= 200
        {
            let park = sim.ball(h).expect("resting ball");
            return Main { handle: h, park };
        }
        if pv(sim, "game_state") != 1 {
            start_game(sim);
        }
    }
    panic!("no served ball came to rest at the plunger within 30 s");
}

/// Presses `action` until `mode_sel` is `idx`.
fn select(sim: &mut Simulation, idx: i64) {
    for _ in 0..5 {
        if pv(sim, "mode_sel") == idx {
            return;
        }
        pulse(sim, "action", 30);
        sim.advance(60);
    }
    assert_eq!(pv(sim, "mode_sel"), idx, "mode selected by action presses");
}

/// What a physics-made mode start produced.
struct Started {
    sim: Simulation,
    main: Main,
    variant: usize,
    count_before: i64,
    /// Score change over the single tick in which `mode:<id>` increased.
    start_tick_points: i64,
}

/// New closing edges of the declared switches, observed tick by tick.
struct HitTrace {
    names: Vec<String>,
    counts: Vec<u32>,
    /// (tick, switch) in order of observation.
    hits: Vec<(u64, String)>,
}

impl HitTrace {
    fn new(sim: &Simulation) -> Self {
        let names: Vec<String> = table().switches.iter().map(|s| s.name.clone()).collect();
        let counts = names.iter().map(|n| hits(sim, n)).collect();
        Self {
            names,
            counts,
            hits: Vec::new(),
        }
    }

    /// Records the switches that closed since the last call (several in one
    /// tick in declaration order).
    fn observe(&mut self, sim: &Simulation) {
        for (i, n) in self.names.iter().enumerate() {
            let c = hits(sim, n);
            for _ in self.counts[i]..c {
                self.hits.push((sim.tick(), n.clone()));
            }
            self.counts[i] = c;
        }
    }

    /// Replays the rules' shot tracking (`shot_edge` in rules.lua) over the
    /// trace for the two-switch shot `order` with its declared `window`
    /// ticks: the window restarts on every opening hit, a hit more than
    /// `window` ticks after the latest opening hit has expired it, and the
    /// finishing hit completes the shot only while it is open. Returns the
    /// tick of the finishing hit of the latest shot completed this way.
    fn last_made(&self, order: &[String], window: u64) -> Option<u64> {
        assert_eq!(order.len(), 2, "two-switch shots: {order:?}");
        let (mut open, mut start, mut made) = (false, 0u64, None);
        for (t, n) in &self.hits {
            if open && *t - start > window {
                open = false;
            }
            if open && *n == order[1] {
                open = false;
                made = Some(*t);
            } else if *n == order[0] {
                open = true;
                start = *t;
            }
        }
        made
    }
}

/// The declared window, in ticks (1 ms each), of the `[[shots]]` entry
/// whose sequence is `order`.
fn shot_window_ticks(order: &[String]) -> u64 {
    let shot = table()
        .manifest
        .shots
        .iter()
        .find(|s| s.sequence == order)
        .unwrap_or_else(|| panic!("table.toml declares a [[shots]] entry for {order:?}"));
    u64::from(shot.window_ms)
}

/// The README's mode start award ("Every mode start scores N").
fn readme_mode_start_award() -> i64 {
    let text = readme_text();
    let key = "Every mode start scores";
    let at = text
        .find(key)
        .unwrap_or_else(|| panic!("README states {key:?}"));
    let digits: String = text[at + key.len()..]
        .chars()
        .skip_while(|c| c.is_whitespace())
        .take_while(|c| c.is_ascii_digit() || *c == ' ' || *c == ',')
        .filter(char::is_ascii_digit)
        .collect();
    digits.parse().expect("the award is a number")
}

/// Starts `mode` by moving the main ball onto the shot's lane with a
/// velocity toward it, trying the variants from `first` on (wrapping) and
/// stopping at the first that starts the mode (`mode:<id>` increases).
fn start_mode_by_physics(mode: &str, first: usize, pre_delay: u64) -> Started {
    let idx = MODES.iter().position(|m| *m == mode).expect("mode") as i64;
    let key_name = format!("mode:{mode}");
    let variants = shot_variants(mode);
    assert!(variants.len() >= 6, "at least 6 start offsets");
    let shot = switches_in(&readme_row(mode).shot_switches);
    let mut sim = load();
    sim.advance(pre_delay);
    let mut tried = Vec::new();
    let mut main: Option<Main> = None;
    for k in 0..variants.len() {
        let vi = (first + k) % variants.len();
        let v = variants[vi].clone();
        // The main ball: the previous one put back on the plunger, or the
        // next served ball.
        let m = match main.take() {
            Some(m) if sim.ball(m.handle).is_some() && pv(&sim, "game_state") == 1 => {
                assert!(sim.set_ball_state(m.handle, &m.park), "park the main ball");
                sim.advance(50);
                m
            }
            _ => main_ball(&mut sim),
        };
        select(&mut sim, idx);
        // Other balls (left over from a failed variant) leave the table.
        for h in sim.ball_handles() {
            if h != m.handle {
                sim.remove_ball(h);
            }
        }
        let (handle, park) = (m.handle, m.park);
        main = Some(m);
        let count_before = pv(&sim, &key_name);
        let mut trace = HitTrace::new(&sim);
        let mut flipped = None;
        match &v {
            Variant::Roll { layer, pos, vel } => {
                let mut b = park;
                b.position_m = *pos;
                b.velocity_m_s = *vel;
                b.angular_velocity_rad_s = [0.0; 3];
                b.layer = *layer;
                assert!(sim.set_ball_state(handle, &b), "move the main ball");
            }
            Variant::Flip { flipper, button, s } => {
                let mut b = park;
                b.position_m = on_bat(&sim, flipper, *s, 0.5, STEEL);
                b.velocity_m_s = [0.0; 3];
                b.angular_velocity_rad_s = [0.0; 3];
                b.layer = 0;
                assert!(sim.set_ball_state(handle, &b), "move the main ball");
                sim.advance(10);
                sim.set_switch(button, true).expect("flipper button");
                flipped = Some(*button);
            }
        }
        let mut last = None;
        let mut score_prev = sim.score(0).unwrap_or(0);
        let mut tick_points = 0;
        let started = run_until(&mut sim, 3_000, |s| {
            trace.observe(s);
            if let Some(b) = s.ball(handle) {
                last = Some((s.tick(), b.position_m, b.velocity_m_s, b.layer));
            }
            let score = s.score(0).unwrap_or(0);
            tick_points = score - score_prev;
            score_prev = score;
            pv(s, &key_name) > count_before
        });
        if sim.counters().balls_escaped > 0 {
            eprintln!(
                "{mode}: start state {vi} {v:?}: a ball escaped; main ball last seen {last:?}"
            );
        }
        if let Some(button) = flipped {
            sim.release_switch(button).expect("flipper button");
        }
        tried.push(vi);
        if started {
            // The README's shot, within its declared window, was completed
            // by a switch edge Lua had received when it changed the
            // counter. Edges closed by the physics of tick N are queued in
            // step 5 and reach Lua in step 2 of tick N + 1 (pin_sim
            // tick/mod.rs `step_tick`, `deliver_edges`), so the finishing
            // hit must be observed before the tick the counter changed.
            let window = shot_window_ticks(&shot);
            let finished = trace.last_made(&shot, window);
            assert!(
                finished.is_some_and(|t| t < sim.tick()),
                "{mode}: mode:{mode} increased at tick {} (start state {vi} {v:?}) without the README shot {shot:?} completed within {window} ticks by an edge Lua had received (latest completion {finished:?}); switch hits since the attempt began: {:?}",
                sim.tick(),
                trace.hits
            );
            eprintln!(
                "{mode}: started by start state {vi} {v:?} (tried {tried:?}); hits {:?}",
                trace.hits
            );
            return Started {
                sim,
                main: main.expect("main ball"),
                variant: vi,
                count_before,
                start_tick_points: tick_points,
            };
        }
        assert_healthy(&sim, mode);
        // Let shot windows expire.
        sim.advance(500);
    }
    panic!("{mode}: no physics start offset made the qualifying shot (tried {tried:?})");
}

fn mode_starts_by_physics(mode: &str) {
    let key_name = format!("mode:{mode}");
    let mut st = start_mode_by_physics(mode, 0, 0);
    let sim = &mut st.sim;
    let ball = pv(sim, "ball");
    let main = st.main.handle;
    assert!(
        sim.ball(main).is_some(),
        "{mode}: the shot ball is on the table at the start tick"
    );
    // The start award alone: the start tick's score change less what the
    // qualifying shot's last switch scores by itself, measured on the same
    // player's ball with the mode running. Meanwhile the main ball rests on
    // the plunger and every other ball leaves the table, so no switch
    // closes until every shot window (<= 10 s) and the flip-shot window
    // have expired.
    let last = switches_in(&readme_row(mode).shot_switches)
        .pop()
        .expect("README shot switches");
    for t in 0..10_500u64 {
        assert!(
            sim.set_ball_state(main, &st.main.park),
            "{mode}: park the main ball"
        );
        for h in sim.ball_handles() {
            if h != main {
                sim.remove_ball(h);
            }
        }
        sim.advance(1);
        if t == 200 {
            assert_eq!(
                pv(sim, &key_name),
                st.count_before + 1,
                "{mode}: mode:{mode} increased by exactly 1 (variant {})",
                st.variant
            );
        }
        assert_eq!(pv(sim, "ball"), ball, "{mode}: the same player's ball");
        assert_eq!(pv(sim, "game_state"), 1, "{mode}: the game goes on");
    }
    for h in sim.ball_handles() {
        if h != main {
            sim.remove_ball(h);
        }
    }
    let before = sim.score(0).unwrap_or(0);
    sim.set_switch(&last, true).expect("shot switch");
    sim.advance(1);
    let alone = sim.score(0).unwrap_or(0) - before;
    sim.set_switch(&last, false).expect("shot switch");
    sim.advance(5);
    sim.release_switch(&last).expect("shot switch");
    let award = readme_mode_start_award();
    assert_eq!(
        st.start_tick_points - alone,
        award,
        "{mode}: the start tick scored {} and {last} alone {alone}: the start award is {award} (README)",
        st.start_tick_points
    );
    // Still exactly one start a while later.
    sim.advance(2_000);
    assert_eq!(pv(sim, &key_name), st.count_before + 1, "{mode}: one start");
    assert_healthy(sim, mode);
}

#[test]
fn c_mode_banana_shots_by_physics() {
    big_stack(|| mode_starts_by_physics("banana_shots"));
}

#[test]
fn c_mode_backbox_play_by_physics() {
    big_stack(|| mode_starts_by_physics("backbox_play"));
}

#[test]
fn c_mode_xl_ball_by_physics() {
    big_stack(|| mode_starts_by_physics("xl_ball"));
}

#[test]
fn c_mode_magnet_compare_by_physics() {
    big_stack(|| mode_starts_by_physics("magnet_compare"));
}

#[test]
fn c_mode_glo_ball_by_physics() {
    big_stack(|| mode_starts_by_physics("glo_ball"));
}

// ---------------------------------------------------------------------------
// (d) extra balls
// ---------------------------------------------------------------------------

/// Simulated ticks an injected ball may take to leave through the trough.
const DRAIN_WAIT_TICKS: u64 = 500;

/// After `mode` starts: parks the main ball back on the plunger, waits for
/// extra balls, drains each (moved onto the trough switch after it lived
/// 2 s, counted once the simulation has removed it, within
/// `DRAIN_WAIT_TICKS`), and runs 30 s: the player's ball and the game go
/// on, also while the injected balls drain. Returns how many extra balls
/// were drained.
fn extra_balls_do_not_end_the_ball(mode: &str) -> usize {
    let mut st = start_mode_by_physics(mode, 0, 0);
    let sim = &mut st.sim;
    let main = st.main.handle;
    let ball = pv(sim, "ball");
    let mut drained = 0usize;
    let mut born: Vec<(BallHandle, u64)> = Vec::new();
    // Balls moved onto the trough, with the tick they were moved.
    let mut injected: Vec<(BallHandle, u64)> = Vec::new();
    let trough = drain_switch();
    let tp = trough.position_m.expect("trough position");
    let end = sim.tick() + 30_000;
    let mut main_gone_at = None;
    while sim.tick() < end || !injected.is_empty() {
        assert!(
            sim.tick() < end + DRAIN_WAIT_TICKS + 1,
            "{mode}: injected balls still on the table at tick {}: {injected:?}",
            sim.tick()
        );
        // Keep the main ball in play, at rest on the plunger.
        if sim.ball(main).is_some() {
            let b = sim.ball(main).unwrap();
            if plan_dist(b.position_m, st.main.park.position_m) > 0.002 {
                assert!(sim.set_ball_state(main, &st.main.park));
            }
        } else if main_gone_at.is_none() {
            main_gone_at = Some(sim.tick());
        }
        // Forget balls that left on their own (their handles may be reused).
        born.retain(|(x, _)| sim.ball(*x).is_some());
        if sim.tick() < end {
            for h in sim.ball_handles() {
                if h != main && !born.iter().any(|(x, _)| *x == h) {
                    born.push((h, sim.tick()));
                }
            }
        }
        let now = sim.tick();
        for (h, t0) in &mut born {
            if now >= *t0 + 2_000
                && !injected.iter().any(|(x, _)| x == h)
                && let Some(mut b) = sim.ball(*h)
            {
                let r = pin_core::profiles::BUILTIN_PROFILE_NAMES
                    .get(usize::from(b.profile.0))
                    .map_or(0.0135, |n| radius_m(n));
                b.position_m = [tp[0], tp[1], tp[2] + r];
                b.velocity_m_s = [0.0; 3];
                b.angular_velocity_rad_s = [0.0; 3];
                b.layer = trough.layer;
                assert!(sim.set_ball_state(*h, &b));
                injected.push((*h, now));
            }
        }
        sim.advance(1);
        // An injected ball counts once the simulation has removed it.
        let now = sim.tick();
        let mut i = 0;
        while i < injected.len() {
            let (h, t) = injected[i];
            if sim.ball(h).is_none() {
                drained += 1;
                injected.swap_remove(i);
                // The handle may be reused by a later ball.
                born.retain(|(x, _)| *x != h);
            } else {
                assert!(
                    now - t <= DRAIN_WAIT_TICKS,
                    "{mode}: the extra ball {h:?} moved onto {} at tick {t} is still on the table at tick {now}: {:?}",
                    trough.name,
                    sim.ball(h)
                );
                i += 1;
            }
        }
        assert_eq!(
            pv(sim, "ball"),
            ball,
            "{mode}: draining an extra ball changed the player's ball (tick {})",
            sim.tick()
        );
        assert_eq!(pv(sim, "game_state"), 1, "{mode}: the game goes on");
    }
    // The last trough edge reaches Lua on a later tick than the one that
    // removed the ball: advance through its delivery and recheck.
    sim.advance(3);
    assert!(
        sim.ball(main).is_some(),
        "{mode}: the main ball is still live after the last trough edge"
    );
    assert_eq!(
        pv(sim, "ball"),
        ball,
        "{mode}: the last trough edge changed the player's ball (tick {})",
        sim.tick()
    );
    assert_eq!(
        pv(sim, "game_state"),
        1,
        "{mode}: the game goes on after the last trough edge"
    );
    assert!(
        main_gone_at.is_none(),
        "{mode}: the main ball left the table at tick {main_gone_at:?}"
    );
    assert!(sim.balls_in_play() >= 1, "{mode}: the main ball is in play");
    let alive = sim.ball_handles().iter().filter(|h| **h != main).count();
    assert_eq!(
        sim.balls_in_play(),
        1 + alive,
        "{mode}: balls_in_play consistent"
    );
    assert_healthy(sim, mode);
    drained
}

#[test]
fn d_extra_balls_never_end_the_players_ball() {
    big_stack(|| {
        let mut total = 0;
        for mode in ["backbox_play", "xl_ball", "magnet_compare", "glo_ball"] {
            total += extra_balls_do_not_end_the_ball(mode);
        }
        assert!(
            total >= 1,
            "at least one mode served an extra ball that was drained"
        );
    });
}

// ---------------------------------------------------------------------------
// (e) magnet comparison
// ---------------------------------------------------------------------------

/// One coil-on episode of the hold magnet: a ball (not the main ball) is
/// "held" once it has stayed within 5 mm (plan) of the pole for 100 ms in a
/// row; from then until the coil goes off it must stay within 5 mm.
#[derive(Default)]
struct Episode {
    on: bool,
    /// (ball, profile, consecutive ticks within 5 mm).
    candidate: Option<(BallHandle, u16, u64)>,
    held: Option<(BallHandle, u16)>,
}

impl Episode {
    /// Observes one tick; returns the held profile when an episode with a
    /// held ball ends (the coil went off).
    fn observe(
        &mut self,
        sim: &Simulation,
        on: bool,
        main: BallHandle,
        pole: [f32; 3],
        variant: usize,
    ) -> Option<u16> {
        if !on {
            let out = if self.on {
                self.held.map(|(_, p)| p)
            } else {
                None
            };
            *self = Episode::default();
            return out;
        }
        self.on = true;
        if let Some((h, prof)) = self.held {
            let b = sim.ball(h).unwrap_or_else(|| {
                panic!("variant {variant}: the held ball left the table while the coil is on")
            });
            let d = plan_dist(b.position_m, pole);
            assert!(
                d <= 0.005,
                "variant {variant}: held ball (profile {prof}) {:.2} mm from the pole while the coil is on (tick {})",
                d * 1e3,
                sim.tick()
            );
            return None;
        }
        let near = sim.ball_handles().into_iter().find(|&h| {
            h != main
                && sim
                    .ball(h)
                    .is_some_and(|b| plan_dist(b.position_m, pole) <= 0.005)
        });
        self.candidate = match (near, self.candidate) {
            (Some(h), Some((c, p, n))) if c == h => Some((c, p, n + 1)),
            (Some(h), _) => Some((h, sim.ball(h).unwrap().profile.0, 1)),
            (None, _) => None,
        };
        if let Some((h, p, n)) = self.candidate
            && n >= 100
        {
            self.held = Some((h, p));
        }
        None
    }
}

/// Minimum `cmp_release_ms_standard - cmp_release_ms_low_rem`, ms (brief
/// S3 e; the README may state a smaller achieved bound with a decision
/// entry, see `release_margin_ms`).
const RELEASE_MARGIN_MS: i64 = 10;

#[test]
fn e_magnet_compare_holds_both_and_standard_releases_later() {
    big_stack(|| {
        let mag = &table().magnets[0];
        let pole = mag.position_m;
        let (std_i, low_i) = (profile_index(STEEL), profile_index(LOW_REM));
        for variant in 0..3usize {
            let mut st = start_mode_by_physics("magnet_compare", variant, 97 * variant as u64);
            let sim = &mut st.sim;
            let main = st.main.handle;
            let mut held_profiles: Vec<u16> = Vec::new();
            let mut ep = Episode::default();
            let mut done = false;
            for _ in 0..60_000 {
                if let Some(b) = sim.ball(main)
                    && plan_dist(b.position_m, st.main.park.position_m) > 0.002
                {
                    assert!(sim.set_ball_state(main, &st.main.park));
                }
                sim.advance(1);
                let on = sim.solver().magnet_power(mag.id) > 0.0;
                if let Some(prof) = ep.observe(sim, on, main, pole, variant) {
                    held_profiles.push(prof);
                }
                let s = pv(sim, "cmp_release_ms_standard");
                let l = pv(sim, "cmp_release_ms_low_rem");
                if held_profiles.contains(&std_i)
                    && held_profiles.contains(&low_i)
                    && s > 0
                    && l > 0
                {
                    done = true;
                    break;
                }
            }
            let s = pv(sim, "cmp_release_ms_standard");
            let l = pv(sim, "cmp_release_ms_low_rem");
            assert!(
                done,
                "variant {variant}: both profiles held and measured (held {held_profiles:?}, standard {s} ms, low_rem {l} ms)"
            );
            assert!(
                s - l >= RELEASE_MARGIN_MS,
                "variant {variant}: standard releases later than low_rem by >= {RELEASE_MARGIN_MS} ms ({s} vs {l})"
            );
            assert_healthy(sim, "magnet compare");
        }
    });
}
