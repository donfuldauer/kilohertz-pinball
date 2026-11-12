//! Spec: every `pinball.*` function maps into `HostCommandQueue` / `HostView`
//! exactly as the subphase 4.2 brief states, and scripted switch sequences
//! produce the expected command queues.

mod spec_support;

use pin_core::{
    CoilId, DispenserId, DiverterId, FlipperId, HostCommandQueue, LampId, MagnetId, PersistKey,
    ProfileId, SceneId, SoundId, SwitchId, TableScriptBackend,
};
use pin_script::NameTables;
use pin_script::lua::{LuaConfig, LuaScriptBackend};
use spec_support::{Host, backend, lamp, names, persisted};

const API: &[u8] = include_bytes!("fixtures/spec_api.lua");
const RULES: &[u8] = include_bytes!("fixtures/spec_rules.lua");
const OVERFLOW: &[u8] = include_bytes!("fixtures/spec_overflow.lua");

/// Runs one dispatch case of the API fixture and returns the resulting queue.
fn run_case(case: u16) -> HostCommandQueue {
    let mut b = backend(API);
    let mut h = Host::new(1);
    h.case(&mut b, case);
    assert!(!b.is_faulted(), "case {case} faulted the script");
    assert_eq!(b.fault_count(), 0);
    assert_eq!(b.overflow_count(), 0);
    h.cmd
}

/// Asserts that only the named list holds commands.
fn assert_only(cmd: &HostCommandQueue, expected_len: usize) {
    assert_eq!(
        cmd.len(),
        expected_len,
        "unexpected extra commands: {cmd:?}"
    );
}

#[test]
fn default_config_is_16_mib_and_one_million_instructions() {
    let c = LuaConfig::default();
    assert_eq!(c.memory_limit_bytes, 16 * 1024 * 1024);
    assert_eq!(c.instruction_budget, 1_000_000);
}

#[test]
fn name_tables_default_is_empty() {
    let n = NameTables::default();
    assert!(n.switches.is_empty());
    assert!(n.coils.is_empty());
    assert!(n.lamps.is_empty());
    assert!(n.magnets.is_empty());
    assert!(n.diverters.is_empty());
    assert!(n.flippers.is_empty());
    assert!(n.dispensers.is_empty());
    assert!(n.profiles.is_empty());
    assert!(n.sounds.is_empty());
    assert!(n.scenes.is_empty());
    assert!(n.persist.is_empty());
}

#[test]
fn backend_is_send_and_usable_as_trait_object() {
    fn assert_send<T: Send>() {}
    assert_send::<LuaScriptBackend>();
    let mut boxed: Box<dyn TableScriptBackend> = Box::new(backend(API));
    let mut h = Host::new(1);
    h.case(boxed.as_mut(), 8);
    assert_eq!(h.cmd.gi_tint, Some([0.25, 0.5, 0.75, 1.0]));
}

#[test]
fn empty_script_has_no_entry_points_and_is_harmless() {
    for src in [&b""[..], b"-- nothing here\n", b"local x = 1"] {
        let mut b = LuaScriptBackend::new(src, &NameTables::default(), LuaConfig::default())
            .expect("empty script loads");
        let mut h = Host::new(1);
        h.init(&mut b);
        h.tick(&mut b);
        h.edge(&mut b, 3, true);
        h.edge(&mut b, 3, false);
        assert!(h.cmd.is_empty());
        assert!(!b.is_faulted());
        assert_eq!(b.fault_count(), 0);
        assert_eq!(b.overflow_count(), 0);
    }
}

#[test]
fn syntax_error_fails_construction() {
    let r = LuaScriptBackend::new(b"function on_tick( end", &names(), LuaConfig::default());
    assert!(r.is_err());
}

#[test]
fn every_documented_function_exists() {
    let cmd = run_case(16);
    assert_eq!(persisted(&cmd, 1), Some(17));
    assert_eq!(
        persisted(&cmd, 0),
        Some(0),
        "some pinball.* functions are missing"
    );
}

#[test]
fn pulse_coil_maps_to_coil_pulses() {
    let cmd = run_case(1);
    assert_eq!(
        cmd.coil_pulses.as_slice(),
        &[(CoilId(3), 30), (CoilId(31), 65535), (CoilId(0), 1)]
    );
    assert_only(&cmd, 3);
}

#[test]
fn hold_coil_maps_to_coil_holds() {
    let cmd = run_case(2);
    assert_eq!(
        cmd.coil_holds.as_slice(),
        &[(CoilId(3), true), (CoilId(1), false)]
    );
    assert_only(&cmd, 2);
}

#[test]
fn set_magnet_maps_to_magnets() {
    let cmd = run_case(3);
    assert_eq!(
        cmd.magnets.as_slice(),
        &[(MagnetId(4), 0.5), (MagnetId(4), 0.0), (MagnetId(7), 1.0)]
    );
    assert_only(&cmd, 3);
}

#[test]
fn set_zipper_maps_to_zippers() {
    let cmd = run_case(4);
    assert_eq!(
        cmd.zippers.as_slice(),
        &[(FlipperId(6), true), (FlipperId(0), false)]
    );
    assert_only(&cmd, 2);
}

#[test]
fn set_diverter_maps_to_diverters() {
    let cmd = run_case(5);
    assert_eq!(
        cmd.diverters.as_slice(),
        &[(DiverterId(5), true), (DiverterId(5), false)]
    );
    assert_only(&cmd, 2);
}

#[test]
fn spawn_ball_maps_to_ball_spawns() {
    let cmd = run_case(6);
    assert_eq!(
        cmd.ball_spawns.as_slice(),
        &[
            (DispenserId(9), ProfileId(11)),
            (DispenserId(9), ProfileId(0))
        ]
    );
    assert_only(&cmd, 2);
}

#[test]
fn set_lamp_maps_to_lamps() {
    let cmd = run_case(7);
    assert_eq!(
        cmd.lamps.as_slice(),
        &[(LampId(200), 0.25), (LampId(2), 1.0), (LampId(255), 0.0)]
    );
    assert_only(&cmd, 3);
}

#[test]
fn set_gi_tint_sets_the_tint() {
    let cmd = run_case(8);
    assert_eq!(cmd.gi_tint, Some([0.25, 0.5, 0.75, 1.0]));
    assert_only(&cmd, 1);
}

#[test]
fn set_score_converts_one_based_player_and_keeps_full_i64() {
    let cmd = run_case(9);
    assert_eq!(
        cmd.scores.as_slice(),
        &[(0u8, 1000i64), (3, 9_007_199_254_740_993), (1, 0)]
    );
    assert_only(&cmd, 3);
}

#[test]
fn show_scene_maps_to_display_scenes() {
    let cmd = run_case(10);
    assert_eq!(cmd.display_scenes.as_slice(), &[SceneId(15), SceneId(14)]);
    assert_only(&cmd, 2);
}

#[test]
fn play_sound_maps_to_sounds() {
    let cmd = run_case(11);
    assert_eq!(
        cmd.sounds.as_slice(),
        &[(SoundId(12), 0.5), (SoundId(13), 1.0)]
    );
    assert_only(&cmd, 2);
}

#[test]
fn persist_set_maps_to_persist_writes() {
    let cmd = run_case(12);
    assert_eq!(
        cmd.persist_writes.as_slice(),
        &[
            (PersistKey(16), -5),
            (PersistKey(63), i64::MAX),
            (PersistKey(0), i64::MIN)
        ]
    );
    assert_only(&cmd, 3);
}

#[test]
fn a_case_that_does_nothing_queues_nothing() {
    assert!(run_case(15).is_empty());
    // An id with no case at all.
    assert!(run_case(99).is_empty());
}

#[test]
fn host_view_reads_reflect_the_view() {
    let mut b = backend(API);
    let mut h = Host::new(1);
    h.view.tick = 123_456_789_012;
    h.view.balls_in_play = 3;
    h.view.persist[16] = -77;
    h.view.persist[63] = i64::MAX;
    h.view.persist[0] = 42;
    h.view.set_switch(SwitchId(7), true);
    h.view.set_switch(SwitchId(255), true);
    h.view.set_switch(SwitchId(64), true);
    h.case(&mut b, 13);
    assert!(!b.is_faulted());

    assert_eq!(persisted(&h.cmd, 0), Some(-77), "persist_get(credits)");
    assert_eq!(persisted(&h.cmd, 1), Some(123_456_789_012), "tick()");
    assert_eq!(persisted(&h.cmd, 2), Some(3), "balls_in_play()");
    assert_eq!(persisted(&h.cmd, 3), Some(i64::MAX), "persist_get(63)");
    assert_eq!(persisted(&h.cmd, 4), Some(42), "persist_get(0)");

    assert_eq!(lamp(&h.cmd, 0), Some(1.0), "switch 7 closed");
    assert_eq!(lamp(&h.cmd, 1), Some(0.0), "switch 8 open");
    assert_eq!(lamp(&h.cmd, 2), Some(1.0), "switch 255 closed");
    assert_eq!(lamp(&h.cmd, 3), Some(0.0), "switch 63 open");
    assert_eq!(lamp(&h.cmd, 4), Some(1.0), "switch 64 closed");
    assert_eq!(lamp(&h.cmd, 5), Some(0.0), "switch 0 open");

    assert_eq!(lamp(&h.cmd, 10), Some(1.0), "tick() is an integer");
    assert_eq!(lamp(&h.cmd, 11), Some(1.0), "switch() is a boolean");
    assert_eq!(lamp(&h.cmd, 12), Some(1.0), "balls_in_play() is an integer");
    assert_eq!(lamp(&h.cmd, 13), Some(1.0), "persist_get() is an integer");
}

#[test]
fn host_view_reads_follow_view_changes_between_callbacks() {
    let mut b = backend(API);
    let mut h = Host::new(1);
    h.case(&mut b, 13);
    assert_eq!(persisted(&h.cmd, 1), Some(0));
    assert_eq!(persisted(&h.cmd, 2), Some(0));
    assert_eq!(lamp(&h.cmd, 0), Some(0.0));

    h.view.tick = 9;
    h.view.balls_in_play = 255;
    h.view.set_switch(SwitchId(7), true);
    h.case(&mut b, 13);
    assert_eq!(persisted(&h.cmd, 1), Some(9));
    assert_eq!(persisted(&h.cmd, 2), Some(255));
    assert_eq!(lamp(&h.cmd, 0), Some(1.0));

    h.view.set_switch(SwitchId(7), false);
    h.case(&mut b, 13);
    assert_eq!(lamp(&h.cmd, 0), Some(0.0));
}

#[test]
fn name_tables_expose_every_name_as_integer_id() {
    let cmd = run_case(14);
    let tables = [
        "sw",
        "coil",
        "lamp",
        "magnet",
        "diverter",
        "flipper",
        "dispenser",
        "profile",
        "sound",
        "scene",
        "persist_key",
    ];
    for (i, t) in tables.iter().enumerate() {
        assert_eq!(
            lamp(&cmd, i as u16),
            Some(1.0),
            "pinball.{t} has wrong contents"
        );
    }
}

// ---------------------------------------------------------------------------
// Scripted switch sequences.

#[test]
fn scripted_switch_sequence_produces_expected_queues() {
    let mut b = backend(RULES);
    let mut h = Host::new(7);

    h.init(&mut b);
    let mut want = HostCommandQueue::default();
    want.show_scene(SceneId(14)).unwrap();
    want.set_gi_tint([1.0, 1.0, 1.0, 0.5]);
    assert_eq!(h.cmd, want, "on_init");

    // Start pressed: ball served.
    h.edge(&mut b, 0, true);
    let mut want = HostCommandQueue::default();
    want.spawn_ball(DispenserId(9), ProfileId(0)).unwrap();
    want.pulse_coil(CoilId(1), 40).unwrap();
    want.show_scene(SceneId(15)).unwrap();
    want.set_score(0, 0).unwrap();
    assert_eq!(h.cmd, want, "start closed");

    // Left sling hit twice.
    for expected in [10, 20] {
        h.edge(&mut b, 7, true);
        let mut want = HostCommandQueue::default();
        want.pulse_coil(CoilId(3), 25).unwrap();
        want.play_sound(SoundId(12), 0.5).unwrap();
        want.set_score(0, expected).unwrap();
        assert_eq!(h.cmd, want, "sling closed");

        h.edge(&mut b, 7, false);
        let mut want = HostCommandQueue::default();
        want.set_lamp(LampId(200), 0.0).unwrap();
        assert_eq!(h.cmd, want, "sling opened");
    }

    // Spinner scores on both edges.
    h.edge(&mut b, 63, true);
    assert_eq!(h.cmd.scores.as_slice(), &[(0, 21)]);
    assert_eq!(h.cmd.len(), 1);
    h.edge(&mut b, 63, false);
    assert_eq!(h.cmd.scores.as_slice(), &[(0, 22)]);
    assert_eq!(h.cmd.len(), 1);

    // An unrelated switch does nothing.
    h.edge(&mut b, 8, true);
    assert!(h.cmd.is_empty());

    // Drain while start is still held: knocker hold follows the switch state.
    h.edge(&mut b, 20, true);
    let mut want = HostCommandQueue::default();
    want.write_persist(PersistKey(0), 22).unwrap();
    want.play_sound(SoundId(13), 1.0).unwrap();
    want.hold_coil(CoilId(31), true).unwrap();
    assert_eq!(h.cmd, want, "drain with start held");

    h.edge(&mut b, 20, false);
    assert!(h.cmd.is_empty(), "trough opening is ignored");

    h.edge(&mut b, 0, false);
    assert!(h.cmd.is_empty(), "start opening is ignored");

    h.edge(&mut b, 20, true);
    assert_eq!(h.cmd.coil_holds.as_slice(), &[(CoilId(31), false)]);

    assert!(!b.is_faulted());
    assert_eq!(b.fault_count(), 0);
    assert_eq!(b.overflow_count(), 0);
}

#[test]
fn on_tick_runs_every_tick_and_reads_the_tick_counter() {
    let mut b = backend(RULES);
    let mut h = Host::new(7);
    for _ in 0..40 {
        h.tick(&mut b);
        if h.view.tick.is_multiple_of(4) {
            assert_eq!(
                h.cmd.lamps.as_slice(),
                &[(LampId(2), 1.0)],
                "tick {}",
                h.view.tick
            );
            assert_eq!(h.cmd.len(), 1);
        } else {
            assert!(h.cmd.is_empty(), "tick {}", h.view.tick);
        }
    }
    assert!(!b.is_faulted());
}

#[test]
fn commands_accumulate_when_the_host_does_not_clear_between_callbacks() {
    // The backend only pushes; clearing is the sim's job.
    let mut b = backend(API);
    let mut h = Host::new(1);
    b.on_switch_edge(SwitchId(1), pin_core::SwitchEdge::Closed, &mut h.ctx());
    b.on_switch_edge(SwitchId(2), pin_core::SwitchEdge::Closed, &mut h.ctx());
    b.on_switch_edge(SwitchId(1), pin_core::SwitchEdge::Opened, &mut h.ctx());
    assert_eq!(h.cmd.coil_pulses.len(), 6);
    assert_eq!(h.cmd.coil_holds.len(), 2);
}

// ---------------------------------------------------------------------------
// Queue overflow: dropped silently, counted.

/// `(case, capacity)` for every fixed-capacity list.
const LISTS: &[(u16, usize)] = &[
    (1, 32),
    (2, 32),
    (3, 8),
    (4, 8),
    (5, 16),
    (6, 8),
    (7, 256),
    (8, 4),
    (9, 8),
    (10, 16),
    (11, 8),
];

fn list_len(cmd: &HostCommandQueue, case: u16) -> usize {
    match case {
        1 => cmd.coil_pulses.len(),
        2 => cmd.coil_holds.len(),
        3 => cmd.magnets.len(),
        4 => cmd.zippers.len(),
        5 => cmd.diverters.len(),
        6 => cmd.ball_spawns.len(),
        7 => cmd.lamps.len(),
        8 => cmd.scores.len(),
        9 => cmd.display_scenes.len(),
        10 => cmd.sounds.len(),
        11 => cmd.persist_writes.len(),
        _ => unreachable!(),
    }
}

#[test]
fn filling_a_list_exactly_does_not_overflow() {
    for &(case, cap) in LISTS {
        let mut b = backend(OVERFLOW);
        let mut h = Host::new(1);
        h.view.persist[0] = cap as i64;
        h.case(&mut b, case);
        assert_eq!(list_len(&h.cmd, case), cap, "case {case}");
        assert_eq!(h.cmd.len(), cap, "case {case}");
        assert_eq!(b.overflow_count(), 0, "case {case}");
        assert!(!b.is_faulted(), "case {case}");
    }
}

#[test]
fn overflowing_commands_are_dropped_and_counted_without_faulting() {
    for &(case, cap) in LISTS {
        for extra in [1usize, 5] {
            let mut b = backend(OVERFLOW);
            let mut h = Host::new(1);
            h.view.persist[0] = (cap + extra) as i64;
            h.case(&mut b, case);
            assert_eq!(
                list_len(&h.cmd, case),
                cap,
                "case {case}: list must stay at capacity"
            );
            assert_eq!(
                h.cmd.len(),
                cap,
                "case {case}: nothing leaks to other lists"
            );
            assert_eq!(b.overflow_count(), extra as u32, "case {case}");
            assert!(!b.is_faulted(), "case {case}: overflow is not an error");
            assert_eq!(b.fault_count(), 0, "case {case}");
        }
    }
}

#[test]
fn overflow_keeps_the_first_commands_in_order() {
    let mut b = backend(OVERFLOW);
    let mut h = Host::new(1);
    h.view.persist[0] = 10;
    h.case(&mut b, 8); // set_score((i % 4) + 1, i), capacity 4
    assert_eq!(
        h.cmd.scores.as_slice(),
        &[(1u8, 1i64), (2, 2), (3, 3), (0, 4)]
    );
    assert_eq!(b.overflow_count(), 6);
}

#[test]
fn overflow_count_accumulates_across_callbacks_and_lists() {
    let mut b = backend(OVERFLOW);
    let mut h = Host::new(1);
    h.view.persist[0] = 10;
    h.case(&mut b, 8); // capacity 4 -> 6 dropped
    assert_eq!(b.overflow_count(), 6);
    h.case(&mut b, 3); // capacity 8 -> 2 dropped
    assert_eq!(b.overflow_count(), 8);
    h.case(&mut b, 1); // capacity 32 -> none dropped
    assert_eq!(b.overflow_count(), 8);
    // The script keeps working after overflows.
    assert_eq!(h.cmd.coil_pulses.len(), 10);
    assert!(!b.is_faulted());
}

#[test]
fn a_queue_already_full_on_entry_counts_every_push() {
    let mut b = backend(OVERFLOW);
    let mut h = Host::new(1);
    h.view.persist[0] = 3;
    h.cmd.clear();
    for i in 0..8 {
        h.cmd.show_scene(SceneId(100 + i)).unwrap();
    }
    b.on_switch_edge(SwitchId(9), pin_core::SwitchEdge::Closed, &mut h.ctx());
    assert_eq!(h.cmd.display_scenes.len(), 8);
    assert_eq!(
        h.cmd.display_scenes[0],
        SceneId(100),
        "existing commands are untouched"
    );
    assert_eq!(b.overflow_count(), 3);
}

#[test]
fn gi_tint_never_overflows_and_last_write_wins() {
    let mut b = backend(OVERFLOW);
    let mut h = Host::new(1);
    h.view.persist[0] = 1001;
    h.case(&mut b, 12);
    assert_eq!(h.cmd.gi_tint, Some([0.0, 0.0, 0.0, 1.0]));
    assert_eq!(b.overflow_count(), 0);
    assert!(!b.is_faulted());
}
