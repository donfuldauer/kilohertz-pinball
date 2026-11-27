//! REQ-WEB-001, D10, R3: `pin_script::rng` equals a reference model of the
//! Lua 5.4 `project` algorithm, and (Lua backend on) `math.random`.

use pin_core::{Pcg32, Rng, SeedableRng};
use pin_script::rng;
use proptest::prelude::*;

fn ref_project(ran: u64, n: u64, r: &mut Pcg32) -> u64 {
    if n & n.wrapping_add(1) == 0 {
        return ran & n;
    }
    let mut lim = n;
    lim |= lim >> 1;
    lim |= lim >> 2;
    lim |= lim >> 4;
    lim |= lim >> 8;
    lim |= lim >> 16;
    lim |= lim >> 32;
    let mut ran = ran & lim;
    while ran > n {
        ran = r.next_u64() & lim;
    }
    ran
}

fn ref_range(lo: i64, hi: i64, r: &mut Pcg32) -> i64 {
    let first = r.next_u64();
    let span = hi.wrapping_sub(lo) as u64;
    lo.wrapping_add(ref_project(first, span, r) as i64)
}

fn ref_unit(r: &mut Pcg32) -> f64 {
    (r.next_u64() >> 11) as f64 * (1.0 / 9007199254740992.0)
}

fn bound() -> impl Strategy<Value = i64> {
    prop_oneof![
        any::<i64>(),
        -1000i64..1000,
        Just(i64::MIN),
        Just(i64::MAX),
        Just(0),
        (0u32..63).prop_map(|k| (1i64 << k) - 1),
        (0u32..63).prop_map(|k| 1i64 << k),
    ]
}

proptest! {
    #[test]
    fn req_web_001_range_matches_reference(seed in any::<u64>(), a in bound(), b in bound()) {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        let mut x = Pcg32::seed_from_u64(seed);
        let mut y = Pcg32::seed_from_u64(seed);
        let got = rng::range(lo, hi, &mut x);
        prop_assert_eq!(got, ref_range(lo, hi, &mut y));
        prop_assert!(got >= lo && got <= hi);
        prop_assert_eq!(x.next_u64(), y.next_u64(), "generator state diverged");
    }

    #[test]
    fn req_web_001_project_matches_reference(seed in any::<u64>(), ran in any::<u64>(), n in prop_oneof![any::<u64>(), 0u64..70, (0u32..64).prop_map(|k| (1u64 << k).wrapping_sub(1)), (0u32..64).prop_map(|k| 1u64 << k)]) {
        let mut x = Pcg32::seed_from_u64(seed);
        let mut y = Pcg32::seed_from_u64(seed);
        let got = rng::project(ran, n, &mut x);
        prop_assert_eq!(got, ref_project(ran, n, &mut y));
        prop_assert!(got <= n);
        prop_assert_eq!(x.next_u64(), y.next_u64());
    }

    #[test]
    fn req_web_001_unit_float_matches_reference(seed in any::<u64>()) {
        let mut x = Pcg32::seed_from_u64(seed);
        let mut y = Pcg32::seed_from_u64(seed);
        for _ in 0..8 {
            let f = rng::unit_float(&mut x);
            prop_assert_eq!(f.to_bits(), ref_unit(&mut y).to_bits());
            prop_assert!((0.0..1.0).contains(&f));
        }
        prop_assert_eq!(x.next_u64(), y.next_u64());
    }
}

#[test]
fn req_web_001_edge_ranges() {
    let cases = [
        (0i64, 0i64),
        (5, 5),
        (i64::MIN, i64::MIN),
        (i64::MAX, i64::MAX),
        (i64::MIN, i64::MAX),
        (i64::MIN, 0),
        (0, i64::MAX),
        (1, 6),
        (0, 7),
        (0, 8),
        (-3, 4),
    ];
    for seed in 0..50u64 {
        for (lo, hi) in cases {
            let mut x = Pcg32::seed_from_u64(seed);
            let mut y = Pcg32::seed_from_u64(seed);
            let got = rng::range(lo, hi, &mut x);
            assert_eq!(got, ref_range(lo, hi, &mut y), "{lo}..={hi} seed {seed}");
            assert!(got >= lo && got <= hi);
            assert_eq!(x.next_u64(), y.next_u64());
        }
    }
}

#[test]
fn req_web_001_all_ones_span_consumes_exactly_one_draw() {
    for k in 0..63u32 {
        let hi = (1i64 << k) - 1;
        let mut x = Pcg32::seed_from_u64(9);
        let mut y = Pcg32::seed_from_u64(9);
        rng::range(0, hi, &mut x);
        y.next_u64();
        assert_eq!(x.next_u64(), y.next_u64(), "span 2^{k}-1 must draw once");
    }
    let mut x = Pcg32::seed_from_u64(3);
    let mut y = Pcg32::seed_from_u64(3);
    assert_eq!(rng::project(0xFFFF, 7, &mut x), 7);
    assert_eq!(x.next_u64(), y.next_u64(), "project of 2^k-1 draws nothing");
}

#[test]
fn req_web_001_same_seed_same_sequence_different_seed_differs() {
    let seq = |seed: u64| {
        let mut r = Pcg32::seed_from_u64(seed);
        (0..8)
            .map(|_| rng::range(1, 1_000_000, &mut r))
            .collect::<Vec<_>>()
    };
    assert_eq!(seq(11), seq(11));
    assert_ne!(seq(11), seq(12));
    assert_ne!(seq(0), seq(1));
}

#[test]
fn req_web_001_unit_float_in_unit_interval_53_bit() {
    let mut x = Pcg32::seed_from_u64(2024);
    let mut y = Pcg32::seed_from_u64(2024);
    for _ in 0..10_000 {
        let f = rng::unit_float(&mut x);
        assert!((0.0..1.0).contains(&f), "{f}");
        let m = f * 9007199254740992.0;
        assert_eq!(m, m.trunc(), "result must be a multiple of 2^-53");
        assert_eq!(f.to_bits(), ref_unit(&mut y).to_bits());
    }
}

#[cfg(feature = "lua")]
mod parity {
    use super::*;
    use pin_core::{HostCommandQueue, HostCtx, HostView, PersistKey};
    use pin_script::{NameTables, load_backend};

    const SRC: &[u8] = b"
function on_tick()
  pinball.persist_set(0, math.random(6))
  pinball.persist_set(1, math.random(10, 20))
  pinball.persist_set(2, math.random(-5, 5))
  pinball.persist_set(3, math.random(1, 1000000007))
  pinball.persist_set(4, math.random(7))
  pinball.persist_set(5, math.floor(math.random() * 9007199254740992))
  pinball.persist_set(6, math.random(0))
end
";

    fn val(cmd: &HostCommandQueue, key: u16) -> i64 {
        cmd.persist_writes
            .iter()
            .rev()
            .find(|(k, _)| *k == PersistKey(key))
            .map(|&(_, v)| v)
            .expect("persist write present")
    }

    /// REQ-WEB-001, D10, R3: Lua `math.random` equals `pin_script::rng`.
    #[test]
    fn req_web_001_math_random_matches_rng_module() {
        for seed in [0u64, 1, 42, 0xDEAD_BEEF, u64::MAX] {
            let mut b = match load_backend("parity.lua", SRC, &NameTables::default()) {
                Ok(b) => b,
                Err(e) => panic!("{e}"),
            };
            let view = HostView::default();
            let mut host_rng = Pcg32::seed_from_u64(seed);
            let mut ref_rng = Pcg32::seed_from_u64(seed);
            for _ in 0..200 {
                let mut cmd = HostCommandQueue::default();
                b.on_tick(&mut HostCtx {
                    view: &view,
                    rng: &mut host_rng,
                    cmd: &mut cmd,
                });
                assert_eq!(val(&cmd, 0), rng::range(1, 6, &mut ref_rng));
                assert_eq!(val(&cmd, 1), rng::range(10, 20, &mut ref_rng));
                assert_eq!(val(&cmd, 2), rng::range(-5, 5, &mut ref_rng));
                assert_eq!(val(&cmd, 3), rng::range(1, 1_000_000_007, &mut ref_rng));
                assert_eq!(val(&cmd, 4), rng::range(1, 7, &mut ref_rng));
                let f = rng::unit_float(&mut ref_rng);
                assert_eq!(val(&cmd, 5), (f * 9007199254740992.0).floor() as i64);
                // math.random(0): a random integer (all bits).
                assert_eq!(val(&cmd, 6), ref_rng.next_u64() as i64);
                assert_eq!(host_rng, ref_rng, "generator state diverged");
            }
        }
    }
}
