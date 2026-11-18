//! 6.4 R9 (and C7, C8): the Feature Lab rules in scripted headless runs.
//!
//! * A full 3-ball game with injected drains: scores only increase, `ball`
//!   runs 1..3, `game_state` goes 0, 1, 2, 0, no script fault or queue
//!   overflow.
//! * Extra balls never end the player's ball.
//! * Per mode: a run with state injection makes the qualifying shot by
//!   physics (no shot switch forced); `mode:<id>` increments exactly once,
//!   the mode's scene is shown and points are awarded.
//! * The selector: `action` and every ball start advance `mode_sel` by one
//!   (mod 5), visiting all five values.

mod feature_lab_support;

use feature_lab_support::*;
use pin_sim::Simulation;

/// Moves ball `h` to `p` (mm) with velocity `v` (m/s), no spin.
fn put(s: &mut Simulation, h: pin_sim::BallHandle, p: [f32; 2], v: [f32; 2]) {
    let mut b = s.ball(h).unwrap();
    b.position_m = mm(p[0], p[1]);
    b.velocity_m_s = [v[0], v[1], 0.0];
    b.angular_velocity_rad_s = [0.0; 3];
    s.set_ball_state(h, &b);
}

/// Drains ball `h` into the main trough (between trough_1 and trough_2).
fn drain(s: &mut Simulation, h: pin_sim::BallHandle) {
    put(s, h, [257.0, 30.0], [0.0, -0.1]);
    s.advance(20);
    assert!(s.ball(h).is_none(), "drained");
}

/// The player's ball (the only one in play).
fn main_ball(s: &Simulation) -> pin_sim::BallHandle {
    let b = balls(s);
    assert_eq!(b.len(), 1, "one ball in play");
    b[0]
}

#[test]
fn full_game_by_trough_drains() {
    on_big_stack(|| {
        let mut s = load();
        s.advance(10);
        assert_eq!(persist(&s, "game_state"), 0);
        assert_eq!(persist(&s, "ball"), 0);
        let mut states = vec![0];
        start_game(&mut s);
        states.push(persist(&s, "game_state"));
        let mut last = s.score(0).unwrap_or(0);
        for ball in 1..=3 {
            assert_eq!(persist(&s, "ball"), ball);
            assert_eq!(persist(&s, "game_state"), 1);
            // Score something: launch round the orbit (shooter lane, orbit
            // top), then drain.
            press(&mut s, "launch", 30);
            for _ in 0..1500 {
                s.advance(1);
                let now = s.score(0).unwrap_or(0);
                assert!(now >= last, "the score never decreases");
                last = now;
            }
            assert!(last > 0, "ball {ball} scored");
            let h = main_ball(&s);
            drain(&mut s, h);
            s.advance(1500);
        }
        states.push(persist(&s, "game_state"));
        assert_eq!(persist(&s, "ball"), 0);
        assert_eq!(persist(&s, "games_played"), 1);
        assert_eq!(persist(&s, "high_score"), s.score(0).unwrap());
        assert_eq!(s.balls_in_play(), 0, "no ball served after the third");
        let shown = advance_scenes(&mut s, 4000);
        states.push(persist(&s, "game_state"));
        assert_eq!(states, [0, 1, 2, 0]);
        assert_eq!(shown, ["scene_attract"]);
        healthy(&s);
        // A second game starts from attract.
        start_game(&mut s);
        healthy(&s);
    });
}

#[test]
fn an_extra_ball_never_ends_the_players_ball() {
    on_big_stack(|| {
        let mut s = load();
        s.advance(10);
        start_game(&mut s);
        select_mode(&mut s, 4);
        let main = main_ball(&s);
        make_shot(&mut s, 4);
        assert_eq!(persist(&s, "mode:glo_ball"), 1);
        // The Glo-Ball is served; drain the player's ball.
        s.advance(200);
        let extra: Vec<_> = balls(&s).into_iter().filter(|h| *h != main).collect();
        assert_eq!(extra.len(), 1, "the Glo-Ball is in play");
        if s.ball(main).is_some() {
            drain(&mut s, main);
        }
        s.advance(3000);
        assert_eq!(persist(&s, "ball"), 1, "the Glo-Ball keeps ball 1 going");
        assert_eq!(persist(&s, "game_state"), 1);
        for h in balls(&s) {
            drain(&mut s, h);
        }
        s.advance(10);
        assert_eq!(persist(&s, "ball"), 2, "the last drain ends the ball");
        healthy(&s);
    });
}

#[test]
fn selector_advances_on_action_and_on_every_ball() {
    on_big_stack(|| {
        let mut s = load();
        s.advance(10);
        let at_load = persist(&s, "mode_sel");
        assert!((0..5).contains(&at_load));
        // Not in attract.
        press(&mut s, "action", 5);
        s.advance(5);
        assert_eq!(persist(&s, "mode_sel"), at_load);
        start_game(&mut s);
        let first = persist(&s, "mode_sel");
        assert_eq!(first, (at_load + 1) % 5, "ball 1 start advances it");
        let mut seen = vec![first];
        for _ in 0..5 {
            press(&mut s, "action", 5);
            s.advance(5);
            let v = persist(&s, "mode_sel");
            assert_eq!(v, (seen.last().unwrap() + 1) % 5, "action advances it");
            seen.push(v);
        }
        let mut all = seen.clone();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all, [0, 1, 2, 3, 4], "{seen:?}");
        let before = persist(&s, "mode_sel");
        let h = main_ball(&s);
        drain(&mut s, h);
        s.advance(100);
        assert_eq!(persist(&s, "ball"), 2);
        assert_eq!(persist(&s, "mode_sel"), (before + 1) % 5, "ball 2 start");
        healthy(&s);
    });
}

/// Start states rolling the ball at a pop bumper (centre `c`, mm) from 16
/// directions at 3 speeds: its kick sends the ball on to the next bumper.
fn around_bumper(h: pin_sim::BallHandle, c: [f32; 2]) -> Vec<Try> {
    let mut v: Vec<Try> = Vec::new();
    for k in 0..16 {
        let a = (k as f32 * 22.5f32).to_radians();
        for speed in [0.8f32, 1.4, 2.0] {
            let p = [c[0] + 60.0 * a.cos(), c[1] + 60.0 * a.sin()];
            let vel = [-speed * a.cos(), -speed * a.sin()];
            v.push(Box::new(move |s: &mut Simulation| put(s, h, p, vel)));
        }
    }
    v
}

/// One start state of a shot attempt.
type Try = Box<dyn Fn(&mut Simulation)>;

/// Makes mode `i`'s qualifying shot with the player's ball, by physics:
/// the ball is placed and launched (state injection) or the plunger fired;
/// the first start state that starts the mode wins.
fn make_shot(s: &mut Simulation, i: usize) {
    let key = format!("mode:{}", MODES[i]);
    let before = persist(s, &key);
    let h = main_ball(s);
    // Start states: (position mm, velocity m/s) or the plunger / a flip.
    let tries: Vec<Try> = match i {
        0 => (0..36)
            .map(|k| {
                let f = 0.3 + 0.05 * (k % 12) as f32;
                let delay = 20 * (k / 12) as u64;
                Box::new(move |s: &mut Simulation| banana_flip(s, h, f, delay)) as Try
            })
            .collect(),
        1 => vec![
            Box::new(move |s: &mut Simulation| put(s, h, [425.0, 500.0], [0.0, 1.5])),
            Box::new(move |s: &mut Simulation| put(s, h, [425.0, 470.0], [0.0, 2.0])),
        ],
        2 => around_bumper(h, [257.0, 810.0]),
        3 => vec![
            Box::new(move |s: &mut Simulation| put(s, h, [359.0, 392.0], [0.73, 1.86])),
            Box::new(move |s: &mut Simulation| put(s, h, [355.0, 380.0], [0.9, 2.3])),
        ],
        _ => around_bumper(h, [324.0, 720.0]),
    };
    for t in &tries {
        // Back on the plunger at rest between tries.
        put(s, h, [492.0, 70.0], [0.0, 0.0]);
        s.advance(50);
        t(s);
        for _ in 0..12_000 {
            s.advance(1);
            if persist(s, &key) > before {
                return;
            }
            // A missed shot coming back down: rescue it before it drains
            // (it would end the ball and move the selector).
            let b = s.ball(h).expect("the shot ball stays in play");
            if b.position_m[0] < 0.47 && b.position_m[1] < 0.14 && b.velocity_m_s[1] < 0.0 {
                break;
            }
        }
    }
    panic!("no start state made the {} shot", MODES[i]);
}

/// Puts ball `h` at rest just above the Banana's chord at fraction `f`,
/// lets it touch down, then flips after `delay` ms.
pub fn banana_flip(s: &mut Simulation, h: pin_sim::BallHandle, f: f32, delay: u64) {
    let a = 210.0f32.to_radians();
    let p = [
        357.0 + f * 76.0 * a.cos(),
        150.0 + f * 76.0 * a.sin() + 45.0,
    ];
    put(s, h, p, [0.0, 0.0]);
    for _ in 0..600 {
        s.advance(1);
        if s.ball(h).unwrap().velocity_m_s[1] > -0.01 {
            break;
        }
    }
    s.advance(10 + delay);
    press(s, "right_flipper", 150);
}

#[test]
fn every_mode_starts_once_on_its_shot_with_scene_and_points() {
    on_big_stack(|| {
        for (i, m) in MODES.iter().enumerate() {
            let mut s = load();
            s.advance(10);
            start_game(&mut s);
            select_mode(&mut s, i as i64);
            let key = format!("mode:{m}");
            assert_eq!(persist(&s, &key), 0);
            let score = s.score(0).unwrap_or(0);
            make_shot(&mut s, i);
            // make_shot stops on the tick the key changed; the rules show
            // the scene in the same callback, so that tick's scene list
            // holds it.
            let scene = s
                .ids()
                .get(pin_assets::IdKind::Scene, &format!("scene_{m}"))
                .expect("scene id");
            assert!(
                s.display_scenes().iter().any(|x| x.0 == scene),
                "{m}: scene_{m} shown"
            );
            assert_eq!(persist(&s, &key), 1, "{m} started once");
            for o in MODES.iter().filter(|o| *o != m) {
                assert_eq!(persist(&s, &format!("mode:{o}")), 0, "{o}");
            }
            s.advance(3000);
            assert_eq!(persist(&s, &key), 1, "{m}: once per ball");
            assert!(
                s.score(0).unwrap_or(0) >= score + 2000,
                "{m}: points awarded"
            );
            healthy(&s);
        }
    });
}
