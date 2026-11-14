//! Flipper staging: which flippers are energised by the current cabinet frame.
//!
//! Owner: Subphase 3.1.
//!
//! Semantics (DECISIONS.md "3.1 dual-stage semantics"):
//! - `DedicatedButton(i)` / `ParallelLinked(i)`: active iff cabinet input `i`
//!   is pressed.
//! - `DualStageThreshold`: a [`FlipperStage::Lower`] flipper is active iff the
//!   axis is `>= lower_threshold`; an [`FlipperStage::Upper`] one iff it is
//!   `>= upper_threshold`.
//! - `AutoSensorThingFlip` is scheduled by the physics/sim layer; the router
//!   always reports it inactive.
//!
//! The router is fixed-capacity and never allocates after construction.

use crate::events::CabinetFrame;
use arrayvec::ArrayVec;
use core::fmt;
use pin_core::flipper::FlipperStagingMode;
use pin_core::ids::FlipperId;

/// Maximum number of flippers one router handles.
pub const MAX_STAGING_ENTRIES: usize = 16;

/// Default lower-stage threshold for dual-stage analog buttons.
pub const DEFAULT_LOWER_THRESHOLD: f32 = 0.30;
/// Default upper-stage threshold for dual-stage analog buttons.
pub const DEFAULT_UPPER_THRESHOLD: f32 = 0.85;

/// Which stage of a (possibly dual-stage) button a flipper belongs to.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum FlipperStage {
    /// Lower playfield / first stage.
    Lower,
    /// Upper flipper / second stage.
    Upper,
}

/// One flipper's staging binding.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct StagingEntry {
    /// The flipper.
    pub flipper: FlipperId,
    /// How input drives it.
    pub mode: FlipperStagingMode,
    /// Its stage (only matters for `DualStageThreshold`).
    pub stage: FlipperStage,
}

/// Why a router could not be built.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum StagingError {
    /// More than [`MAX_STAGING_ENTRIES`] entries.
    TooManyEntries(usize),
    /// The same flipper id appears twice.
    DuplicateFlipper(FlipperId),
    /// Dual-stage thresholds not finite or not `0 <= lower <= upper <= 1`.
    InvalidThresholds {
        /// The offending flipper.
        flipper: FlipperId,
        /// Its lower threshold.
        lower: f32,
        /// Its upper threshold.
        upper: f32,
    },
}

impl fmt::Display for StagingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StagingError::TooManyEntries(n) => {
                write!(f, "{n} staging entries (max {MAX_STAGING_ENTRIES})")
            }
            StagingError::DuplicateFlipper(id) => {
                write!(f, "flipper {} has more than one staging entry", id.0)
            }
            StagingError::InvalidThresholds {
                flipper,
                lower,
                upper,
            } => write!(
                f,
                "flipper {}: dual-stage thresholds must satisfy 0 <= lower ({lower}) <= upper ({upper}) <= 1",
                flipper.0
            ),
        }
    }
}

impl std::error::Error for StagingError {}

/// Whether `entry` is active in `frame` (stateless).
pub fn entry_active(entry: &StagingEntry, frame: &CabinetFrame) -> bool {
    match entry.mode {
        FlipperStagingMode::DedicatedButton(i) | FlipperStagingMode::ParallelLinked(i) => {
            frame.pressed(i)
        }
        FlipperStagingMode::DualStageThreshold {
            axis,
            lower_threshold,
            upper_threshold,
        } => {
            let v = frame.axis(axis);
            match entry.stage {
                FlipperStage::Lower => v >= lower_threshold,
                FlipperStage::Upper => v >= upper_threshold,
            }
        }
        FlipperStagingMode::AutoSensorThingFlip { .. } => false,
    }
}

/// Turns cabinet frames into flipper activation changes.
#[derive(Clone, Debug)]
pub struct FlipperStagingRouter {
    entries: ArrayVec<StagingEntry, MAX_STAGING_ENTRIES>,
    active: [bool; MAX_STAGING_ENTRIES],
}

impl FlipperStagingRouter {
    /// Validates `entries` and builds a router with every flipper inactive.
    pub fn new(entries: &[StagingEntry]) -> Result<Self, StagingError> {
        if entries.len() > MAX_STAGING_ENTRIES {
            return Err(StagingError::TooManyEntries(entries.len()));
        }
        let mut list = ArrayVec::new();
        for e in entries {
            if list.iter().any(|p: &StagingEntry| p.flipper == e.flipper) {
                return Err(StagingError::DuplicateFlipper(e.flipper));
            }
            if let FlipperStagingMode::DualStageThreshold {
                lower_threshold: lower,
                upper_threshold: upper,
                ..
            } = e.mode
            {
                let ok = lower.is_finite()
                    && upper.is_finite()
                    && 0.0 <= lower
                    && lower <= upper
                    && upper <= 1.0;
                if !ok {
                    return Err(StagingError::InvalidThresholds {
                        flipper: e.flipper,
                        lower,
                        upper,
                    });
                }
            }
            list.push(*e);
        }
        Ok(Self {
            entries: list,
            active: [false; MAX_STAGING_ENTRIES],
        })
    }

    /// The entries, in construction order.
    pub fn entries(&self) -> &[StagingEntry] {
        &self.entries
    }

    /// Recomputes every flipper's state from `frame` and pushes each change
    /// since the previous update (`true` = became active) onto `out`, in entry
    /// order. Changes that do not fit in `out` are dropped (pass an empty
    /// `out`; it can always hold one change per entry).
    pub fn update(&mut self, frame: &CabinetFrame, out: &mut ArrayVec<(FlipperId, bool), 16>) {
        for (i, e) in self.entries.iter().enumerate() {
            let now = entry_active(e, frame);
            if now != self.active[i] {
                self.active[i] = now;
                let _ = out.try_push((e.flipper, now));
            }
        }
    }

    /// Whether flipper `f` is active as of the last update (`false` for an
    /// unknown flipper).
    pub fn is_active(&self, f: FlipperId) -> bool {
        self.entries
            .iter()
            .position(|e| e.flipper == f)
            .is_some_and(|i| self.active[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pin_core::ids::SwitchId;
    use pin_core::input::{CabinetAxis, CabinetInput};

    fn dual(id: u16, stage: FlipperStage) -> StagingEntry {
        StagingEntry {
            flipper: FlipperId(id),
            mode: FlipperStagingMode::DualStageThreshold {
                axis: CabinetAxis::LeftFlipper,
                lower_threshold: DEFAULT_LOWER_THRESHOLD,
                upper_threshold: DEFAULT_UPPER_THRESHOLD,
            },
            stage,
        }
    }

    fn frame_axis(v: f32) -> CabinetFrame {
        let mut f = CabinetFrame::default();
        f.axes[3] = v;
        f
    }

    #[test]
    fn errors() {
        let e = dual(1, FlipperStage::Lower);
        assert_eq!(
            FlipperStagingRouter::new(&[e, e]).unwrap_err(),
            StagingError::DuplicateFlipper(FlipperId(1))
        );
        let many: Vec<_> = (0..17).map(|i| dual(i, FlipperStage::Lower)).collect();
        assert_eq!(
            FlipperStagingRouter::new(&many).unwrap_err(),
            StagingError::TooManyEntries(17)
        );
        assert!(FlipperStagingRouter::new(&many[..16]).is_ok());
        for (lo, hi) in [
            (0.9, 0.1),
            (-0.1, 0.5),
            (0.1, 1.1),
            (f32::NAN, 0.5),
            (0.1, f32::INFINITY),
        ] {
            let bad = StagingEntry {
                mode: FlipperStagingMode::DualStageThreshold {
                    axis: CabinetAxis::RightFlipper,
                    lower_threshold: lo,
                    upper_threshold: hi,
                },
                ..e
            };
            let err = FlipperStagingRouter::new(&[bad]).unwrap_err();
            assert!(matches!(err, StagingError::InvalidThresholds { .. }));
            assert!(!err.to_string().is_empty());
        }
        assert!(!StagingError::TooManyEntries(17).to_string().is_empty());
        assert!(
            !StagingError::DuplicateFlipper(FlipperId(1))
                .to_string()
                .is_empty()
        );
    }

    #[test]
    fn dual_stage_thresholds() {
        let mut r = FlipperStagingRouter::new(&[
            dual(0, FlipperStage::Lower),
            dual(1, FlipperStage::Upper),
        ])
        .unwrap();
        let mut out = ArrayVec::new();
        r.update(&frame_axis(0.29), &mut out);
        assert!(out.is_empty());
        r.update(&frame_axis(0.30), &mut out);
        assert_eq!(out.as_slice(), &[(FlipperId(0), true)]);
        out.clear();
        r.update(&frame_axis(0.85), &mut out);
        assert_eq!(out.as_slice(), &[(FlipperId(1), true)]);
        assert!(r.is_active(FlipperId(0)) && r.is_active(FlipperId(1)));
        out.clear();
        r.update(&frame_axis(0.0), &mut out);
        assert_eq!(
            out.as_slice(),
            &[(FlipperId(0), false), (FlipperId(1), false)]
        );
        assert!(!r.is_active(FlipperId(7)));
    }

    #[test]
    fn buttons_and_auto() {
        let entries = [
            StagingEntry {
                flipper: FlipperId(0),
                mode: FlipperStagingMode::DedicatedButton(CabinetInput::LeftFlipper),
                stage: FlipperStage::Lower,
            },
            StagingEntry {
                flipper: FlipperId(1),
                mode: FlipperStagingMode::ParallelLinked(CabinetInput::LeftFlipper),
                stage: FlipperStage::Upper,
            },
            StagingEntry {
                flipper: FlipperId(2),
                mode: FlipperStagingMode::AutoSensorThingFlip {
                    trigger_switch: SwitchId(3),
                    delay_ms: 10.0,
                    hold_duration_ms: 100.0,
                },
                stage: FlipperStage::Lower,
            },
        ];
        let mut r = FlipperStagingRouter::new(&entries).unwrap();
        assert_eq!(r.entries().len(), 3);
        let mut out = ArrayVec::new();
        let f = CabinetFrame {
            buttons: 0xFF,
            axes: [1.0; 5],
            ..CabinetFrame::default()
        };
        r.update(&f, &mut out);
        assert_eq!(
            out.as_slice(),
            &[(FlipperId(0), true), (FlipperId(1), true)]
        );
        assert!(!r.is_active(FlipperId(2)));
    }
}
