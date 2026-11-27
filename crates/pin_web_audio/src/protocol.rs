//! The trigger message protocol (REQ-WEB-022): 16-byte command records, their
//! parser and the fixed pending list that schedules them on render quanta.
//!
//! Plain safe Rust with no allocation after [`Scheduler::new`], so it is
//! property-tested natively and runs unchanged inside the worklet.
//!
//! # Record format (16 bytes, little-endian)
//! | bytes | field | meaning |
//! |-------|-------|---------|
//! | 0..4  | `u32 op` | [`OP_TRIGGER`] (1); anything else is rejected |
//! | 4..8  | `u32 sound_id` | values above 65535 are rejected |
//! | 8..12 | `f32 gain` | any bit pattern (NaN plays silently) |
//! | 12..16 | `u32 at` | [`NEXT_QUANTUM`], or the quantum index at which to apply |
//!
//! # Scheduling
//! Let `q` be the quantum index (successful renders so far, i.e. the index of
//! the next render). A record's effective quantum is `q` for
//! [`NEXT_QUANTUM`], else `max(at, q)`. Accepted records enter a pending
//! list of [`PENDING_CAPACITY`] entries kept in (effective quantum, arrival)
//! order; a full list drops the record and counts it in
//! [`Scheduler::dropped`]. At the start of each render every record whose
//! effective quantum is `<= q` is released in that order
//! ([`Scheduler::release_due`]); then the quantum index advances
//! ([`Scheduler::advance`]).

use pin_audio::SoundTrigger;
use pin_core::SoundId;

/// Size of one command record in bytes.
pub const RECORD_BYTES: usize = 16;

/// Capacity of the engine's command staging buffer in bytes (256 records).
pub const CMD_BUFFER_BYTES: usize = 4096;

/// Entries of the fixed pending list.
pub const PENDING_CAPACITY: usize = 256;

/// The `op` code of a trigger record.
pub const OP_TRIGGER: u32 = 1;

/// The `at` value meaning "apply at the next quantum".
pub const NEXT_QUANTUM: u32 = u32::MAX;

/// Encodes a trigger record (`op = 1`, `sound_id`, `gain`, `at`). `at` is a
/// quantum index or [`NEXT_QUANTUM`].
pub fn encode_trigger(sound_id: u32, gain: f32, at: u32) -> [u8; RECORD_BYTES] {
    let mut r = [0u8; RECORD_BYTES];
    r[0..4].copy_from_slice(&OP_TRIGGER.to_le_bytes());
    r[4..8].copy_from_slice(&sound_id.to_le_bytes());
    r[8..12].copy_from_slice(&gain.to_bits().to_le_bytes());
    r[12..16].copy_from_slice(&at.to_le_bytes());
    r
}

/// When a parsed command applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum At {
    /// At the next quantum ([`NEXT_QUANTUM`] on the wire).
    Next,
    /// At this quantum index, or the next quantum if it is in the past.
    Quantum(u32),
}

impl At {
    /// Decodes an `at` field.
    pub fn from_wire(at: u32) -> Self {
        if at == NEXT_QUANTUM {
            At::Next
        } else {
            At::Quantum(at)
        }
    }

    /// The effective quantum given the current quantum index `q`.
    pub fn effective(self, q: u64) -> u64 {
        match self {
            At::Next => q,
            At::Quantum(at) => u64::from(at).max(q),
        }
    }
}

/// A valid trigger command.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Command {
    /// Sound to start.
    pub sound: SoundId,
    /// Voice gain, bit for bit as sent.
    pub gain: f32,
    /// When it applies.
    pub at: At,
}

/// Why a record was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reject {
    /// `op` is not [`OP_TRIGGER`] (carries the op).
    UnknownOp(u32),
    /// `sound_id` is above 65535 (carries the id).
    SoundIdOutOfRange(u32),
    /// Fewer than [`RECORD_BYTES`] bytes (a trailing partial record).
    Truncated,
}

fn le_u32(b: &[u8], at: usize) -> u32 {
    let mut w = [0u8; 4];
    if let Some(s) = at.checked_add(4).and_then(|end| b.get(at..end)) {
        w.copy_from_slice(s);
    }
    u32::from_le_bytes(w)
}

/// Parses one record. Bytes past the first [`RECORD_BYTES`] are ignored;
/// a shorter slice is [`Reject::Truncated`]. Never panics.
pub fn parse_record(bytes: &[u8]) -> Result<Command, Reject> {
    if bytes.len() < RECORD_BYTES {
        return Err(Reject::Truncated);
    }
    let op = le_u32(bytes, 0);
    if op != OP_TRIGGER {
        return Err(Reject::UnknownOp(op));
    }
    let id = le_u32(bytes, 4);
    let Ok(id16) = u16::try_from(id) else {
        return Err(Reject::SoundIdOutOfRange(id));
    };
    Ok(Command {
        sound: SoundId(id16),
        gain: f32::from_bits(le_u32(bytes, 8)),
        at: At::from_wire(le_u32(bytes, 12)),
    })
}

/// One pending trigger.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pending {
    quantum: u64,
    trigger: SoundTrigger,
}

/// The fixed pending list and the quantum clock (module docs).
#[derive(Debug)]
pub struct Scheduler {
    /// Sorted by (effective quantum, arrival); capacity [`PENDING_CAPACITY`],
    /// never exceeded, so inserts and removals never allocate.
    pending: Vec<Pending>,
    quantum: u64,
    dropped: u64,
    rejected: u64,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler {
    /// An empty scheduler at quantum 0. This is its only allocation.
    pub fn new() -> Self {
        Self {
            pending: Vec::with_capacity(PENDING_CAPACITY),
            quantum: 0,
            dropped: 0,
            rejected: 0,
        }
    }

    /// The quantum index (number of [`advance`](Self::advance) calls).
    pub fn quantum(&self) -> u64 {
        self.quantum
    }

    /// Commands waiting in the pending list.
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Triggers dropped so far: pending list full, or (counted by
    /// [`release_due`](Self::release_due)) refused by the sink.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Records rejected so far ([`Reject`]).
    pub fn rejected(&self) -> u64 {
        self.rejected
    }

    /// Counts one rejected record.
    pub fn reject(&mut self) {
        self.rejected = self.rejected.saturating_add(1);
    }

    /// Queues a valid command; returns `false` (counted in
    /// [`dropped`](Self::dropped)) when the pending list is full.
    pub fn push(&mut self, cmd: Command) -> bool {
        if self.pending.len() >= PENDING_CAPACITY {
            self.dropped = self.dropped.saturating_add(1);
            return false;
        }
        let quantum = cmd.at.effective(self.quantum);
        // After every entry with an effective quantum <= this one: stable,
        // so equal quanta keep arrival order.
        let pos = self.pending.partition_point(|p| p.quantum <= quantum);
        self.pending.insert(
            pos,
            Pending {
                quantum,
                trigger: SoundTrigger {
                    sound: cmd.sound,
                    gain: cmd.gain,
                },
            },
        );
        true
    }

    /// Parses and queues one record; returns whether it entered the pending
    /// list. A rejected record counts in [`rejected`](Self::rejected).
    pub fn push_record(&mut self, bytes: &[u8]) -> bool {
        match parse_record(bytes) {
            Ok(cmd) => self.push(cmd),
            Err(_) => {
                self.reject();
                false
            }
        }
    }

    /// Parses `bytes` as consecutive records and queues the valid ones;
    /// returns how many entered the pending list. A trailing partial record
    /// counts as one rejected record. Never allocates or panics.
    pub fn submit(&mut self, bytes: &[u8]) -> u32 {
        let mut accepted = 0u32;
        for rec in bytes.chunks(RECORD_BYTES) {
            if self.push_record(rec) {
                accepted = accepted.saturating_add(1);
            }
        }
        accepted
    }

    /// Hands every pending trigger whose effective quantum is `<=` the
    /// current quantum to `sink`, in (effective quantum, arrival) order, and
    /// removes them; a `false` from `sink` counts in
    /// [`dropped`](Self::dropped). Returns how many were released.
    pub fn release_due(&mut self, mut sink: impl FnMut(SoundTrigger) -> bool) -> usize {
        let q = self.quantum;
        let due = self.pending.partition_point(|p| p.quantum <= q);
        for p in self.pending.iter().take(due) {
            if !sink(p.trigger) {
                self.dropped = self.dropped.saturating_add(1);
            }
        }
        self.pending.drain(..due);
        due
    }

    /// Advances the quantum index by one (after a successful render).
    pub fn advance(&mut self) {
        self.quantum = self.quantum.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(id: u16, gain: f32, at: At) -> Command {
        Command {
            sound: SoundId(id),
            gain,
            at,
        }
    }

    /// REQ-WEB-022: the record layout is little-endian op, id, gain bits, at.
    #[test]
    fn encode_layout() {
        let r = encode_trigger(0x0102, 1.0, 7);
        assert_eq!(&r[0..4], &[1, 0, 0, 0]);
        assert_eq!(&r[4..8], &[2, 1, 0, 0]);
        assert_eq!(&r[8..12], &1.0f32.to_bits().to_le_bytes());
        assert_eq!(&r[12..16], &[7, 0, 0, 0]);
        assert_eq!(parse_record(&r), Ok(cmd(0x0102, 1.0, At::Quantum(7))));
        let n = parse_record(&encode_trigger(5, 0.5, NEXT_QUANTUM)).unwrap();
        assert_eq!(n.at, At::Next);
    }

    /// REQ-WEB-022: rejects bad ops, ids above 65535 and short records.
    #[test]
    fn parse_rejects() {
        let mut r = encode_trigger(1, 1.0, 0);
        r[0] = 2;
        assert_eq!(parse_record(&r), Err(Reject::UnknownOp(2)));
        r[0] = 0;
        assert_eq!(parse_record(&r), Err(Reject::UnknownOp(0)));
        assert_eq!(
            parse_record(&encode_trigger(65_536, 1.0, 0)),
            Err(Reject::SoundIdOutOfRange(65_536))
        );
        assert!(parse_record(&encode_trigger(65_535, 1.0, 0)).is_ok());
        assert_eq!(parse_record(&r[..15]), Err(Reject::Truncated));
        assert_eq!(parse_record(&[]), Err(Reject::Truncated));
    }

    /// REQ-WEB-022: effective quantum = q for next, max(at, q) otherwise.
    #[test]
    fn effective_quantum() {
        assert_eq!(At::Next.effective(9), 9);
        assert_eq!(At::Quantum(3).effective(9), 9);
        assert_eq!(At::Quantum(12).effective(9), 12);
        assert_eq!(At::from_wire(u32::MAX), At::Next);
        assert_eq!(At::from_wire(u32::MAX - 1), At::Quantum(u32::MAX - 1));
    }

    /// REQ-WEB-022: release order is (effective quantum, arrival); past `at`
    /// applies at the next quantum; future entries wait.
    #[test]
    fn schedule_order() {
        let mut s = Scheduler::new();
        s.advance();
        s.advance(); // q = 2
        assert!(s.push(cmd(1, 1.0, At::Quantum(4))));
        assert!(s.push(cmd(2, 1.0, At::Quantum(3))));
        assert!(s.push(cmd(3, 1.0, At::Quantum(0)))); // past -> 2
        assert!(s.push(cmd(4, 1.0, At::Next))); // 2
        assert!(s.push(cmd(5, 1.0, At::Quantum(3))));
        let mut out = Vec::new();
        let run = |s: &mut Scheduler| {
            let mut v = Vec::new();
            s.release_due(|t| {
                v.push(t.sound.0);
                true
            });
            s.advance();
            v
        };
        out.push(run(&mut s));
        out.push(run(&mut s));
        out.push(run(&mut s));
        out.push(run(&mut s));
        assert_eq!(out, vec![vec![3, 4], vec![2, 5], vec![1], vec![]]);
        assert_eq!(s.pending_len(), 0);
        assert_eq!(s.quantum(), 6);
    }

    /// REQ-WEB-022: a full pending list drops and counts; a refusing sink
    /// counts as dropped too.
    #[test]
    fn pending_full_and_sink_refusal() {
        let mut s = Scheduler::new();
        for i in 0..PENDING_CAPACITY {
            assert!(s.push(cmd(i as u16, 1.0, At::Next)));
        }
        assert!(!s.push(cmd(9, 1.0, At::Next)));
        assert_eq!(s.dropped(), 1);
        let mut n = 0;
        let released = s.release_due(|_| {
            n += 1;
            n <= 10
        });
        assert_eq!(released, PENDING_CAPACITY);
        assert_eq!(s.dropped(), 1 + (PENDING_CAPACITY as u64 - 10));
    }

    /// REQ-WEB-022: submit counts accepted, rejected and a trailing partial.
    #[test]
    fn submit_counts() {
        let mut s = Scheduler::new();
        let mut b = Vec::new();
        b.extend_from_slice(&encode_trigger(1, 1.0, NEXT_QUANTUM));
        b.extend_from_slice(&encode_trigger(70_000, 1.0, NEXT_QUANTUM));
        b.extend_from_slice(&encode_trigger(2, f32::NAN, 5));
        b.extend_from_slice(&[1, 0, 0]);
        assert_eq!(s.submit(&b), 2);
        assert_eq!(s.rejected(), 2);
        assert_eq!(s.pending_len(), 2);
        assert_eq!(s.submit(&[]), 0);
        let mut r = encode_trigger(1, 1.0, 0);
        r[1] = 1; // op 257
        assert!(!s.push_record(&r));
        assert_eq!(s.rejected(), 3);
        assert_eq!(Scheduler::default().quantum(), 0);
    }
}
