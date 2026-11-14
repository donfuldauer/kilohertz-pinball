mod sim_support;
use pin_core::sim::SimStep;
use sim_support::*;
#[test]
fn dbg() {
    on_sim_thread(|| {
        let mut sim = pin_sim::Simulation::load(&fixture(), Default::default()).unwrap();
        eprintln!("bounds {:?}", sim.snapshot().ball_count);
        let mut out = *sim.snapshot();
        let mut buf = Vec::with_capacity(16);
        for _ in 0..10000 {
            let t = sim.tick() + 1; buf.clear(); pin_sim::cli::pattern_events(t, &mut buf);
            sim.step_1ms(t, &buf, &mut out);
            if t % 250 == 0 {
                let b: Vec<_> = sim.ball_handles().iter().filter_map(|h| sim.ball(*h)).map(|b| (b.position_m, b.velocity_m_s)).collect();
                eprintln!("t {t} balls {:?} c {:?} score {:?} ring {:?} trough {:?}", b, sim.counters(), sim.score(1), sim.switch_hits("bumper_ring"), sim.switch_hits("trough"));
            }
        }
    });
}
