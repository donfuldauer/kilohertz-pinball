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

/// Largest distance (m) a ball may move between two frames and still be
/// recognised as the same ball (0.25 m: 15 m/s at 60 frames per second).
/// A ball that moved further, or appeared, starts over without a probe of
/// its own.
pub const TRACK_MAX_JUMP_M: f32 = 0.25;

/// Round-robin probe scheduler under a per-frame budget.
///
/// Snapshots carry no ball ids, so the scheduler tracks balls from frame
/// to frame by position: each ball of the new frame is matched to the
/// nearest unmatched ball of the previous frame within
/// [`TRACK_MAX_JUMP_M`] (closest pairs first, ties → lower indices). A
/// matched ball keeps its probe slot, its capture and its age, whatever
/// its index in the list (a ball draining from the middle of the list does
/// not hand its probe to the next ball). An unmatched ball gets the lowest
/// free slot (up to [`MAX_PROBES`]) with no capture yet, and samples the
/// nearest captured probe until its own is captured; slots of balls that
/// left are freed and forget their capture.
///
/// Each [`schedule`](Self::schedule) call is one frame. It captures at
/// most `budget` slots: balls without a capture first, then the oldest
/// captures (ties → lower ball index). With a steady `n ≤ MAX_PROBES` balls
/// this is a round robin and every age (frames since the capture, 0 = this
/// frame) stays ≤ ⌈n / budget⌉ − 1; after any change of the ball set the
/// order stays oldest-first, so no ball waits more than ⌈n / budget⌉
/// frames once the set is steady.
#[derive(Clone, Debug)]
pub struct ProbeScheduler {
    budget: u32,
    frame: u64,
    // Per ball of the last frame (index = position in the list).
    positions: Vec<[f32; 3]>,
    slot: Vec<Option<usize>>,
    first_frame: Vec<u64>,
    ages: Vec<u32>,
    // Per slot.
    captured: Vec<Option<[f32; 3]>>,
    capture_frame: Vec<u64>,
    used: Vec<bool>,
    // Scratch.
    matched: Vec<Option<usize>>,
    taken: Vec<bool>,
    pairs: Vec<(f32, usize, usize)>,
    next_slot: Vec<Option<usize>>,
    next_first: Vec<u64>,
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
            positions: Vec::with_capacity(MAX_PROBES),
            slot: Vec::with_capacity(MAX_PROBES),
            first_frame: Vec::with_capacity(MAX_PROBES),
            ages: Vec::with_capacity(MAX_PROBES),
            captured: vec![None; MAX_PROBES],
            capture_frame: vec![0; MAX_PROBES],
            used: vec![false; MAX_PROBES],
            matched: Vec::with_capacity(MAX_PROBES),
            taken: Vec::with_capacity(MAX_PROBES),
            pairs: Vec::with_capacity(MAX_PROBES * MAX_PROBES),
            next_slot: Vec::with_capacity(MAX_PROBES),
            next_first: Vec::with_capacity(MAX_PROBES),
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

    /// Matches the new frame's balls to the last frame's (`self.matched`).
    fn track(&mut self, positions: &[[f32; 3]]) {
        let n = positions.len();
        let prev = &self.positions;
        let near = |a: [f32; 3], b: [f32; 3]| -> Option<f32> {
            let d2 = (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2);
            (d2 <= TRACK_MAX_JUMP_M * TRACK_MAX_JUMP_M).then_some(d2)
        };
        self.matched.clear();
        // Fast path: the same balls in the same order.
        if n == prev.len()
            && positions
                .iter()
                .zip(prev)
                .all(|(&a, &b)| near(a, b).is_some())
        {
            self.matched.extend((0..n).map(Some));
            return;
        }
        self.matched.resize(n, None);
        self.pairs.clear();
        for (j, &p) in positions.iter().enumerate() {
            for (k, &q) in prev.iter().enumerate() {
                if let Some(d2) = near(p, q) {
                    self.pairs.push((d2, j, k));
                }
            }
        }
        self.pairs
            .sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        self.taken.clear();
        self.taken.resize(prev.len(), false);
        for &(_, j, k) in &self.pairs {
            if self.matched[j].is_none() && !self.taken[k] {
                self.matched[j] = Some(k);
                self.taken[k] = true;
            }
        }
    }

    /// Age of ball `j` of the current frame before this frame's captures.
    fn age_of(&self, j: usize) -> u64 {
        match self.slot[j] {
            Some(s) if self.captured[s].is_some() => self.frame - self.capture_frame[s],
            _ => self.frame - self.first_frame[j],
        }
    }

    /// Advances one frame with the balls at `positions` (centres, world) and
    /// returns the ball indices whose probes are captured this frame (at
    /// most the budget; each such ball has a slot, see
    /// [`slot_of`](Self::slot_of)).
    pub fn schedule(&mut self, positions: &[[f32; 3]]) -> &[usize] {
        self.frame += 1;
        let f = self.frame;
        let n = positions.len();
        self.track(positions);

        // Carry slots and first frames over to the new indices.
        self.next_slot.clear();
        self.next_first.clear();
        for j in 0..n {
            match self.matched[j] {
                Some(k) => {
                    self.next_slot.push(self.slot[k]);
                    self.next_first.push(self.first_frame[k]);
                }
                None => {
                    self.next_slot.push(None);
                    self.next_first.push(f);
                }
            }
        }
        core::mem::swap(&mut self.slot, &mut self.next_slot);
        core::mem::swap(&mut self.first_frame, &mut self.next_first);
        // Free the slots of balls that left; give free slots to new balls.
        self.used.iter_mut().for_each(|u| *u = false);
        for s in self.slot.iter().flatten() {
            self.used[*s] = true;
        }
        for s in 0..MAX_PROBES {
            if !self.used[s] {
                self.captured[s] = None;
            }
        }
        let mut free = 0;
        for j in 0..n {
            if self.slot[j].is_some() {
                continue;
            }
            while free < MAX_PROBES && self.used[free] {
                free += 1;
            }
            if free == MAX_PROBES {
                break;
            }
            self.used[free] = true;
            self.slot[j] = Some(free);
        }
        self.positions.clear();
        self.positions.extend_from_slice(positions);

        // Captures: uncaptured first, then oldest, ties → lower index.
        self.updates.clear();
        for _ in 0..self.budget {
            let mut best: Option<(bool, u64, usize)> = None;
            for j in 0..n {
                let Some(s) = self.slot[j] else { continue };
                if self.updates.contains(&j) {
                    continue;
                }
                let key = (self.captured[s].is_none(), self.age_of(j), j);
                let better = match best {
                    None => true,
                    Some((u, a, _)) => (key.0, key.1) > (u, a),
                };
                if better {
                    best = Some(key);
                }
            }
            let Some((_, _, j)) = best else { break };
            let s = self.slot[j].unwrap_or(0);
            self.captured[s] = Some(positions[j]);
            self.capture_frame[s] = f;
            self.updates.push(j);
        }

        // Ages after the captures; balls without a slot report the age of
        // the probe they reuse.
        self.ages.clear();
        for j in 0..n {
            let age = match self.slot[j] {
                Some(_) => self.age_of(j),
                None => match self.probe_for_ball(j) {
                    Some(s) => f - self.capture_frame[s],
                    None => f - self.first_frame[j],
                },
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

    /// The probe slot ball `i` of the last frame owns (`None` past
    /// [`MAX_PROBES`] balls or for an unknown ball). Its capture may not
    /// exist yet; [`probe_for_ball`](Self::probe_for_ball) is what it
    /// samples.
    pub fn slot_of(&self, i: usize) -> Option<usize> {
        self.slot.get(i).copied().flatten()
    }

    /// The probe slot ball `i` samples: its own slot once captured, else the
    /// captured slot whose capture position is nearest to the ball (ties →
    /// lower slot); `None` before any capture or for an unknown ball.
    pub fn probe_for_ball(&self, i: usize) -> Option<usize> {
        let p = *self.positions.get(i)?;
        if let Some(s) = self.slot_of(i) {
            if self.captured[s].is_some() {
                return Some(s);
            }
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

    /// Per ball: frames since its probe was last captured (0 = this frame).
    /// A ball without a capture of its own counts from its first frame (a
    /// ball that jumped further than [`TRACK_MAX_JUMP_M`] counts as new); a
    /// ball past `MAX_PROBES` reports the age of the probe it reuses.
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
