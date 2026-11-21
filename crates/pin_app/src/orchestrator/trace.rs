//! Latency trace (`pin_app --latency-trace <file>`, REQ-LAT-003, KP-002).
//!
//! The trace records, for every button event the sim loop delivers to the
//! step, the SDL timestamp, the tick that applied it, the tick the bat first
//! moved, the first presented frame that drew it and that frame's present
//! time, so the windowed harness (`crates/pin_app/tests/windowed/`) can check
//! REQ-LAT-001 (key to photon, in presented frames), REQ-LAT-002 (dropped
//! ticks) and REQ-LAT-004 (key to coil) on the real windowed app.
//!
//! # Real-time constraints (D5, R4)
//!
//! [`latency_trace`] allocates every buffer before the threads start: one
//! [`pin_core::spsc`] ring per producer (input rows and bat-move rows from
//! the sim thread, frame rows from the render thread). The sim thread
//! ([`SimTraceWriter`], driven by [`SimLoop`](super::SimLoop)) and the
//! render thread ([`FrameTraceWriter`]) only push fixed-size records; a
//! full ring is a counted overflow ([`TraceOverflows`]), never a panic, a
//! block or an allocation. The main thread drains the rings with
//! [`TraceCollector::collect`] after every thread was joined and writes the
//! file ([`CollectedTrace::to_csv`]). Without `--latency-trace` none of this
//! exists: no writer is installed and the loops run their default paths.
//!
//! # File format (version 1)
//!
//! UTF-8 text, `\n` line ends, comma separated, integers only (ns, ticks,
//! frame indices); an empty field means "none". Three parts, in order:
//!
//! 1. The **event table**: the header line
//!    [`EVENT_HEADER`] followed by one row per delivered button event, in
//!    delivery order:
//!    * `event_seq`: 0-based sequence number of the recorded button event
//!      (a ring overflow leaves a gap);
//!    * `input`: the cabinet input ([`CabinetInput::name`], e.g.
//!      `left_flipper`);
//!    * `down`: `1` press, `0` release;
//!    * `sdl_ts_ns`: the raw SDL event timestamp (`SDL_GetTicksNS` clock);
//!    * `apply_tick`: the tick whose sim-loop iteration delivered the event
//!      to the step;
//!    * `apply_tick_end_sdl_ns`: that tick's end on the SDL clock,
//!      `tick_end_ns(epoch at that tick, apply_tick)`;
//!    * `prev_tick_end_sdl_ns`: the end of the previous executed tick on the
//!      SDL clock (its epoch; the sim start for the first tick);
//!    * `drain_sdl_ns`: the SDL clock read when the applying tick started
//!      (when it drained the input ring);
//!    * `prev_drain_sdl_ns`: the same for the previous executed tick (the
//!      sim start for the first tick);
//!    * `dropped_ticks_at_apply`: ticks dropped by the catch-up cap so far;
//!    * `stall`: `1` when the sim thread was not keeping up right before the
//!      applying tick: the gap `drain_sdl_ns - prev_drain_sdl_ns` exceeds
//!      [`STALL_GAP_NS`], or ticks were dropped since the previous executed
//!      tick. It is derived only from the sim thread's own scheduling, never
//!      from the event's lateness;
//!    * `move_tick`: for a press of an input bound to flippers
//!      ([`FlipperBindings`]), the first tick from the applying tick on at
//!      whose end one of those bats' `angle_rad` differs from its value at
//!      the applying tick's start; empty otherwise (a release, an unbound
//!      input, or no move within [`MOVE_TIMEOUT_TICKS`]);
//!    * `frame`: the index of the first presented frame whose snapshot tick
//!      is `>= move_tick` (empty without a move or such a frame);
//!    * `frame_snap_tick`, `present_ns`: that frame's snapshot tick and
//!      present time (SDL clock, read when the present call returned);
//!    * `frames_to_photon`: `frame - F0`, where `F0` is the index of the
//!      last presented frame whose `present_ns <= sdl_ts_ns` (0 when none).
//! 2. One empty line, then the **frame table**: the header line
//!    [`FRAME_HEADER`] (`frame,snap_tick,present_ns`) and one row per
//!    presented frame (0-based index in present order, the snapshot tick it
//!    drew, its present time on the SDL clock).
//! 3. The **trailer**: one line
//!    `# overflows: input=<n> move=<n> frame=<n> pending=<n>`, the rows lost
//!    to full rings ([`TraceOverflows`]).
//!
//! Column names are stable (later units read this file: Gate A3, KP-043).

use pin_core::spsc::{SpscConsumer, SpscProducer, channel};
use pin_core::{
    CabinetInput, FrameStateSnapshot, InputEvent, InputEventKind, flipper::FlipperConfig,
    flipper::FlipperStagingMode, snapshot::MAX_FLIPPERS,
};
use pin_render::gpu::SurfaceStatus;
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Default capacity of the input-row ring (and of the bat-move ring).
pub const INPUT_TRACE_CAPACITY: usize = 65_536;
/// Default capacity of the frame-row ring.
pub const FRAME_TRACE_CAPACITY: usize = 262_144;
/// Presses waiting for their bat to move at once; more are counted as
/// `pending` overflows (their `move_tick` stays empty).
pub const MAX_PENDING_MOVES: usize = 32;
/// A press whose bat has not moved this many ticks after it was applied
/// stops being watched (`move_tick` empty).
pub const MOVE_TIMEOUT_TICKS: u64 = 5_000;
/// A gap between the starts of two executed ticks longer than this (2 ms:
/// two tick periods) marks the later tick as applied after a stall.
pub const STALL_GAP_NS: u64 = 2_000_000;

/// Header line of the event table (format version 1).
pub const EVENT_HEADER: &str = "event_seq,input,down,sdl_ts_ns,apply_tick,apply_tick_end_sdl_ns,\
prev_tick_end_sdl_ns,drain_sdl_ns,prev_drain_sdl_ns,dropped_ticks_at_apply,stall,move_tick,\
frame,frame_snap_tick,present_ns,frames_to_photon";
/// Header line of the frame table.
pub const FRAME_HEADER: &str = "frame,snap_tick,present_ns";
/// Prefix of the trailer line.
pub const TRAILER_PREFIX: &str = "# overflows:";

/// Which flippers (bit `i` = `FlipperId(i)`) each cabinet input drives.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct FlipperBindings {
    masks: [u16; CabinetInput::COUNT],
}

impl FlipperBindings {
    /// No bindings (no press is watched for a bat move).
    pub const fn new() -> Self {
        Self {
            masks: [0; CabinetInput::COUNT],
        }
    }

    /// Binds `input` to flipper id `flipper` (ids `>= MAX_FLIPPERS` are
    /// ignored).
    pub fn bind(&mut self, input: CabinetInput, flipper: u16) {
        if usize::from(flipper) < MAX_FLIPPERS {
            self.masks[input.index()] |= 1 << flipper;
        }
    }

    /// The flippers `input` drives, as a bit mask.
    pub fn mask(&self, input: CabinetInput) -> u16 {
        self.masks[input.index()]
    }

    /// The bindings of a table's flippers: a `DedicatedButton` or
    /// `ParallelLinked` flipper is bound to its button.
    pub fn of_flippers(flippers: &[FlipperConfig]) -> Self {
        let mut b = Self::new();
        for f in flippers {
            match f.staging {
                FlipperStagingMode::DedicatedButton(i) | FlipperStagingMode::ParallelLinked(i) => {
                    b.bind(i, u16::from(f.id));
                }
                _ => {}
            }
        }
        b
    }
}

/// One delivered button event, as the sim thread records it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct InputRecord {
    /// `event_seq`.
    pub seq: u64,
    /// `input`.
    pub input: CabinetInput,
    /// `down`.
    pub down: bool,
    /// `sdl_ts_ns`.
    pub sdl_ts_ns: u64,
    /// `apply_tick`.
    pub apply_tick: u64,
    /// `apply_tick_end_sdl_ns`.
    pub apply_tick_end_sdl_ns: u64,
    /// `prev_tick_end_sdl_ns`.
    pub prev_tick_end_sdl_ns: u64,
    /// `drain_sdl_ns`.
    pub drain_sdl_ns: u64,
    /// `prev_drain_sdl_ns`.
    pub prev_drain_sdl_ns: u64,
    /// `dropped_ticks_at_apply`.
    pub dropped_ticks_at_apply: u64,
    /// `stall`.
    pub stall: bool,
}

/// The first tick a press's bat moved at.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct MoveRecord {
    /// `event_seq` of the press.
    pub seq: u64,
    /// `move_tick`.
    pub move_tick: u64,
}

/// One presented frame.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct FrameRecord {
    /// 0-based index in present order.
    pub frame: u64,
    /// The snapshot tick the frame drew.
    pub snap_tick: u64,
    /// SDL clock when the present call returned, ns.
    pub present_ns: u64,
}

/// One event-table row: the sim's record joined with its bat move and the
/// frame that drew it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TraceRow {
    /// The sim thread's record.
    pub input: InputRecord,
    /// `move_tick`.
    pub move_tick: Option<u64>,
    /// `frame`.
    pub frame: Option<u64>,
    /// `frame_snap_tick`.
    pub frame_snap_tick: Option<u64>,
    /// `present_ns`.
    pub present_ns: Option<u64>,
    /// `frames_to_photon`.
    pub frames_to_photon: Option<u64>,
}

/// Rows lost to full rings or a full pending-move table.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct TraceOverflows {
    /// Input rows lost (input ring full).
    pub input: u64,
    /// Bat-move rows lost (move ring full).
    pub moves: u64,
    /// Frame rows lost (frame ring full).
    pub frames: u64,
    /// Presses not watched for a move ([`MAX_PENDING_MOVES`] were pending).
    pub pending: u64,
}

impl TraceOverflows {
    /// Sum of every counter.
    pub fn total(&self) -> u64 {
        self.input + self.moves + self.frames + self.pending
    }
}

#[derive(Debug, Default)]
struct Counters {
    input: AtomicU64,
    moves: AtomicU64,
    frames: AtomicU64,
    pending: AtomicU64,
}

impl Counters {
    fn read(&self) -> TraceOverflows {
        TraceOverflows {
            input: self.input.load(Ordering::Relaxed),
            moves: self.moves.load(Ordering::Relaxed),
            frames: self.frames.load(Ordering::Relaxed),
            pending: self.pending.load(Ordering::Relaxed),
        }
    }
}

/// Scheduling marks of one executed tick.
#[derive(Copy, Clone, Debug, Default)]
struct TickMark {
    drain_ns: u64,
    end_ns: u64,
    dropped: u64,
}

/// A press waiting for its bat to move.
#[derive(Copy, Clone, Debug)]
struct PendingMove {
    seq: u64,
    mask: u16,
    base: [f32; MAX_FLIPPERS],
    deadline: u64,
}

/// Bat angles of `snap` by flipper id (0 for a flipper not in it).
fn angles(snap: &FrameStateSnapshot) -> [f32; MAX_FLIPPERS] {
    let mut a = [0.0; MAX_FLIPPERS];
    for st in snap.flippers() {
        if let Some(slot) = a.get_mut(usize::from(u16::from(st.id))) {
            *slot = st.angle_rad;
        }
    }
    a
}

/// Whether one of `mask`'s bats in `snap` differs from `base`.
fn moved(snap: &FrameStateSnapshot, mask: u16, base: &[f32; MAX_FLIPPERS]) -> bool {
    snap.flippers().iter().any(|st| {
        let id = usize::from(u16::from(st.id));
        id < MAX_FLIPPERS && mask & (1 << id) != 0 && st.angle_rad.to_bits() != base[id].to_bits()
    })
}

/// The sim thread's half of the trace, installed with
/// [`SimLoop::set_latency_trace`](super::SimLoop::set_latency_trace). Every
/// method is allocation-, lock- and I/O-free (D5).
pub struct SimTraceWriter {
    inputs: SpscProducer<InputRecord>,
    moves: SpscProducer<MoveRecord>,
    counters: Arc<Counters>,
    clock: fn() -> u64,
    bindings: FlipperBindings,
    next_seq: u64,
    dropped: u64,
    prev: TickMark,
    cur: TickMark,
    cur_stall: bool,
    base: [f32; MAX_FLIPPERS],
    pending: arrayvec::ArrayVec<PendingMove, MAX_PENDING_MOVES>,
}

impl SimTraceWriter {
    /// The sim starts at `epoch_ns` (SDL clock): the "previous tick" marks
    /// of the first executed tick.
    pub fn start(&mut self, epoch_ns: u64) {
        self.prev = TickMark {
            drain_ns: epoch_ns,
            end_ns: epoch_ns,
            dropped: self.dropped,
        };
    }

    /// `ticks` more ticks were dropped by the catch-up cap.
    pub fn ticks_dropped(&mut self, ticks: u64) {
        self.dropped = self.dropped.saturating_add(ticks);
    }

    /// A tick ending at `end_ns` (SDL clock) starts; `before` is the
    /// snapshot of the previous tick (the bats at this tick's start). Reads
    /// the trace clock once.
    pub fn begin_tick(&mut self, end_ns: u64, before: &FrameStateSnapshot) {
        let drain_ns = (self.clock)();
        self.cur = TickMark {
            drain_ns,
            end_ns,
            dropped: self.dropped,
        };
        self.cur_stall = drain_ns.saturating_sub(self.prev.drain_ns) > STALL_GAP_NS
            || self.dropped > self.prev.dropped;
        self.base = angles(before);
    }

    /// The current tick (`tick`) delivers `ev` (raw SDL stamp) to the step.
    /// Button events are recorded; a press of a bound input is watched for
    /// its bat to move.
    pub fn delivered(&mut self, tick: u64, ev: &InputEvent) {
        let InputEventKind::Button { input, pressed } = ev.kind else {
            return;
        };
        let seq = self.next_seq;
        self.next_seq += 1;
        let rec = InputRecord {
            seq,
            input,
            down: pressed,
            sdl_ts_ns: ev.timestamp_ns,
            apply_tick: tick,
            apply_tick_end_sdl_ns: self.cur.end_ns,
            prev_tick_end_sdl_ns: self.prev.end_ns,
            drain_sdl_ns: self.cur.drain_ns,
            prev_drain_sdl_ns: self.prev.drain_ns,
            dropped_ticks_at_apply: self.cur.dropped,
            stall: self.cur_stall,
        };
        if self.inputs.push(rec).is_err() {
            self.counters.input.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let mask = self.bindings.mask(input);
        if pressed && mask != 0 {
            let p = PendingMove {
                seq,
                mask,
                base: self.base,
                deadline: tick.saturating_add(MOVE_TIMEOUT_TICKS),
            };
            if self.pending.try_push(p).is_err() {
                self.counters.pending.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// The current tick (`tick`) was stepped; `after` is its snapshot.
    /// Records the presses whose bat moved.
    pub fn end_tick(&mut self, tick: u64, after: &FrameStateSnapshot) {
        let (moves, counters) = (&mut self.moves, &self.counters);
        self.pending.retain(|p| {
            if moved(after, p.mask, &p.base) {
                let m = MoveRecord {
                    seq: p.seq,
                    move_tick: tick,
                };
                if moves.push(m).is_err() {
                    counters.moves.fetch_add(1, Ordering::Relaxed);
                }
                false
            } else {
                tick < p.deadline
            }
        });
        self.prev = self.cur;
    }
}

/// The render thread's half of the trace ([`RenderConfig::latency_trace`](super::RenderConfig::latency_trace)).
/// Allocation-, lock- and I/O-free (D5).
pub struct FrameTraceWriter {
    frames: SpscProducer<FrameRecord>,
    counters: Arc<Counters>,
    clock: fn() -> u64,
    next_frame: u64,
}

impl FrameTraceWriter {
    /// Records one render-loop iteration that drew `snap_tick`: when one of
    /// `statuses` is a present, a frame row with the trace clock read now
    /// (call it right after the present returned). Returns whether a frame
    /// was counted.
    pub fn presented(&mut self, statuses: &[SurfaceStatus], snap_tick: u64) -> bool {
        if !statuses
            .iter()
            .any(|s| matches!(s, SurfaceStatus::Presented))
        {
            return false;
        }
        let rec = FrameRecord {
            frame: self.next_frame,
            snap_tick,
            present_ns: (self.clock)(),
        };
        self.next_frame += 1;
        if self.frames.push(rec).is_err() {
            self.counters.frames.fetch_add(1, Ordering::Relaxed);
        }
        true
    }
}

/// The main thread's half: drains the rings after the threads were joined.
pub struct TraceCollector {
    inputs: SpscConsumer<InputRecord>,
    moves: SpscConsumer<MoveRecord>,
    frames: SpscConsumer<FrameRecord>,
    counters: Arc<Counters>,
}

/// A latency trace with the default capacities ([`INPUT_TRACE_CAPACITY`],
/// [`FRAME_TRACE_CAPACITY`]): the collector, the sim thread's writer and
/// the render thread's writer, every buffer allocated now. `clock` is the
/// SDL event clock (the app passes `SDL_GetTicksNS`).
pub fn latency_trace(
    bindings: FlipperBindings,
    clock: fn() -> u64,
) -> (TraceCollector, SimTraceWriter, FrameTraceWriter) {
    latency_trace_with_capacity(bindings, clock, INPUT_TRACE_CAPACITY, FRAME_TRACE_CAPACITY)
}

/// [`latency_trace`] with explicit ring capacities (each at least 1).
pub fn latency_trace_with_capacity(
    bindings: FlipperBindings,
    clock: fn() -> u64,
    input_capacity: usize,
    frame_capacity: usize,
) -> (TraceCollector, SimTraceWriter, FrameTraceWriter) {
    let (itx, irx) = channel(input_capacity.max(1));
    let (mtx, mrx) = channel(input_capacity.max(1));
    let (ftx, frx) = channel(frame_capacity.max(1));
    let counters = Arc::new(Counters::default());
    let sim = SimTraceWriter {
        inputs: itx,
        moves: mtx,
        counters: Arc::clone(&counters),
        clock,
        bindings,
        next_seq: 0,
        dropped: 0,
        prev: TickMark::default(),
        cur: TickMark::default(),
        cur_stall: false,
        base: [0.0; MAX_FLIPPERS],
        pending: arrayvec::ArrayVec::new(),
    };
    let render = FrameTraceWriter {
        frames: ftx,
        counters: Arc::clone(&counters),
        clock,
        next_frame: 0,
    };
    let collector = TraceCollector {
        inputs: irx,
        moves: mrx,
        frames: frx,
        counters,
    };
    (collector, sim, render)
}

impl TraceCollector {
    /// Drains every ring and joins the rows ([`join_trace`]). Call it after
    /// the writers' threads were joined.
    pub fn collect(mut self) -> CollectedTrace {
        let mut inputs = Vec::with_capacity(self.inputs.len());
        while let Some(r) = self.inputs.pop() {
            inputs.push(r);
        }
        let mut moves = Vec::with_capacity(self.moves.len());
        while let Some(m) = self.moves.pop() {
            moves.push(m);
        }
        let mut frames = Vec::with_capacity(self.frames.len());
        while let Some(f) = self.frames.pop() {
            frames.push(f);
        }
        CollectedTrace {
            rows: join_trace(&inputs, &moves, &frames),
            frames,
            overflows: self.counters.read(),
        }
    }
}

/// Index of the last frame of `frames` (in present order) presented at or
/// before `t_ns`, or 0 when none.
pub fn last_frame_at_or_before(frames: &[FrameRecord], t_ns: u64) -> u64 {
    let n = frames.partition_point(|f| f.present_ns <= t_ns);
    n.checked_sub(1).map_or(0, |i| frames[i].frame)
}

/// Joins the sim's input and move records with the frame table: each row
/// gets its `move_tick`, the first frame whose snapshot tick is
/// `>= move_tick`, that frame's snapshot tick and present time, and
/// `frames_to_photon = frame - F0` ([`last_frame_at_or_before`] the
/// press's SDL timestamp). `frames` must be in present order.
pub fn join_trace(
    inputs: &[InputRecord],
    moves: &[MoveRecord],
    frames: &[FrameRecord],
) -> Vec<TraceRow> {
    inputs
        .iter()
        .map(|r| {
            let move_tick = moves.iter().find(|m| m.seq == r.seq).map(|m| m.move_tick);
            let drawn = move_tick.and_then(|t| frames.iter().find(|f| f.snap_tick >= t));
            TraceRow {
                input: *r,
                move_tick,
                frame: drawn.map(|f| f.frame),
                frame_snap_tick: drawn.map(|f| f.snap_tick),
                present_ns: drawn.map(|f| f.present_ns),
                frames_to_photon: drawn.map(|f| {
                    f.frame
                        .saturating_sub(last_frame_at_or_before(frames, r.sdl_ts_ns))
                }),
            }
        })
        .collect()
}

/// A complete trace: the event rows, the frame table and the overflows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CollectedTrace {
    /// The event table.
    pub rows: Vec<TraceRow>,
    /// The frame table, in present order.
    pub frames: Vec<FrameRecord>,
    /// Rows lost to full rings.
    pub overflows: TraceOverflows,
}

fn opt(v: Option<u64>) -> String {
    v.map_or_else(String::new, |v| v.to_string())
}

impl CollectedTrace {
    /// Button events recorded.
    pub fn events(&self) -> usize {
        self.rows.len()
    }

    /// The file text (format version 1, see the module documentation).
    pub fn to_csv(&self) -> String {
        let mut s = String::with_capacity(160 * (self.rows.len() + 2) + 40 * self.frames.len());
        s.push_str(EVENT_HEADER);
        s.push('\n');
        for r in &self.rows {
            let i = &r.input;
            let _ = writeln!(
                s,
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
                i.seq,
                i.input.name(),
                u8::from(i.down),
                i.sdl_ts_ns,
                i.apply_tick,
                i.apply_tick_end_sdl_ns,
                i.prev_tick_end_sdl_ns,
                i.drain_sdl_ns,
                i.prev_drain_sdl_ns,
                i.dropped_ticks_at_apply,
                u8::from(i.stall),
                opt(r.move_tick),
                opt(r.frame),
                opt(r.frame_snap_tick),
                opt(r.present_ns),
                opt(r.frames_to_photon),
            );
        }
        s.push('\n');
        s.push_str(FRAME_HEADER);
        s.push('\n');
        for f in &self.frames {
            let _ = writeln!(s, "{},{},{}", f.frame, f.snap_tick, f.present_ns);
        }
        let o = &self.overflows;
        let _ = writeln!(
            s,
            "{TRAILER_PREFIX} input={} move={} frame={} pending={}",
            o.input, o.moves, o.frames, o.pending
        );
        s
    }

    /// Parses a file written by [`to_csv`](Self::to_csv).
    ///
    /// # Errors
    /// A missing header or trailer, a row with the wrong number of fields,
    /// a field that is not an unsigned integer (or empty where allowed), an
    /// unknown input name or a flag that is not `0` / `1`. Never panics.
    pub fn parse(text: &str) -> Result<CollectedTrace, String> {
        let mut lines = text.split('\n');
        if lines.next() != Some(EVENT_HEADER) {
            return Err("missing event header".to_owned());
        }
        let mut out = CollectedTrace::default();
        let mut section_done = false;
        for (n, line) in lines.by_ref().enumerate() {
            if line.is_empty() {
                section_done = true;
                break;
            }
            out.rows
                .push(parse_row(line).map_err(|e| format!("event row {}: {e}", n + 1))?);
        }
        if !section_done || lines.next() != Some(FRAME_HEADER) {
            return Err("missing frame header".to_owned());
        }
        let mut trailer = None;
        for (n, line) in lines.by_ref().enumerate() {
            if let Some(rest) = line.strip_prefix(TRAILER_PREFIX) {
                trailer = Some(rest);
                break;
            }
            let f = fields::<3>(line).map_err(|e| format!("frame row {}: {e}", n + 1))?;
            out.frames.push(FrameRecord {
                frame: int(f[0])?,
                snap_tick: int(f[1])?,
                present_ns: int(f[2])?,
            });
        }
        let trailer = trailer.ok_or_else(|| "missing trailer".to_owned())?;
        out.overflows = parse_trailer(trailer)?;
        if lines.any(|l| !l.is_empty()) {
            return Err("text after the trailer".to_owned());
        }
        Ok(out)
    }
}

fn fields<const N: usize>(line: &str) -> Result<[&str; N], String> {
    let mut out = [""; N];
    let mut it = line.split(',');
    for slot in &mut out {
        *slot = it.next().ok_or_else(|| format!("expected {N} fields"))?;
    }
    if it.next().is_some() {
        return Err(format!("expected {N} fields"));
    }
    Ok(out)
}

fn int(s: &str) -> Result<u64, String> {
    s.parse::<u64>().map_err(|e| format!("{s:?}: {e}"))
}

fn opt_int(s: &str) -> Result<Option<u64>, String> {
    if s.is_empty() {
        Ok(None)
    } else {
        int(s).map(Some)
    }
}

fn flag(s: &str) -> Result<bool, String> {
    match s {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(format!("{s:?}: not 0 or 1")),
    }
}

fn parse_row(line: &str) -> Result<TraceRow, String> {
    let f = fields::<16>(line)?;
    let input = CabinetInput::from_name(f[1]).ok_or_else(|| format!("input {:?}", f[1]))?;
    Ok(TraceRow {
        input: InputRecord {
            seq: int(f[0])?,
            input,
            down: flag(f[2])?,
            sdl_ts_ns: int(f[3])?,
            apply_tick: int(f[4])?,
            apply_tick_end_sdl_ns: int(f[5])?,
            prev_tick_end_sdl_ns: int(f[6])?,
            drain_sdl_ns: int(f[7])?,
            prev_drain_sdl_ns: int(f[8])?,
            dropped_ticks_at_apply: int(f[9])?,
            stall: flag(f[10])?,
        },
        move_tick: opt_int(f[11])?,
        frame: opt_int(f[12])?,
        frame_snap_tick: opt_int(f[13])?,
        present_ns: opt_int(f[14])?,
        frames_to_photon: opt_int(f[15])?,
    })
}

fn parse_trailer(rest: &str) -> Result<TraceOverflows, String> {
    let mut o = TraceOverflows::default();
    let mut seen = 0;
    for kv in rest.split_whitespace() {
        let (k, v) = kv
            .split_once('=')
            .ok_or_else(|| format!("trailer field {kv:?}"))?;
        let v = int(v)?;
        match k {
            "input" => o.input = v,
            "move" => o.moves = v,
            "frame" => o.frames = v,
            "pending" => o.pending = v,
            _ => return Err(format!("trailer field {k:?}")),
        }
        seen += 1;
    }
    if seen != 4 {
        return Err("trailer needs input, move, frame and pending".to_owned());
    }
    Ok(o)
}

/// The line `pin_app` prints after its shutdown lines when tracing:
/// `pin_app: latency trace: <path>, <n> events, <m> frames, <k> overflows`.
pub fn summary_line(path: &std::path::Path, trace: &CollectedTrace) -> String {
    format!(
        "pin_app: latency trace: {}, {} events, {} frames, {} overflows",
        path.display(),
        trace.events(),
        trace.frames.len(),
        trace.overflows.total()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::SimLoop;
    use pin_core::sim::tick_end_ns;
    use pin_core::snapshot::FlipperState;
    use pin_core::spsc::SpscProducer;
    use pin_core::{FlipperId, SimStep, TICK_NS, TripleBuffer};
    use std::cell::Cell;

    thread_local! { static NOW: Cell<u64> = const { Cell::new(0) }; }

    /// The controlled trace clock (per test thread).
    fn now() -> u64 {
        NOW.with(Cell::get)
    }

    fn set_now(t: u64) {
        NOW.with(|c| c.set(t));
    }

    fn rec(seq: u64, ts: u64, tick: u64) -> InputRecord {
        InputRecord {
            seq,
            input: CabinetInput::LeftFlipper,
            down: true,
            sdl_ts_ns: ts,
            apply_tick: tick,
            apply_tick_end_sdl_ns: tick * TICK_NS,
            prev_tick_end_sdl_ns: (tick - 1) * TICK_NS,
            drain_sdl_ns: tick * TICK_NS + 10,
            prev_drain_sdl_ns: (tick - 1) * TICK_NS + 10,
            dropped_ticks_at_apply: 0,
            stall: false,
        }
    }

    fn frame(frame: u64, snap_tick: u64, present_ns: u64) -> FrameRecord {
        FrameRecord {
            frame,
            snap_tick,
            present_ns,
        }
    }

    /// REQ-LAT-003: the CSV round-trips every documented column, with the
    /// header, the frame table and the trailer.
    #[test]
    fn req_lat_003_csv_round_trip() {
        let mut rows = join_trace(
            &[rec(0, 1_500_000, 2), rec(1, 7_000_000, 7)],
            &[MoveRecord {
                seq: 0,
                move_tick: 3,
            }],
            &[
                frame(0, 1, 1_000_000),
                frame(1, 2, 2_000_000),
                frame(2, 4, 3_000_000),
            ],
        );
        rows[1].input.input = CabinetInput::RightFlipper;
        rows[1].input.down = false;
        rows[1].input.stall = true;
        rows[1].input.dropped_ticks_at_apply = 12;
        let t = CollectedTrace {
            rows,
            frames: vec![
                frame(0, 1, 1_000_000),
                frame(1, 2, 2_000_000),
                frame(2, 4, 3_000_000),
            ],
            overflows: TraceOverflows {
                input: 1,
                moves: 2,
                frames: 3,
                pending: 4,
            },
        };
        let text = t.to_csv();
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some(EVENT_HEADER));
        assert_eq!(
            lines.next(),
            Some("0,left_flipper,1,1500000,2,2000000,1000000,2000010,1000010,0,0,3,2,4,3000000,2")
        );
        assert_eq!(
            lines.next(),
            Some("1,right_flipper,0,7000000,7,7000000,6000000,7000010,6000010,12,1,,,,,")
        );
        assert_eq!(lines.next(), Some(""));
        assert_eq!(lines.next(), Some(FRAME_HEADER));
        assert_eq!(lines.next(), Some("0,1,1000000"));
        assert_eq!(
            text.lines().last(),
            Some("# overflows: input=1 move=2 frame=3 pending=4")
        );
        assert!(text.ends_with('\n') && !text.contains('\r'));
        assert_eq!(CollectedTrace::parse(&text).unwrap(), t);
        assert_eq!(t.overflows.total(), 10);
        assert_eq!(EVENT_HEADER.split(',').count(), 16);
    }

    /// REQ-LAT-003: an empty trace is a header, an empty frame table and
    /// the trailer, and parses back.
    #[test]
    fn req_lat_003_empty_trace() {
        let t = CollectedTrace::default();
        let text = t.to_csv();
        assert_eq!(
            text,
            format!(
                "{EVENT_HEADER}\n\n{FRAME_HEADER}\n# overflows: input=0 move=0 frame=0 pending=0\n"
            )
        );
        assert_eq!(CollectedTrace::parse(&text).unwrap(), t);
        assert_eq!(t.events(), 0);
    }

    /// REQ-LAT-003: a press that never moved its bat leaves `move_tick`,
    /// `frame` and the frame columns empty.
    #[test]
    fn req_lat_003_unmoved_press_leaves_move_and_frame_empty() {
        let rows = join_trace(&[rec(0, 500, 1)], &[], &[frame(0, 5, 10)]);
        assert_eq!(rows[0].move_tick, None);
        assert_eq!(rows[0].frame, None);
        assert_eq!(rows[0].frames_to_photon, None);
        // A move after the last frame: no frame drew it.
        let rows = join_trace(
            &[rec(0, 500, 1)],
            &[MoveRecord {
                seq: 0,
                move_tick: 9,
            }],
            &[frame(0, 5, 10)],
        );
        assert_eq!(rows[0].move_tick, Some(9));
        assert_eq!(rows[0].frame, None);
    }

    /// REQ-LAT-001: the join picks the first frame whose snapshot tick is
    /// `>= move_tick`, and `frames_to_photon = frame - F0` with F0 the last
    /// frame presented at or before the press (0 when none).
    #[test]
    fn req_lat_001_join_first_frame_at_or_after_the_move() {
        let frames = [
            frame(0, 1, 100),
            frame(1, 3, 200),
            frame(2, 6, 300),
            frame(3, 7, 400),
        ];
        let mv = |t| MoveRecord {
            seq: 0,
            move_tick: t,
        };
        for (ts, mt, want_frame, want_f2p) in [
            (250u64, 4u64, 2u64, 1u64),
            (250, 6, 2, 1),
            (250, 7, 3, 2),
            (200, 7, 3, 2), // present at exactly the press counts as F0
            (50, 2, 1, 1),  // no frame before the press: F0 = 0
        ] {
            let r = join_trace(&[rec(0, ts, 1)], &[mv(mt)], &frames);
            assert_eq!(r[0].frame, Some(want_frame), "ts {ts} move {mt}");
            assert_eq!(r[0].frames_to_photon, Some(want_f2p), "ts {ts} move {mt}");
            assert_eq!(
                r[0].frame_snap_tick,
                Some(frames[want_frame as usize].snap_tick)
            );
            assert_eq!(
                r[0].present_ns,
                Some(frames[want_frame as usize].present_ns)
            );
        }
        assert_eq!(last_frame_at_or_before(&frames, 99), 0);
        assert_eq!(last_frame_at_or_before(&frames, 1000), 3);
        assert_eq!(last_frame_at_or_before(&[], 1000), 0);
    }

    /// REQ-LAT-003: full rings are counted overflows (trailer), never a
    /// panic or a block.
    #[test]
    fn req_lat_003_overflows_are_counted_in_the_trailer() {
        set_now(0);
        let mut b = FlipperBindings::new();
        b.bind(CabinetInput::LeftFlipper, 0);
        let (c, mut sim, mut fw) = latency_trace_with_capacity(b, now, 2, 1);
        let snap = FrameStateSnapshot::default();
        sim.begin_tick(TICK_NS, &snap);
        for i in 0..5 {
            sim.delivered(1, &InputEvent::button(i, CabinetInput::LeftFlipper, true));
        }
        sim.end_tick(1, &snap);
        for _ in 0..3 {
            assert!(fw.presented(&[SurfaceStatus::Presented], 1));
        }
        assert!(!fw.presented(&[SurfaceStatus::Skipped], 1));
        let t = c.collect();
        assert_eq!(t.rows.len(), 2);
        assert_eq!(t.frames.len(), 1);
        assert_eq!(
            t.overflows,
            TraceOverflows {
                input: 3,
                moves: 0,
                frames: 2,
                pending: 0
            }
        );
        assert!(
            t.to_csv()
                .ends_with("# overflows: input=3 move=0 frame=2 pending=0\n")
        );
    }

    /// More pending presses than [`MAX_PENDING_MOVES`] are counted, and a press that never moves stops
    /// being watched after [`MOVE_TIMEOUT_TICKS`].
    #[test]
    fn req_lat_003_pending_and_move_overflows() {
        set_now(0);
        let mut b = FlipperBindings::new();
        b.bind(CabinetInput::LeftFlipper, 0);
        let (c, mut sim, _fw) = latency_trace_with_capacity(b, now, 64, 1);
        let rest = FrameStateSnapshot::default();
        sim.begin_tick(TICK_NS, &rest);
        for i in 0..(MAX_PENDING_MOVES as u64 + 3) {
            sim.delivered(1, &InputEvent::button(i, CabinetInput::LeftFlipper, true));
        }
        sim.end_tick(1, &rest);
        let mut up = FrameStateSnapshot::default();
        assert!(up.push_flipper(FlipperState {
            id: FlipperId(0),
            angle_rad: 0.5,
            ..FlipperState::default()
        }));
        sim.begin_tick(2 * TICK_NS, &rest);
        sim.end_tick(2, &up);
        // A press that never moves: watched until the timeout.
        sim.begin_tick(3 * TICK_NS, &up);
        sim.delivered(3, &InputEvent::button(5, CabinetInput::LeftFlipper, true));
        for k in 3..(3 + MOVE_TIMEOUT_TICKS + 2) {
            sim.end_tick(k, &up);
            sim.begin_tick((k + 1) * TICK_NS, &up);
        }
        let mut moved_later = up;
        moved_later.flippers[0].angle_rad = 0.1;
        sim.end_tick(10_000, &moved_later);
        let t = c.collect();
        assert_eq!(t.overflows.pending, 3);
        assert_eq!(t.overflows.moves, 0);
        let with_move: Vec<u64> = t.rows.iter().filter_map(|r| r.move_tick).collect();
        assert_eq!(with_move, vec![2; MAX_PENDING_MOVES]);
        assert_eq!(t.rows.len(), MAX_PENDING_MOVES + 4);
        assert_eq!(t.rows.last().unwrap().move_tick, None, "timed out");
    }

    /// Bindings come from a table's flipper staging; out-of-range ids are
    /// ignored.
    #[test]
    fn bindings_of_flippers_and_bind() {
        let mut cfg = FlipperConfig {
            id: FlipperId(0),
            coil: pin_core::CoilId(1),
            eos_switch: pin_core::SwitchId(2),
            collision_layer: 0,
            base_position_mm: [0.0; 3],
            surface_normal: [0.0, 0.0, 1.0],
            rest_angle_deg: 0.0,
            sweep_angle_deg: 50.0,
            eos_angle_fraction: 0.9,
            shape: pin_core::FlipperShape::StraightCapsule {
                length_mm: 80.0,
                base_radius_mm: 10.0,
                tip_radius_mm: 6.0,
            },
            inertia_kg_m2: 1.0e-4,
            power_torque_nm: 1.2,
            hold_torque_nm: 0.15,
            return_spring_torque_nm: 0.03,
            stop_restitution: 0.15,
            rubber_restitution: 0.65,
            rubber_damping: 0.35,
            staging: FlipperStagingMode::DedicatedButton(CabinetInput::LeftFlipper),
            zipper_slide_axis_mm: None,
            zipper_slide_speed_mm_s: 0.0,
        };
        let mut cfgs = Vec::new();
        for (id, staging) in [
            (
                0u16,
                FlipperStagingMode::DedicatedButton(CabinetInput::LeftFlipper),
            ),
            (
                1,
                FlipperStagingMode::ParallelLinked(CabinetInput::RightFlipper),
            ),
            (
                2,
                FlipperStagingMode::ParallelLinked(CabinetInput::RightFlipper),
            ),
            (
                3,
                FlipperStagingMode::DualStageThreshold {
                    axis: pin_core::CabinetAxis::Plunger,
                    lower_threshold: 0.2,
                    upper_threshold: 0.8,
                },
            ),
        ] {
            cfg.id = FlipperId(id);
            cfg.staging = staging;
            cfgs.push(cfg.clone());
        }
        let b = FlipperBindings::of_flippers(&cfgs);
        assert_eq!(b.mask(CabinetInput::LeftFlipper), 0b1);
        assert_eq!(b.mask(CabinetInput::RightFlipper), 0b110);
        assert_eq!(b.mask(CabinetInput::Launch), 0);
        let mut b = FlipperBindings::new();
        b.bind(CabinetInput::Launch, MAX_FLIPPERS as u16);
        assert_eq!(b, FlipperBindings::default());
    }

    /// Records each step's delivered stamps (on the tick clock).
    #[derive(Default)]
    struct Seen(Vec<(u64, usize)>);

    impl SimStep for Seen {
        fn step_1ms(&mut self, tick: u64, inputs: &[InputEvent], out: &mut FrameStateSnapshot) {
            if !inputs.is_empty() {
                self.0.push((tick, inputs.len()));
            }
            out.tick = tick;
        }
    }

    /// REQ-LAT-003, REQ-LAT-004: the recorded `apply_tick` is the tick the
    /// sim loop delivered the event in, with epoch 0 and with a nonzero
    /// epoch plus dropped ticks; tick ends, drains, dropped ticks and the
    /// stall flag come from the loop's own scheduling.
    #[test]
    fn req_lat_004_apply_tick_is_the_delivering_tick() {
        for (epoch, drops) in [(0u64, 0u64), (2_000_000_000, 0), (2_000_000_000, 37)] {
            let (mut tx, rx): (SpscProducer<InputEvent>, _) = pin_core::spsc::channel(64);
            let (w, _r) = TripleBuffer::new(FrameStateSnapshot::default());
            let mut l = SimLoop::new(Seen::default(), rx, w);
            let (c, sim, _fw) = latency_trace(FlipperBindings::new(), now);
            l.set_latency_trace(sim);
            l.set_epoch_ns(epoch);
            // Ticks run on time: the clock at tick k's start is its end
            // plus 50 us (the loop runs a tick once it is due).
            let run = |l: &mut SimLoop<Seen>, k: u64| {
                set_now(tick_end_ns(l.epoch_ns(), k) + 50_000);
                l.run_ticks(1);
            };
            for k in 1..=3 {
                run(&mut l, k);
            }
            l.skip_ticks(drops);
            let e = l.epoch_ns();
            // Mid tick 5, exactly tick 6's end, mid tick 7, and a release
            // stamped before the epoch.
            for (ts, down) in [
                (e + 4 * TICK_NS + 1, true),
                (e + 6 * TICK_NS, false),
                (e + 6 * TICK_NS + 400_000, true),
                (0, false),
            ] {
                tx.push(InputEvent::button(ts, CabinetInput::LeftFlipper, down))
                    .unwrap();
            }
            for k in 4..=8 {
                run(&mut l, k);
            }
            let t = c.collect();
            let ticks: Vec<u64> = t.rows.iter().map(|r| r.input.apply_tick).collect();
            let seen: Vec<u64> = l
                .sim()
                .0
                .iter()
                .flat_map(|(k, n)| std::iter::repeat_n(*k, *n))
                .collect();
            assert_eq!(ticks, seen, "epoch {epoch} drops {drops}");
            assert_eq!(ticks, vec![5, 6, 7, 7]);
            for r in &t.rows {
                let i = &r.input;
                assert_eq!(i.apply_tick_end_sdl_ns, tick_end_ns(e, i.apply_tick));
                assert_eq!(i.drain_sdl_ns, tick_end_ns(e, i.apply_tick) + 50_000);
                assert_eq!(i.prev_tick_end_sdl_ns, tick_end_ns(e, i.apply_tick - 1));
                assert!(i.apply_tick_end_sdl_ns >= i.sdl_ts_ns);
                assert_eq!(i.dropped_ticks_at_apply, drops);
            }
            // Tick 4 (the first after the drop) is a stall when ticks were
            // dropped; ticks 5..7 run on time.
            assert!(t.rows.iter().all(|r| !r.input.stall));
            let seqs: Vec<u64> = t.rows.iter().map(|r| r.input.seq).collect();
            assert_eq!(seqs, vec![0, 1, 2, 3]);
        }
    }

    /// The stall flag comes from the drain gap or dropped ticks before the
    /// applying tick, never from the event's lateness.
    #[test]
    fn req_lat_004_stall_flag_from_scheduling_gaps() {
        let (mut tx, rx) = pin_core::spsc::channel::<InputEvent>(64);
        let (w, _r) = TripleBuffer::new(FrameStateSnapshot::default());
        let mut l = SimLoop::new(Seen::default(), rx, w);
        let (c, sim, _fw) = latency_trace(FlipperBindings::new(), now);
        l.set_epoch_ns(1_000_000_000);
        l.set_latency_trace(sim);
        set_now(1_000_000_000 + TICK_NS);
        l.run_ticks(1);
        // An event 700 ms old, applied by an on-time tick: no stall.
        tx.push(InputEvent::button(
            300_000_000,
            CabinetInput::RightFlipper,
            true,
        ))
        .unwrap();
        set_now(1_000_000_000 + 2 * TICK_NS);
        l.run_ticks(1);
        // A 20 ms gap: stall (8 ticks run, 12 dropped).
        tx.push(InputEvent::button(
            1_000_000_000 + 7 * TICK_NS,
            CabinetInput::RightFlipper,
            false,
        ))
        .unwrap();
        set_now(1_000_000_000 + 22 * TICK_NS);
        l.skip_ticks(12);
        l.run_ticks(8);
        // Only dropped ticks (no long gap): still a stall.
        tx.push(InputEvent::button(
            1_000_000_000 + 22 * TICK_NS,
            CabinetInput::RightFlipper,
            true,
        ))
        .unwrap();
        l.skip_ticks(1);
        set_now(1_000_000_000 + 22 * TICK_NS + 10);
        l.run_ticks(1);
        let t = c.collect();
        let stalls: Vec<bool> = t.rows.iter().map(|r| r.input.stall).collect();
        assert_eq!(stalls, vec![false, true, true]);
        assert_eq!(
            t.rows[1].input.drain_sdl_ns - t.rows[1].input.prev_drain_sdl_ns,
            20 * TICK_NS
        );
        assert_eq!(t.rows[2].input.dropped_ticks_at_apply, 13);
    }

    /// Records the bat move: the first tick at whose end a bound bat's
    /// angle differs from its value at the applying tick's start.
    struct Bat {
        lag: u64,
        pressed_at: Option<u64>,
    }

    impl SimStep for Bat {
        fn step_1ms(&mut self, tick: u64, inputs: &[InputEvent], out: &mut FrameStateSnapshot) {
            if inputs.iter().any(|e| {
                matches!(
                    e.kind,
                    InputEventKind::Button {
                        input: CabinetInput::RightFlipper,
                        pressed: true
                    }
                )
            }) {
                self.pressed_at = Some(tick);
            }
            *out = FrameStateSnapshot {
                tick,
                ..FrameStateSnapshot::default()
            };
            let a = match self.pressed_at {
                Some(p) if tick >= p + self.lag => 0.01 * (tick - p - self.lag + 1) as f32,
                _ => 0.0,
            };
            for id in 0..2u16 {
                out.push_flipper(FlipperState {
                    id: FlipperId(id),
                    angle_rad: if id == 1 { a } else { 0.0 },
                    ..FlipperState::default()
                });
            }
        }
    }

    /// REQ-LAT-003: `move_tick` is the tick the bound bat first moved; the
    /// joined frame and `frames_to_photon` follow from the frame table.
    #[test]
    fn req_lat_003_move_tick_and_frame_join_on_the_loop() {
        for lag in [0u64, 3] {
            let (mut tx, rx) = pin_core::spsc::channel::<InputEvent>(64);
            let (w, _r) = TripleBuffer::new(FrameStateSnapshot::default());
            let mut l = SimLoop::new(
                Bat {
                    lag,
                    pressed_at: None,
                },
                rx,
                w,
            );
            let mut b = FlipperBindings::new();
            b.bind(CabinetInput::RightFlipper, 1);
            let (c, sim, mut fw) = latency_trace(b, now);
            l.set_latency_trace(sim);
            set_now(0);
            for k in 1..=10u64 {
                if k == 4 {
                    tx.push(InputEvent::button(
                        3 * TICK_NS + 5,
                        CabinetInput::RightFlipper,
                        true,
                    ))
                    .unwrap();
                    // An unbound press: recorded, never watched.
                    tx.push(InputEvent::button(
                        3 * TICK_NS + 6,
                        CabinetInput::Start,
                        true,
                    ))
                    .unwrap();
                }
                set_now(k * TICK_NS);
                l.run_ticks(1);
                // A frame every 2 ticks, drawing the latest tick.
                if k % 2 == 0 {
                    fw.presented(&[SurfaceStatus::Presented], k);
                }
            }
            let t = c.collect();
            assert_eq!(t.rows.len(), 2);
            let r = t.rows[0];
            assert_eq!(r.input.apply_tick, 4);
            assert_eq!(r.move_tick, Some(4 + lag), "lag {lag}");
            let f = t.frames.iter().find(|f| f.snap_tick >= 4 + lag).unwrap();
            assert_eq!(r.frame, Some(f.frame));
            // Press at 3 ms + 5 ns: F0 is frame 0 (presented at 2 ms).
            assert_eq!(r.frames_to_photon, Some(f.frame));
            assert_eq!(t.rows[1].move_tick, None);
        }
    }

    /// R11: the parser survives bounded random bytes and truncations of a
    /// valid trace without panicking.
    #[test]
    fn parse_survives_random_bytes_and_truncation() {
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for _ in 0..2000 {
            let len = (next() % 300) as usize;
            let bytes: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            let _ = CollectedTrace::parse(&String::from_utf8_lossy(&bytes));
        }
        let t = CollectedTrace {
            rows: join_trace(&[rec(0, 1_500_000, 2)], &[], &[frame(0, 1, 1)]),
            frames: vec![frame(0, 1, 1)],
            overflows: TraceOverflows::default(),
        };
        let text = t.to_csv();
        // (Dropping only the final newline leaves a complete trace.)
        for cut in 0..text.len() - 1 {
            let r = CollectedTrace::parse(&text[..cut]);
            assert!(r.is_err(), "truncated at {cut} parsed");
        }
        // Mutated fields are errors, not panics.
        for bad in [
            text.replace("left_flipper", "nope"),
            text.replace(",1,1500000", ",2,1500000"),
            text.replace("0,1,1\n", "0,1\n"),
            text.replace("pending=0", "pending=x"),
            text.replace("pending=0", "other=0"),
            text.replace(" pending=0", ""),
            format!("{text}junk\n"),
        ] {
            assert!(CollectedTrace::parse(&bad).is_err(), "{bad}");
        }
    }

    /// The summary line names the path and the counts.
    #[test]
    fn req_lat_003_summary_line() {
        let t = CollectedTrace {
            rows: join_trace(&[rec(0, 1, 1)], &[], &[]),
            frames: vec![frame(0, 1, 1), frame(1, 2, 2)],
            overflows: TraceOverflows {
                frames: 2,
                ..TraceOverflows::default()
            },
        };
        assert_eq!(
            summary_line(std::path::Path::new("/tmp/t.csv"), &t),
            "pin_app: latency trace: /tmp/t.csv, 1 events, 2 frames, 2 overflows"
        );
    }
}
