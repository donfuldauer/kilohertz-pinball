//! Subphase 2.5 spec tests (Independent Tester): the script contract as the
//! simulation must honour it: engine limits (100 000 instructions and 2 MiB
//! per callback, not the 1 M / 16 MiB tool defaults), a faulted script never
//! stalls or stops the tick, queue overflow drops commands.

#[path = "fixtures/spec_2_5/support.rs"]
mod support;

use pin_core::input::CabinetInput;
use pin_core::snapshot::FrameStateSnapshot;
use support::*;

/// `action` burns ~150 000 loop iterations (≥ 300 000 VM instructions: over
/// the engine budget, under the 1 M default); `launch` burns 15 000
/// (≤ 60 000: under the engine budget). Player 1 = on_tick count.
const BUDGET_RULES: &str = r#"
local ticks = 0
local burn = 0
local sw_action, sw_launch = -1, -1
function on_init()
  sw_action = pinball.sw.action
  sw_launch = pinball.sw.launch
end
function on_switch(id, closed)
  if closed and id == sw_action then burn = 150000 end
  if closed and id == sw_launch then burn = 15000 end
end
function on_tick()
  ticks = ticks + 1
  pinball.set_score(1, ticks)
  local x = 0
  for i = 1, burn do x = x + i end
  burn = 0
  pinball.set_score(2, x)
end
"#;

/// Keeps one more 64 KiB string alive every tick (a cheap callback: the
/// probe is the heap cap, not the instruction budget). The 2 MiB engine heap
/// is full after about 30 ticks; the 16 MiB default would last about 250.
const MEMORY_RULES: &str = r#"
local n = 0
local held = {}
function on_tick()
  n = n + 1
  held[n] = string.rep("x", 65536)
  pinball.set_score(1, n)
end
"#;

fn press(sim: &mut pin_sim::Simulation, name: &str) {
    sim.set_switch(name, true).unwrap();
    sim.advance(2);
    sim.set_switch(name, false).unwrap();
    sim.advance(2);
}

#[test]
fn instruction_budget_is_the_engine_budget() {
    on_sim_thread(|| {
        let dir = bundle_with_script("budget", BUDGET_RULES);
        let mut sim = load_at(&dir, DEFAULT_SEED);
        sim.advance(10);
        assert_eq!(sim.score(0), Some(10));

        // Under the budget: the script keeps running.
        press(&mut sim, "launch");
        assert_eq!(sim.score(0), Some(14));
        assert_eq!(sim.score(1), Some(0), "the loop result of the last tick");
        sim.advance(50);
        assert_eq!(
            sim.score(0),
            Some(64),
            "15 000 iterations fit the engine budget"
        );

        // Over the engine budget (but under the 1 M default): the script is
        // aborted and stays in its fault state; its callbacks stop.
        press(&mut sim, "action");
        let frozen = sim.score(0).unwrap();
        assert!(
            (64..=66).contains(&frozen),
            "on_tick must have been aborted on the burning tick: score {frozen}"
        );
        sim.advance(500);
        assert_eq!(
            sim.score(0),
            Some(frozen),
            "150 000 loop iterations in one callback exceed 100 000 instructions: with \
             LuaConfig::engine() the script faults and is no longer called \
             (still running => the 1 M default budget is in use)"
        );
        assert_eq!(sim.tick(), 10 + 4 + 50 + 4 + 500, "the tick never stalls");
    });
}

#[test]
fn memory_cap_is_the_engine_cap() {
    on_sim_thread(|| {
        let dir = bundle_with_script("memory", MEMORY_RULES);
        let mut sim = load_at(&dir, DEFAULT_SEED);
        sim.advance(5);
        assert_eq!(sim.score(0), Some(5), "the first 64 KiB strings fit");
        sim.advance(595);
        let frozen = sim.score(0).unwrap();
        assert!(
            (8..=40).contains(&frozen),
            "holding 64 KiB more per tick must hit the 2 MiB engine heap cap after about \
             30 ticks (the 16 MiB default lasts about 250): the script ran {frozen} ticks"
        );
        assert_eq!(sim.tick(), 600, "the tick never stalls");
        sim.advance(100);
        assert_eq!(sim.score(0), Some(frozen));
    });
}

/// A faulted script (runaway loop, runtime error) never stalls the tick:
/// time, physics, switches, flippers and snapshots carry on.
#[test]
fn faulted_script_does_not_stop_the_simulation() {
    on_sim_thread(|| {
        for (tag, fault) in [
            ("loop", "while true do end"),
            ("error", "error('boom')"),
            ("badarg", "pinball.set_score(99, 1)"),
            ("nilcall", "local f = nil; f()"),
        ] {
            let lua = format!(
                "local ticks = 0\nlocal sw = -1\n\
                 function on_init() sw = pinball.sw.action end\n\
                 function on_switch(id, closed)\n  if closed and id == sw then {fault} end\nend\n\
                 function on_tick()\n  ticks = ticks + 1\n  pinball.set_score(1, ticks)\nend\n"
            );
            let dir = bundle_with_script(tag, &lua);
            let run = || {
                let mut sim = load_at(&dir, DEFAULT_SEED);
                let h = sim
                    .spawn_ball("standard_steel", [0.09, 0.50, R], [0.0, -0.3, 0.0])
                    .unwrap();
                sim.advance(20);
                assert_eq!(sim.score(0), Some(20), "{tag}");
                sim.set_switch("action", true).unwrap();
                sim.advance(1); // the edge is delivered and the script faults
                assert_eq!(sim.tick(), 21, "{tag}");
                let frozen = sim.score(0).unwrap();
                assert!(frozen == 20 || frozen == 21, "{tag}: {frozen}");
                let y0 = sim.ball(h).unwrap().position_m[1];

                // Everything but the script carries on.
                let mut out = Box::new(FrameStateSnapshot::default());
                let rest = sim.snapshot().flippers()[0].angle_rad;
                let t = sim.tick() + 1;
                step(
                    &mut sim,
                    &[button(t, CabinetInput::LeftFlipper, true)],
                    &mut out,
                );
                for _ in 0..600 {
                    step(&mut sim, &[], &mut out);
                }
                assert_eq!(sim.tick(), 622, "{tag}");
                assert_eq!(out.tick, 622, "{tag}");
                assert_eq!(
                    sim.score(0),
                    Some(frozen),
                    "{tag}: a faulted script is not called"
                );
                let b = sim.ball(h).expect("ball still simulated");
                assert!(b.position_m[1] < y0 - 0.05, "{tag}: physics stopped");
                assert_eq!(
                    sim.switch_hits("side_opto"),
                    Some(1),
                    "{tag}: switch edges are still collected"
                );
                let moved = out
                    .flippers()
                    .iter()
                    .any(|f| (f.angle_rad - rest).abs() > 0.5);
                assert!(
                    moved,
                    "{tag}: flipper buttons still work after a script fault"
                );
                sim.state_hash()
            };
            assert_eq!(run(), run(), "{tag}: a faulting run is still deterministic");
        }
    });
}

/// A script whose first on_tick already faults still loads and simulates.
#[test]
fn script_faulting_on_every_first_tick_still_simulates() {
    on_sim_thread(|| {
        let dir = bundle_with_script("tickfault", "function on_tick() while true do end end\n");
        let mut sim = load_at(&dir, DEFAULT_SEED);
        let h = sim
            .spawn_ball("standard_steel", [0.06, 0.95, R], [0.0; 3])
            .unwrap();
        sim.advance(2000);
        assert_eq!(sim.tick(), 2000);
        assert!(sim.ball(h).unwrap().position_m[1] < 0.9);
    });
}

/// Queue overflow drops the overflowing commands (the first four score
/// reports of a tick are kept) and does not fault the script.
#[test]
fn queue_overflow_drops_commands_without_faulting() {
    on_sim_thread(|| {
        let lua = "local ticks = 0\n\
                   function on_tick()\n  ticks = ticks + 1\n\
                   for i = 1, 6 do pinball.set_score(1, ticks * 10 + i) end\n\
                   for i = 1, 40 do pinball.pulse_coil(pinball.coil.bumper_1_coil, 5) end\n\
                   for i = 1, 300 do pinball.set_lamp(pinball.lamp.lamp_a, 1) end\n\
                   end\n";
        let dir = bundle_with_script("overflow", lua);
        let mut sim = load_at(&dir, DEFAULT_SEED);
        sim.advance(1);
        assert_eq!(
            sim.score(0),
            Some(14),
            "scores holds 4 entries per tick: reports 5 and 6 are dropped"
        );
        sim.advance(99);
        assert_eq!(sim.score(0), Some(1004), "overflow is not a fault");
        assert_eq!(sim.tick(), 100);
    });
}

/// The queue is cleared after it is applied: a one-shot command acts once.
#[test]
fn command_queue_is_cleared_every_tick() {
    on_sim_thread(|| {
        // Spawn exactly once, on the first tick.
        let lua = "local done = false\n\
                   function on_tick()\n\
                   if not done then\n\
                   pinball.spawn_ball(pinball.dispenser.feed, pinball.profile.standard_steel)\n\
                   done = true\n end\n end\n";
        let dir = bundle_with_script("oneshot", lua);
        let mut sim = load_at(&dir, DEFAULT_SEED);
        sim.advance(1);
        assert_eq!(sim.balls_in_play(), 1);
        sim.advance(200);
        assert_eq!(
            sim.balls_in_play(),
            1,
            "a spawn queued once must not be re-applied on later ticks"
        );
    });
}
