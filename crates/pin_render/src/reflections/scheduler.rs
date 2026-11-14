//! GPU-free probe scheduling: the per-frame capture budget, round-robin
//! order, probe ages and the "nearest probe" fallback.

use super::{MAX_PROBES, PROBE_BUDGET};

/// Probe budget counters.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct ProbeStats {
    /// Frames scheduled.
    pub frames: u64,
    /// Probe captures over all frames.
    pub total_updates: u64,
    /// Probe captures in the last frame.
    pub last_frame_updates: u32,
    /// Most probe captures in any one frame.
    pub max_frame_updates: u32,
    /// Largest ball probe age after the last frame (frames; see
    /// [`ProbeScheduler::ages`]).
    pub max_age: u32,
    /// Largest ball probe age after any frame.
    pub max_age_ever: u32,
    /// The per-frame capture budget.
    pub budget: u32,
    /// Balls in the last frame.
    pub balls: u32,
}

/// Largest distance (m) a ball may move between two frames and still keep
/// its probe as its own (0.25 m: 15 m/s at 60 frames per second).
pub const TRACK_MAX_JUMP_M: f32 = 0.25;

/// Round-robin probe scheduler under a per-frame budget.
///
/// Ball `i` (its index in the frame's ball list, i.e. the snapshot order)
/// owns probe slot `i` for `i < MAX_PROBES`. Snapshots carry no ball ids,
/// so a slot's capture is **stale** for its ball when the ball at index
/// `i` is no longer the ball that was there: it moved further than
/// [`TRACK_MAX_JUMP_M`] since the last frame, or the nearest ball of the
/// last frame was another index (a ball before it in the list drained, so
/// the list shifted). A ball with a stale or missing capture samples the
/// captured probe nearest to it (by capture position) until its own slot
/// is captured again. In a tight multiball a ball can be flagged although
/// it kept its index (another ball's previous position is nearer than its
/// own); it then samples the nearest capture, a neighbour a few cm away,
/// which is harmless.
///
/// Each [`schedule`](Self::schedule) call is one frame and captures the
/// next `budget` ball indices after the last captured one, cyclically
/// modulo the current ball count (0 1 | 2 3 | 4 5 | 6 0 | 1 2 … for seven
/// balls and budget 2). Positions never change the order. With a steady
/// `n` balls every ball is captured once every ⌈n / budget⌉ frames or more
/// often, so its age (frames since its own slot's capture, 0 = this frame)
/// stays ≤ ⌈n / budget⌉ − 1 (steady count only: right after a count change
/// the worst age is < 2·⌈n / budget⌉, and with the count changing every
/// frame it can be higher, though the cursor keeps advancing so no ball
/// starves), and over any window of `n` frames the capture
/// counts differ by at most 1. After each frame the cursor is (last
/// captured index + 1) mod (that frame's count); the next frame starts at
/// the cursor mod its own count, whether the count shrank or grew, so a
/// count change never restarts it (3 balls: [0, 1], [2, 0]; then 7 balls:
/// [1, 2], [3, 4], …). Slots past the ball count are freed.
#[derive(Clone, Debug)]
pub struct ProbeScheduler {
    budget: u32,
    frame: u64,
    /// Next ball index to capture.
    cursor: usize,
    // Per ball index of the last frame.
    positions: Vec<[f32; 3]>,
    first_frame: Vec<u64>,
    ages: Vec<u32>,
    // Per slot.
    captured: Vec<Option<[f32; 3]>>,
    capture_frame: Vec<u64>,
    stale: Vec<bool>,
    updates: Vec<usize>,
    stats: ProbeStats,
}

impl ProbeScheduler {
    /// A scheduler capturing at most `budget` probes per frame (clamped to
    /// `1..=PROBE_BUDGET`).
    pub fn new(budget: u32) -> ProbeScheduler {
        let budget = budget.clamp(1, PROBE_BUDGET);
        ProbeScheduler {
            budget,
            frame: 0,
            cursor: 0,
            positions: Vec::with_capacity(MAX_PROBES),
            first_frame: Vec::with_capacity(MAX_PROBES),
            ages: Vec::with_capacity(MAX_PROBES),
            captured: vec![None; MAX_PROBES],
            capture_frame: vec![0; MAX_PROBES],
            stale: vec![false; MAX_PROBES],
            updates: Vec::with_capacity(PROBE_BUDGET as usize),
            stats: ProbeStats {
                budget,
                ..ProbeStats::default()
            },
        }
    }

    /// The per-frame capture budget.
    pub fn budget(&self) -> u32 {
        self.budget
    }

    /// Whether the ball now at index `i` (at `p`) is a different ball than
    /// the one at index `i` last frame.
    fn replaced(&self, i: usize, p: [f32; 3]) -> bool {
        let d2 = |a: [f32; 3], b: [f32; 3]| {
            let d = (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2);
            if d.is_nan() { f32::INFINITY } else { d }
        };
        let Some(&q) = self.positions.get(i) else {
            return true;
        };
        let own = d2(p, q);
        if own > TRACK_MAX_JUMP_M * TRACK_MAX_JUMP_M {
            return true;
        }
        self.positions.iter().any(|&o| d2(p, o) < own)
    }

    /// Age of ball `i` (frames since its slot's capture, or since its first
    /// frame when the slot holds none).
    fn age_of(&self, i: usize) -> u64 {
        match self.captured.get(i) {
            Some(Some(_)) => self.frame - self.capture_frame[i],
            _ => self.frame - self.first_frame[i],
        }
    }

    /// Advances one frame with the balls at `positions` (centres, world) and
    /// returns the ball indices whose probes are captured this frame (at
    /// most the budget, each `< MAX_PROBES`). Balls dropped since the last
    /// frame (a shorter list) forget their probes.
    pub fn schedule(&mut self, positions: &[[f32; 3]]) -> &[usize] {
        self.frame += 1;
        let f = self.frame;
        let n = positions.len();
        let owned = n.min(MAX_PROBES);
        // Which slots no longer describe their ball.
        for (i, &p) in positions.iter().enumerate().take(owned) {
            if self.captured[i].is_some() && self.replaced(i, p) {
                self.stale[i] = true;
            }
        }
        for s in owned..MAX_PROBES {
            self.captured[s] = None;
            self.stale[s] = false;
        }
        self.first_frame.truncate(n);
        self.first_frame.resize(n, f);
        self.positions.clear();
        self.positions.extend_from_slice(positions);

        // Captures: a cyclic cursor over the ball indices (positions never
        // change the order); a count change keeps the cursor.
        self.updates.clear();
        if owned > 0 {
            self.cursor %= owned;
            let k = (self.budget as usize).min(owned);
            for j in 0..k {
                let i = (self.cursor + j) % owned;
                self.captured[i] = Some(positions[i]);
                self.capture_frame[i] = f;
                self.stale[i] = false;
                self.updates.push(i);
            }
            self.cursor = (self.cursor + k) % owned;
        }

        // Ages; balls past MAX_PROBES report the age of the probe they reuse.
        self.ages.clear();
        for i in 0..n {
            let age = if i < owned {
                self.age_of(i)
            } else {
                match self.probe_for_ball(i) {
                    Some(s) => f - self.capture_frame[s],
                    None => f - self.first_frame[i],
                }
            };
            self.ages.push(u32::try_from(age).unwrap_or(u32::MAX));
        }

        let updates = self.updates.len() as u32;
        let max_age = self.ages.iter().copied().max().unwrap_or(0);
        let st = &mut self.stats;
        st.frames += 1;
        st.total_updates += u64::from(updates);
        st.last_frame_updates = updates;
        st.max_frame_updates = st.max_frame_updates.max(updates);
        st.max_age = max_age;
        st.max_age_ever = st.max_age_ever.max(max_age);
        st.balls = n as u32;
        &self.updates
    }

    /// The probe slot ball `i` samples: its own slot once captured (and not
    /// stale), else the captured slot whose capture position is nearest to
    /// the ball (ties → lower slot); `None` before any capture or for an
    /// unknown ball.
    pub fn probe_for_ball(&self, i: usize) -> Option<usize> {
        let p = *self.positions.get(i)?;
        if i < MAX_PROBES && self.captured[i].is_some() && !self.stale[i] {
            return Some(i);
        }
        let mut best: Option<(f32, usize)> = None;
        for (slot, c) in self.captured.iter().enumerate() {
            let Some(c) = c else { continue };
            let d2 = (c[0] - p[0]).powi(2) + (c[1] - p[1]).powi(2) + (c[2] - p[2]).powi(2);
            let d2 = if d2.is_nan() { f32::INFINITY } else { d2 };
            if best.is_none_or(|(b, _)| d2 < b) {
                best = Some((d2, slot));
            }
        }
        best.map(|(_, s)| s)
    }

    /// Whether ball `i`'s own capture is stale (see the type docs).
    pub fn is_stale(&self, i: usize) -> bool {
        self.stale.get(i).copied().unwrap_or(false)
    }

    /// Per ball: frames since its probe was last captured (0 = this frame).
    /// A ball not captured yet counts from its first frame; a ball past
    /// `MAX_PROBES` reports the age of the probe it reuses.
    pub fn ages(&self) -> &[u32] {
        &self.ages
    }

    /// Where slot `slot` was captured (`None`: no capture).
    pub fn capture_position(&self, slot: usize) -> Option<[f32; 3]> {
        self.captured.get(slot).copied().flatten()
    }

    /// Whether slot `slot` holds a capture.
    pub fn is_captured(&self, slot: usize) -> bool {
        self.captured.get(slot).is_some_and(Option::is_some)
    }

    /// The ball indices captured in the last frame.
    pub fn last_updates(&self) -> &[usize] {
        &self.updates
    }

    /// Counters.
    pub fn stats(&self) -> ProbeStats {
        self.stats
    }
}
