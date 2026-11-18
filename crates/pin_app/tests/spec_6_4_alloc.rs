//! Subphase 6.4 spec test (Independent Tester), brief S5: the real Feature
//! Lab simulation, with a game started and balls in play (the served ball
//! plus a Glo-Ball, an XL ball and a ball fed into the backbox VUK), makes no
//! heap allocation in `step_1ms` over 20 000 ticks after a 2 000-tick
//! warm-up (PLAN §2.3 rule 4, D5). Allocations of the Lua VM inside its
//! exempt arena scope are counted apart by `pin_sim::alloc_guard`.

#[path = "fixtures/spec_6_4/common.rs"]
mod common;

use common::*;
use pin_core::input::{CabinetInput, InputEvent};
use pin_core::sim::{SimStep, TICK_NS};
use pin_core::snapshot::FrameStateSnapshot;
use pin_sim::alloc_guard::{CountingAlloc, count_allocs_split, is_installed};

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

const WARM_UP: u64 = 2_000;
const MEASURED: u64 = 20_000;

/// Cabinet inputs of tick `t` (start, launch, flippers, upper flipper,
/// action), written into `buf` (no allocation: capacity reserved outside).
fn inputs(t: u64, buf: &mut Vec<InputEvent>) {
    buf.clear();
    let ts = t * TICK_NS - TICK_NS / 2;
    let mut push = |i: CabinetInput, p: bool| buf.push(InputEvent::button(ts, i, p));
    match t {
        50 => push(CabinetInput::Start, true),
        120 => push(CabinetInput::Start, false),
        1_500 => push(CabinetInput::Launch, true),
        1_600 => push(CabinetInput::Launch, false),
        _ => {}
    }
    if t >= 1_700 {
        let phase = t % 1_000;
        match phase {
            0 => push(CabinetInput::LeftFlipper, true),
            90 => push(CabinetInput::LeftFlipper, false),
            250 => push(CabinetInput::RightFlipper, true),
            340 => push(CabinetInput::RightFlipper, false),
            500 => push(CabinetInput::LeftUpper, true),
            580 => push(CabinetInput::LeftUpper, false),
            _ => {}
        }
        if t % 4_000 == 700 {
            push(CabinetInput::Action, true);
        }
        if t % 4_000 == 760 {
            push(CabinetInput::Action, false);
        }
    }
}

#[test]
fn feature_lab_ticks_do_not_allocate() {
    assert!(is_installed(), "CountingAlloc is the global allocator");
    big_stack(|| {
        let mut sim = load();
        let mut out = FrameStateSnapshot::default();
        let mut buf: Vec<InputEvent> = Vec::with_capacity(16);
        let mut step = |sim: &mut pin_sim::Simulation, buf: &mut Vec<InputEvent>| {
            let t = sim.tick() + 1;
            inputs(t, buf);
            count_allocs_split(|| sim.step_1ms(t, buf, &mut out))
        };
        // Start the game and launch (ticks 1..1 700).
        while sim.tick() < 1_700 {
            step(&mut sim, &mut buf);
        }
        assert_eq!(pv(&sim, "game_state"), 1, "a game is running");
        // More balls in play (spawned outside the measured ticks).
        let glo = table()
            .dispensers
            .iter()
            .find(|d| d.name == "glo_feed")
            .expect("glo_feed");
        sim.spawn_ball(GLO, glo.exit_position_m, glo.exit_velocity_m_s)
            .expect("spawn a Glo-Ball");
        let xl = table()
            .dispensers
            .iter()
            .find(|d| d.name == "xl_feed")
            .expect("xl_feed");
        sim.spawn_ball(XL, xl.exit_position_m, [0.0, 0.5, 0.0])
            .expect("spawn an XL ball");
        let vuk = table()
            .mechs
            .iter()
            .find(|m| m.name == BACKBOX_VUK)
            .and_then(|m| m.position_m)
            .expect("backbox VUK position");
        sim.spawn_ball(STEEL, [vuk[0], vuk[1], vuk[2] + radius_m(STEEL)], [0.0; 3])
            .expect("spawn into the backbox VUK");
        // Warm-up.
        for _ in 0..WARM_UP {
            step(&mut sim, &mut buf);
        }
        assert!(sim.balls_in_play() > 0, "balls in play when measuring");
        let mut worst = (0u64, 0u64);
        let mut with_balls = 0u64;
        let mut exempt = 0u64;
        for _ in 0..MEASURED {
            let ((), n, e) = step(&mut sim, &mut buf);
            if n > worst.0 {
                worst = (n, sim.tick());
            }
            exempt += e;
            if sim.balls_in_play() > 0 {
                with_balls += 1;
            }
        }
        assert_eq!(
            worst.0, 0,
            "a warmed-up tick allocated {} time(s) (tick {})",
            worst.0, worst.1
        );
        assert!(
            with_balls >= MEASURED / 2,
            "balls were in play for {with_balls} of {MEASURED} measured ticks"
        );
        eprintln!("exempt Lua-arena allocations over {MEASURED} ticks: {exempt}");
        assert_healthy(&sim, "after the guarded run");
    });
}
