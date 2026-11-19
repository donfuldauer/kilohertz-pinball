//! 7.1 (D5, rule 4): recording into the profiler from the sim thread
//! allocates nothing.
//!
//! * Every sim-side writer call (tick time, input latency, overflow
//!   totals) and the presenter's frame record, under the counting global
//!   allocator of `pin_sim::alloc_guard`.
//! * The real table sim wrapped in [`ProfiledSim`] and driven by `SimLoop`
//!   (the orchestrator's tick loop) on the Sky Deck: no allocation per tick
//!   outside the Lua VM's arena (the rule-4 exemption), with the profiler
//!   recording every tick, and the same final `state_hash` as the
//!   unprofiled run.

mod profiler_support;

use pin_app::orchestrator::{SimLoop, TableSim, monotonic_now_ns};
use pin_app::profiler::{ProfiledSim, Profiler};
use pin_core::spsc::channel;
use pin_core::{CabinetAxis, CabinetInput, FrameStateSnapshot, InputEvent, TICK_NS, TripleBuffer};
use pin_sim::alloc_guard::{CountingAlloc, count_allocs, count_allocs_split, is_installed};
use profiler_support::{Player, Table, load, on_big_stack};

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

#[test]
fn writer_calls_do_not_allocate() {
    assert!(is_installed());
    let (v, n) = count_allocs(|| std::hint::black_box(vec![1u8; 8]));
    assert!(n >= 1 && v.len() == 8, "guard not armed");
    let mut p = Profiler::new();
    let evs = [
        InputEvent::button(5, CabinetInput::LeftFlipper, true),
        InputEvent::axis(6, CabinetAxis::Plunger, 0.3),
        InputEvent::button(u64::MAX, CabinetInput::Launch, true),
    ];
    let ((), n) = count_allocs(|| {
        for i in 0..100_000u64 {
            p.sim.record_tick_ns(i * 7);
            p.sim.record_tick(i, i + 90_000);
            p.sim.record_input(i, i + 1_000_000);
            p.sim.record_inputs(&evs, i * TICK_NS);
            p.sim.set_script_queue_overflows(i / 1000);
            p.sim_loop.set_input_overflow_ticks(i / 500);
            p.present.record_frame_ns(i * 1_000);
            p.present.record_frame(i, u64::MAX);
        }
    });
    assert_eq!(n, 0, "profiler writers allocated {n} times");
    let s = p.reader.snapshot();
    assert_eq!(s.tick.count, 200_000);
    assert_eq!(s.input.count, 300_000);
    // Every LAUNCH (stamped u64::MAX) plus the flipper event at i = 0
    // (stamped 5 ns, after the tick start 0).
    assert_eq!(s.input_clamped, 100_001);
}

#[test]
fn profiled_table_sim_does_not_allocate_per_tick() {
    assert!(is_installed());
    let (plain_hash, profiled_hash, allocs, ticks) = on_big_stack(|| {
        const TICKS: u64 = 20_000;
        // The scripted player's inputs for TICKS ticks (recorded on a
        // plain sim; both runs below replay them).
        let rec = profiler_support::record(Table::SkyDeck, TICKS);
        assert!(rec.stream.injections.is_empty());
        let plain = profiler_support::replay_plain(Table::SkyDeck, &rec.stream).hash;

        let mut p = Profiler::new();
        let (mut tx, rx) = channel(1 << 16);
        for &(t, ev) in &rec.stream.recording.events {
            // Re-stamp onto the loop's tick clock (epoch 0: tick t ends
            // at t · TICK_NS), so the loop hands each event to its tick.
            let mut ev = ev;
            ev.timestamp_ns = t * TICK_NS;
            tx.push(ev).unwrap();
        }
        let sim = TableSim::new(load(Table::SkyDeck), None);
        let profiled = ProfiledSim::new(sim, p.sim, monotonic_now_ns);
        let (w, _r) = TripleBuffer::new(FrameStateSnapshot::default());
        let mut lp = SimLoop::new(profiled, rx, w);
        lp.run_ticks(500); // Warm-up (first Lua calls size the VM).
        let mut allocs = 0;
        for _ in 0..((TICKS - 500) / 100) {
            let ((), n, _exempt) = count_allocs_split(|| {
                lp.run_ticks(100);
                p.sim_loop
                    .set_input_overflow_ticks(lp.input_overflow_ticks());
            });
            allocs += n;
        }
        assert_eq!(lp.tick(), TICKS);
        let profiled = lp.sim().inner().sim().state_hash();
        (plain, profiled, allocs, p.reader.snapshot().tick.count)
    });
    assert_eq!(allocs, 0, "step_1ms with the profiler allocated");
    assert_eq!(ticks, 20_000);
    assert_eq!(
        profiled_hash, plain_hash,
        "the profiler changed the simulation"
    );
}

#[test]
fn player_is_usable_standalone() {
    // The scripted player only reads the sim; the first ticks start a game.
    on_big_stack(|| {
        let info = pin_sim::autoplay::TableInfo::load(Table::FeatureLab.bundle()).unwrap();
        let sim = load(Table::FeatureLab);
        let mut player = Player::new(&sim, &info);
        let mut buf = Vec::new();
        player.inputs(&sim, &mut buf);
        assert!(buf.is_empty(), "nothing at tick 1");
    });
}
