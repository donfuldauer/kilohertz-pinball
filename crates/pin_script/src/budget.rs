//! The per-tick script budget (D45, R11, SPEC §3.4).
//!
//! One allowance of [`TICK_BUDGET_UNITS`] exact units is shared by every
//! callback of one sim tick (the `on_switch_edge` calls and `on_tick`). It
//! resets once per [`pin_core::HostView::tick`], and each callback runs with
//! [`TickBudget::grant`]`(per_callback)`, the smaller of its own budget and
//! what is left. Units are counted by the caller (Lua: VM instructions
//! including KHZ charges; Wasm: fuel); [`TickBudget`] only does the
//! arithmetic. It is pure: no allocation, no clock and no I/O (R4, R6), and
//! it is built under every feature combination (REQ-WEB-001).

/// The units one tick's callbacks may use together (D45).
pub const TICK_BUDGET_UNITS: u64 = 200_000;

/// What is left of one tick's [`TICK_BUDGET_UNITS`] allowance.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TickBudget {
    /// Units still available this tick.
    left: u64,
    /// The tick of the last [`TickBudget::begin_tick`], if any.
    tick: Option<u64>,
}

impl Default for TickBudget {
    fn default() -> Self {
        Self::new()
    }
}

impl TickBudget {
    /// A full allowance: [`TickBudget::left`] is [`TICK_BUDGET_UNITS`].
    pub const fn new() -> Self {
        Self {
            left: TICK_BUDGET_UNITS,
            tick: None,
        }
    }

    /// Refills the allowance to [`TICK_BUDGET_UNITS`].
    pub fn reset(&mut self) {
        self.left = TICK_BUDGET_UNITS;
    }

    /// Starts sim tick `tick`: refills the allowance when `tick` differs from
    /// the tick of the previous call (the first call always refills). A
    /// second call with the same tick changes nothing, so every callback of
    /// a tick may call it.
    pub fn begin_tick(&mut self, tick: u64) {
        if self.tick != Some(tick) {
            self.tick = Some(tick);
            self.reset();
        }
    }

    /// Units still available this tick.
    pub const fn left(&self) -> u64 {
        self.left
    }

    /// The budget ONE callback may run with: `min(per_callback, left())`.
    pub fn grant(&self, per_callback: u64) -> u64 {
        per_callback.min(self.left)
    }

    /// Charges `used` units to this tick (saturating at 0).
    pub fn consume(&mut self, used: u64) {
        self.left = self.left.saturating_sub(used);
    }

    /// True when nothing is left this tick (`left() == 0`).
    pub const fn is_exhausted(&self) -> bool {
        self.left == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// REQ-WEB-001 / D45: the allowance is 200 000 units and a fresh budget
    /// holds all of it.
    #[test]
    fn new_is_full_req_web_001() {
        assert_eq!(TICK_BUDGET_UNITS, 200_000);
        const B: TickBudget = TickBudget::new();
        assert_eq!(B.left(), TICK_BUDGET_UNITS);
        assert!(!B.is_exhausted());
        assert_eq!(TickBudget::default(), B);
        assert!(format!("{B:?}").contains("TickBudget"));
    }

    /// D45: callbacks share one allowance; each runs with
    /// `min(per_callback, left)`; exhaustion saturates at 0.
    #[test]
    fn grant_and_consume_share_the_allowance() {
        let mut b = TickBudget::new();
        b.begin_tick(0);
        assert_eq!(b.grant(100_000), 100_000);
        b.consume(99_999);
        assert_eq!(b.left(), 100_001);
        assert_eq!(b.grant(100_000), 100_000);
        b.consume(100_000);
        assert_eq!(b.left(), 1);
        assert_eq!(b.grant(100_000), 1);
        assert_eq!(b.grant(0), 0);
        b.consume(5);
        assert_eq!(b.left(), 0);
        assert!(b.is_exhausted());
        assert_eq!(b.grant(100_000), 0);
        b.consume(u64::MAX);
        assert_eq!(b.left(), 0);
        b.reset();
        assert_eq!(b.left(), TICK_BUDGET_UNITS);
        b.consume(u64::MAX);
        assert!(b.is_exhausted());
    }

    /// D45: the allowance resets once per `HostView::tick`; repeated calls
    /// within a tick change nothing; the first call always resets.
    #[test]
    fn begin_tick_resets_once_per_tick() {
        let mut b = TickBudget::new();
        b.consume(10);
        b.begin_tick(7);
        assert_eq!(b.left(), TICK_BUDGET_UNITS, "first call resets");
        b.consume(150_000);
        b.begin_tick(7);
        assert_eq!(b.left(), 50_000, "same tick: unchanged");
        b.begin_tick(8);
        assert_eq!(b.left(), TICK_BUDGET_UNITS);
        b.consume(TICK_BUDGET_UNITS);
        b.begin_tick(8);
        assert!(b.is_exhausted());
        // Any different tick resets, also an earlier one (a rewound sim).
        b.begin_tick(3);
        assert_eq!(b.left(), TICK_BUDGET_UNITS);
        // A fresh budget's first call resets even for tick 0.
        let mut c = TickBudget::new();
        c.consume(1);
        c.begin_tick(0);
        assert_eq!(c.left(), TICK_BUDGET_UNITS);
        c.consume(1);
        c.begin_tick(0);
        assert_eq!(c.left(), TICK_BUDGET_UNITS - 1);
        // `reset` does not forget the tick.
        c.reset();
        c.consume(2);
        c.begin_tick(0);
        assert_eq!(c.left(), TICK_BUDGET_UNITS - 2);
    }

    #[derive(Clone, Debug)]
    enum Op {
        Grant(u64),
        /// Consume a fraction (per mille) of the preceding grant, or an
        /// arbitrary amount when `free`.
        Consume {
            per_mille: u16,
            free: Option<u64>,
        },
        Reset,
        BeginTick(u8),
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            4 => prop_oneof![0u64..=300_000, any::<u64>()].prop_map(Op::Grant),
            4 => (0u16..=1000, proptest::option::weighted(0.1, any::<u64>()))
                .prop_map(|(per_mille, free)| Op::Consume { per_mille, free }),
            1 => Just(Op::Reset),
            2 => (0u8..4).prop_map(Op::BeginTick),
        ]
    }

    proptest! {
        /// REQ-WEB-001 / D45 invariants over any operation sequence:
        /// `left <= TICK_BUDGET_UNITS`, `grant(x) <= x`, `grant(x) <= left`,
        /// `left` only grows through `reset` / a new tick, and when every
        /// `consume` is bounded by the preceding `grant`, one tick's total
        /// never exceeds the allowance.
        #[test]
        fn invariants_req_web_001(ops in proptest::collection::vec(op(), 0..64)) {
            let mut b = TickBudget::new();
            let mut last_grant = 0u64;
            // Consumed since the last refill, counting only bounded consumes;
            // `None` once an unbounded consume happened in this period.
            let mut used: Option<u64> = Some(0);
            for op in ops {
                let before = b;
                match op {
                    Op::Grant(x) => {
                        let g = b.grant(x);
                        prop_assert!(g <= x);
                        prop_assert!(g <= b.left());
                        prop_assert_eq!(g, x.min(b.left()));
                        prop_assert_eq!(b, before, "grant does not change the budget");
                        last_grant = g;
                    }
                    Op::Consume { per_mille, free } => {
                        let n = match free {
                            Some(n) => {
                                used = None;
                                n
                            }
                            None => {
                                let n = last_grant * u64::from(per_mille) / 1000;
                                used = used.map(|u| u + n);
                                n
                            }
                        };
                        b.consume(n);
                        prop_assert!(b.left() <= before.left());
                        prop_assert_eq!(b.left(), before.left().saturating_sub(n));
                        // A grant is only valid until the next consume.
                        last_grant = b.grant(last_grant);
                    }
                    Op::Reset => {
                        b.reset();
                        prop_assert_eq!(b.left(), TICK_BUDGET_UNITS);
                        used = Some(0);
                        last_grant = 0;
                    }
                    Op::BeginTick(t) => {
                        b.begin_tick(u64::from(t));
                        if b.left() != before.left() {
                            prop_assert_eq!(b.left(), TICK_BUDGET_UNITS);
                        }
                        if b != before {
                            // A new tick: refilled.
                            prop_assert_eq!(b.left(), TICK_BUDGET_UNITS);
                            used = Some(0);
                            last_grant = 0;
                        }
                    }
                }
                prop_assert!(b.left() <= TICK_BUDGET_UNITS);
                prop_assert_eq!(b.is_exhausted(), b.left() == 0);
                if let Some(u) = used {
                    prop_assert!(u <= TICK_BUDGET_UNITS);
                    prop_assert_eq!(b.left(), TICK_BUDGET_UNITS - u);
                }
            }
        }
    }
}
