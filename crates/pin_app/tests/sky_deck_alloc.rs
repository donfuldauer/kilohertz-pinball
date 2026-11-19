//! 6.2 T5, D5: the real Sky Deck simulation stepped through
//! `pin_app::orchestrator::SimLoop` makes no heap allocation per tick once
//! warmed up: normal play with every flipper, the Sky Bridge opening and
//! closing, and a Night Overdrive start (UV tint, scene, Glo-Ball spawn and
//! its VUK lift onto the upper deck). Guard: `pin_sim::alloc_guard` (the Lua
//! arena is exempt by its rules).

mod sky_deck_support;

use pin_app::orchestrator::{
    DISPLAY_RING_CAPACITY, DisplayCommand, SOUND_RING_CAPACITY, SimLoop, TableSim, load_validated,
};
use pin_audio::mixer::trigger_ring;
use pin_core::input::{CabinetInput, HapticsTelemetryEvent, InputEvent};
use pin_core::sim::TICK_NS;
use pin_core::snapshot::FrameStateSnapshot;
use pin_core::spsc;
use pin_core::triple_buffer::TripleBuffer;
use pin_input::haptics::haptics_ring;
use pin_sim::SimConfig;
use pin_sim::alloc_guard::{CountingAlloc, count_allocs, is_installed};
use sky_deck_support::{is_uv, key, on_big_stack, sky_deck_bundle};

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

#[test]
fn sky_deck_ticks_do_not_allocate_through_a_night_overdrive_start() {
    let dir = sky_deck_bundle().to_path_buf();
    on_big_stack(move || {
        assert!(is_installed());
        let sim = load_validated(&dir, SimConfig::default()).unwrap();
        let (htx, mut hrx) = haptics_ring::<HapticsTelemetryEvent>(64);
        let (stx, mut srx) = trigger_ring(SOUND_RING_CAPACITY);
        let (dtx, mut drx) = haptics_ring::<DisplayCommand>(DISPLAY_RING_CAPACITY);
        let table = TableSim::new(sim, Some(htx))
            .with_sounds(stx)
            .with_displays(dtx);
        let (mut input, consumer) = spsc::channel::<InputEvent>(4096);
        let (writer, _reader) = TripleBuffer::new(FrameStateSnapshot::default());
        let mut sim_loop = SimLoop::new(table, consumer, writer);
        let mut push = |tick: u64, i: CabinetInput, p: bool| {
            input
                .push(InputEvent::button(tick * TICK_NS - TICK_NS / 2, i, p))
                .unwrap();
        };
        push(50, CabinetInput::Start, true);
        push(60, CabinetInput::Start, false);
        // Sky Bridge open (8 s) and closed again inside the guarded run.
        push(2500, CabinetInput::Action, true);
        push(2520, CabinetInput::Action, false);
        let mut k = 2000;
        while k < 21_000 {
            push(k, CabinetInput::LeftFlipper, true);
            push(k + 60, CabinetInput::LeftFlipper, false);
            push(k + 150, CabinetInput::RightUpper, true);
            push(k + 210, CabinetInput::RightUpper, false);
            k += 300;
        }
        // Warm-up: attract, game start, ball served.
        sim_loop.run_ticks(1800);
        let mut worst = 0u64;
        let mut saw_uv = false;
        let mut saw_bridge = false;
        let mut max_balls = 0;
        for t in 0..19_000u64 {
            // State injection between ticks (outside the guard): arm Night
            // Overdrive (Newton ball, three drops) and put the ball in the
            // VUK, which starts it.
            {
                let s = sim_loop.sim_mut().sim_mut();
                let names = ["newton_sw", "drop_1_sw", "drop_2_sw", "drop_3_sw", "vuk_sw"];
                if t >= 11_000 && t % 100 == 0 && t < 11_500 {
                    s.set_switch(names[((t - 11_000) / 100) as usize], true)
                        .unwrap();
                }
                if t >= 11_000 && t % 100 == 5 && t < 11_500 {
                    s.release_switch(names[((t - 11_000) / 100) as usize])
                        .unwrap();
                }
            }
            let ((), n) = count_allocs(|| sim_loop.run_ticks(1));
            worst = worst.max(n);
            let s = sim_loop.sim().sim();
            saw_uv |= is_uv(s.snapshot().gi);
            saw_bridge |= s.persist(key::BRIDGE) == 1;
            max_balls = max_balls.max(s.balls_in_play());
            // Consumers drain on "other threads" (outside the guard).
            while srx.pop().is_some() {}
            drx.drain(|_| {});
            hrx.drain(|_| {});
        }
        assert_eq!(worst, 0, "a tick allocated {worst} times");
        let s = sim_loop.sim().sim();
        assert!(!s.script_faulted(), "{:?}", s.script_error());
        assert_eq!(s.script_queue_overflows(), 0);
        assert!(saw_bridge, "the Sky Bridge opened");
        assert_eq!(s.persist(key::BRIDGE), 0, "and closed");
        assert!(s.persist(key::MODE) >= 1, "Night Overdrive started");
        assert!(saw_uv, "UV tint seen");
        assert!(max_balls >= 2, "the Glo-Ball was spawned");
    });
}
