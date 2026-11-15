//! Subphase 2.9 gate step 6: the bot drives the simulation from outside the
//! tick path, so the ticks of a bot-played game still allocate nothing (the
//! Lua VM's arena aside, PLAN.md §2.3 rule 4) and the tick itself is the
//! same code the replay hashes cover.

#[path = "fixtures/autoplay/support.rs"]
mod support;

use pin_sim::alloc_guard::{CountingAlloc, count_allocs_split, is_installed};
use pin_sim::autoplay::{Bot, GameMonitor, SkillLevel, TableInfo};
use pin_sim::{SimConfig, Simulation};
use support::*;

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

#[test]
fn bot_driven_ticks_allocate_nothing() {
    assert!(is_installed());
    let bundle = fixture("clean");
    let (allocs, flips) = on_sim_thread(move || {
        let mut sim = Simulation::load(&bundle, SimConfig::default()).unwrap();
        let mut bot = Bot::new(11, SkillLevel::Expert);
        // Warm up (the bot builds its flipper models on its first tick).
        for _ in 0..2_000 {
            bot.tick(&mut sim);
            sim.advance(1);
        }
        let mut total = 0;
        for _ in 0..30_000 {
            bot.tick(&mut sim);
            let ((), n, _exempt) = count_allocs_split(|| sim.advance(1));
            assert_eq!(n, 0, "tick {}: {n} allocation(s)", sim.tick());
            total += n;
        }
        assert!(!sim.script_faulted());
        (total, bot.stats().flips_on_prediction)
    });
    assert_eq!(allocs, 0);
    // The guarded ticks did see the bot's button presses.
    assert!(flips > 0);
}

/// Work item A1-endless-detector requirement 5: the monitor's per-tick
/// path (stuck, displacement and endless detectors, switch-hit crediting)
/// allocates nothing on a tick that records no finding and no ball leaving
/// the table; its per-ball state is fixed-size.
#[test]
fn monitor_observe_allocates_nothing_in_steady_state() {
    assert!(is_installed());
    let bundle = fixture("clean");
    let (checked, credited) = on_sim_thread(move || {
        let mut sim = Simulation::load(&bundle, SimConfig::default()).unwrap();
        let info = TableInfo::load(&bundle).unwrap();
        let mut bot = Bot::new(11, SkillLevel::Expert);
        // The age check runs on every tick; the limit lies beyond the run.
        let mut mon = GameMonitor::new(&info, &sim, 0, 10_000).with_endless_ticks(1_000_000);
        mon.observe(&sim);
        let mut checked = 0u64;
        let mut credited = 0u64;
        for _ in 0..30_000 {
            bot.tick(&mut sim);
            let before = (sim.ball_handles(), mon.findings().len());
            let bits = sim.switch_bits();
            sim.advance(1);
            let ((), n, _exempt) = count_allocs_split(|| mon.observe(&sim));
            let after = (sim.ball_handles(), mon.findings().len());
            if before == after {
                assert_eq!(n, 0, "tick {}: {n} allocation(s) in observe", sim.tick());
                checked += 1;
                if bits != sim.switch_bits() {
                    credited += 1;
                }
            }
        }
        (checked, credited)
    });
    assert!(checked > 20_000, "{checked}");
    // Switch edges (hit crediting) happened on checked ticks.
    assert!(credited > 0);
}
