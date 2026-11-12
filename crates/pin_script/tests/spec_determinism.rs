//! Spec: `pinball.random` and `math.random` draw from the host `Pcg32`, so the
//! same seed and the same inputs always produce identical command queues.
//! Also: arbitrary arguments to the API never panic or emit out-of-contract
//! commands.

mod spec_support;

use pin_core::{HostCommandQueue, Pcg32, SeedableRng};
use proptest::prelude::*;
use spec_support::{Host, backend, lamp, persisted};

const RANDOM: &[u8] = include_bytes!("fixtures/spec_random.lua");

#[derive(Clone, Debug)]
enum Input {
    Tick,
    Edge(u16, bool),
}

/// Runs `inputs` against a fresh backend and records the queue after each callback.
fn run(source: &[u8], seed: u64, inputs: &[Input]) -> (Vec<HostCommandQueue>, Pcg32) {
    let mut b = backend(source);
    let mut h = Host::new(seed);
    let mut out = Vec::with_capacity(inputs.len() + 1);
    h.init(&mut b);
    out.push(h.cmd.clone());
    for input in inputs {
        match *input {
            Input::Tick => h.tick(&mut b),
            Input::Edge(sw, closed) => h.edge(&mut b, sw, closed),
        }
        out.push(h.cmd.clone());
    }
    assert!(!b.is_faulted(), "the random fixture must never fault");
    assert_eq!(b.overflow_count(), 0);
    (out, h.rng)
}

fn script() -> Vec<Input> {
    let mut v = Vec::new();
    for i in 0..300u16 {
        v.push(Input::Tick);
        if i % 3 == 0 {
            v.push(Input::Edge(i % 40, true));
        }
        if i % 5 == 0 {
            v.push(Input::Edge(i % 40, false));
        }
    }
    v
}

#[test]
fn same_seed_and_inputs_give_identical_queues() {
    let inputs = script();
    for seed in [0u64, 1, 42, 0xDEAD_BEEF, u64::MAX] {
        let (a, rng_a) = run(RANDOM, seed, &inputs);
        let (b, rng_b) = run(RANDOM, seed, &inputs);
        assert_eq!(a, b, "seed {seed}");
        assert_eq!(rng_a, rng_b, "generator state diverged for seed {seed}");
    }
}

#[test]
fn different_seeds_give_different_queues() {
    let inputs = script();
    let (a, _) = run(RANDOM, 1, &inputs);
    let (b, _) = run(RANDOM, 2, &inputs);
    assert_ne!(
        a, b,
        "the script's randomness does not depend on the host seed"
    );
}

#[test]
fn pinball_random_alone_depends_on_the_host_seed() {
    let src = b"function on_tick() pinball.persist_set(0, pinball.random(1000000000)) end";
    let inputs = vec![Input::Tick; 50];
    assert_eq!(run(src, 9, &inputs).0, run(src, 9, &inputs).0);
    assert_ne!(run(src, 9, &inputs).0, run(src, 10, &inputs).0);
}

#[test]
fn math_random_alone_depends_on_the_host_seed() {
    for src in [
        &b"function on_tick() pinball.persist_set(0, math.random(1000000000)) end"[..],
        b"function on_tick() pinball.persist_set(0, math.random(1, 1000000000)) end",
        b"function on_tick() pinball.persist_set(0, math.floor(math.random() * 1000000000)) end",
    ] {
        let inputs = vec![Input::Tick; 50];
        // Lua 5.4 seeds its own generator randomly at start-up: only routing
        // through the host generator makes two runs agree.
        assert_eq!(run(src, 9, &inputs).0, run(src, 9, &inputs).0);
        assert_ne!(run(src, 9, &inputs).0, run(src, 10, &inputs).0);
    }
}

#[test]
fn math_randomseed_cannot_detach_the_script_from_the_host_generator() {
    let src = b"function on_tick()
                  pcall(function() math.randomseed(42) end)
                  pinball.persist_set(0, math.random(1000000000))
                  pinball.persist_set(1, pinball.random(1000000000))
                end";
    let inputs = vec![Input::Tick; 50];
    let (a, _) = run(src, 9, &inputs);
    let (b, _) = run(src, 10, &inputs);
    assert_eq!(a, run(src, 9, &inputs).0);
    assert_ne!(a, b, "math.randomseed replaced the host generator");
    // Reseeding every tick must not make every tick draw the same number.
    let firsts: std::collections::BTreeSet<_> =
        a.iter().skip(1).map(|q| persisted(q, 0).unwrap()).collect();
    assert!(
        firsts.len() > 40,
        "math.random repeats after math.randomseed: {firsts:?}"
    );
}

#[test]
fn drawing_advances_the_host_generator() {
    let mut b = backend(RANDOM);
    let mut h = Host::new(5);
    h.tick(&mut b);
    assert_ne!(
        h.rng,
        Pcg32::seed_from_u64(5),
        "random() did not touch HostCtx.rng"
    );

    let src = b"function on_tick() pinball.persist_set(0, math.random(100)) end";
    let mut b = backend(src);
    let mut h = Host::new(5);
    h.tick(&mut b);
    assert_ne!(
        h.rng,
        Pcg32::seed_from_u64(5),
        "math.random did not touch HostCtx.rng"
    );
}

/// The backend keeps no generator of its own: what a callback draws depends
/// only on the generator handed in with that callback.
#[test]
fn randomness_comes_from_the_generator_passed_to_each_callback() {
    // Backend X has already run under another seed; Y is fresh.
    let mut x = backend(RANDOM);
    let mut hx = Host::new(1);
    for _ in 0..25 {
        hx.tick(&mut x);
        hx.edge(&mut x, 3, true);
    }
    let mut y = backend(RANDOM);
    let mut hy = Host::new(2);

    hx.rng = Pcg32::seed_from_u64(777);
    hy.rng = Pcg32::seed_from_u64(777);
    for _ in 0..25 {
        hx.tick(&mut x);
        hy.tick(&mut y);
        assert_eq!(hx.cmd, hy.cmd);
        hx.edge(&mut x, 9, false);
        hy.edge(&mut y, 9, false);
        assert_eq!(hx.cmd, hy.cmd);
    }
    assert_eq!(hx.rng, hy.rng);
}

#[test]
fn random_ranges_are_inclusive_and_fully_covered() {
    let inputs = vec![Input::Tick; 3000];
    let (queues, _) = run(RANDOM, 2024, &inputs);
    let mut seen_d6 = [false; 7];
    let mut seen_range = [false; 21];
    let mut seen_m8 = [false; 9];
    let mut seen_m100 = [false; 106];
    let mut seen_neg = [false; 5];
    let mut seen_sym = [false; 5];
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for q in queues.iter().skip(1) {
        let p = &q.coil_pulses;
        assert_eq!(p.len(), 4);
        let (d6, r, m8, m100) = (p[0].0.0, p[1].0.0, p[2].0.0, p[3].0.0);
        assert!((1..=6).contains(&d6), "random(6) gave {d6}");
        assert!((10..=20).contains(&r), "random(10, 20) gave {r}");
        assert!((1..=8).contains(&m8), "math.random(8) gave {m8}");
        assert!(
            (100..=105).contains(&m100),
            "math.random(100, 105) gave {m100}"
        );
        seen_d6[d6 as usize] = true;
        seen_range[r as usize] = true;
        seen_m8[m8 as usize] = true;
        seen_m100[m100 as usize] = true;

        let f = lamp(q, 0).unwrap();
        // The f64 draw is in [0, 1); narrowing to f32 may round up to 1.0.
        assert!((0.0..=1.0).contains(&f), "math.random() gave {f}");
        lo = lo.min(f);
        hi = hi.max(f);

        let big = persisted(q, 0).unwrap();
        assert!((1..=1_000_000).contains(&big));
        let big = persisted(q, 1).unwrap();
        assert!((1..=1_000_000).contains(&big));
        assert_eq!(persisted(q, 2), Some(1), "random(1) is always 1");
        assert_eq!(persisted(q, 3), Some(5), "random(5, 5) is always 5");
        let neg = persisted(q, 4).unwrap();
        assert!((-5..=-1).contains(&neg), "random(-5, -1) gave {neg}");
        seen_neg[(neg + 5) as usize] = true;
        let sym = persisted(q, 5).unwrap();
        assert!((-2..=2).contains(&sym), "math.random(-2, 2) gave {sym}");
        seen_sym[(sym + 2) as usize] = true;
        assert_eq!(persisted(q, 6), Some(1), "integer/float result types");
    }
    assert!(
        seen_d6[1..].iter().all(|&s| s),
        "random(6) never produced some face"
    );
    assert!(
        seen_range[10..].iter().all(|&s| s),
        "random(10, 20) missed a value"
    );
    assert!(
        seen_m8[1..].iter().all(|&s| s),
        "math.random(8) missed a value"
    );
    assert!(
        seen_m100[100..].iter().all(|&s| s),
        "math.random(100, 105) missed a value"
    );
    assert!(seen_neg.iter().all(|&s| s), "random(-5, -1) missed a value");
    assert!(
        seen_sym.iter().all(|&s| s),
        "math.random(-2, 2) missed a value"
    );
    assert!(
        lo < 0.05 && hi > 0.95,
        "math.random() is not spread over [0, 1): {lo}..{hi}"
    );
}

#[test]
fn random_is_roughly_uniform() {
    let src = b"function on_tick()
                  for i = 1, 30 do pinball.pulse_coil(pinball.random(6), 1) end
                end";
    let (queues, _) = run(src, 31337, &vec![Input::Tick; 400]);
    let mut counts = [0u32; 7];
    for q in queues.iter().skip(1) {
        for &(c, _) in &q.coil_pulses {
            counts[c.0 as usize] += 1;
        }
    }
    // 12 000 draws, 2 000 expected per face; a fixed seed makes this exact, the
    // band only rejects a broken mapping (e.g. modulo of a tiny range).
    for (face, &n) in counts.iter().enumerate().skip(1) {
        assert!(
            (1700..=2300).contains(&n),
            "face {face} drawn {n} times of 12000"
        );
    }
}

#[test]
fn invalid_random_ranges_fault_like_any_argument_error() {
    for call in [
        "math.random(0, -1)",
        "math.random(5, 1)",
        "math.random(1, 2, 3)",
    ] {
        let src = format!("function on_tick() pinball.persist_set(0, {call}) end");
        let mut b = backend(src.as_bytes());
        let mut h = Host::new(1);
        h.tick(&mut b);
        assert!(b.is_faulted(), "`{call}` is an error in Lua 5.4");
        assert!(h.cmd.persist_writes.is_empty());
    }
}

#[test]
fn determinism_holds_across_reload() {
    let run_with_reload = |seed: u64| {
        let mut b = backend(RANDOM);
        let mut h = Host::new(seed);
        let mut out = Vec::new();
        for i in 0..60 {
            if i == 30 {
                pin_core::TableScriptBackend::apply_reload(&mut b, RANDOM).unwrap();
            }
            h.tick(&mut b);
            out.push(h.cmd.clone());
        }
        out
    };
    assert_eq!(run_with_reload(4), run_with_reload(4));
    assert_ne!(run_with_reload(4), run_with_reload(5));
    // A reload does not disturb the random stream either.
    let plain = run(RANDOM, 4, &vec![Input::Tick; 60]).0;
    assert_eq!(&plain[1..], run_with_reload(4).as_slice());
}

fn input_strategy() -> impl Strategy<Value = Input> {
    prop_oneof![
        Just(Input::Tick),
        (0u16..300, any::<bool>()).prop_map(|(s, c)| Input::Edge(s, c)),
    ]
}

/// Lua expressions used as arbitrary arguments.
const ARGS: &[&str] = &[
    "nil",
    "true",
    "false",
    "0",
    "1",
    "-1",
    "2",
    "4",
    "5",
    "0.5",
    "1.5",
    "-0.5",
    "3.0",
    "255",
    "256",
    "65535",
    "65536",
    "70000",
    "-70000",
    "1e308",
    "-1e308",
    "0/0",
    "math.huge",
    "-math.huge",
    "math.maxinteger",
    "math.mininteger",
    "2^53",
    "'x'",
    "'3'",
    "{}",
    "print",
    "pinball",
    "pinball.sw",
];

const FUNCS: &[&str] = &[
    "pulse_coil",
    "hold_coil",
    "set_magnet",
    "set_lamp",
    "set_gi_tint",
    "set_score",
    "spawn_ball",
    "set_diverter",
    "set_zipper",
    "show_scene",
    "play_sound",
    "switch",
    "tick",
    "balls_in_play",
    "random",
    "persist_get",
    "persist_set",
];

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn any_seed_and_input_sequence_is_deterministic(
        seed in any::<u64>(),
        inputs in proptest::collection::vec(input_strategy(), 0..60),
    ) {
        let (a, rng_a) = run(RANDOM, seed, &inputs);
        let (b, rng_b) = run(RANDOM, seed, &inputs);
        prop_assert_eq!(a, b);
        prop_assert_eq!(rng_a, rng_b);
    }

    #[test]
    fn random_stays_in_range_for_any_bounds(
        seed in any::<u64>(),
        m in -1_000_000i64..1_000_000,
        span in 0i64..1_000_000,
    ) {
        let n = m + span;
        let src = format!(
            "function on_tick()
               pinball.persist_set(0, pinball.random({m}, {n}))
               pinball.persist_set(1, math.random({m}, {n}))
               pinball.persist_set(2, pinball.random({}))
               pinball.persist_set(3, math.random({}))
             end",
            span + 1,
            span + 1
        );
        let mut b = backend(src.as_bytes());
        let mut h = Host::new(seed);
        for _ in 0..8 {
            h.tick(&mut b);
            prop_assert!(!b.is_faulted());
            for key in [0, 1] {
                let v = persisted(&h.cmd, key).unwrap();
                prop_assert!((m..=n).contains(&v), "random({}, {}) gave {}", m, n, v);
            }
            for key in [2, 3] {
                let v = persisted(&h.cmd, key).unwrap();
                prop_assert!((1..=span + 1).contains(&v), "random({}) gave {}", span + 1, v);
            }
        }
    }

    /// Whatever a script passes, the host neither panics nor emits a command
    /// outside the documented ranges; a rejected call faults the script.
    #[test]
    fn arbitrary_arguments_never_panic_or_break_the_queue_contract(
        func in proptest::sample::select(FUNCS),
        args in proptest::collection::vec(proptest::sample::select(ARGS), 0..6),
        seed in any::<u64>(),
    ) {
        let src = format!(
            "function on_tick() local r = pinball.{func}({}) pinball.show_scene(1) end",
            args.join(", ")
        );
        let mut b = backend(src.as_bytes());
        let mut h = Host::new(seed);
        h.tick(&mut b);

        for &(_, power) in &h.cmd.magnets {
            prop_assert!((0.0..=1.0).contains(&power), "magnet power {} from {}", power, src);
        }
        for &(_, intensity) in &h.cmd.lamps {
            prop_assert!((0.0..=1.0).contains(&intensity), "lamp {} from {}", intensity, src);
        }
        for &(player, _) in &h.cmd.scores {
            prop_assert!(player < 4, "player index {} from {}", player, src);
        }
        if b.is_faulted() {
            // The call raised: the rest of the callback did not run.
            prop_assert!(h.cmd.display_scenes.is_empty(), "callback continued after {}", src);
            prop_assert_eq!(b.fault_count(), 1);
        } else {
            prop_assert_eq!(h.cmd.display_scenes.len(), 1 + usize::from(func == "show_scene"));
        }

        // The backend stays usable either way.
        prop_assert!(pin_core::TableScriptBackend::apply_reload(
            &mut b, b"function on_tick() pinball.show_scene(2) end").is_ok());
        h.tick(&mut b);
        prop_assert_eq!(h.cmd.display_scenes.len(), 1);
    }
}
