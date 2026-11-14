//! Input event ingestion (SDL nanosecond timestamps).
//!
//! Owner: Subphase 3.1.
//!
//! The main thread forwards timestamped [`InputEvent`]s; the sim thread pushes
//! them into an [`InputIngest`] and calls [`InputIngest::advance`] once per
//! tick to get the debounced [`CabinetFrame`] for that tick. Everything here
//! is fixed-capacity: no heap allocation after construction (PLAN §2.3 rule 4).

use arrayvec::ArrayVec;
use pin_core::input::{CabinetAxis, CabinetInput, InputEvent, InputEventKind};

/// Maximum number of events waiting to be applied.
pub const INGEST_CAPACITY: usize = 256;

/// Nanoseconds per sim tick (1 kHz).
pub const NS_PER_TICK: u64 = 1_000_000;

/// Ingestion settings.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct IngestConfig {
    /// Event-clock time of tick 0, nanoseconds. Tick `t` covers sim time
    /// `epoch_ns + t * 1_000_000`.
    pub epoch_ns: u64,
    /// Button debounce window, milliseconds (= ticks). 0 disables debouncing.
    pub debounce_ms: u16,
}

/// The cabinet state for one sim tick.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct CabinetFrame {
    /// Sim tick this frame belongs to.
    pub tick: u64,
    /// Bit `i` set = cabinet input with [`CabinetInput::index`] `i` pressed.
    pub buttons: u8,
    /// Axis values in [`CabinetAxis::ALL`] order. Plunger and flipper axes are
    /// in `0..=1`, nudge in `-1..=1`.
    pub axes: [f32; 5],
}

impl CabinetFrame {
    /// Whether cabinet input `i` is pressed.
    #[inline]
    pub fn pressed(&self, i: CabinetInput) -> bool {
        self.buttons & (1u8 << i.index()) != 0
    }

    /// Value of axis `a`.
    #[inline]
    pub fn axis(&self, a: CabinetAxis) -> f32 {
        self.axes[axis_index(a)]
    }
}

/// Position of `a` in [`CabinetAxis::ALL`].
#[inline]
pub const fn axis_index(a: CabinetAxis) -> usize {
    match a {
        CabinetAxis::Plunger => 0,
        CabinetAxis::NudgeX => 1,
        CabinetAxis::NudgeY => 2,
        CabinetAxis::LeftFlipper => 3,
        CabinetAxis::RightFlipper => 4,
    }
}

/// Valid range of axis `a`.
#[inline]
pub const fn axis_range(a: CabinetAxis) -> (f32, f32) {
    match a {
        CabinetAxis::NudgeX | CabinetAxis::NudgeY => (-1.0, 1.0),
        CabinetAxis::Plunger | CabinetAxis::LeftFlipper | CabinetAxis::RightFlipper => (0.0, 1.0),
    }
}

/// Converts timestamped events into per-tick, debounced cabinet frames.
///
/// Pending events are kept sorted by timestamp (stable for equal timestamps)
/// in a fixed-capacity buffer, so slightly out-of-order delivery is fine.
#[derive(Clone, Debug)]
pub struct InputIngest {
    cfg: IngestConfig,
    pending: ArrayVec<InputEvent, INGEST_CAPACITY>,
    /// Latest raw (undebounced) button states.
    raw: u8,
    /// Debounced button states (what the frame reports).
    applied: u8,
    /// Per input: edges are ignored while `tick < lock_until[i]`.
    lock_until: [u64; CabinetInput::COUNT],
    axes: [f32; 5],
}

impl InputIngest {
    /// A new ingest with no pending events, all buttons released and all axes 0.
    pub fn new(cfg: IngestConfig) -> Self {
        Self {
            cfg,
            pending: ArrayVec::new(),
            raw: 0,
            applied: 0,
            lock_until: [0; CabinetInput::COUNT],
            axes: [0.0; 5],
        }
    }

    /// The configuration.
    pub fn config(&self) -> IngestConfig {
        self.cfg
    }

    /// Number of events waiting to be applied.
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Queues an event. Returns it back as `Err` when [`INGEST_CAPACITY`]
    /// events are already pending.
    pub fn push(&mut self, ev: InputEvent) -> Result<(), InputEvent> {
        if self.pending.is_full() {
            return Err(ev);
        }
        // Insert after every event with timestamp <= ev's (stable order).
        let mut idx = self.pending.len();
        while idx > 0 && self.pending[idx - 1].timestamp_ns > ev.timestamp_ns {
            idx -= 1;
        }
        self.pending.insert(idx, ev);
        Ok(())
    }

    /// Event-clock time covered by `tick`, saturating.
    #[inline]
    pub fn tick_time_ns(&self, tick: u64) -> u64 {
        self.cfg
            .epoch_ns
            .saturating_add(tick.saturating_mul(NS_PER_TICK))
    }

    /// Applies every pending event with `timestamp_ns <= epoch_ns + tick * 1e6`
    /// (in timestamp order) and returns the frame for `tick`.
    pub fn advance(&mut self, tick: u64) -> CabinetFrame {
        let cutoff = self.tick_time_ns(tick);
        let n = self
            .pending
            .iter()
            .position(|e| e.timestamp_ns > cutoff)
            .unwrap_or(self.pending.len());
        for i in 0..n {
            let ev = self.pending[i];
            self.apply(ev.kind);
        }
        self.pending.drain(..n);

        // Debounce on the tick-sampled raw state: a change since the last
        // applied state is an edge; it is accepted unless the input is inside
        // a window opened by an earlier accepted edge. A window that has ended
        // settles to the current raw state the same way.
        let changed = self.raw ^ self.applied;
        if changed != 0 {
            for i in 0..CabinetInput::COUNT {
                if changed & (1u8 << i) != 0 && tick >= self.lock_until[i] {
                    self.accept_edge(i, tick);
                }
            }
        }

        CabinetFrame {
            tick,
            buttons: self.applied,
            axes: self.axes,
        }
    }

    fn apply(&mut self, kind: InputEventKind) {
        match kind {
            InputEventKind::Button { input, pressed } => {
                let bit = 1u8 << input.index();
                if pressed {
                    self.raw |= bit;
                } else {
                    self.raw &= !bit;
                }
            }
            InputEventKind::Axis { axis, value } => {
                if !value.is_nan() {
                    let (lo, hi) = axis_range(axis);
                    self.axes[axis_index(axis)] = value.clamp(lo, hi);
                }
            }
        }
    }

    /// Copies raw bit `i` into the applied state and opens a debounce window.
    fn accept_edge(&mut self, i: usize, tick: u64) {
        let bit = 1u8 << i;
        self.applied = (self.applied & !bit) | (self.raw & bit);
        self.lock_until[i] = tick.saturating_add(u64::from(self.cfg.debounce_ms));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = NS_PER_TICK;

    fn ing(debounce_ms: u16) -> InputIngest {
        InputIngest::new(IngestConfig {
            epoch_ns: 0,
            debounce_ms,
        })
    }

    #[test]
    fn axis_index_matches_all_order() {
        for (i, a) in CabinetAxis::ALL.into_iter().enumerate() {
            assert_eq!(axis_index(a), i);
        }
    }

    #[test]
    fn button_applied_at_tick_of_timestamp() {
        let mut g = ing(0);
        g.push(InputEvent::button(3 * MS + 1, CabinetInput::Start, true))
            .unwrap();
        assert!(!g.advance(3).pressed(CabinetInput::Start));
        assert!(g.advance(4).pressed(CabinetInput::Start));
        assert_eq!(g.pending_len(), 0);
    }

    #[test]
    fn exact_boundary_inclusive() {
        let mut g = ing(0);
        g.push(InputEvent::button(5 * MS, CabinetInput::Tilt, true))
            .unwrap();
        assert!(!g.advance(4).pressed(CabinetInput::Tilt));
        assert!(g.advance(5).pressed(CabinetInput::Tilt));
    }

    #[test]
    fn epoch_offsets_ticks() {
        let mut g = InputIngest::new(IngestConfig {
            epoch_ns: 1_000_000_000,
            debounce_ms: 0,
        });
        g.push(InputEvent::button(
            1_000_000_000 + 2 * MS,
            CabinetInput::Action,
            true,
        ))
        .unwrap();
        assert!(!g.advance(1).pressed(CabinetInput::Action));
        assert!(g.advance(2).pressed(CabinetInput::Action));
    }

    #[test]
    fn press_release_same_tick_final_state() {
        let mut g = ing(0);
        g.push(InputEvent::button(100, CabinetInput::Launch, true))
            .unwrap();
        g.push(InputEvent::button(200, CabinetInput::Launch, false))
            .unwrap();
        assert!(!g.advance(1).pressed(CabinetInput::Launch));
    }

    #[test]
    fn out_of_order_applied_in_timestamp_order() {
        let mut g = ing(0);
        g.push(InputEvent::button(500, CabinetInput::Start, false))
            .unwrap();
        g.push(InputEvent::button(100, CabinetInput::Start, true))
            .unwrap();
        assert!(!g.advance(1).pressed(CabinetInput::Start));

        // Equal timestamps keep push order.
        g.push(InputEvent::axis(2 * MS, CabinetAxis::Plunger, 0.2))
            .unwrap();
        g.push(InputEvent::axis(2 * MS, CabinetAxis::Plunger, 0.7))
            .unwrap();
        assert_eq!(g.advance(2).axis(CabinetAxis::Plunger), 0.7);
    }

    #[test]
    fn late_events_applied_next_advance() {
        let mut g = ing(0);
        assert!(!g.advance(10).pressed(CabinetInput::Start));
        g.push(InputEvent::button(2 * MS, CabinetInput::Start, true))
            .unwrap();
        assert!(g.advance(11).pressed(CabinetInput::Start));
    }

    #[test]
    fn capacity_err() {
        let mut g = ing(0);
        for i in 0..INGEST_CAPACITY {
            g.push(InputEvent::axis(i as u64, CabinetAxis::NudgeX, 0.0))
                .unwrap();
        }
        let ev = InputEvent::axis(0, CabinetAxis::NudgeX, 0.5);
        assert_eq!(g.push(ev), Err(ev));
        g.advance(1);
        assert!(g.push(ev).is_ok());
    }

    #[test]
    fn clamp_and_nan() {
        let mut g = ing(0);
        g.push(InputEvent::axis(0, CabinetAxis::Plunger, 2.0))
            .unwrap();
        g.push(InputEvent::axis(0, CabinetAxis::NudgeX, -5.0))
            .unwrap();
        g.push(InputEvent::axis(0, CabinetAxis::NudgeY, f32::INFINITY))
            .unwrap();
        g.push(InputEvent::axis(0, CabinetAxis::LeftFlipper, -0.5))
            .unwrap();
        let f = g.advance(0);
        assert_eq!(f.axis(CabinetAxis::Plunger), 1.0);
        assert_eq!(f.axis(CabinetAxis::NudgeX), -1.0);
        assert_eq!(f.axis(CabinetAxis::NudgeY), 1.0);
        assert_eq!(f.axis(CabinetAxis::LeftFlipper), 0.0);
        g.push(InputEvent::axis(MS, CabinetAxis::Plunger, f32::NAN))
            .unwrap();
        assert_eq!(g.advance(1).axis(CabinetAxis::Plunger), 1.0);
    }

    #[test]
    fn debounce_window() {
        let mut g = ing(5);
        g.push(InputEvent::button(10 * MS, CabinetInput::LeftFlipper, true))
            .unwrap();
        // Chatter inside the window.
        g.push(InputEvent::button(
            11 * MS,
            CabinetInput::LeftFlipper,
            false,
        ))
        .unwrap();
        g.push(InputEvent::button(12 * MS, CabinetInput::LeftFlipper, true))
            .unwrap();
        g.push(InputEvent::button(
            13 * MS,
            CabinetInput::LeftFlipper,
            false,
        ))
        .unwrap();
        for t in 0..10 {
            assert!(!g.advance(t).pressed(CabinetInput::LeftFlipper));
        }
        for t in 10..15 {
            assert!(g.advance(t).pressed(CabinetInput::LeftFlipper), "tick {t}");
        }
        // Window ends at 15: settles to released.
        assert!(!g.advance(15).pressed(CabinetInput::LeftFlipper));
    }

    #[test]
    fn debounce_window_end_no_change_when_equal() {
        let mut g = ing(3);
        g.push(InputEvent::button(0, CabinetInput::Start, true))
            .unwrap();
        g.push(InputEvent::button(MS, CabinetInput::Start, false))
            .unwrap();
        g.push(InputEvent::button(2 * MS, CabinetInput::Start, true))
            .unwrap();
        assert!(g.advance(0).pressed(CabinetInput::Start));
        assert!(g.advance(1).pressed(CabinetInput::Start));
        assert!(g.advance(2).pressed(CabinetInput::Start));
        assert!(g.advance(3).pressed(CabinetInput::Start));
        // Edge right at the window end is accepted immediately.
        g.push(InputEvent::button(4 * MS, CabinetInput::Start, false))
            .unwrap();
        assert!(!g.advance(4).pressed(CabinetInput::Start));
    }

    #[test]
    fn debounce_is_per_input() {
        let mut g = ing(10);
        g.push(InputEvent::button(0, CabinetInput::Start, true))
            .unwrap();
        g.push(InputEvent::button(MS, CabinetInput::Tilt, true))
            .unwrap();
        let f0 = g.advance(0);
        assert!(f0.pressed(CabinetInput::Start) && !f0.pressed(CabinetInput::Tilt));
        assert!(g.advance(1).pressed(CabinetInput::Tilt));
    }

    #[test]
    fn huge_tick_saturates() {
        let mut g = InputIngest::new(IngestConfig {
            epoch_ns: u64::MAX - 5,
            debounce_ms: u16::MAX,
        });
        g.push(InputEvent::button(u64::MAX, CabinetInput::Start, true))
            .unwrap();
        assert!(g.advance(u64::MAX).pressed(CabinetInput::Start));
        assert_eq!(g.config().debounce_ms, u16::MAX);
    }
}
