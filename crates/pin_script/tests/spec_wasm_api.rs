//! Spec (4.5 FIXED API / Wasm ABI): every `pinball` import maps one-to-one to
//! a host command or query, validates its arguments exactly like the Lua API
//! (a violation traps and faults the script), and a fault has the Lua "tilt"
//! semantics: the callback's commands are rolled back, held coils are
//! released, later callbacks are no-ops.

#[path = "spec_fixtures/wasm/support.rs"]
mod support;

use pin_core::{
    CoilId, DispenserId, DiverterId, FlipperId, HostCommandQueue, LampId, MagnetId, Pcg32,
    PersistKey, ProfileId, SceneId, ScriptError, SeedableRng, SoundId, SwitchId,
};
use pin_script::wasm::{WasmConfig, WasmScriptBackend};
use proptest::prelude::*;
use support::{Driver, Host, backend, fa, guest, lua_random, markers, names, op};

const BAD_IDS: [i64; 6] = [-1, 65536, 70000, i32::MAX as i64, i32::MIN as i64, -65536];
const BAD_BOOLS: [i64; 6] = [2, -1, 256, i32::MAX as i64, i32::MIN as i64, 0x1_0000];

/// A fresh driver must fault on `op(args)`, leaving nothing but (possibly) nothing queued.
fn assert_faults(opcode: u16, args: &[i64]) {
    let mut d = Driver::new();
    let q = d.call(opcode, args).clone();
    assert!(
        d.b.is_faulted(),
        "op {opcode} with args {args:?} must trap and fault"
    );
    assert_eq!(d.b.fault_count(), 1);
    assert!(d.b.last_error().is_some());
    assert!(
        q.is_empty(),
        "a faulting call must queue nothing, got {q:?} for op {opcode} {args:?}"
    );
}

/// A fresh driver must accept `op(args)` and return the queue.
fn accept(opcode: u16, args: &[i64]) -> HostCommandQueue {
    let mut d = Driver::new();
    let q = d.call(opcode, args).clone();
    assert!(
        !d.b.is_faulted(),
        "op {opcode} with args {args:?} must be accepted: {:?}",
        d.b.last_error()
    );
    assert_eq!(d.b.fault_count(), 0);
    assert_eq!(d.b.overflow_count(), 0);
    assert!(d.b.last_error().is_none());
    q
}

#[test]
fn config_defaults_match_the_brief() {
    let d = WasmConfig::default();
    assert_eq!(d.memory_limit_bytes, 16 * 1024 * 1024);
    assert_eq!(d.fuel_per_callback, 1_000_000);
    let e = WasmConfig::engine();
    assert_eq!(e.memory_limit_bytes, 2 * 1024 * 1024);
    assert_eq!(e.fuel_per_callback, 100_000);
    let b = support::backend_with(support::DRIVER_WAT, e);
    assert_eq!(b.config(), e);
    assert_eq!(Driver::new().b.config(), d);
    // Copy, Clone, Debug, PartialEq, Eq.
    let c = d;
    assert_eq!(c, d);
    assert_ne!(d, e);
    assert!(!format!("{d:?}").is_empty());
}

#[test]
fn a_fresh_backend_is_healthy_and_callbacks_reach_the_guest() {
    let mut d = Driver::new();
    assert!(!d.b.is_faulted());
    assert_eq!(d.b.fault_count(), 0);
    assert_eq!(d.b.overflow_count(), 0);
    assert!(d.b.last_error().is_none());
    d.h.init(&mut d.b);
    assert_eq!(d.h.cmd.display_scenes.as_slice(), &[SceneId(1)]);
    assert_eq!(d.tick().display_scenes.as_slice(), &[SceneId(7)]);
    assert_eq!(d.h.cmd.len(), 1);
}

#[test]
fn on_switch_edge_receives_id_and_closed_flag() {
    let src = guest(
        "",
        "",
        "(call $set_score (i32.const 1) (i64.extend_i32_s (local.get $id)))
         (call $set_score (i32.const 2) (i64.extend_i32_s (local.get $closed)))",
    );
    let mut b = backend(&src);
    let mut h = Host::new(1);
    for (sw, closed) in [(0u16, true), (7, false), (255, true), (256, false), (65535, true)] {
        h.edge(&mut b, sw, closed);
        assert_eq!(
            h.cmd.scores.as_slice(),
            &[(0, i64::from(sw)), (1, i64::from(closed))],
            "switch {sw} closed {closed}"
        );
    }
    assert!(!b.is_faulted());
}

#[test]
fn every_command_import_queues_exactly_its_command() {
    let q = accept(op::PULSE_COIL, &[31, 250]);
    assert_eq!(q.coil_pulses.as_slice(), &[(CoilId(31), 250)]);
    assert_eq!(q.len(), 1);

    let q = accept(op::HOLD_COIL, &[3, 1]);
    assert_eq!(q.coil_holds.as_slice(), &[(CoilId(3), true)]);
    let q = accept(op::HOLD_COIL, &[3, 0]);
    assert_eq!(q.coil_holds.as_slice(), &[(CoilId(3), false)]);
    assert_eq!(q.len(), 1);

    let q = accept(op::SET_MAGNET, &[4, fa(0.75)]);
    assert_eq!(q.magnets.as_slice(), &[(MagnetId(4), 0.75)]);
    assert_eq!(q.len(), 1);

    let q = accept(op::SET_ZIPPER, &[6, 1]);
    assert_eq!(q.zippers.as_slice(), &[(FlipperId(6), true)]);
    let q = accept(op::SET_ZIPPER, &[6, 0]);
    assert_eq!(q.zippers.as_slice(), &[(FlipperId(6), false)]);

    let q = accept(op::SET_DIVERTER, &[5, 1]);
    assert_eq!(q.diverters.as_slice(), &[(DiverterId(5), true)]);
    let q = accept(op::SET_DIVERTER, &[5, 0]);
    assert_eq!(q.diverters.as_slice(), &[(DiverterId(5), false)]);

    let q = accept(op::SPAWN_BALL, &[9, 11]);
    assert_eq!(q.ball_spawns.as_slice(), &[(DispenserId(9), ProfileId(11))]);
    assert_eq!(q.len(), 1);

    let q = accept(op::SET_LAMP, &[200, fa(0.125)]);
    assert_eq!(q.lamps.as_slice(), &[(LampId(200), 0.125)]);
    assert_eq!(q.len(), 1);

    let q = accept(op::SET_GI_TINT, &[fa(1.0), fa(0.5), fa(0.25), fa(0.0)]);
    assert_eq!(q.gi_tint, Some([1.0, 0.5, 0.25, 0.0]));
    assert_eq!(q.len(), 1);

    // Players are 1..=4 for the guest and 0..=3 in the queue, as in Lua.
    for p in 1..=4i64 {
        let q = accept(op::SET_SCORE, &[p, 1_000_000 * p]);
        assert_eq!(q.scores.as_slice(), &[((p - 1) as u8, 1_000_000 * p)]);
        assert_eq!(q.len(), 1);
    }
    for s in [i64::MIN, -1, 0, i64::MAX] {
        let q = accept(op::SET_SCORE, &[1, s]);
        assert_eq!(q.scores.as_slice(), &[(0, s)]);
    }

    let q = accept(op::SHOW_SCENE, &[15]);
    assert_eq!(q.display_scenes.as_slice(), &[SceneId(15)]);
    assert_eq!(q.len(), 1);

    let q = accept(op::PLAY_SOUND, &[12, fa(2.5)]);
    assert_eq!(q.sounds.as_slice(), &[(SoundId(12), 2.5)]);
    assert_eq!(q.len(), 1);

    for v in [i64::MIN, -5, 0, i64::MAX] {
        let q = accept(op::PERSIST_SET, &[63, v]);
        assert_eq!(q.persist_writes.as_slice(), &[(PersistKey(63), v)]);
        assert_eq!(q.len(), 1);
    }
}

#[test]
fn ids_cover_the_whole_u16_range_and_nothing_else() {
    // (op, index of the id argument, a valid companion argument list)
    let id_ops: [(u16, usize, [i64; 2]); 12] = [
        (op::PULSE_COIL, 0, [0, 10]),
        (op::HOLD_COIL, 0, [0, 1]),
        (op::SET_MAGNET, 0, [0, fa(0.5)]),
        (op::SET_ZIPPER, 0, [0, 1]),
        (op::SET_DIVERTER, 0, [0, 0]),
        (op::SPAWN_BALL, 0, [0, 3]),
        (op::SPAWN_BALL, 1, [3, 0]),
        (op::SET_LAMP, 0, [0, fa(1.0)]),
        (op::SHOW_SCENE, 0, [0, 0]),
        (op::PLAY_SOUND, 0, [0, fa(1.0)]),
        (op::SWITCH, 0, [0, 0]),
        (op::HOLD_COIL, 0, [0, 0]),
    ];
    for (opcode, idx, base) in id_ops {
        for good in [0i64, 1, 255, 256, 65535] {
            let mut args = base;
            args[idx] = good;
            accept(opcode, &args);
        }
        for bad in BAD_IDS {
            let mut args = base;
            args[idx] = bad;
            assert_faults(opcode, &args);
        }
    }
    let q = accept(op::PULSE_COIL, &[65535, 65535]);
    assert_eq!(q.coil_pulses.as_slice(), &[(CoilId(65535), 65535)]);
    let q = accept(op::SPAWN_BALL, &[65535, 65535]);
    assert_eq!(q.ball_spawns.as_slice(), &[(DispenserId(65535), ProfileId(65535))]);
}

#[test]
fn pulse_duration_is_0_to_65535_ms() {
    for ms in [0i64, 1, 65535] {
        let q = accept(op::PULSE_COIL, &[1, ms]);
        assert_eq!(q.coil_pulses.as_slice(), &[(CoilId(1), ms as u16)]);
    }
    for ms in [-1i64, 65536, i32::MAX as i64, i32::MIN as i64] {
        assert_faults(op::PULSE_COIL, &[1, ms]);
    }
}

#[test]
fn booleans_other_than_0_and_1_trap() {
    for opcode in [op::HOLD_COIL, op::SET_ZIPPER, op::SET_DIVERTER] {
        for bad in BAD_BOOLS {
            assert_faults(opcode, &[1, bad]);
        }
    }
}

#[test]
fn player_must_be_1_to_4() {
    for bad in [0i64, 5, -1, 255, 256, i32::MAX as i64, i32::MIN as i64] {
        assert_faults(op::SET_SCORE, &[bad, 10]);
    }
}

#[test]
fn persist_keys_are_below_64() {
    for bad in [64i64, 65, 255, 65535, 65536, -1, i32::MIN as i64] {
        assert_faults(op::PERSIST_SET, &[bad, 1]);
        assert_faults(op::PERSIST_GET, &[bad]);
    }
    let mut d = Driver::new();
    for k in [4usize, 5, 31, 62, 63] {
        d.h.view.persist[k] = -(k as i64) * 1_000_000_007;
    }
    for k in [4usize, 5, 31, 62, 63] {
        assert_eq!(
            d.value(op::PERSIST_GET, &[k as i64]),
            Some(-(k as i64) * 1_000_000_007)
        );
    }
    d.h.view.persist[7] = i64::MIN;
    assert_eq!(d.value(op::PERSIST_GET, &[7]), Some(i64::MIN));
    // persist_get reads the view, not the writes queued in this callback.
    d.call(op::PERSIST_SET, &[7, 5]);
    assert_eq!(d.value(op::PERSIST_GET, &[7]), Some(i64::MIN));
    assert!(!d.b.is_faulted());
}

#[test]
fn switch_reads_the_view_bitset() {
    let mut d = Driver::new();
    for sw in [0u16, 3, 63, 64, 200, 255] {
        assert_eq!(d.value(op::SWITCH, &[i64::from(sw)]), Some(0), "switch {sw}");
        d.h.view.set_switch(SwitchId(sw), true);
        assert_eq!(d.value(op::SWITCH, &[i64::from(sw)]), Some(1), "switch {sw}");
    }
    d.h.view.set_switch(SwitchId(64), false);
    assert_eq!(d.value(op::SWITCH, &[64]), Some(0));
    assert_eq!(d.value(op::SWITCH, &[63]), Some(1));
    // Ids in u16 range beyond the 256 tracked switches read as open.
    d.h.view.switches = [u64::MAX; 4];
    for sw in [256i64, 257, 1000, 65535] {
        assert_eq!(d.value(op::SWITCH, &[sw]), Some(0), "switch {sw}");
    }
    assert!(!d.b.is_faulted());
}

#[test]
fn tick_and_balls_in_play_read_the_view() {
    let mut d = Driver::new();
    for t in [0u64, 1, 123_456_789, (i64::MAX as u64)] {
        d.h.view.tick = t;
        assert_eq!(d.value(op::TICK, &[]), Some(t as i64));
    }
    for b in [0u8, 1, 6, 255] {
        d.h.view.balls_in_play = b;
        assert_eq!(d.value(op::BALLS_IN_PLAY, &[]), Some(i64::from(b)));
    }
    assert!(!d.b.is_faulted());
}

#[test]
fn random_draws_the_lua_sequence_from_the_host_pcg() {
    let mut d = Driver::new();
    let mut oracle = d.h.rng.clone();
    let ranges = [
        (1i64, 6i64),
        (0, 999),
        (-5, 5),
        (7, 7),
        (0, 1),
        (0, 255),
        (i64::MIN, i64::MAX),
        (i64::MIN, 0),
        (-1, i64::MAX),
        (0, 1 << 40),
        (i64::MAX - 2, i64::MAX),
        (i64::MIN, i64::MIN + 2),
    ];
    for round in 0..40 {
        for (m, n) in ranges {
            let want = lua_random(m, n, &mut oracle);
            assert_eq!(
                d.value(op::RANDOM, &[m, n]),
                Some(want),
                "random({m}, {n}) round {round}"
            );
            assert_eq!(d.h.rng, oracle, "PCG state after random({m}, {n})");
        }
    }
    assert!(!d.b.is_faulted());
}

#[test]
fn random_with_empty_interval_traps_without_drawing() {
    for (m, n) in [(1i64, 0i64), (6, 1), (i64::MAX, i64::MIN), (0, -1)] {
        let mut d = Driver::new();
        let before = d.h.rng.clone();
        let q = d.call(op::RANDOM, &[m, n]).clone();
        assert!(d.b.is_faulted(), "random({m}, {n}) must trap");
        assert!(q.is_empty());
        assert_eq!(d.h.rng, before, "an invalid random() must not consume the PCG");
    }
}

#[test]
fn id_of_resolves_every_kind() {
    let mut d = Driver::new();
    // (kind, ptr, len, expected id)
    let cases: [(i64, i64, i64, i64); 18] = [
        (0, 1024, 10, 7),   // sw.left_sling
        (1, 1024, 10, 3),   // coil.left_sling: same name, other kind
        (1, 1040, 12, 1),   // coil.trough_eject
        (2, 1056, 7, 200),  // lamp.jackpot
        (3, 1072, 6, 4),    // magnet.center
        (4, 1088, 9, 5),    // diverter.ramp_gate
        (5, 1104, 5, 6),    // flipper.upper
        (6, 1120, 6, 9),    // dispenser.trough
        (7, 1136, 7, 11),   // profile.ceramic
        (8, 1152, 5, 13),   // sound.drain
        (9, 1168, 9, 15),   // scene.multiball
        (10, 1184, 7, 16),  // persist_key.credits
        (0, 1216, 3, 255),  // sw.far
        (0, 65531, 5, 0),   // sw.start, the last bytes of memory
        // Unknown names are -1, not a fault.
        (0, 1200, 4, -1),   // "nope"
        (2, 1024, 10, -1),  // left_sling is not a lamp
        (6, 1120, 5, -1),   // "troug": a prefix is not a match
        (1, 1040, 6, -1),   // "trough" is a dispenser, not a coil
    ];
    for (kind, ptr, len, want) in cases {
        assert_eq!(
            d.value(op::ID_OF, &[kind, ptr, len]),
            Some(want),
            "id_of({kind}, {ptr}, {len})"
        );
    }
    // "trough_1" is not in memory; "trough_eject"[..8] = "trough_e" is unknown.
    assert_eq!(d.value(op::ID_OF, &[0, 1040, 8]), Some(-1));
    // Non-UTF-8 bytes name nothing.
    assert_eq!(d.value(op::ID_OF, &[0, 1232, 3]), Some(-1));
    assert!(!d.b.is_faulted(), "{:?}", d.b.last_error());
    assert!(d.b.last_error().is_none());
}

#[test]
fn id_of_with_out_of_bounds_pointer_traps() {
    // Memory is one page (65536 bytes).
    for (ptr, len) in [
        (65536i64, 1i64),
        (65531, 6),
        (65535, 2),
        (0, 65537),
        (-1, 1),
        (i32::MIN as i64, 4),
        (1024, -1),
        (i32::MAX as i64, i32::MAX as i64),
        (1_000_000, 5),
    ] {
        assert_faults(op::ID_OF, &[0, ptr, len]);
    }
}

#[test]
fn id_of_with_unknown_kind_never_resolves() {
    for kind in [11i64, 12, -1, i32::MAX as i64, i32::MIN as i64] {
        let mut d = Driver::new();
        let v = d.value(op::ID_OF, &[kind, 1024, 10]);
        // The brief leaves "trap or -1" open for an unknown kind; it must never
        // resolve to an id.
        assert!(
            d.b.is_faulted() || v == Some(-1),
            "id_of(kind {kind}) gave {v:?}"
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Unit-interval arguments: accepted iff in [0, 1] (NaN and infinities trap).
    #[test]
    fn unit_interval_arguments(bits in any::<u32>(), which in 0usize..6, pick in 0u8..4) {
        // Mix raw bit patterns with values near the interval.
        let x = match pick {
            0 => f32::from_bits(bits),
            1 => (bits as f32) / (u32::MAX as f32),
            2 => 1.0 + (bits as f32) / (u32::MAX as f32) * 1e-3,
            _ => -((bits as f32) / (u32::MAX as f32)) * 1e-3,
        };
        let valid = (0.0..=1.0).contains(&x);
        let mut d = Driver::new();
        let q = match which {
            0 => d.call(op::SET_LAMP, &[2, fa(x)]).clone(),
            1 => d.call(op::SET_MAGNET, &[4, fa(x)]).clone(),
            n => {
                let mut tint = [fa(0.5); 4];
                tint[n - 2] = fa(x);
                d.call(op::SET_GI_TINT, &tint).clone()
            }
        };
        prop_assert_eq!(d.b.is_faulted(), !valid, "x = {:?} ({:#x})", x, x.to_bits());
        if valid {
            match which {
                0 => prop_assert_eq!(q.lamps.as_slice(), &[(LampId(2), x)]),
                1 => prop_assert_eq!(q.magnets.as_slice(), &[(MagnetId(4), x)]),
                n => {
                    let mut tint = [0.5f32; 4];
                    tint[n - 2] = x;
                    prop_assert_eq!(q.gi_tint, Some(tint));
                }
            }
            prop_assert_eq!(q.len(), 1);
        } else {
            prop_assert!(q.is_empty());
        }
    }

    /// Gain: accepted iff finite and >= 0.
    #[test]
    fn sound_gain_is_finite_and_non_negative(bits in any::<u32>(), small in any::<bool>()) {
        let x = if small { (bits as f32 / 65536.0) - 100.0 } else { f32::from_bits(bits) };
        let valid = x.is_finite() && x >= 0.0;
        let mut d = Driver::new();
        let q = d.call(op::PLAY_SOUND, &[13, fa(x)]).clone();
        prop_assert_eq!(d.b.is_faulted(), !valid, "gain = {:?}", x);
        if valid {
            prop_assert_eq!(q.sounds.as_slice(), &[(SoundId(13), x)]);
        } else {
            prop_assert!(q.is_empty());
        }
    }

    /// random(m, n): the Lua draw for m <= n, a trap otherwise.
    #[test]
    fn random_matches_the_lua_oracle(seed in any::<u64>(), a in any::<i64>(), b in any::<i64>(),
                                     narrow in any::<bool>()) {
        let (m, n) = if narrow { (a % 1000, b % 1000) } else { (a, b) };
        let mut d = Driver::new();
        d.h.rng = Pcg32::seed_from_u64(seed);
        let mut oracle = Pcg32::seed_from_u64(seed);
        for _ in 0..4 {
            let got = d.value(op::RANDOM, &[m, n]);
            if m <= n {
                let want = lua_random(m, n, &mut oracle);
                prop_assert_eq!(got, Some(want));
                prop_assert!(m <= want && want <= n);
                prop_assert_eq!(&d.h.rng, &oracle);
            } else {
                prop_assert!(d.b.is_faulted());
                prop_assert_eq!(got, None);
                prop_assert_eq!(&d.h.rng, &oracle);
            }
        }
    }

    /// Integer arguments: accepted iff every one is in its documented range.
    #[test]
    fn integer_argument_ranges(which in 0usize..9, a in arg_i32(), b in arg_i32()) {
        let id = |v: i64| (0..=65535).contains(&v);
        let flag = |v: i64| v == 0 || v == 1;
        let (opcode, valid) = match which {
            0 => (op::PULSE_COIL, id(a) && id(b)),
            1 => (op::HOLD_COIL, id(a) && flag(b)),
            2 => (op::SET_ZIPPER, id(a) && flag(b)),
            3 => (op::SET_DIVERTER, id(a) && flag(b)),
            4 => (op::SPAWN_BALL, id(a) && id(b)),
            5 => (op::SET_SCORE, (1..=4).contains(&a)),
            6 => (op::SHOW_SCENE, id(a)),
            7 => (op::PERSIST_SET, (0..64).contains(&a)),
            _ => (op::SWITCH, id(a)),
        };
        let mut d = Driver::new();
        let q = d.call(opcode, &[a, b]).clone();
        prop_assert_eq!(d.b.is_faulted(), !valid, "op {} args ({}, {})", opcode, a, b);
        prop_assert_eq!(q.len(), usize::from(valid));
        prop_assert_eq!(d.b.fault_count(), u32::from(!valid));
    }

    /// Any operation with any argument bits: never panics, and whatever is
    /// queued is within the contract.
    #[test]
    fn arbitrary_calls_only_emit_in_contract_commands(
        calls in proptest::collection::vec((0u16..18, any::<[i64; 4]>(), any::<bool>()), 1..12)
    ) {
        let mut d = Driver::new();
        for (opcode, args, marked) in calls {
            let was_faulted = d.b.is_faulted();
            let q = if marked { d.call_marked(opcode, &args).clone() } else { d.call(opcode, &args).clone() };
            for &(_, v) in &q.lamps { prop_assert!((0.0..=1.0).contains(&v)); }
            for &(_, v) in &q.magnets { prop_assert!((0.0..=1.0).contains(&v)); }
            for &(_, v) in &q.sounds { prop_assert!(v.is_finite() && v >= 0.0); }
            if let Some(t) = q.gi_tint { prop_assert!(t.iter().all(|c| (0.0..=1.0).contains(c))); }
            for &(p, _) in &q.scores { prop_assert!(p < 4); }
            for &(k, _) in &q.persist_writes { prop_assert!(k.index() < 64); }
            if d.b.is_faulted() {
                // Only tilt releases may be queued by a faulted script.
                prop_assert_eq!(q.len(), q.coil_holds.len(), "{:?}", q);
                prop_assert!(q.coil_holds.iter().all(|&(_, on)| !on));
                if was_faulted { prop_assert!(q.is_empty()); }
            }
            prop_assert!(d.b.fault_count() <= 1);
            prop_assert_eq!(d.b.is_faulted(), d.b.fault_count() == 1);
        }
    }
}

/// i32-range values biased towards the interesting boundaries.
fn arg_i32() -> impl Strategy<Value = i64> {
    prop_oneof![
        (-3i64..70),
        (65530i64..65540),
        any::<i32>().prop_map(i64::from),
        Just(i64::from(i32::MIN)),
        Just(i64::from(i32::MAX)),
    ]
}

// ---------------------------------------------------------------- faults

#[test]
fn a_faulting_callback_is_rolled_back_and_tilts() {
    let mut d = Driver::new();
    // A healthy marked call queues exactly the markers plus the command.
    let q = d.call_marked(op::SHOW_SCENE, &[3]).clone();
    let mut want = markers();
    want.show_scene(SceneId(3)).unwrap();
    assert_eq!(q, want);
    assert!(!d.b.is_faulted());

    // Earlier callbacks of the same tick stay queued; the faulting callback's
    // own commands (markers included) are dropped.
    d.h.cmd.clear();
    d.call_keep(op::PULSE_COIL, &[1, 10]);
    d.call_keep(op::SET_GI_TINT, &[fa(1.0), fa(0.0), fa(1.0), fa(0.0)]);
    d.call_keep(op::SET_LAMP, &[5, fa(0.5)]);
    let before = d.h.cmd.clone();
    d.set_marked_fault();
    let q = d.h.cmd.clone();
    assert!(d.b.is_faulted());
    assert_eq!(d.b.fault_count(), 1);
    assert!(d.b.last_error().is_some());
    assert_eq!(q.coil_pulses, before.coil_pulses);
    assert_eq!(q.lamps, before.lamps);
    assert_eq!(q.gi_tint, before.gi_tint, "GI tint restored to its pre-callback value");
    assert!(q.scores.is_empty() && q.persist_writes.is_empty() && q.display_scenes.is_empty());
    // Coil 8 was held by the first (healthy) marked call: it is released.
    assert_eq!(q.coil_holds.as_slice(), &[(CoilId(8), false)]);

    // Later callbacks are no-ops and the fault count stays.
    for _ in 0..5 {
        assert!(d.tick().is_empty());
        assert!(d.call(op::PULSE_COIL, &[1, 1]).is_empty());
        assert!(d.call_marked(op::SHOW_SCENE, &[1]).is_empty());
    }
    d.h.cmd.clear();
    d.h.init(&mut d.b);
    assert!(d.h.cmd.is_empty());
    assert!(d.b.is_faulted());
    assert_eq!(d.b.fault_count(), 1);
}

impl Driver {
    /// An `Opened` (marked) call that faults on a bad coil id, queue kept.
    fn set_marked_fault(&mut self) {
        self.h.view.persist[0] = 70000;
        self.h.view.persist[1] = 1;
        self.h.edge_keep(&mut self.b, op::PULSE_COIL, false);
    }
}

#[test]
fn a_hold_queued_by_the_faulting_callback_is_released() {
    // Lua rollback semantics: a dropped hold command marks its coil held so the
    // tilt release covers it (a dropped release must never strand a coil).
    let mut d = Driver::new();
    d.h.cmd.clear();
    d.set_marked_fault();
    assert!(d.b.is_faulted());
    assert_eq!(d.h.cmd.coil_holds.as_slice(), &[(CoilId(8), false)]);
    assert_eq!(d.h.cmd.len(), 1);
    assert!(d.tick().is_empty());
}

#[test]
fn held_coils_are_released_once_and_released_coils_are_not() {
    let mut d = Driver::new();
    d.call(op::HOLD_COIL, &[5, 1]);
    d.call(op::HOLD_COIL, &[65535, 1]);
    d.call(op::HOLD_COIL, &[6, 1]);
    d.call(op::HOLD_COIL, &[6, 0]);
    d.call(op::HOLD_COIL, &[0, 1]);
    let q = d.call(op::UNREACHABLE, &[]).clone();
    assert!(d.b.is_faulted());
    let mut released: Vec<_> = q.coil_holds.iter().copied().collect();
    released.sort_by_key(|&(c, _)| c.0);
    assert_eq!(
        released,
        vec![(CoilId(0), false), (CoilId(5), false), (CoilId(65535), false)]
    );
    assert_eq!(q.len(), 3);
    assert!(d.tick().is_empty());
    assert!(d.call(op::HOLD_COIL, &[1, 1]).is_empty());
}

#[test]
fn more_held_coils_than_the_queue_holds_are_released_over_later_callbacks() {
    let mut d = Driver::new();
    assert_eq!(d.call(op::HOLD_RANGE, &[1000, 32]).coil_holds.len(), 32);
    assert_eq!(d.call(op::HOLD_RANGE, &[2000, 30]).coil_holds.len(), 30);
    assert_eq!(d.b.overflow_count(), 0);
    let mut released = Vec::new();
    let q = d.call(op::DIV_ZERO, &[0, 0, 0, 0]).clone();
    assert!(d.b.is_faulted());
    assert_eq!(q.coil_holds.len(), 32);
    released.extend(q.coil_holds.iter().copied());
    let q = d.tick().clone();
    assert_eq!(q.coil_holds.len(), 30);
    assert_eq!(q.len(), 30);
    released.extend(q.coil_holds.iter().copied());
    assert!(d.tick().is_empty());
    assert!(released.iter().all(|&(_, on)| !on));
    let mut ids: Vec<u16> = released.iter().map(|&(c, _)| c.0).collect();
    ids.sort_unstable();
    let want: Vec<u16> = (1000..1032).chain(2000..2030).collect();
    assert_eq!(ids, want);
    assert_eq!(d.b.overflow_count(), 0, "tilt releases are not script overflows");
}

#[test]
fn queue_overflow_drops_and_counts_but_does_not_fault() {
    let mut d = Driver::new();
    let q = d.call(op::PULSE_40, &[]).clone();
    assert!(!d.b.is_faulted());
    assert_eq!(q.coil_pulses.len(), 32);
    for (i, &(c, ms)) in q.coil_pulses.iter().enumerate() {
        assert_eq!((c, usize::from(ms)), (CoilId(2), i), "the first 32 are kept in order");
    }
    assert_eq!(d.b.overflow_count(), 8);
    d.call(op::PULSE_40, &[]);
    assert_eq!(d.b.overflow_count(), 16);
    // Without clearing: everything overflows.
    d.call_keep(op::PULSE_40, &[]);
    assert_eq!(d.b.overflow_count(), 56);
    assert!(!d.b.is_faulted());
    assert_eq!(d.b.fault_count(), 0);
}

#[test]
fn overflows_of_a_faulting_callback_are_not_counted() {
    // Lua semantics: the rollback restores the overflow counter.
    let mut d = Driver::new();
    d.call(op::PULSE_40, &[]);
    assert_eq!(d.b.overflow_count(), 8);
    let q = d.call(op::SPIN_CALLING_HOST, &[]).clone();
    assert!(d.b.is_faulted());
    assert!(q.is_empty(), "{q:?}");
    assert_eq!(d.b.overflow_count(), 8);
}

#[test]
fn guest_traps_fault_the_script() {
    for (opcode, args) in [
        (op::UNREACHABLE, [0i64; 4]),
        (op::DIV_ZERO, [0; 4]),
        (op::OOB_LOAD, [65536, 0, 0, 0]),
        (op::OOB_LOAD, [65529, 0, 0, 0]),
        (op::OOB_LOAD, [-1, 0, 0, 0]),
        (op::OOB_STORE, [65536, 0, 0, 0]),
        (op::OOB_STORE, [i32::MIN as i64, 0, 0, 0]),
        (op::NULL_INDIRECT, [0; 4]),
        (op::RECURSE, [0; 4]),
    ] {
        let mut d = Driver::new();
        let q = d.call_marked(opcode, &args).clone();
        assert!(d.b.is_faulted(), "op {opcode} {args:?} must fault");
        assert_eq!(d.b.fault_count(), 1);
        let err = d.b.last_error().expect("a fault records its error").clone();
        assert!(!err.to_string().is_empty());
        assert!(
            !matches!(err, ScriptError::UnsupportedBackend(_) | ScriptError::Load(_)),
            "op {opcode}: {err:?} is not a runtime failure"
        );
        // Rolled back; only the marker's hold is released.
        assert_eq!(q.coil_holds.as_slice(), &[(CoilId(8), false)], "op {opcode}");
        assert_eq!(q.len(), 1, "op {opcode}: {q:?}");
        assert!(d.tick().is_empty());
    }
    // The in-bounds neighbours of the trapping accesses work.
    let mut d = Driver::new();
    assert!(d.value(op::OOB_LOAD, &[65528]).is_some());
    d.call(op::OOB_STORE, &[65528]);
    assert_eq!(d.value(op::OOB_LOAD, &[65528]), Some(-1));
    assert_eq!(d.value(op::DIV_ZERO, &[0, 0, 0, 1]), Some(1));
    assert!(!d.b.is_faulted());
}

#[test]
fn a_fault_in_on_init_tilts_too() {
    let src = guest(
        "(call $hold_coil (i32.const 4) (i32.const 1))
         (call $show_scene (i32.const 2))
         (call $set_score (i32.const 9) (i64.const 1))",
        "(call $show_scene (i32.const 7))",
        "(call $show_scene (i32.const 8))",
    );
    let mut b = backend(&src);
    let mut h = Host::new(3);
    h.init(&mut b);
    assert!(b.is_faulted());
    assert_eq!(h.cmd.coil_holds.as_slice(), &[(CoilId(4), false)]);
    assert_eq!(h.cmd.len(), 1);
    h.tick(&mut b);
    assert!(h.cmd.is_empty());
    h.edge(&mut b, 1, true);
    assert!(h.cmd.is_empty());
    assert_eq!(b.fault_count(), 1);
}

#[test]
fn nan_results_are_canonical() {
    // Config::cranelift_nan_canonicalization(true): arithmetic on a NaN with a
    // payload yields the canonical quiet NaN on every platform.
    let mut d = Driver::new();
    for bits in [0x7fa0_0001u32, 0x7fc1_2345, 0xffc0_0001, 0xff80_0001, 0x7fff_ffff] {
        let got = d.value(op::NAN_ADD, &[i64::from(bits), fa(1.0)]).unwrap();
        assert_eq!(got, 0x7fc0_0000, "NaN {bits:#x} + 1.0 gave {got:#x}");
    }
    let got = d
        .value(op::NAN_ADD, &[fa(f32::INFINITY), fa(f32::NEG_INFINITY)])
        .unwrap();
    assert_eq!(got, 0x7fc0_0000);
    assert_eq!(d.value(op::NAN_ADD, &[fa(1.5), fa(2.25)]), Some(fa(3.75)));
}

#[test]
fn binary_and_text_modules_behave_identically() {
    assert_eq!(&support::DRIVER_WASM[..4], b"\0asm");
    let mut t = Driver::new();
    let mut b = Driver::binary();
    let script: [(u16, [i64; 4], bool); 12] = [
        (op::PULSE_COIL, [3, 30, 0, 0], false),
        (op::HOLD_COIL, [3, 1, 0, 0], true),
        (op::RANDOM, [1, 6, 0, 0], false),
        (op::RANDOM, [-100, 100, 0, 0], true),
        (op::ID_OF, [1, 1040, 12, 0], false),
        (op::SET_GI_TINT, [fa(0.25), fa(0.5), fa(0.75), fa(1.0)], false),
        (op::PULSE_40, [0; 4], false),
        (op::PLAY_SOUND, [12, fa(0.5), 0, 0], true),
        (op::SET_SCORE, [4, -9, 0, 0], false),
        (op::SET_LAMP, [2, fa(2.0), 0, 0], true), // faults
        (op::SHOW_SCENE, [1, 0, 0, 0], false),
        (op::HOLD_COIL, [3, 1, 0, 0], false),
    ];
    for (opcode, args, marked) in script {
        let (qt, qb) = if marked {
            (t.call_marked(opcode, &args).clone(), b.call_marked(opcode, &args).clone())
        } else {
            (t.call(opcode, &args).clone(), b.call(opcode, &args).clone())
        };
        assert_eq!(qt, qb, "op {opcode}");
        assert_eq!(t.h.rng, b.h.rng);
        assert_eq!(t.b.is_faulted(), b.b.is_faulted());
        assert_eq!(t.b.overflow_count(), b.b.overflow_count());
        assert_eq!(t.tick(), b.tick());
    }
    assert!(t.b.is_faulted() && b.b.is_faulted());
}

#[test]
fn backend_is_send_and_usable_as_a_trait_object() {
    fn assert_send<T: Send>() {}
    assert_send::<WasmScriptBackend>();
    let b = backend(support::DRIVER_WAT);
    let mut boxed: Box<dyn pin_core::TableScriptBackend> = Box::new(b);
    let q = std::thread::spawn(move || {
        let mut h = Host::new(1);
        h.tick(boxed.as_mut());
        h.cmd
    })
    .join()
    .unwrap();
    assert_eq!(q.display_scenes.as_slice(), &[SceneId(7)]);
    let _ = names();
}
