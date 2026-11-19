//! 6.2 T4: the Sky Deck rules (`rules.lua`) in the headless simulation.
//!
//! * A full 3-ball game by injection: attract → START → three balls →
//!   game over (score > 0, no ball on the table, high score persisted) →
//!   attract.
//! * Sky Bridge: `action` opens it for exactly 8000 ticks (`bridge_open`),
//!   a press while open is ignored, and the open bridge physically carries
//!   a ramp ball onto the upper deck (layer 1) while the closed one returns
//!   it to the main deck (layer 0).
//! * Night Overdrive: armed by the Newton ball and the three drop targets,
//!   started by the next VUK entry or an `action` press; `mode:night_overdrive`
//!   increments, the GI is a UV tint while it runs and white after, the
//!   Glo-Ball reaches the upper deck (layer 1), the ball count survives the
//!   two balls draining in either order, once per ball; no script fault and
//!   no queue overflow. The VUK path is swept over seeds.

mod sky_deck_support;

use pin_sim::Simulation;
use sky_deck_support::{
    VUK_HOLD_TICKS, WHITE, balls_of, drain, healthy, hit, is_uv, key, layers, load, load_seeded,
    on_big_stack, only_ball, press, put_in_vuk, start_game,
};

/// The Glo-Ball is on the upper deck (layer 1) within this many ticks of
/// the Night Overdrive start, by either start path (the VUK holds a ball
/// only 0.5 s until the Glo-Ball is lifted).
const GLO_LIFT_TICKS: u64 = 3000;

/// Arms Night Overdrive: the Newton ball and the three drop targets.
fn arm(s: &mut Simulation) {
    assert_eq!(s.persist(key::READY), 0, "not armed yet");
    hit(s, "newton_sw");
    hit(s, "drop_1_sw");
    hit(s, "drop_2_sw");
    assert_eq!(s.persist(key::READY), 0, "two drops do not arm it");
    hit(s, "drop_3_sw");
    assert_eq!(s.persist(key::READY), 1, "armed");
}

/// Runs until a ball on layer 1 exists (at most `limit` ticks); returns the
/// ticks it took.
fn until_layer_1(s: &mut Simulation, limit: u64) -> Option<u64> {
    for t in 0..limit {
        if layers(s).contains(&1) {
            return Some(t);
        }
        s.advance(1);
    }
    None
}

/// Starts Night Overdrive through the VUK with the served ball; returns
/// once the Glo-Ball is on the upper deck.
fn start_overdrive_by_vuk(s: &mut Simulation) {
    let mode = s.persist(key::MODE);
    let steel = only_ball(s);
    put_in_vuk(s, steel);
    s.advance(5);
    assert_eq!(
        s.switch_closed("vuk_sw"),
        Some(true),
        "steel ball in the scoop"
    );
    assert_eq!(s.persist(key::MODE), mode + 1, "mode started by the VUK");
    assert_eq!(s.persist(key::READY), 0, "no longer armed");
    assert!(is_uv(s.snapshot().gi), "UV tint: {:?}", s.snapshot().gi);
    // The steel ball is lifted first, then the Glo-Ball is dispensed into the
    // pocket and lifted after it.
    let mut glo_on_deck = false;
    for _ in 0..GLO_LIFT_TICKS {
        s.advance(1);
        let glo = balls_of(s, "uv_globall");
        if let Some(&g) = glo.first()
            && s.ball(g).is_some_and(|b| b.layer == 1)
        {
            glo_on_deck = true;
            break;
        }
    }
    assert!(glo_on_deck, "the Glo-Ball reached the upper deck");
    assert!(is_uv(s.snapshot().gi), "still UV: {:?}", s.snapshot().gi);
    assert_eq!(s.balls_in_play(), 2);
}

#[test]
fn full_three_ball_game_by_injection() {
    on_big_stack(|| {
        let mut s = load();
        s.advance(10);
        assert_eq!(s.persist(key::STATE), 0, "attract");
        start_game(&mut s);
        for ball in 1..=3 {
            assert_eq!(s.persist(key::BALL), ball);
            hit(&mut s, "sling_left_sw");
            hit(&mut s, "loop_entry");
            hit(&mut s, "loop_top");
            let h = only_ball(&s);
            drain(&mut s, h);
            s.advance(1500);
            healthy(&s);
        }
        assert_eq!(s.persist(key::STATE), 2, "game over");
        assert_eq!(s.persist(key::BALL), 0);
        assert_eq!(s.balls_in_play(), 0);
        let score = s.score(0).expect("score reported");
        // sling 1000, loop entry 1000, loop top 1000 + loop shot bonus 50000.
        assert_eq!(score, 3 * (1000 + 1000 + 1000 + 50_000));
        assert_eq!(s.persist(key::HIGH), score);
        assert_eq!(s.persist(key::GAMES), 1);
        assert_eq!(s.snapshot().gi, WHITE);
        s.advance(4000);
        assert_eq!(s.persist(key::STATE), 0, "attract again");
        // A second, lower game keeps the high score.
        start_game(&mut s);
        for _ in 0..3 {
            let h = only_ball(&s);
            drain(&mut s, h);
            s.advance(1500);
        }
        assert_eq!(s.persist(key::GAMES), 2);
        assert_eq!(s.persist(key::HIGH), score);
        healthy(&s);
    });
}

#[test]
fn sky_bridge_opens_for_exactly_8000_ticks_and_ignores_presses() {
    on_big_stack(|| {
        let mut s = load();
        s.advance(10);
        // Not in attract mode.
        hit(&mut s, "action");
        assert_eq!(s.persist(key::BRIDGE), 0);
        start_game(&mut s);
        s.set_switch("action", true).unwrap();
        s.advance(1);
        s.release_switch("action").unwrap();
        let mut open = 0u64;
        while s.persist(key::BRIDGE) == 1 {
            open += 1;
            if open == 3000 {
                // A press while open is ignored (no extension).
                s.set_switch("action", true).unwrap();
                s.advance(1);
                s.release_switch("action").unwrap();
                open += 1;
                assert_eq!(s.persist(key::BRIDGE), 1);
            }
            s.advance(1);
            assert!(open < 20_000, "bridge never closed");
        }
        assert_eq!(open, 8000, "bridge open ticks");
        // It opens again after closing.
        hit(&mut s, "action");
        assert_eq!(s.persist(key::BRIDGE), 1);
        s.advance(8000);
        assert_eq!(s.persist(key::BRIDGE), 0);
        healthy(&s);
    });
}

/// Shoots the served ball up the left ramp's entry lane at `speed` m/s;
/// returns the layers it visited (bit per layer) before it drained or the
/// run ended, and whether it ended on layer 1 at some point after the
/// landing.
fn ramp_shot(s: &mut Simulation, speed: f32) -> u8 {
    let h = only_ball(s);
    let mut b = s.ball(h).unwrap();
    b.position_m = sky_deck_support::mm(175.0, 480.0);
    b.velocity_m_s = [0.0, speed, 0.0];
    b.angular_velocity_rad_s = [-speed / 0.0135, 0.0, 0.0];
    b.layer = 0;
    assert!(s.set_ball_state(h, &b));
    let mut seen = 0u8;
    for _ in 0..3000 {
        s.advance(1);
        match s.ball(h) {
            Some(b) => seen |= 1 << b.layer,
            None => break,
        }
    }
    seen
}

#[test]
fn sky_bridge_routes_ramp_balls_by_diverter_position() {
    on_big_stack(|| {
        for speed in [2.2f32, 2.6, 3.0] {
            // Closed: up the ramp (layer 2), back down to the main deck,
            // never on the upper deck.
            let mut s = load();
            s.advance(10);
            start_game(&mut s);
            let made = s.switch_hits("ramp_made").unwrap();
            let seen = ramp_shot(&mut s, speed);
            assert_eq!(seen & 0b110, 0b100, "closed, {speed} m/s: layers {seen:b}");
            assert!(s.switch_hits("ramp_made").unwrap() > made, "ramp made");
            assert_eq!(layers(&s), [0], "back on the main deck");
            // Open: over the Sky Bridge onto the upper deck.
            let mut s = load();
            s.advance(10);
            start_game(&mut s);
            hit(&mut s, "action");
            s.advance(100);
            assert_eq!(s.persist(key::BRIDGE), 1);
            let made = s.switch_hits("ramp_made").unwrap();
            let seen = ramp_shot(&mut s, speed);
            assert_eq!(seen & 0b110, 0b110, "open, {speed} m/s: layers {seen:b}");
            assert!(s.switch_hits("ramp_made").unwrap() > made, "ramp made");
            healthy(&s);
        }
    });
}

#[test]
fn newton_and_drops_arm_night_overdrive_which_starts_at_the_vuk() {
    on_big_stack(|| {
        let mut s = load();
        s.advance(10);
        start_game(&mut s);
        // The drop bank alone does not arm it; the bank completed earlier
        // in the ball still counts once the Newton ball is struck (both in
        // the current ball, either order). `arm` (ball 2 below) checks the
        // other order and that the Newton ball and two drops do not.
        hit(&mut s, "drop_1_sw");
        hit(&mut s, "drop_2_sw");
        hit(&mut s, "drop_3_sw");
        assert_eq!(s.persist(key::READY), 0);
        s.advance(3000); // the bank resets
        assert_eq!(s.persist(key::READY), 0);
        hit(&mut s, "newton_sw");
        assert_eq!(s.persist(key::READY), 1, "armed: bank then Newton");
        assert_eq!(s.snapshot().gi, WHITE);
        start_overdrive_by_vuk(&mut s);
        // Upper-deck scoring doubles while the mode runs (both balls have
        // left deck_opto's zone by now).
        s.advance(1500);
        let before = s.score(0).unwrap();
        hit(&mut s, "deck_opto");
        s.advance(2);
        assert_eq!(s.score(0).unwrap() - before, 200);
        // The Glo-Ball drains first: the mode ends, the ball goes on.
        let glo = balls_of(&s, "uv_globall")[0];
        drain(&mut s, glo);
        s.advance(5);
        assert_eq!(s.snapshot().gi, WHITE, "GI restored at mode end");
        assert_eq!(s.persist(key::BALL), 1, "still ball 1");
        assert_eq!(balls_of(&s, "standard_steel").len(), 1);
        let before = s.score(0).unwrap();
        hit(&mut s, "deck_opto");
        s.advance(2);
        assert_eq!(s.score(0).unwrap() - before, 100, "single points again");
        // Once per ball: arming again does nothing, the VUK starts nothing.
        // (A few ticks of free play only: the steel ball cannot drain on
        // its own meanwhile.)
        hit(&mut s, "newton_sw");
        hit(&mut s, "drop_1_sw");
        hit(&mut s, "drop_2_sw");
        hit(&mut s, "drop_3_sw");
        assert_eq!(s.persist(key::READY), 0, "once per ball");
        let steel = only_ball(&s);
        put_in_vuk(&mut s, steel);
        s.advance(10);
        assert_eq!(s.persist(key::MODE), 1, "no second start in the ball");
        // Let the VUK lift it (a held ball cannot be moved to the drain),
        // then drain it at once.
        s.advance(VUK_HOLD_TICKS + 50);
        assert_eq!(s.ball(steel).map(|b| b.layer), Some(1), "lifted");
        drain(&mut s, steel);
        s.advance(1500);
        assert_eq!(s.persist(key::BALL), 2, "next ball");
        // A new ball: it can be armed and started again.
        arm(&mut s);
        start_overdrive_by_vuk(&mut s);
        assert_eq!(s.persist(key::MODE), 2);
        healthy(&s);
    });
}

#[test]
fn steel_ball_draining_first_keeps_the_ball_alive() {
    on_big_stack(|| {
        let mut s = load();
        s.advance(10);
        start_game(&mut s);
        arm(&mut s);
        start_overdrive_by_vuk(&mut s);
        let steel = balls_of(&s, "standard_steel")[0];
        drain(&mut s, steel);
        s.advance(5);
        assert_eq!(s.snapshot().gi, WHITE, "mode ends at the first drain");
        assert_eq!(s.persist(key::BALL), 1, "the Glo-Ball keeps the ball alive");
        assert_eq!(s.balls_in_play(), 1);
        let glo = balls_of(&s, "uv_globall")[0];
        drain(&mut s, glo);
        s.advance(1500);
        assert_eq!(s.persist(key::BALL), 2, "one ball of the game used");
        assert_eq!(balls_of(&s, "standard_steel").len(), 1, "ball 2 served");
        assert_eq!(s.balls_in_play(), 1);
        // Both balls in the same tick.
        arm(&mut s);
        start_overdrive_by_vuk(&mut s);
        for h in s.ball_handles() {
            drain(&mut s, h);
        }
        s.advance(1500);
        assert_eq!(s.persist(key::BALL), 3);
        assert_eq!(s.balls_in_play(), 1);
        assert_eq!(s.snapshot().gi, WHITE);
        // Game over in the mode: GI white, game over state.
        arm(&mut s);
        start_overdrive_by_vuk(&mut s);
        let glo = balls_of(&s, "uv_globall")[0];
        drain(&mut s, glo);
        s.advance(2);
        let steel = only_ball(&s);
        drain(&mut s, steel);
        s.advance(10);
        assert_eq!(s.persist(key::STATE), 2, "game over");
        assert_eq!(s.balls_in_play(), 0);
        assert_eq!(s.snapshot().gi, WHITE);
        healthy(&s);
    });
}

#[test]
fn action_starts_night_overdrive_when_armed_instead_of_the_bridge() {
    on_big_stack(|| {
        let mut s = load();
        s.advance(10);
        start_game(&mut s);
        arm(&mut s);
        hit(&mut s, "action");
        assert_eq!(s.persist(key::MODE), 1, "action started the mode");
        assert_eq!(s.persist(key::BRIDGE), 0, "not the bridge");
        assert!(is_uv(s.snapshot().gi));
        // The Glo-Ball rolls into the VUK and is lifted onto the deck while
        // the steel ball stays on the plunger below.
        let limit = GLO_LIFT_TICKS;
        let t = until_layer_1(&mut s, limit).expect("a layer-1 ball");
        assert!(t < limit);
        assert_eq!(balls_of(&s, "uv_globall").len(), 1);
        assert_eq!(balls_of(&s, "standard_steel").len(), 1);
        let steel = balls_of(&s, "standard_steel")[0];
        assert_eq!(s.ball(steel).unwrap().layer, 0, "steel ball below");
        // While the mode runs, action does nothing.
        hit(&mut s, "action");
        assert_eq!(s.persist(key::BRIDGE), 0);
        assert_eq!(s.persist(key::MODE), 1);
        healthy(&s);
    });
}

#[test]
fn night_overdrive_glo_ball_reaches_the_deck_over_a_seed_sweep() {
    on_big_stack(|| {
        for seed in 1..=8u64 {
            let mut s = load_seeded(seed);
            s.advance(10 + seed);
            start_game(&mut s);
            arm(&mut s);
            if seed % 2 == 0 {
                start_overdrive_by_vuk(&mut s);
            } else {
                hit(&mut s, "action");
                assert_eq!(s.persist(key::MODE), 1, "seed {seed}");
                let t = until_layer_1(&mut s, GLO_LIFT_TICKS);
                assert!(t.is_some(), "seed {seed}: no layer-1 ball");
                let glo = balls_of(&s, "uv_globall")[0];
                assert_eq!(s.ball(glo).unwrap().layer, 1, "seed {seed}");
            }
            // The mode ends and the GI returns to white at the end of the ball.
            for h in s.ball_handles() {
                drain(&mut s, h);
            }
            s.advance(1500);
            assert_eq!(s.snapshot().gi, WHITE, "seed {seed}");
            assert_eq!(s.persist(key::BALL), 2, "seed {seed}");
            healthy(&s);
        }
    });
}

#[test]
fn the_game_never_stalls_on_a_lost_ball() {
    on_big_stack(|| {
        let mut s = load();
        s.advance(10);
        start_game(&mut s);
        press(&mut s, "launch", 400);
        s.advance(2000);
        // A ball removed without draining (test API): the rules notice the
        // empty table and serve the next ball instead of stalling.
        let h = only_ball(&s);
        assert!(s.remove_ball(h));
        s.advance(1000);
        assert_eq!(s.persist(key::BALL), 2, "next ball served");
        assert_eq!(s.balls_in_play(), 1);
        healthy(&s);
    });
}
