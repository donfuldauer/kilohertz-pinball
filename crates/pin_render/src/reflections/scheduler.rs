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

/// Round-robin probe scheduler under a per-frame budget.
///
/// Ball `i` (its index in the frame's ball list, i.e. the snapshot order)
/// owns probe slot `i` for `i < MAX_PROBES`. Each [`schedule`](Self::schedule)
/// call is one frame: it captures at most `budget` slots, continuing the
/// round-robin where the previous frame stopped, so with a steady `n` balls
/// every ball's probe is recaptured at least every ⌈n / budget⌉ frames and
/// its age (frames since the capture, 0 = this frame) stays ≤ ⌈n / budget⌉ − 1.
#[derive(Clone, Debug)]
pub struct ProbeScheduler {
    budget: u32,
    cursor: usize,
    ages: Vec<u32>,
    /// Capture position of each owned slot (`None` = never captured).
    captured: Vec<Option<[f32; 3]>>,
    positions: Vec<[f32; 3]>,
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
            cursor: 0,
            ages: Vec::with_capacity(MAX_PROBES),
            captured: Vec::with_capacity(MAX_PROBES),
            positions: Vec::with_capacity(MAX_PROBES),
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

    /// Advances one frame with the balls at `positions` (centres, world) and
    /// returns the ball indices whose probes are captured this frame (at
    /// most the budget, each `< MAX_PROBES`). Balls dropped since the last
    /// frame (a shorter list) forget their probes.
    pub fn schedule(&mut self, positions: &[[f32; 3]]) -> &[usize] {
        let n = positions.len();
        // Existing balls age by one frame; new balls start at 0.
        self.ages.truncate(n);
        for a in &mut self.ages {
            *a = a.saturating_add(1);
        }
        self.ages.resize(n, 0);
        let owned = n.min(MAX_PROBES);
        self.captured.truncate(owned);
        self.captured.resize(owned, None);
        self.positions.clear();
        self.positions.extend_from_slice(positions);

        self.updates.clear();
        if owned > 0 {
            self.cursor %= owned;
            let k = (self.budget as usize).min(owned);
            for j in 0..k {
                let i = (self.cursor + j) % owned;
                self.updates.push(i);
                self.ages[i] = 0;
                self.captured[i] = Some(positions[i]);
            }
            self.cursor = (self.cursor + k) % owned;
        } else {
            self.cursor = 0;
        }
        // Balls past MAX_PROBES have no slot: report the age of the probe
        // they reuse.
        for i in owned..n {
            if let Some(slot) = self.probe_for_ball(i) {
                self.ages[i] = self.ages[slot];
            }
        }

        let updates = self.updates.len() as u32;
        let max_age = self.ages.iter().copied().max().unwrap_or(0);
        let s = &mut self.stats;
        s.frames += 1;
        s.total_updates += u64::from(updates);
        s.last_frame_updates = updates;
        s.max_frame_updates = s.max_frame_updates.max(updates);
        s.max_age = max_age;
        s.max_age_ever = s.max_age_ever.max(max_age);
        s.balls = n as u32;
        &self.updates
    }

    /// The probe slot ball `i` samples: its own slot once captured, else the
    /// captured slot whose capture position is nearest to the ball (ties →
    /// lower slot); `None` before any capture or for an unknown ball.
    pub fn probe_for_ball(&self, i: usize) -> Option<usize> {
        if i < self.captured.len() && self.captured[i].is_some() {
            return Some(i);
        }
        let p = *self.positions.get(i)?;
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

    /// Per ball: frames since its probe was last captured (0 = this frame).
    /// A ball not captured yet counts from its first frame; a ball past
    /// `MAX_PROBES` reports the age of the probe it reuses.
    pub fn ages(&self) -> &[u32] {
        &self.ages
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
