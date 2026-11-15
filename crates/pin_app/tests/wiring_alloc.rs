//! Subphase 6.0 wiring, D5: with the new accessors and ring pushes active
//! (haptics, sound triggers, display commands), a `TableSim` stepped through
//! `SimLoop` makes no heap allocation per tick once warmed up. Guard:
//! `pin_sim::alloc_guard` (Lua arena allocations are exempt by its rules).

mod support;

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
use support::{greybox_bundle, on_big_stack};

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

#[test]
fn table_sim_with_all_rings_does_not_allocate() {
    let dir = greybox_bundle().to_path_buf();
    on_big_stack(move || {
        assert!(is_installed());
        let sim = load_validated(&dir, SimConfig::default()).unwrap();
        let (htx, mut hrx) = haptics_ring::<HapticsTelemetryEvent>(64);
        let (stx, mut srx) = trigger_ring(SOUND_RING_CAPACITY);
        let (dtx, mut drx) = haptics_ring::<DisplayCommand>(DISPLAY_RING_CAPACITY);
        let table = TableSim::new(sim, Some(htx))
            .with_sounds(stx)
            .with_displays(dtx);
        let (mut input, consumer) = spsc::channel::<InputEvent>(1024);
        let (writer, _reader) = TripleBuffer::new(FrameStateSnapshot::default());
        let mut sim_loop = SimLoop::new(table, consumer, writer);
        let mut push = |tick: u64, i: CabinetInput, p: bool| {
            input
                .push(InputEvent::button(tick * TICK_NS - TICK_NS / 2, i, p))
                .unwrap();
        };
        push(50, CabinetInput::Start, true);
        push(60, CabinetInput::Start, false);
        let mut k = 2000;
        while k < 7000 {
            push(k, CabinetInput::LeftFlipper, true);
            push(k + 60, CabinetInput::LeftFlipper, false);
            k += 300;
        }
        // Warm-up: attract, game start, ball served.
        sim_loop.run_ticks(1800);
        let (mut sounds, mut displays, mut haptics) = (0u64, 0u64, 0u64);
        let mut worst = 0u64;
        for t in 0..5000u64 {
            // State injection between ticks (outside the guard): bumper and
            // sling hits make the rules play sounds and report scores.
            if t % 250 == 0 {
                let name =
                    ["bumper_1_ring", "sling_left_sw", "bumper_2_ring"][(t / 250 % 3) as usize];
                sim_loop.sim_mut().sim_mut().set_switch(name, true).unwrap();
            }
            if t % 250 == 20 {
                for name in ["bumper_1_ring", "sling_left_sw", "bumper_2_ring"] {
                    sim_loop.sim_mut().sim_mut().release_switch(name).unwrap();
                }
            }
            let ((), n) = count_allocs(|| sim_loop.run_ticks(1));
            worst = worst.max(n);
            // Consumers drain on "other threads" (outside the guard).
            while srx.pop().is_some() {
                sounds += 1;
            }
            displays += drx.drain(|_| {}) as u64;
            haptics += hrx.drain(|_| {}) as u64;
        }
        assert_eq!(worst, 0, "a tick with ring pushes allocated {worst} times");
        assert!(sounds >= 10, "sounds pushed: {sounds}");
        assert!(displays >= 10, "display commands pushed: {displays}");
        assert!(haptics >= 10, "haptics pushed: {haptics}");
        let t = sim_loop.sim();
        assert_eq!(
            t.sounds_dropped() + t.displays_dropped() + t.haptics_dropped(),
            0
        );
        assert!(!t.sim().script_faulted(), "{:?}", t.sim().script_error());
    });
}

#[test]
fn full_rings_drop_without_allocating() {
    let dir = greybox_bundle().to_path_buf();
    on_big_stack(move || {
        let sim = load_validated(&dir, SimConfig::default()).unwrap();
        let (stx, _srx) = trigger_ring(1);
        let (dtx, _drx) = haptics_ring::<DisplayCommand>(1);
        let mut table = TableSim::new(sim, None).with_sounds(stx).with_displays(dtx);
        let mut out = FrameStateSnapshot::default();
        use pin_core::SimStep;
        let mut tick = 1;
        table.sim_mut().set_switch("start", true).unwrap();
        for _ in 0..1600 {
            table.step_1ms(tick, &[], &mut out);
            tick += 1;
        }
        let mut worst = 0;
        for i in 0..2000u64 {
            if i % 100 == 0 {
                table
                    .sim_mut()
                    .set_switch("bumper_1_ring", i % 200 == 0)
                    .unwrap();
            }
            let ((), n) = count_allocs(|| table.step_1ms(tick, &[], &mut out));
            worst = worst.max(n);
            tick += 1;
        }
        assert_eq!(worst, 0);
        assert!(table.sounds_dropped() > 0 && table.displays_dropped() > 0);
    });
}
