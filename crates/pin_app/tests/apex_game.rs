//! 6.1 T2 (PLAN 6.1 verification): a headless scripted 3-ball game on the
//! Apex Circuit with state injection (`spawn_ball`, `set_ball_state`,
//! `set_switch`, `advance`), simulated time only.
//!
//! Covered: attract → start → ball 1 (non-lit top lane in the skill window,
//! a steel ball through the magnet pair does not start Powerball Mania) →
//! ball 2 (the ceramic Powerball: its plunge starts Mania, scoring doubles) →
//! ball 3 (combo steps 2x..5x with decay and the `combo_chain` mode, action
//! without Overdrive is a no-op, Overdrive by combo steps, Apex Jackpot with
//! its decay and floor, Firewall Save closing the zippers for 5000 ticks,
//! the lit-lane skill shot worth 10,000 × ball, the cross-shot ramp) →
//! bonus at each drain → high-score entry → game over → attract. The display
//! scene sequence is read from `Simulation::display_scenes` every tick;
//! no script fault and no queue overflow.

mod apex_support;

use apex_support::{apex_bundle, mm, on_big_stack, persist_key, scene_id};
use pin_core::ids::{FlipperId, ProfileId};
use pin_sim::{SimConfig, Simulation};
use std::collections::BTreeMap;

/// Points per closing edge for the multiplied playfield switches
/// (rules.lua; drop-bank completion, lanes and fixed awards excluded).
const POINTS: [(&str, i64); 17] = [
    ("bumper_1_ring", 100),
    ("bumper_2_ring", 100),
    ("bumper_3_ring", 100),
    ("sling_left_sw", 10),
    ("sling_right_sw", 10),
    ("left_spinner", 50),
    ("orbit_top", 2000),
    ("right_orbit_entry", 1000),
    ("magnet_opto", 2500),
    ("ramp_entry", 500),
    ("ramp_made", 10000),
    ("drop_1_sw", 750),
    ("drop_2_sw", 750),
    ("drop_3_sw", 750),
    ("lock_lane", 100),
    ("lock_scoop_sw", 5000),
    ("left_inlane", 100),
];

/// More switches watched for counters.
const WATCHED: [&str; 7] = [
    "right_inlane",
    "lane_1",
    "lane_2",
    "lane_3",
    "shooter_lane",
    "magnet_eddy",
    "trough_1",
];

/// Switches whose hits the bonus counts (rules.lua `shot_hit` / `lane`).
const COUNTED: [&str; 10] = [
    "orbit_top",
    "right_orbit_entry",
    "ramp_made",
    "lock_scoop_sw",
    "drop_1_sw",
    "drop_2_sw",
    "drop_3_sw",
    "lane_1",
    "lane_2",
    "lane_3",
];

type Hits = BTreeMap<&'static str, u32>;

fn hits(s: &Simulation) -> Hits {
    POINTS
        .iter()
        .map(|(n, _)| *n)
        .chain(WATCHED)
        .chain(["trough_2"])
        .map(|n| (n, s.switch_hits(n).expect(n)))
        .collect()
}

fn d(a: &Hits, b: &Hits, n: &str) -> i64 {
    i64::from(b[n] - a[n])
}

/// The rules' multiplied points for the switch hits between `a` and `b`
/// (lane hits outside the skill window score 1,000 each, multiplied).
fn points(a: &Hits, b: &Hits) -> i64 {
    let lanes: i64 = ["lane_1", "lane_2", "lane_3"]
        .iter()
        .map(|n| d(a, b, n) * 1000)
        .sum();
    POINTS.iter().map(|(n, p)| d(a, b, n) * p).sum::<i64>() + lanes + d(a, b, "right_inlane") * 100
}

/// A simulation with a log of every display scene shown.
struct Game {
    s: Simulation,
    scenes: Vec<u16>,
    k: BTreeMap<&'static str, u16>,
}

const KEYS: [&str; 12] = [
    "ball",
    "game_state",
    "high_score",
    "games_played",
    "firewall_save_count",
    "apex_jackpot_count",
    "skill_shot_count",
    "mode:powerball_mania",
    "mode:combo_chain",
    "combo_level",
    "overdrive_ready",
    "last_bonus",
];

impl Game {
    fn new() -> Game {
        let s = Simulation::load(apex_bundle(), SimConfig::default()).unwrap();
        let scenes = s.display_scenes().iter().map(|x| x.0).collect();
        let k = KEYS.iter().map(|n| (*n, persist_key(&s, n))).collect();
        Game { s, scenes, k }
    }
    fn p(&self, key: &str) -> i64 {
        self.s.persist(self.k[key])
    }
    fn score(&self) -> i64 {
        self.s.score(0).unwrap()
    }
    fn step(&mut self, n: u64) {
        for _ in 0..n {
            self.s.advance(1);
            self.scenes
                .extend(self.s.display_scenes().iter().map(|x| x.0));
        }
    }
    fn press(&mut self, name: &str) {
        self.s.set_switch(name, true).unwrap();
        self.step(1);
        self.s.release_switch(name).unwrap();
        self.step(1);
    }
    fn scene(&self, name: &str) -> u16 {
        scene_id(&self.s, name)
    }
    fn last_scene(&self) -> u16 {
        *self.scenes.last().unwrap()
    }
    /// Moves the (only) ball.
    fn put(&mut self, p: [f32; 3], v: [f32; 3]) {
        let h = self.s.ball_handles()[0];
        let mut st = self.s.ball(h).unwrap();
        st.position_m = p;
        st.velocity_m_s = v;
        st.angular_velocity_rad_s = [0.0; 3];
        st.layer = 0;
        assert!(self.s.set_ball_state(h, &st));
    }
    /// Steps until switch `name` has closed once more (at most `limit`).
    fn until_hit(&mut self, name: &str, limit: u64) {
        let n = self.s.switch_hits(name).unwrap();
        for _ in 0..limit {
            self.step(1);
            if self.s.switch_hits(name).unwrap() > n {
                return;
            }
        }
        panic!("{name} did not close within {limit} ticks");
    }
    fn healthy(&self) {
        assert!(!self.s.script_faulted(), "{:?}", self.s.script_error());
        assert_eq!(self.s.script_queue_overflows(), 0);
        let c = self.s.counters();
        assert_eq!(c.edges_dropped, 0, "{c:?}");
        assert_eq!(c.spawns_failed, 0, "{c:?}");
        assert_eq!(c.balls_escaped, 0, "{c:?}");
    }
    /// The served ball rests on the plunger.
    fn wait_served(&mut self, ball: i64) {
        for _ in 0..3000 {
            if self.p("ball") == ball && self.s.switch_closed("plunger_tip") == Some(true) {
                self.step(200);
                return;
            }
            self.step(1);
        }
        panic!("ball {ball} not served onto the plunger");
    }
    /// Drains the ball; checks the bonus; returns once the next ball (or
    /// game over) came.
    fn drain(&mut self, since_serve: &Hits) {
        let combo = self.p("combo_level");
        let now = hits(&self.s);
        let counted: i64 = COUNTED.iter().map(|n| d(since_serve, &now, n)).sum();
        let before = self.score();
        let ball = self.p("ball");
        self.put(mm(257.0, 120.0), [0.0, -0.5, 0.0]);
        let mut drained = false;
        for _ in 0..1000 {
            self.step(1);
            if self.s.balls_in_play() == 0 {
                drained = true;
                break;
            }
        }
        assert!(drained, "ball {ball} drained");
        self.step(5);
        let bonus = counted * 1000 * combo;
        assert_eq!(self.p("last_bonus"), bonus, "bonus of ball {ball}");
        assert_eq!(self.score() - before, bonus, "bonus awarded");
        assert_eq!(self.last_scene(), self.scene("scene_bonus"));
        assert_eq!(self.p("combo_level"), 1, "combo reset at the drain");
        assert_eq!(self.p("overdrive_ready"), 0);
    }
    fn launch(&mut self) {
        self.s.set_switch("launch", true).unwrap();
        self.step(20);
        self.s.release_switch("launch").unwrap();
    }
}

fn zipper_mm(s: &Simulation, name: &str) -> f32 {
    let id: FlipperId = s.ids().flipper(name).unwrap();
    s.flipper_set().base_offset_mm(id)
}

fn profile(name: &str) -> ProfileId {
    ProfileId(
        pin_core::profiles::BUILTIN_PROFILE_NAMES
            .iter()
            .position(|n| *n == name)
            .unwrap() as u16,
    )
}

/// The scripted game; returns (final score, state hash).
fn scripted_game() -> (i64, u64) {
    let mut g = Game::new();
    g.step(100);
    assert_eq!(g.p("game_state"), 0, "attract");
    assert_eq!(g.scenes, [g.scene("scene_attract")]);
    assert_eq!(g.s.balls_in_play(), 0);

    // --- Start: ball 1 (steel) on the plunger. ---
    g.press("start");
    assert_eq!(g.p("game_state"), 1);
    assert_eq!(g.last_scene(), g.scene("scene_score"));
    g.wait_served(1);
    let h1 = g.s.ball_handles()[0];
    assert_eq!(g.s.ball(h1).unwrap().profile, profile("standard_steel"));
    let serve1 = hits(&g.s);

    // ACTION without Overdrive Ready does nothing.
    let before = g.score();
    g.press("action");
    g.step(100);
    assert_eq!(g.score(), before);
    assert_eq!(zipper_mm(&g.s, "left_flipper"), 0.0);
    assert_eq!(g.p("firewall_save_count") + g.p("apex_jackpot_count"), 0);

    // Plunge; in the skill window a non-lit lane (ball 1 lights lane_1)
    // scores a fixed 1,000 and no skill shot.
    g.launch();
    g.until_hit("shooter_lane", 500);
    let before = g.score();
    let a = hits(&g.s);
    g.put(mm(340.0, 950.0), [0.0, -0.3, 0.0]);
    g.until_hit("lane_2", 200);
    g.step(1);
    let b = hits(&g.s);
    assert_eq!(d(&a, &b, "lane_2"), 1);
    assert_eq!(g.score() - before, 1000 + points(&a, &b) - 1000);
    assert_eq!(g.p("skill_shot_count"), 0);

    // Cross-shot ramp (the grey box's straight-up injection into the lane);
    // no left-orbit feed on this ball, so the combo stays at 1x.
    assert_eq!(g.p("combo_level"), 1);
    let a = hits(&g.s);
    let before = g.score();
    g.put(mm(420.0, 330.0), [0.0, 2.3, 0.0]);
    g.until_hit("ramp_made", 1500);
    g.step(2);
    let b = hits(&g.s);
    assert!(d(&a, &b, "ramp_entry") >= 1);
    assert_eq!(d(&a, &b, "ramp_made"), 1);
    assert_eq!(g.p("combo_level"), 1);
    assert_eq!(g.score() - before, points(&a, &b), "ramp points");

    // A steel ball through the magnet pair closes eddy and opto: no Mania.
    let a = hits(&g.s);
    g.put(mm(492.0, 860.0), [0.0, 1.2, 0.0]);
    g.until_hit("magnet_opto", 300);
    g.step(100);
    let b = hits(&g.s);
    assert_eq!(d(&a, &b, "magnet_eddy"), 1, "steel closes the eddy sensor");
    assert_eq!(g.p("mode:powerball_mania"), 0, "steel starts no Mania");
    assert!(!g.scenes.contains(&g.scene("scene_powerball_mania")));
    g.drain(&serve1);
    assert_eq!(g.p("ball"), 1, "ball 1 ends after the bonus");

    // --- Ball 2: the ceramic Powerball; its plunge passes the detector. ---
    g.wait_served(2);
    let pb = g.s.ball_handles()[0];
    assert_eq!(g.s.ball(pb).unwrap().profile, profile("ceramic_powerball"));
    let serve2 = hits(&g.s);
    g.launch();
    for _ in 0..3000 {
        g.step(1);
        if g.p("mode:powerball_mania") == 1 {
            break;
        }
    }
    assert_eq!(g.p("mode:powerball_mania"), 1, "the Powerball starts Mania");
    assert_eq!(
        g.last_scene(),
        g.scene("scene_powerball_mania"),
        "Mania start scene"
    );
    assert_eq!(
        d(&serve2, &hits(&g.s), "magnet_eddy"),
        0,
        "ceramic: no eddy"
    );
    // Doubled scoring: a bumper shot.
    g.step(2500);
    let combo = g.p("combo_level");
    let before = g.score();
    let a = hits(&g.s);
    g.put(mm(190.0, 640.0), [0.0, 1.0, 0.0]);
    g.step(150);
    let b = hits(&g.s);
    assert!(d(&a, &b, "bumper_1_ring") >= 1);
    assert_eq!(g.p("combo_level"), combo);
    assert_eq!(g.score() - before, 2 * combo * points(&a, &b), "Mania x2");
    g.drain(&serve2);

    // --- Ball 3 (steel): rules features while it waits on the plunger. ---
    g.wait_served(3);
    let h3 = g.s.ball_handles()[0];
    assert_eq!(g.s.ball(h3).unwrap().profile, profile("standard_steel"));
    let serve3 = hits(&g.s);
    let before = g.score();
    // Combo: the spinner (a left-orbit pass) feeds; shot switches within
    // 4 s step 2x, 3x, 4x, 5x (no further).
    let chains = g.p("mode:combo_chain");
    g.press("left_spinner");
    g.step(500);
    let mut expect = before + 50;
    for level in 2..=6 {
        g.press("right_orbit_entry");
        let mult = (level - 1).min(5);
        expect += 1000 * mult;
        assert_eq!(g.p("combo_level"), level.min(5), "combo step {level}");
        assert_eq!(g.score(), expect, "multiplied entry at level {level}");
        if level == 2 {
            assert_eq!(g.p("mode:combo_chain"), chains + 1, "combo_chain started");
            assert!(g.scenes.contains(&g.scene("scene_combo_chain")));
        }
        g.step(1000);
    }
    // Five combo steps lit Overdrive.
    assert_eq!(g.p("overdrive_ready"), 1);
    assert_eq!(g.last_scene(), g.scene("scene_overdrive"));
    // Apex Jackpot (no inlane in the last 3 s): 50,000 − 1,000 per 100 ms
    // since Overdrive was lit (on the 5th step, 1,000 + 2 ticks ago).
    g.step(250);
    let before = g.score();
    g.s.set_switch("action", true).unwrap();
    g.step(1);
    g.s.release_switch("action").unwrap();
    // Lit 1,252 ticks ago: 50,000 - 12 x 1,000.
    assert_eq!(g.score() - before, 38_000, "decayed jackpot");
    g.step(1);
    assert_eq!(g.p("apex_jackpot_count"), 1);
    assert_eq!(g.p("overdrive_ready"), 0, "Overdrive used up");
    assert!(g.scenes.contains(&g.scene("scene_apex_jackpot")));
    // Used up: a second press does nothing.
    let before = g.score();
    g.press("action");
    assert_eq!(g.score(), before);
    // Decay: 12 s without a combo hit drops one level each.
    let level = g.p("combo_level");
    assert_eq!(level, 5);
    g.step(12_000);
    assert_eq!(g.p("combo_level"), 4, "decay after 12 s");
    g.step(12_000);
    assert_eq!(g.p("combo_level"), 3, "decay after 24 s");
    // Re-light Overdrive (5 more combo steps) and let the jackpot decay to
    // its floor.
    g.press("left_spinner");
    for _ in 0..5 {
        g.press("right_orbit_entry");
        g.step(500);
    }
    assert_eq!(g.p("overdrive_ready"), 1);
    g.step(10_000);
    let before = g.score();
    g.press("action");
    assert_eq!(g.score() - before, 5000, "jackpot floor");
    assert_eq!(g.p("apex_jackpot_count"), 2);
    // Firewall Save: Overdrive again, an inlane closed within 3 s, one ball.
    g.press("left_spinner");
    for _ in 0..5 {
        g.press("right_orbit_entry");
        g.step(500);
    }
    assert_eq!(g.p("overdrive_ready"), 1);
    g.press("left_inlane");
    g.step(1000);
    let before = g.score();
    g.s.set_switch("action", true).unwrap();
    g.step(1);
    g.s.release_switch("action").unwrap();
    let t0 = g.s.tick();
    assert_eq!(g.score(), before, "Firewall Save scores nothing");
    g.step(100);
    assert_eq!(g.p("firewall_save_count"), 1);
    assert!(g.scenes.contains(&g.scene("scene_firewall_save")));
    for f in ["left_flipper", "right_flipper"] {
        assert!(
            (zipper_mm(&g.s, f) - 12.0).abs() < 1e-3,
            "{f} zipped closed"
        );
    }
    g.step(t0 + 4998 - g.s.tick());
    for f in ["left_flipper", "right_flipper"] {
        assert!(
            (zipper_mm(&g.s, f) - 12.0).abs() < 1e-3,
            "{f} still closed at +4998"
        );
    }
    g.step(t0 + 5150 - g.s.tick());
    for f in ["left_flipper", "right_flipper"] {
        assert!(
            zipper_mm(&g.s, f).abs() < 1e-3,
            "{f} reopened after 5000 ticks"
        );
    }
    // Plunge: the lit lane (lane_3 on ball 3) is the skill shot,
    // 10,000 × 3, fixed.
    g.launch();
    g.until_hit("shooter_lane", 500);
    let before = g.score();
    let a = hits(&g.s);
    g.put(mm(380.0, 950.0), [0.0, -0.3, 0.0]);
    g.until_hit("lane_3", 200);
    g.step(1);
    let b = hits(&g.s);
    let combo = g.p("combo_level");
    assert_eq!(
        g.score() - before,
        30_000 + combo * (points(&a, &b) - 1000),
        "skill shot"
    );
    assert_eq!(g.p("skill_shot_count"), 1);
    assert!(g.scenes.contains(&g.scene("scene_skill_shot")));
    g.healthy();
    g.drain(&serve3);
    g.step(2100);

    // --- Game over: high-score entry, then game over, then attract. ---
    let score = g.score();
    assert!(score > 0);
    assert_eq!(g.p("game_state"), 2);
    assert_eq!(g.p("ball"), 0);
    assert_eq!(g.p("games_played"), 1);
    assert_eq!(g.p("high_score"), score);
    assert_eq!(g.last_scene(), g.scene("scene_high_score"));
    assert_eq!(g.s.balls_in_play(), 0);
    g.step(3000);
    assert_eq!(g.last_scene(), g.scene("scene_game_over"));
    g.step(4000);
    assert_eq!(g.p("game_state"), 0, "back to attract");
    assert_eq!(g.last_scene(), g.scene("scene_attract"));
    assert_eq!(g.s.counters().balls_drained, 3);
    g.healthy();

    // The scene sequence of the game (first occurrences, in order).
    let order = [
        "scene_attract",
        "scene_score",
        "scene_bonus",
        "scene_powerball_mania",
        "scene_combo_chain",
        "scene_overdrive",
        "scene_apex_jackpot",
        "scene_firewall_save",
        "scene_skill_shot",
        "scene_high_score",
        "scene_game_over",
    ];
    let first: Vec<usize> = order
        .iter()
        .map(|n| {
            let id = g.scene(n);
            g.scenes.iter().position(|x| *x == id).expect(n)
        })
        .collect();
    assert!(first.windows(2).all(|w| w[0] < w[1]), "{order:?} {first:?}");
    (score, g.s.state_hash())
}

#[test]
fn scripted_three_ball_game() {
    let (score, hash) = on_big_stack(scripted_game);
    eprintln!("apex game score {score}, state hash {hash:016x}");
    // Reproducible (D9): the same injected game gives the same result.
    assert_eq!(on_big_stack(scripted_game), (score, hash));
}

/// A second game beats nothing when it scores less: no high-score entry.
#[test]
fn lower_score_shows_game_over_without_entry() {
    on_big_stack(|| {
        let mut g = Game::new();
        g.step(10);
        for game in 1..=2 {
            g.press("start");
            for ball in 1..=3 {
                g.wait_served(ball);
                let since = hits(&g.s);
                if game == 1 && ball == 1 {
                    // Some points in game 1 only.
                    g.press("left_spinner");
                    g.press("right_orbit_entry");
                }
                g.drain(&since);
            }
            g.step(2100);
            assert_eq!(g.p("game_state"), 2);
            assert_eq!(g.p("games_played"), game);
            if game == 1 {
                assert_eq!(g.last_scene(), g.scene("scene_high_score"));
                assert!(g.p("high_score") > 0);
            } else {
                assert_eq!(g.last_scene(), g.scene("scene_game_over"));
                assert!(g.score() < g.p("high_score"));
            }
            g.step(7100);
            assert_eq!(g.p("game_state"), 0);
        }
        g.healthy();
    });
}
