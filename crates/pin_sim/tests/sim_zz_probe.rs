mod sim_support;
use sim_support::*;
#[test]
fn probe() {
    on_sim_thread(|| {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/spec_2_5/table");
        for vx in [0.0f32, 0.001] {
            let mut sim = pin_sim::Simulation::load(&p, Default::default()).unwrap();
            let h = sim.spawn_ball("standard_steel", [0.2604739, 0.6412948, 0.0135], [vx, 0.0, 0.0]).unwrap();
            for t in [1u64, 10, 44, 88] {
                while sim.tick() < t { sim.advance(1); }
                eprintln!("vx {vx} t {t} {:?} {:?} n {} hash {:x}", sim.ball(h).map(|b| (b.position_m, b.velocity_m_s)), sim.counters(), sim.balls_in_play(), sim.state_hash());
            }
        }
    });
}
