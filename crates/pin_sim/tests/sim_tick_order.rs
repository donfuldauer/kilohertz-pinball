//! Subphase 2.5 deliverable 1: the PLAN.md §2.2 tick order, observed from a
//! rules script.

mod sim_support;

use pin_core::ScriptError;
use pin_core::input::{CabinetInput, InputEvent};
use pin_core::sim::{SimStep, TICK_NS};
use pin_core::snapshot::FrameStateSnapshot;
use pin_sim::{SimConfig, Simulation};
use sim_support::*;

const RULES: &str = r#"
function on_switch(id, closed)
  if closed and id == pinball.sw.center_rollover then pinball.set_score(1, pinball.tick()) end
  if closed and id == pinball.sw.start then pinball.set_score(2, pinball.tick()) end
  if id == pinball.sw.left_eos then pinball.set_score(3, pinball.tick()) end
end
function on_tick()
  if pinball.tick() == 5 then pinball.pulse_coil(pinball.coil.left_flipper_coil, 40) end
  if pinball.tick() == 60 then pinball.set_lamp(pinball.lamp.blink_lamp, 1) end
  if pinball.tick() == 80 then error("rules fault on purpose") end
end
"#;

fn step(sim: &mut Simulation, inputs: &[InputEvent]) -> FrameStateSnapshot {
    let mut out = FrameStateSnapshot::default();
    let t = sim.tick() + 1;
    sim.step_1ms(t, inputs, &mut out);
    out
}

#[test]
fn tick_order_as_seen_by_the_script() {
    let bundle = with_rules("tick_order", RULES);
    on_sim_thread(move || {
        let mut sim = Simulation::load(&bundle, SimConfig::default()).unwrap();
        assert_eq!(sim.balls_in_play(), 0);
        let ball = sim
            .spawn_ball("standard_steel", [0.257, 0.446, 0.0135], [0.0, -0.5, 0.0])
            .unwrap();

        // Step 1 → 2: a button event inside tick 3 is a cabinet edge the
        // script sees in the same tick.
        step(&mut sim, &[]);
        step(&mut sim, &[]);
        let press = InputEvent::button(2 * TICK_NS + 400_000, CabinetInput::Start, true);
        step(&mut sim, &[press]);
        assert_eq!(sim.tick(), 3);
        assert_eq!(sim.score(1), Some(3));
        assert_eq!(sim.switch_closed("start"), Some(true));

        // Steps 2 → 3 → 4: a coil pulsed in on_tick of tick 5 moves the bat
        // in tick 5's physics.
        step(&mut sim, &[]);
        assert_eq!(sim.flipper("lower_left").unwrap().0, 0.0);
        step(&mut sim, &[]);
        assert_eq!(sim.tick(), 5);
        let (angle, omega) = sim.flipper("lower_left").unwrap();
        assert!(angle > 0.0 && omega > 0.0, "{angle} {omega}");
        assert!(
            sim.haptic_events()
                .iter()
                .any(|h| h.tick == 5 && h.source == 1),
            "{:?}",
            sim.haptic_events()
        );

        // Step 5 → next tick's step 2: a physics edge collected in tick N is
        // delivered to on_switch_edge in tick N + 1.
        let mut closed_at = None;
        for _ in 0..40 {
            step(&mut sim, &[]);
            if sim.switch_closed("center_rollover") == Some(true) {
                closed_at = Some(sim.tick());
                break;
            }
        }
        let n = closed_at.expect("the ball reached the rollover");
        assert!(sim.ball(ball).is_some());
        assert_eq!(sim.score(0), None, "not delivered in the tick it happened");
        step(&mut sim, &[]);
        assert_eq!(sim.score(0), Some(n as i64 + 1));
        assert_eq!(sim.switch_hits("center_rollover"), Some(1));
        // Step 6: lamps reach the snapshot in the tick they are set.
        while sim.tick() < 59 {
            step(&mut sim, &[]);
        }
        assert_eq!(sim.snapshot().lamps[1], 0.0);
        let out = step(&mut sim, &[]);
        assert_eq!(out.lamps[1], 1.0);
        assert_eq!(out.tick, 60);
        // The EOS edge of the raised bat reached the script after tick 5.
        let eos = sim.score(2).expect("EOS edge delivered");
        assert!(eos > 5, "{eos}");

        // A script fault is state, not a stall: ticks go on.
        while sim.tick() < 79 {
            step(&mut sim, &[]);
        }
        assert!(!sim.script_faulted());
        step(&mut sim, &[]);
        assert!(sim.script_faulted());
        assert_eq!(sim.script_fault_count(), 1);
        assert!(matches!(sim.script_error(), Some(ScriptError::Runtime(_))));
        sim.advance(100);
        assert_eq!(sim.tick(), 180);
        assert!(sim.script_faulted());
    });
}

#[test]
fn input_events_after_the_tick_wait_for_their_tick() {
    let bundle = with_rules("late_input", RULES);
    on_sim_thread(move || {
        let mut sim = Simulation::load(&bundle, SimConfig::default()).unwrap();
        // Timestamped in tick 4 but handed over with tick 2.
        let press = InputEvent::button(3 * TICK_NS + 1, CabinetInput::Start, true);
        step(&mut sim, &[]);
        step(&mut sim, &[press]);
        assert_eq!(sim.switch_closed("start"), Some(false));
        step(&mut sim, &[]);
        assert_eq!(sim.score(1), None);
        step(&mut sim, &[]);
        assert_eq!(sim.score(1), Some(4));
        assert_eq!(sim.counters().inputs_dropped, 0);
    });
}

/// A flipper pulse issued in `on_init` is energised for its full length from
/// the first physics tick (tick 1), like the same pulse issued in tick 1's
/// `on_tick`.
#[test]
fn on_init_pulse_keeps_its_full_length() {
    const AT_INIT: &str = r#"
function on_init() pinball.pulse_coil(pinball.coil.left_flipper_coil, 4) end
function on_tick() end
function on_switch(id, closed) end
"#;
    const AT_TICK_1: &str = r#"
local fired = false
function on_init() end
function on_tick()
  if not fired then
    fired = true
    pinball.pulse_coil(pinball.coil.left_flipper_coil, 4)
  end
end
function on_switch(id, closed) end
"#;
    const ONE_MS: &str = r#"
function on_init() pinball.pulse_coil(pinball.coil.left_flipper_coil, 1) end
function on_tick() end
function on_switch(id, closed) end
"#;
    let at_init = with_rules("init_pulse", AT_INIT);
    let at_tick_1 = with_rules("tick1_pulse", AT_TICK_1);
    let one_ms = with_rules("init_pulse_1ms", ONE_MS);
    on_sim_thread(move || {
        let run = |bundle: &std::path::Path, ticks: u64| {
            let mut sim = Simulation::load(bundle, SimConfig::default()).unwrap();
            assert!(!sim.script_faulted());
            let mut trace = Vec::new();
            for _ in 0..ticks {
                sim.advance(1);
                let (a, w) = sim.flipper("lower_left").unwrap();
                trace.push((a.to_bits(), w.to_bits()));
            }
            assert!(!sim.script_faulted());
            trace
        };
        let a = run(&at_init, 30);
        let b = run(&at_tick_1, 30);
        assert_eq!(a, b, "on_init pulse differs from the same pulse at tick 1");
        // A 1 ms on_init pulse still drives one physics tick.
        let one = run(&one_ms, 1);
        assert!(
            f32::from_bits(one[0].1) != 0.0,
            "a 1 ms on_init pulse had no physics effect"
        );
    });
}
