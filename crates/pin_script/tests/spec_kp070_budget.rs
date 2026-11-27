//! REQ-WEB-001, D45, R11: the shared per-tick budget does exact, bounded arithmetic.
//! Ungated: builds under every feature combination.

use pin_script::budget::{TICK_BUDGET_UNITS, TickBudget};
use proptest::prelude::*;

const FULL: u64 = 200_000;

fn partial(used: u64) -> TickBudget {
    let mut b = TickBudget::new();
    b.consume(used);
    b
}

#[test]
fn req_web_001_budget_starts_full() {
    assert_eq!(TICK_BUDGET_UNITS, FULL);
    let b = TickBudget::new();
    assert_eq!(b.left(), FULL);
    assert_eq!(TickBudget::default(), b);
    assert!(!b.is_exhausted());
}

#[test]
fn req_web_001_grant_is_min_of_request_and_left() {
    for used in [0u64, 1, 99_999, 100_000, 199_999] {
        let b = partial(used);
        let left = b.left();
        assert_eq!(left, FULL - used);
        for x in [
            0,
            1,
            left.saturating_sub(1),
            left,
            left + 1,
            100_000,
            u64::MAX,
        ] {
            let before = b;
            assert_eq!(b.grant(x), x.min(left), "used {used} x {x}");
            assert_eq!(b, before, "grant must not change the state");
        }
    }
}

#[test]
fn req_web_001_consume_subtracts_and_saturates() {
    let mut b = TickBudget::new();
    b.consume(0);
    assert_eq!(b.left(), FULL);
    b.consume(1);
    assert_eq!(b.left(), FULL - 1);
    b.consume(FULL - 2);
    assert_eq!(b.left(), 1);
    assert!(!b.is_exhausted());
    b.consume(1);
    assert_eq!(b.left(), 0);
    assert!(b.is_exhausted());
    assert_eq!(b.grant(0), 0);
    assert_eq!(b.grant(100_000), 0);
    assert_eq!(b.grant(u64::MAX), 0);
    b.consume(5);
    assert_eq!(b.left(), 0);
    for used in [0u64, 1, 1000, FULL - 1, FULL] {
        let mut b = partial(used);
        b.consume(u64::MAX);
        assert_eq!(b.left(), 0);
        assert!(b.is_exhausted());
    }
    let mut b = TickBudget::new();
    b.consume(FULL + 1);
    assert_eq!(b.left(), 0);
}

#[test]
fn req_web_001_reset_and_begin_tick_refill() {
    let mut b = partial(150_000);
    b.reset();
    assert_eq!(b.left(), FULL);

    let mut b = TickBudget::new();
    b.consume(FULL);
    b.begin_tick(7); // first call always refills
    assert_eq!(b.left(), FULL);
    b.consume(1234);
    b.begin_tick(7);
    assert_eq!(b.left(), FULL - 1234, "same tick must not refill");
    b.begin_tick(8);
    assert_eq!(b.left(), FULL);
    b.consume(10);
    b.begin_tick(3); // lower tick refills
    assert_eq!(b.left(), FULL);
    b.consume(10);
    b.begin_tick(u64::MAX);
    assert_eq!(b.left(), FULL);
    b.consume(10);
    b.begin_tick(u64::MAX);
    assert_eq!(b.left(), FULL - 10);
}

#[test]
fn req_web_001_first_begin_tick_refills_even_for_tick_zero() {
    for t in [0u64, 1, u64::MAX] {
        let mut b = partial(500);
        b.begin_tick(t);
        assert_eq!(b.left(), FULL, "first call with tick {t}");
    }
}

#[test]
fn req_web_001_w11_scenario_512_callbacks_share_one_allowance() {
    let mut b = TickBudget::new();
    b.begin_tick(1);
    let mut consumed = 0u64;
    let mut grants = Vec::new();
    for _ in 0..512 {
        let g = b.grant(100_000);
        b.consume(g);
        consumed += g;
        grants.push(g);
    }
    assert_eq!(consumed, FULL);
    assert_eq!(&grants[..3], &[100_000, 100_000, 0]);
    assert!(grants[2..].iter().all(|&g| g == 0));
    assert!(b.is_exhausted());

    // 512 callbacks of cost 390: 512 * 390 = 199_680 all fit, 320 left.
    let mut b = TickBudget::new();
    b.begin_tick(2);
    for _ in 0..512 {
        let g = b.grant(100_000);
        assert!(g >= 390);
        b.consume(390);
    }
    assert_eq!(b.left(), FULL - 512 * 390);
    assert_eq!(b.left(), 320);
    assert!(!b.is_exhausted());
}

#[derive(Clone, Debug)]
enum Op {
    Grant(u64),
    ConsumeGrant(u64),
    ConsumeAny(u64),
    Reset,
    Begin(u64),
}

fn op() -> impl Strategy<Value = Op> {
    let amount = prop_oneof![
        0u64..300_000,
        Just(0u64),
        Just(1),
        Just(FULL),
        Just(FULL + 1),
        Just(u64::MAX),
    ];
    prop_oneof![
        amount.clone().prop_map(Op::Grant),
        amount.clone().prop_map(Op::ConsumeGrant),
        amount.prop_map(Op::ConsumeAny),
        Just(Op::Reset),
        (0u64..4).prop_map(Op::Begin),
        Just(Op::Begin(u64::MAX)),
    ]
}

proptest! {
    #[test]
    fn req_web_001_budget_matches_reference_model(ops in proptest::collection::vec(op(), 0..200)) {
        let mut b = TickBudget::new();
        let mut model = FULL;
        let mut last_tick: Option<u64> = None;
        let mut spent_this_tick = 0u64;
        for o in ops {
            let before = b.left();
            let mut refilled = false;
            match o {
                Op::Grant(x) => {
                    let g = b.grant(x);
                    prop_assert_eq!(g, x.min(model));
                    prop_assert!(g <= x && g <= b.left());
                }
                Op::ConsumeGrant(x) => {
                    let g = b.grant(x);
                    b.consume(g);
                    model -= g;
                    spent_this_tick += g;
                }
                Op::ConsumeAny(x) => {
                    b.consume(x);
                    let used = x.min(model);
                    model -= used;
                    spent_this_tick += used;
                }
                Op::Reset => {
                    b.reset();
                    model = FULL;
                    refilled = true;
                    spent_this_tick = 0;
                }
                Op::Begin(t) => {
                    b.begin_tick(t);
                    if last_tick != Some(t) {
                        model = FULL;
                        refilled = true;
                        spent_this_tick = 0;
                    }
                    last_tick = Some(t);
                }
            }
            prop_assert_eq!(b.left(), model);
            prop_assert!(b.left() <= TICK_BUDGET_UNITS);
            prop_assert_eq!(b.is_exhausted(), model == 0);
            if !refilled {
                prop_assert!(b.left() <= before, "left grew without reset or new tick");
            }
            prop_assert!(spent_this_tick <= FULL);
        }
    }
}
