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
//! file ([`CollectedTrace::to_csv`]). The trace file is created on the main
//! thread at startup and written on the main thread after every sim, render
//! and audio thread was joined; no thread that runs a tick or draws a frame
//! touches the filesystem (the same class as the startup table load).
//! Without `--latency-trace` none of this exists: no writer is installed and
//! the loops run their default paths.
//!
//! # Bat angles and the raised direction
//!
//! [`FlipperState::angle_rad`](pin_core::snapshot::FlipperState::angle_rad)
//! is the bat's angle relative to its rest angle, signed like the flipper's
//! `sweep_angle_deg` (`pin_physics`' `FlipperSet::relative_angle_rad` is
//! `sign * q`, where the travel `q` runs from 0 at the rest stop to the
//! sweep at the end stop and `sign` is the sign of `sweep_angle_deg`). The
//! energised direction, the one that raises the bat, is therefore the sign
//! of the sweep: increasing `angle_rad` for a positive sweep, decreasing for
//! a negative one (the grey-box table's right flipper). [`FlipperBindings`]
//! carries that sign per flipper ([`FlipperBindings::of_flippers`]).
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
//!      (when it drained the input ring); for a row with `stall = 1`, the
//!      end of the stall instead (the start of the first tick after it);
//!    * `prev_drain_sdl_ns`: the same for the previous executed tick (the
//!      sim start for the first tick); for a row with `stall = 1`, the
//!      start of the stall (the start of the last tick before it), so
//!      `drain_sdl_ns - prev_drain_sdl_ns` is the stall's length;
//!    * `dropped_ticks_at_apply`: ticks dropped by the catch-up cap so far;
//!    * `stall`: `1` when the event was stamped inside a stall, that is at or
//!      after the stall's start and before its end: a tick
//!      started more than [`STALL_GAP_NS`] after the previous executed tick,
//!      or ticks were dropped since it, and the applying tick is that tick
//!      or a later tick of the same catch-up batch (each was already due
//!      when the tick before it started), and `sdl_ts_ns` is at or after the
//!      stall's start and before its end (an event stamped before the start
//!      reached the ring late and keeps the ordinary bound). It is derived only from the sim thread's own
//!      scheduling and the timestamp, never from the event's lateness;
//!    * `move_tick`: for a press of an input bound to flippers
//!      ([`FlipperBindings`]), the first executed tick from the applying
//!      tick on at whose end one of those bats is raised by at least
//!      [`RAISE_THRESHOLD_RAD`] (1 degree) from its angle at the applying
//!      tick's start, in its energised direction (see above). Motion the
//!      other way (a bat still returning from an earlier release) never sets
//!      it. A press's watch ends at its release: a raise at the release tick
//!      still counts, a later one (another press's) never does. Empty
//!      otherwise (a release, an unbound input, no such raise before the
//!      release, or none within [`MOVE_TIMEOUT_TICKS`]);
//!    * `frame`: the index of the first presented frame, in present order,
//!      whose snapshot tick is `>= move_tick` and `<= release_tick` (the
//!      `apply_tick` of the next release, `down = 0`, of the same input
//!      after this press; unbounded when there is none) and whose snapshot
//!      shows one of the press's bats raised by at least 1 degree from its
//!      angle at the applying tick's start, in its energised direction (the
//!      same test as `move_tick`): the first frame that showed the bat
//!      displaced while the key was held. A bat that crossed 1 degree and
//!      fell back below it (a ball impact, a stop rebound) is matched only
//!      to a later frame that shows it raised again; a frame drawn at the
//!      release tick counts only when it still shows the bat raised.
//!      `move_tick` only selects the earliest candidate frame. Empty without
//!      a move or such a frame;
//!    * `frame_snap_tick`, `present_ns`: that frame's snapshot tick and
//!      present time (see the frame table);
//!    * `frames_to_photon`: `frame - F0`, where `F0` is the index of the
//!      last presented frame whose `present_ns <= sdl_ts_ns`: the raw count
//!      from the press. A press before the first presented frame counts from
//!      frame index 0 (`frame + 1`).
//! 2. One empty line, then the **frame table**: the header line
//!    [`FRAME_HEADER`] (`frame,snap_tick,present_ns`) and one row per
//!    presented playfield frame (0-based index in present order, the
//!    snapshot tick it drew, its present time). On the table presenter
//!    path (every run with a table whose playfield can be drawn directly,
//!    the windowed harness included; `--debug-overlay` only adds debug
//!    lines over the PBR image and does not change the presenter)
//!    `present_ns` is the SDL clock read immediately after the playfield
//!    `queue.present` returned, before a suboptimal surface is reconfigured
//!    and before any other surface (backglass, score display) is drawn
//!    ([`AppSurfaces::render_with_playfield_hook`](super::AppSurfaces::render_with_playfield_hook)).
//!    One limit remains: the debug presenter (`pin_render::SurfaceSet`),
//!    used only when no table is loaded or the table presenter cannot be
//!    used (the playfield offers no 8-bit format), lives in `pin_render`,
//!    outside this unit, so there `present_ns` is sampled after the whole
//!    render call: late by that frame's remaining work and, on a suboptimal
//!    surface, one reconfigure. Frame counts on that path can shift only by
//!    what lands in that interval. A frame counts only when the playfield
//!    surface presented (a backglass-only present is not a frame here). The
//!    bat angles each frame drew are kept in memory for the join and are
//!    not written.
//! 3. The **trailer**: two lines,
//!    `# overflows: input=<n> move=<n> frame=<n> pending=<n>` (the rows lost
//!    to full rings, [`TraceOverflows`]; each counter exactly once), then
//!    `# run_end_ns=<n>`: the SDL clock read on the main thread when the
//!    run loop ended, before the threads were joined (the run's end in the
//!    trace's own clock domain).
//!
//! Column names are stable (later units read this file: Gate A3, KP-043).

use pin_core::spsc::{SpscConsumer, SpscProducer, channel};
use pin_core::{
    CabinetInput, FrameStateSnapshot, InputEvent, InputEventKind, flipper::FlipperConfig,
    flipper::FlipperStagingMode, snapshot::MAX_FLIPPERS,
};
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
/// The raise that counts as the bat having moved: 1 degree
/// (0.017 453 292 rad), REQ-LAT-001's "moved at least 1°".
pub const RAISE_THRESHOLD_RAD: f32 = 0.017_453_292;

/// Header line of the event table (format version 1).
pub const EVENT_HEADER: &str = "event_seq,input,down,sdl_ts_ns,apply_tick,apply_tick_end_sdl_ns,\
prev_tick_end_sdl_ns,drain_sdl_ns,prev_drain_sdl_ns,dropped_ticks_at_apply,stall,move_tick,\
frame,frame_snap_tick,present_ns,frames_to_photon";
/// Header line of the frame table.
pub const FRAME_HEADER: &str = "frame,snap_tick,present_ns";
/// Prefix of the trailer's overflow line.
pub const TRAILER_PREFIX: &str = "# overflows:";
/// Prefix of the trailer's run-end line (`# run_end_ns=<n>`).
pub const RUN_END_PREFIX: &str = "# run_end_ns=";

/// Which flippers (bit `i` = `FlipperId(i)`) each cabinet input drives, and
/// which flippers raise towards a negative angle (a negative sweep).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct FlipperBindings {
    masks: [u16; CabinetInput::COUNT],
    negative: u16,
}

impl FlipperBindings {
    /// No bindings (no press is watched for a bat move).
    pub const fn new() -> Self {
        Self {
            masks: [0; CabinetInput::COUNT],
            negative: 0,
        }
    }

    /// Declares flipper `flipper`'s energised direction: `negative` when
    /// its sweep is negative (its raised angles are below 0), else
    /// positive (the default). Ids `>= MAX_FLIPPERS` are ignored.
    pub fn set_negative_sweep(&mut self, flipper: u16, negative: bool) {
        if usize::from(flipper) < MAX_FLIPPERS {
            if negative {
                self.negative |= 1 << flipper;
            } else {
                self.negative &= !(1 << flipper);
            }
        }
    }

    /// The sign of flipper `flipper`'s energised direction: `-1.0` for a
    /// negative sweep, else `1.0`.
    pub fn raise_sign(&self, flipper: u16) -> f32 {
        if usize::from(flipper) < MAX_FLIPPERS && self.negative & (1 << flipper) != 0 {
            -1.0
        } else {
            1.0
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
    /// `ParallelLinked` flipper is bound to its button, and every flipper's
    /// energised direction is the sign of its `sweep_angle_deg`.
    pub fn of_flippers(flippers: &[FlipperConfig]) -> Self {
        let mut b = Self::new();
        for f in flippers {
            b.set_negative_sweep(u16::from(f.id), f.sweep_angle_deg < 0.0);
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

/// A stall the sim thread observed: the drain times of the last tick before
/// it and of the first tick after it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct Stall {
    start_drain: u64,
    end_drain: u64,
}

/// The first tick a press's bat moved at, with what the frame join needs
/// to find the first frame that showed it.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MoveRecord {
    /// `event_seq` of the press.
    pub seq: u64,
    /// `move_tick`.
    pub move_tick: u64,
    /// The flippers the press drives (bit `i` = `FlipperId(i)`).
    pub mask: u16,
    /// Every bat's angle at the applying tick's start (by flipper id): the
    /// base the raise is measured from.
    pub base: [f32; MAX_FLIPPERS],
}

/// One presented frame.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct FrameRecord {
    /// 0-based index in present order.
    pub frame: u64,
    /// The snapshot tick the frame drew.
    pub snap_tick: u64,
    /// The trace clock read right after the playfield present, ns (see the
    /// module documentation for the limits).
    pub present_ns: u64,
    /// The bats' angles in the snapshot the frame drew, by flipper id (0
    /// for a flipper not in it). Not written to the file: parsed frames
    /// carry zeros.
    pub angles: [f32; MAX_FLIPPERS],
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
    /// Playfield frames counted by the render thread (recorded or lost).
    presented: AtomicU64,
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

/// A press waiting for its bat to move. Its watch ends at its first raise,
/// at the end of the tick that delivered its release (`released_at`), or at
/// `deadline`, whichever comes first.
#[derive(Copy, Clone, Debug)]
struct PendingMove {
    seq: u64,
    /// The pressed input (its release ends this watch).
    input: CabinetInput,
    mask: u16,
    base: [f32; MAX_FLIPPERS],
    deadline: u64,
    /// The tick that delivered the release of `input` after this press.
    released_at: Option<u64>,
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

/// Whether one of `mask`'s bats at `angles` (by flipper id, [`angles`]) is
/// raised by at least [`RAISE_THRESHOLD_RAD`] from `base`, in its
/// energised direction ([`FlipperBindings::raise_sign`]). Motion the other
/// way never counts. The one predicate for `move_tick` and the frame join.
fn raised(
    angles: &[f32; MAX_FLIPPERS],
    mask: u16,
    bindings: &FlipperBindings,
    base: &[f32; MAX_FLIPPERS],
) -> bool {
    (0..MAX_FLIPPERS).any(|i| {
        // `i < MAX_FLIPPERS <= 16`: the id fits a `u16` bit of `mask`.
        let id = i as u16;
        mask & (1 << id) != 0
            && (angles[i] - base[i]) * bindings.raise_sign(id) >= RAISE_THRESHOLD_RAD
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
    stall: Option<Stall>,
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
        self.stall = None;
    }

    /// `ticks` more ticks were dropped by the catch-up cap.
    pub fn ticks_dropped(&mut self, ticks: u64) {
        self.dropped = self.dropped.saturating_add(ticks);
    }

    /// A tick ending at `end_ns` (SDL clock) starts; `before` is the
    /// snapshot of the previous tick (the bats at this tick's start). Reads
    /// the trace clock once.
    ///
    /// Stall tracking: a tick that starts more than [`STALL_GAP_NS`] after
    /// the previous executed tick, or after dropped ticks, records the stall
    /// (the previous tick's drain to this tick's drain). The record is kept
    /// for the later ticks of the same catch-up batch (a tick that was
    /// already due when the previous tick started, `end_ns <=` the previous
    /// drain) and cleared by the first tick that was not. An event is in the
    /// stall when it is stamped at or after the stall's start and before its
    /// end (`delivered`).
    pub fn begin_tick(&mut self, end_ns: u64, before: &FrameStateSnapshot) {
        let drain_ns = (self.clock)();
        self.cur = TickMark {
            drain_ns,
            end_ns,
            dropped: self.dropped,
        };
        if drain_ns.saturating_sub(self.prev.drain_ns) > STALL_GAP_NS
            || self.dropped > self.prev.dropped
        {
            self.stall = Some(Stall {
                start_drain: self.prev.drain_ns,
                end_drain: drain_ns,
            });
        } else if end_ns > self.prev.drain_ns {
            // Not due when the previous tick started: the batch is over.
            self.stall = None;
        }
        self.base = angles(before);
    }

    /// The current tick (`tick`) delivers `ev` (raw SDL stamp) to the step.
    /// Button events are recorded; a press of a bound input is watched for
    /// its bat to move, until its release: a release marks every pending
    /// watch of the same input released at `tick` (retired at the end of
    /// `tick` unless that tick raised the bat, [`end_tick`](Self::end_tick)).
    pub fn delivered(&mut self, tick: u64, ev: &InputEvent) {
        let InputEventKind::Button { input, pressed } = ev.kind else {
            return;
        };
        if !pressed {
            for p in self.pending.iter_mut() {
                if p.input == input && p.released_at.is_none() {
                    p.released_at = Some(tick);
                }
            }
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        let stall = self
            .stall
            .filter(|s| (s.start_drain..s.end_drain).contains(&ev.timestamp_ns));
        let rec = InputRecord {
            seq,
            input,
            down: pressed,
            sdl_ts_ns: ev.timestamp_ns,
            apply_tick: tick,
            apply_tick_end_sdl_ns: self.cur.end_ns,
            prev_tick_end_sdl_ns: self.prev.end_ns,
            drain_sdl_ns: stall.map_or(self.cur.drain_ns, |s| s.end_drain),
            prev_drain_sdl_ns: stall.map_or(self.prev.drain_ns, |s| s.start_drain),
            dropped_ticks_at_apply: self.cur.dropped,
            stall: stall.is_some(),
        };
        if self.inputs.push(rec).is_err() {
            self.counters.input.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let mask = self.bindings.mask(input);
        if pressed && mask != 0 {
            let p = PendingMove {
                seq,
                input,
                mask,
                base: self.base,
                deadline: tick.saturating_add(MOVE_TIMEOUT_TICKS),
                released_at: None,
            };
            if self.pending.try_push(p).is_err() {
                self.counters.pending.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// The current tick (`tick`) was stepped; `after` is its snapshot.
    /// Records the presses whose bat is now raised by at least
    /// [`RAISE_THRESHOLD_RAD`] in its energised direction (a raise at the
    /// release tick still counts, like the join's `snap_tick <=
    /// release_tick`), then stops watching the presses that were not raised
    /// and were released at or before `tick`, or reached their deadline, so
    /// a later press's raise is never recorded for an earlier press.
    pub fn end_tick(&mut self, tick: u64, after: &FrameStateSnapshot) {
        let (moves, counters, bindings) = (&mut self.moves, &self.counters, &self.bindings);
        let now = angles(after);
        self.pending.retain(|p| {
            if raised(&now, p.mask, bindings, &p.base) {
                let m = MoveRecord {
                    seq: p.seq,
                    move_tick: tick,
                    mask: p.mask,
                    base: p.base,
                };
                if moves.push(m).is_err() {
                    counters.moves.fetch_add(1, Ordering::Relaxed);
                }
                false
            } else {
                tick < p.deadline && !p.released_at.is_some_and(|r| tick >= r)
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
    /// The trace clock (the SDL event clock), read now. The table presenter
    /// path reads it inside the playfield hook
    /// ([`AppSurfaces::render_with_playfield_hook`](super::AppSurfaces::render_with_playfield_hook)),
    /// immediately after the playfield `queue.present` returned and before
    /// any reconfigure, and passes it to
    /// [`presented_at`](Self::presented_at).
    pub fn now_ns(&self) -> u64 {
        (self.clock)()
    }

    /// The playfield hook's body on the table presenter path: when
    /// `status` is [`SurfaceStatus::Presented`](pin_render::gpu::SurfaceStatus::Presented)
    /// (the playfield slot presented, the predicate of
    /// `pin_render::gpu::paced_by`) and `stamp` is still empty, stores
    /// [`now_ns`](Self::now_ns) in it; otherwise reads no clock. The first
    /// presented playfield slot of the frame sets the present time.
    pub fn stamp_playfield(&self, stamp: &mut Option<u64>, status: pin_render::gpu::SurfaceStatus) {
        if stamp.is_none() && status == pin_render::gpu::SurfaceStatus::Presented {
            *stamp = Some(self.now_ns());
        }
    }

    /// Records one render-loop iteration that drew `snap`. Reads the trace
    /// clock first (call it right after the render call returned) and
    /// calls [`presented_at`](Self::presented_at) with it. Used where the
    /// playfield's own present time cannot be read: the debug presenter
    /// (`pin_render::SurfaceSet`), used only when no table is loaded or the
    /// table presenter cannot be used; `present_ns` is then late by the
    /// frame's remaining work and, on a suboptimal surface, one
    /// reconfigure. `--debug-overlay` does not select it.
    pub fn presented(&mut self, playfield_presented: bool, snap: &FrameStateSnapshot) -> bool {
        let present_ns = (self.clock)();
        self.presented_at(playfield_presented, snap, present_ns)
    }

    /// Records one render-loop iteration that drew `snap`, presented at
    /// `present_ns` (trace clock; this does not read the clock). When
    /// `playfield_presented` (the playfield surface presented, e.g.
    /// `pin_render::gpu::paced_by` of the frame's statuses) a frame row with
    /// `snap`'s tick, that time and `snap`'s bat angles is pushed. A
    /// backglass- or score-only present is not a frame. Returns whether a
    /// frame was counted.
    pub fn presented_at(
        &mut self,
        playfield_presented: bool,
        snap: &FrameStateSnapshot,
        present_ns: u64,
    ) -> bool {
        if !playfield_presented {
            return false;
        }
        let rec = FrameRecord {
            frame: self.next_frame,
            snap_tick: snap.tick,
            present_ns,
            angles: angles(snap),
        };
        self.next_frame += 1;
        self.counters.presented.fetch_add(1, Ordering::Relaxed);
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
    bindings: FlipperBindings,
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
        stall: None,
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
        bindings,
    };
    (collector, sim, render)
}

impl TraceCollector {
    /// Playfield frames the render thread has counted so far (readable
    /// while the threads run; the readiness line `pin_app: latency trace:
    /// first frame presented` waits for the first). A backglass-only
    /// present never counts.
    pub fn frames_presented(&self) -> u64 {
        self.counters.presented.load(Ordering::Relaxed)
    }

    /// Drains every ring and joins the rows ([`join_trace`]). Call it after
    /// the writers' threads were joined; `run_end_ns` is the trace clock
    /// read when the run loop ended (the trailer's `run_end_ns`).
    pub fn collect(mut self, run_end_ns: u64) -> CollectedTrace {
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
            rows: join_trace(&inputs, &moves, &frames, &self.bindings),
            frames,
            overflows: self.counters.read(),
            run_end_ns,
        }
    }
}

/// Index of the last frame of `frames` (in present order) presented at or
/// before `t_ns`, or `None` when no frame was.
pub fn last_frame_at_or_before(frames: &[FrameRecord], t_ns: u64) -> Option<u64> {
    let n = frames.partition_point(|f| f.present_ns <= t_ns);
    n.checked_sub(1).map(|i| frames[i].frame)
}

/// `frames_to_photon` of a press stamped `t_ns` whose bat was first shown
/// raised in frame `frame`: the presented frames from the press up to and
/// including that one, `frame - F0` with `F0` the last frame presented at or
/// before the press ([`last_frame_at_or_before`]), or `frame + 1` when no
/// frame was presented before the press (frames 0..=frame all count).
pub fn frames_to_photon(frames: &[FrameRecord], t_ns: u64, frame: u64) -> u64 {
    match last_frame_at_or_before(frames, t_ns) {
        Some(f0) => frame.saturating_sub(f0),
        None => frame.saturating_add(1),
    }
}

/// For each of `inputs` (in delivery order), the `apply_tick` of the next
/// release of the same input after it (`None` when there is none).
pub fn release_ticks(inputs: &[InputRecord]) -> Vec<Option<u64>> {
    let mut next = [None; CabinetInput::COUNT];
    let mut out = vec![None; inputs.len()];
    for (i, r) in inputs.iter().enumerate().rev() {
        out[i] = next[r.input.index()];
        if !r.down {
            next[r.input.index()] = Some(r.apply_tick);
        }
    }
    out
}

/// Joins the sim's input and move records with the frame table: each row
/// gets its `move_tick` and the first frame, in present order, whose
/// snapshot tick is `>= move_tick` and `<= release_tick` (the next release
/// of the same input, [`release_ticks`]; unbounded without one) and whose
/// snapshot shows one of the press's bats raised by at least
/// [`RAISE_THRESHOLD_RAD`] (1 degree) from the press's base (its applying
/// tick's start) in the energised direction of `bindings`: the same
/// predicate as `move_tick`, so a bat that crossed 1 degree and fell back
/// (a ball impact, a stop rebound) is matched only to a later frame that
/// shows it raised again, and a frame at the release tick counts only when
/// it still shows the bat raised. A press no eligible frame shows raised
/// gets no frame. Then that frame's snapshot tick and present time, and
/// [`frames_to_photon`]: `frame - F0` ([`last_frame_at_or_before`] the
/// press's SDL timestamp; `frame + 1` for a press before the first
/// presented frame: the raw count starts from the press, `move_tick` only
/// selects the frame). `frames` must be in present order.
pub fn join_trace(
    inputs: &[InputRecord],
    moves: &[MoveRecord],
    frames: &[FrameRecord],
    bindings: &FlipperBindings,
) -> Vec<TraceRow> {
    let releases = release_ticks(inputs);
    inputs
        .iter()
        .zip(releases)
        .map(|(r, release)| {
            let mv = moves.iter().find(|m| m.seq == r.seq);
            let move_tick = mv.map(|m| m.move_tick);
            let until = release.unwrap_or(u64::MAX);
            let drawn = mv.and_then(|m| {
                frames.iter().find(|f| {
                    f.snap_tick >= m.move_tick
                        && f.snap_tick <= until
                        && raised(&f.angles, m.mask, bindings, &m.base)
                })
            });
            TraceRow {
                input: *r,
                move_tick,
                frame: drawn.map(|f| f.frame),
                frame_snap_tick: drawn.map(|f| f.snap_tick),
                present_ns: drawn.map(|f| f.present_ns),
                frames_to_photon: drawn.map(|f| frames_to_photon(frames, r.sdl_ts_ns, f.frame)),
            }
        })
        .collect()
}

/// A complete trace: the event rows, the frame table and the overflows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CollectedTrace {
    /// The event table.
    pub rows: Vec<TraceRow>,
    /// The frame table, in present order.
    pub frames: Vec<FrameRecord>,
    /// Rows lost to full rings.
    pub overflows: TraceOverflows,
    /// The trace clock when the run loop ended (`# run_end_ns=`).
    pub run_end_ns: u64,
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
        let _ = writeln!(s, "{RUN_END_PREFIX}{}", self.run_end_ns);
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
                angles: [0.0; MAX_FLIPPERS],
            });
        }
        let trailer = trailer.ok_or_else(|| "missing trailer".to_owned())?;
        out.overflows = parse_trailer(trailer)?;
        out.run_end_ns = lines
            .next()
            .and_then(|l| l.strip_prefix(RUN_END_PREFIX))
            .ok_or_else(|| "missing run_end_ns line".to_owned())
            .and_then(int)?;
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
    let mut seen = 0u8;
    for kv in rest.split_whitespace() {
        let (k, v) = kv
            .split_once('=')
            .ok_or_else(|| format!("trailer field {kv:?}"))?;
        let v = int(v)?;
        let (bit, slot) = match k {
            "input" => (1u8, &mut o.input),
            "move" => (2, &mut o.moves),
            "frame" => (4, &mut o.frames),
            "pending" => (8, &mut o.pending),
            _ => return Err(format!("trailer field {k:?}")),
        };
        if seen & bit != 0 {
            return Err(format!("trailer field {k:?} twice"));
        }
        seen |= bit;
        *slot = v;
    }
    if seen != 15 {
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

    /// A frame as the file holds it (no bat angles).
    fn frame(frame: u64, snap_tick: u64, present_ns: u64) -> FrameRecord {
        FrameRecord {
            frame,
            snap_tick,
            present_ns,
            angles: [0.0; MAX_FLIPPERS],
        }
    }

    /// A frame whose snapshot shows flipper 0 raised 0.5 rad (above 1
    /// degree from a zero base, positive direction).
    fn shown(f: u64, snap_tick: u64, present_ns: u64) -> FrameRecord {
        let mut r = frame(f, snap_tick, present_ns);
        r.angles[0] = 0.5;
        r
    }

    /// The move of press `seq` at `move_tick`: flipper 0, zero base.
    fn mv0(seq: u64, move_tick: u64) -> MoveRecord {
        MoveRecord {
            seq,
            move_tick,
            mask: 1,
            base: [0.0; MAX_FLIPPERS],
        }
    }

    /// Bindings of the join tests: left flipper to flipper 0, positive.
    fn left0() -> FlipperBindings {
        let mut b = FlipperBindings::new();
        b.bind(CabinetInput::LeftFlipper, 0);
        b
    }

    /// A snapshot at `tick` without bats.
    fn at(tick: u64) -> FrameStateSnapshot {
        FrameStateSnapshot {
            tick,
            ..FrameStateSnapshot::default()
        }
    }

    /// REQ-LAT-003: the CSV round-trips every documented column, with the
    /// header, the frame table and the trailer.
    #[test]
    fn req_lat_003_csv_round_trip() {
        let mut rows = join_trace(
            &[rec(0, 1_500_000, 2), rec(1, 7_000_000, 7)],
            &[mv0(0, 3)],
            &[
                shown(0, 1, 1_000_000),
                shown(1, 2, 2_000_000),
                shown(2, 4, 3_000_000),
            ],
            &left0(),
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
            run_end_ns: 9_000_000,
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
        let mut tail = text.lines().rev();
        assert_eq!(tail.next(), Some("# run_end_ns=9000000"));
        assert_eq!(
            tail.next(),
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
                "{EVENT_HEADER}\n\n{FRAME_HEADER}\n# overflows: input=0 move=0 frame=0 pending=0\n\
                 # run_end_ns=0\n"
            )
        );
        assert_eq!(CollectedTrace::parse(&text).unwrap(), t);
        assert_eq!(t.events(), 0);
    }

    /// REQ-LAT-003: a press that never moved its bat leaves `move_tick`,
    /// `frame` and the frame columns empty.
    #[test]
    fn req_lat_003_unmoved_press_leaves_move_and_frame_empty() {
        let rows = join_trace(&[rec(0, 500, 1)], &[], &[shown(0, 5, 10)], &left0());
        assert_eq!(rows[0].move_tick, None);
        assert_eq!(rows[0].frame, None);
        assert_eq!(rows[0].frames_to_photon, None);
        // A move after the last frame: no frame drew it.
        let rows = join_trace(
            &[rec(0, 500, 1)],
            &[mv0(0, 9)],
            &[shown(0, 5, 10)],
            &left0(),
        );
        assert_eq!(rows[0].move_tick, Some(9));
        assert_eq!(rows[0].frame, None);
    }

    /// REQ-LAT-001: the join picks the first frame whose snapshot tick is
    /// `>= move_tick`, and `frames_to_photon = frame - F0` with F0 the last
    /// frame presented at or before the press (`frame + 1` when none: a
    /// press before the first presented frame counts from frame index 0).
    #[test]
    fn req_lat_001_join_first_frame_at_or_after_the_move() {
        let frames = [
            shown(0, 1, 100),
            shown(1, 3, 200),
            shown(2, 6, 300),
            shown(3, 7, 400),
        ];
        let mv = |t| mv0(0, t);
        for (ts, mt, want_frame, want_f2p) in [
            (250u64, 4u64, 2u64, 1u64),
            (250, 6, 2, 1),
            (250, 7, 3, 2),
            (200, 7, 3, 2), // present at exactly the press counts as F0
            (50, 2, 1, 2),  // no frame before the press: frames 0 and 1 count
        ] {
            let r = join_trace(&[rec(0, ts, 1)], &[mv(mt)], &frames, &left0());
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
        // O1: "no frame before the press" is not "frame 0 was".
        assert_eq!(last_frame_at_or_before(&frames, 99), None);
        assert_eq!(last_frame_at_or_before(&frames, 100), Some(0));
        assert_eq!(last_frame_at_or_before(&frames, 1000), Some(3));
        assert_eq!(last_frame_at_or_before(&[], 1000), None);
    }

    /// REQ-LAT-001, REQ-LAT-003 (O1): a press before the first presented
    /// frame whose bat is first shown raised in the frame with index 2
    /// counts frames 0, 1 and 2: `frames_to_photon` 3. A press after frame
    /// 0 was presented, raised in frame 2, counts frames 1 and 2: 2.
    #[test]
    fn req_lat_001_press_before_the_first_frame_counts_from_frame_zero() {
        let frames = [frame(0, 1, 100), frame(1, 2, 200), shown(2, 3, 300)];
        let r = join_trace(&[rec(0, 50, 1)], &[mv0(0, 1)], &frames, &left0());
        assert_eq!(r[0].frame, Some(2));
        assert_eq!(r[0].frames_to_photon, Some(3));
        let r = join_trace(&[rec(0, 150, 1)], &[mv0(0, 1)], &frames, &left0());
        assert_eq!(r[0].frame, Some(2));
        assert_eq!(r[0].frames_to_photon, Some(2));
        assert_eq!(frames_to_photon(&frames, 50, 2), 3);
        assert_eq!(frames_to_photon(&frames, 100, 2), 2);
        assert_eq!(frames_to_photon(&[], 0, 0), 1);
    }

    /// Flipper 0 at `angle` (radians) at `tick`.
    fn bat0(tick: u64, angle: f32) -> FrameStateSnapshot {
        let mut s = at(tick);
        assert!(s.push_flipper(FlipperState {
            id: FlipperId(0),
            angle_rad: angle,
            ..FlipperState::default()
        }));
        s
    }

    /// REQ-LAT-001, REQ-LAT-003 (C6): a press whose bat never rose 1 degree
    /// before its release stops being watched at the release, so the raise
    /// of a later press of the same input is recorded only for that later
    /// press (press A: no `move_tick`; press B: its own).
    #[test]
    fn req_lat_003_unraised_press_does_not_take_a_later_press_move() {
        set_now(0);
        let (c, mut sim, _fw) = latency_trace_with_capacity(left0(), now, 64, 1);
        let l = CabinetInput::LeftFlipper;
        let mut prev = bat0(0, 0.0);
        let mut step = |sim: &mut SimTraceWriter, tick: u64, evs: &[InputEvent], a: f32| {
            sim.begin_tick(tick * TICK_NS, &prev);
            for e in evs {
                sim.delivered(tick, e);
            }
            prev = bat0(tick, a);
            sim.end_tick(tick, &prev);
        };
        step(&mut sim, 1, &[InputEvent::button(1, l, true)], 0.0); // A
        step(&mut sim, 2, &[], 0.0);
        step(&mut sim, 3, &[InputEvent::button(3, l, false)], 0.0);
        step(&mut sim, 4, &[InputEvent::button(4, l, true)], 0.0); // B
        step(&mut sim, 5, &[], 0.5);
        let t = c.collect(0);
        assert_eq!(t.rows.len(), 3);
        assert_eq!(t.rows[0].move_tick, None, "A never rose");
        assert_eq!(t.rows[1].move_tick, None, "a release");
        assert_eq!(t.rows[2].move_tick, Some(5), "B's own move");
    }

    /// REQ-LAT-001, REQ-LAT-003 (C6): a rise at the release tick still
    /// counts for the press (inclusive like the join's `snap_tick <=
    /// release_tick`), also for a press and release in the same tick.
    #[test]
    fn req_lat_003_rise_on_the_release_tick_still_counts() {
        set_now(0);
        let l = CabinetInput::LeftFlipper;
        let (c, mut sim, _fw) = latency_trace_with_capacity(left0(), now, 64, 1);
        let rest = bat0(0, 0.0);
        sim.begin_tick(TICK_NS, &rest);
        sim.delivered(1, &InputEvent::button(1, l, true));
        sim.end_tick(1, &bat0(1, 0.0));
        sim.begin_tick(2 * TICK_NS, &bat0(1, 0.0));
        sim.delivered(2, &InputEvent::button(2, l, false));
        sim.end_tick(2, &bat0(2, 0.5));
        // Press and release in one tick, raised at that tick's end.
        sim.begin_tick(3 * TICK_NS, &rest);
        sim.delivered(3, &InputEvent::button(3, l, true));
        sim.delivered(3, &InputEvent::button(3, l, false));
        sim.end_tick(3, &bat0(3, 0.5));
        // Press and release in one tick, not raised: retired at once, so
        // the next press's raise is its own.
        sim.begin_tick(4 * TICK_NS, &rest);
        sim.delivered(4, &InputEvent::button(4, l, true));
        sim.delivered(4, &InputEvent::button(4, l, false));
        sim.end_tick(4, &rest);
        sim.begin_tick(5 * TICK_NS, &rest);
        sim.delivered(5, &InputEvent::button(5, l, true));
        sim.end_tick(5, &bat0(5, 0.5));
        let t = c.collect(0);
        let moves: Vec<Option<u64>> = t.rows.iter().map(|r| r.move_tick).collect();
        assert_eq!(
            moves,
            vec![Some(2), None, Some(3), None, None, None, Some(5)]
        );
    }

    /// REQ-LAT-001, REQ-LAT-003 (C6): overlapping presses of different
    /// inputs keep separate watches: releasing one does not end the other's.
    #[test]
    fn req_lat_003_overlapping_presses_of_different_inputs_are_independent() {
        set_now(0);
        let (l, r) = (CabinetInput::LeftFlipper, CabinetInput::RightFlipper);
        let mut b = left0();
        b.bind(r, 1);
        let (c, mut sim, _fw) = latency_trace_with_capacity(b, now, 64, 1);
        let bats = |tick: u64, a0: f32, a1: f32| {
            let mut s = bat0(tick, a0);
            assert!(s.push_flipper(FlipperState {
                id: FlipperId(1),
                angle_rad: a1,
                ..FlipperState::default()
            }));
            s
        };
        sim.begin_tick(TICK_NS, &bats(0, 0.0, 0.0));
        sim.delivered(1, &InputEvent::button(1, l, true));
        sim.delivered(1, &InputEvent::button(1, r, true));
        sim.end_tick(1, &bats(1, 0.0, 0.0));
        // Right released unraised; left still watched and rises at 3.
        sim.begin_tick(2 * TICK_NS, &bats(1, 0.0, 0.0));
        sim.delivered(2, &InputEvent::button(2, r, false));
        sim.end_tick(2, &bats(2, 0.0, 0.0));
        sim.begin_tick(3 * TICK_NS, &bats(2, 0.0, 0.0));
        sim.end_tick(3, &bats(3, 0.5, 0.5));
        let t = c.collect(0);
        let moves: Vec<Option<u64>> = t.rows.iter().map(|r| r.move_tick).collect();
        assert_eq!(moves, vec![Some(3), None, None]);
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
            assert!(fw.presented(true, &at(1)));
        }
        assert!(!fw.presented(false, &at(1)));
        let t = c.collect(0);
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
                .ends_with("# overflows: input=3 move=0 frame=2 pending=0\n# run_end_ns=0\n")
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
        let t = c.collect(0);
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
            let t = c.collect(0);
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
        let t = c.collect(0);
        let stalls: Vec<bool> = t.rows.iter().map(|r| r.input.stall).collect();
        assert_eq!(stalls, vec![false, true, true]);
        assert_eq!(
            t.rows[1].input.drain_sdl_ns - t.rows[1].input.prev_drain_sdl_ns,
            20 * TICK_NS
        );
        assert_eq!(t.rows[2].input.dropped_ticks_at_apply, 13);
    }

    /// A bat that rises 0.01 rad per tick, `lag` ticks after the press.
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

    /// REQ-LAT-003: `move_tick` is the first tick the bound bat is raised
    /// by at least 1 degree; the joined frame and `frames_to_photon`
    /// follow from the frame table.
    #[test]
    fn req_lat_003_move_tick_and_frame_join_on_the_loop() {
        for lag in [0u64, 3] {
            let (mut tx, rx) = pin_core::spsc::channel::<InputEvent>(64);
            let (w, mut r) = TripleBuffer::new(FrameStateSnapshot::default());
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
                // A frame every 2 ticks, drawing the latest snapshot (its
                // real bat angles).
                if k % 2 == 0 {
                    let snap = r.read();
                    assert_eq!(snap.tick, k);
                    fw.presented(true, snap);
                }
            }
            let t = c.collect(0);
            assert_eq!(t.rows.len(), 2);
            let r = t.rows[0];
            assert_eq!(r.input.apply_tick, 4);
            // The bat rises 0.01 rad per tick from tick 4 + lag: 0.01 rad
            // (under 1 degree) at its end, 0.02 rad one tick later.
            assert_eq!(r.move_tick, Some(4 + lag + 1), "lag {lag}");
            let f = t.frames.iter().find(|f| f.snap_tick > 4 + lag).unwrap();
            // The frame carries the drawn snapshot's angles: bat 1 raised.
            assert!(f.angles[1] >= RAISE_THRESHOLD_RAD, "{f:?}");
            assert_eq!(f.angles[0], 0.0);
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
            rows: join_trace(&[rec(0, 1_500_000, 2)], &[], &[frame(0, 1, 1)], &left0()),
            frames: vec![frame(0, 1, 1)],
            overflows: TraceOverflows::default(),
            run_end_ns: 5,
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
            rows: join_trace(&[rec(0, 1, 1)], &[], &[], &left0()),
            frames: vec![frame(0, 1, 1), frame(1, 2, 2)],
            overflows: TraceOverflows {
                frames: 2,
                ..TraceOverflows::default()
            },
            run_end_ns: 0,
        };
        assert_eq!(
            summary_line(std::path::Path::new("/tmp/t.csv"), &t),
            "pin_app: latency trace: /tmp/t.csv, 1 events, 2 frames, 2 overflows"
        );
    }

    /// A snapshot with bats `(id, angle_rad)`.
    fn bats(tick: u64, a: &[(u16, f32)]) -> FrameStateSnapshot {
        let mut s = FrameStateSnapshot {
            tick,
            ..FrameStateSnapshot::default()
        };
        for &(id, angle_rad) in a {
            assert!(s.push_flipper(FlipperState {
                id: FlipperId(id),
                angle_rad,
                ..FlipperState::default()
            }));
        }
        s
    }

    /// Drives a writer directly: a press of the left flipper (bound to
    /// flipper 0, sweep sign `neg`) applied in tick 1 whose bat is at
    /// `angles[0]` at the tick's start and at `angles[k]` at the end of tick
    /// `k`. Returns the recorded `move_tick`.
    fn move_tick_of(neg: bool, angles: &[f32]) -> Option<u64> {
        set_now(0);
        let mut b = FlipperBindings::new();
        b.bind(CabinetInput::LeftFlipper, 0);
        b.set_negative_sweep(0, neg);
        let (c, mut sim, _fw) = latency_trace_with_capacity(b, now, 16, 1);
        let mut before = bats(0, &[(0, angles[0])]);
        for (k, &a) in angles.iter().enumerate().skip(1) {
            let k = k as u64;
            sim.begin_tick(k * TICK_NS, &before);
            if k == 1 {
                sim.delivered(
                    1,
                    &InputEvent::button(500_000, CabinetInput::LeftFlipper, true),
                );
            }
            let after = bats(k, &[(0, a)]);
            sim.end_tick(k, &after);
            before = after;
        }
        c.collect(0).rows[0].move_tick
    }

    const DEG: f32 = 0.017_453_292;

    /// REQ-LAT-001, REQ-LAT-005: `move_tick` needs a raise of at least 1
    /// degree from the applying tick's start, in the energised direction:
    /// 0.2 degree then still is no move; 1.5 degree in steps sets it at the
    /// first tick at or beyond 1 degree; a negative sweep raises towards
    /// negative angles.
    #[test]
    fn req_lat_001_move_tick_needs_one_degree_raised() {
        assert_eq!(
            move_tick_of(false, &[0.0, 0.1 * DEG, 0.2 * DEG, 0.2 * DEG, 0.2 * DEG]),
            None
        );
        assert_eq!(
            move_tick_of(false, &[0.0, 0.5 * DEG, 0.99 * DEG, 1.0 * DEG, 1.5 * DEG]),
            Some(3)
        );
        assert_eq!(move_tick_of(false, &[0.0, 0.0, 1.5 * DEG]), Some(2));
        // Negative sweep: raising is decreasing the angle.
        assert_eq!(move_tick_of(true, &[0.0, -0.5 * DEG, -1.5 * DEG]), Some(2));
        assert_eq!(
            move_tick_of(true, &[0.0, 0.5 * DEG, 1.5 * DEG, 3.0 * DEG]),
            None
        );
        // The threshold is 1 degree in radians.
        assert_eq!(RAISE_THRESHOLD_RAD, DEG);
        assert!((RAISE_THRESHOLD_RAD - 1.0f32.to_radians()).abs() < 1e-9);
    }

    /// REQ-LAT-001, REQ-LAT-003: a bat still returning from an earlier
    /// release when the press is applied (moving the other way) sets no
    /// `move_tick`; the genuine raise that follows does, at the first tick
    /// 1 degree above the applying tick's start.
    #[test]
    fn req_lat_003_return_motion_never_sets_move_tick() {
        let base = 20.0 * DEG;
        // Positive sweep: returning = decreasing.
        let up = [
            base,
            base - 2.0 * DEG,
            base - 3.0 * DEG,
            base - 1.0 * DEG,
            base + 0.5 * DEG,
            base + 1.2 * DEG,
        ];
        assert_eq!(move_tick_of(false, &up), Some(5));
        assert_eq!(move_tick_of(false, &up[..5]), None, "return only");
        // Negative sweep, mirrored.
        let down: Vec<f32> = up.iter().map(|a| -a).collect();
        assert_eq!(move_tick_of(true, &down), Some(5));
    }

    /// REQ-LAT-003: only a playfield present is a trace frame: a frame
    /// whose only present was the backglass's records nothing and raises
    /// no readiness; a playfield present records one frame.
    #[test]
    fn req_lat_003_backglass_only_present_is_not_a_frame() {
        use pin_render::gpu::{SurfaceStatus as S, paced_by};
        use pin_render::topology::SurfaceRole as R;
        set_now(77);
        let (c, _sim, mut fw) = latency_trace(FlipperBindings::new(), now);
        let roles = [R::Playfield, R::Backglass];
        assert!(!fw.presented(paced_by(roles, &[S::Skipped, S::Presented]), &at(3)));
        assert!(!fw.presented(paced_by(roles, &[S::Occluded, S::Presented]), &at(4)));
        assert_eq!(c.frames_presented(), 0, "no readiness from the backglass");
        assert!(fw.presented(paced_by(roles, &[S::Presented, S::Skipped]), &at(5)));
        assert_eq!(c.frames_presented(), 1);
        let t = c.collect(0);
        assert_eq!(t.frames, vec![frame(0, 5, 77)]);
    }

    /// REQ-LAT-001: a frame drawn after the press's release was applied
    /// does not show the bat raised while held: the join takes the first
    /// frame with `move_tick <= snap_tick <= release_tick`, and none when
    /// every later frame is past the release.
    #[test]
    fn req_lat_001_frame_after_release_does_not_count() {
        let mut release = rec(1, 9 * TICK_NS, 10);
        release.down = false;
        let mut other = rec(2, 9 * TICK_NS, 11);
        other.input = CabinetInput::RightFlipper;
        other.down = false;
        let inputs = [rec(0, 1_500_000, 2), release, other];
        let mv = [mv0(0, 3)];
        // A frame at snap 10 (the release tick itself) still counts.
        let rows = join_trace(
            &inputs,
            &mv,
            &[shown(0, 1, 1_000_000), shown(1, 10, 20_000_000)],
            &left0(),
        );
        assert_eq!(rows[0].frame, Some(1));
        assert_eq!(rows[0].frames_to_photon, Some(1));
        // Only frames after the release: none showed it.
        let rows = join_trace(
            &inputs,
            &mv,
            &[shown(0, 1, 1_000_000), shown(1, 11, 20_000_000)],
            &left0(),
        );
        assert_eq!(rows[0].move_tick, Some(3));
        assert_eq!(rows[0].frame, None);
        assert_eq!(rows[0].frames_to_photon, None);
        assert_eq!(release_ticks(&inputs), vec![Some(10), None, None]);
    }

    /// REQ-LAT-001 (F4): the trailer's `run_end_ns` line round-trips and is
    /// required; each overflow counter must appear exactly once.
    #[test]
    fn req_lat_003_trailer_run_end_and_strict_counters() {
        let t = CollectedTrace {
            run_end_ns: 123_456_789,
            ..CollectedTrace::default()
        };
        let text = t.to_csv();
        assert!(
            text.ends_with("pending=0\n# run_end_ns=123456789\n"),
            "{text}"
        );
        assert_eq!(CollectedTrace::parse(&text).unwrap(), t);
        for bad in [
            text.replace("# run_end_ns=123456789\n", ""),
            text.replace("run_end_ns=123456789", "run_end_ns=x"),
            text.replace("move=0", "input=0"),
            text.replace(" pending=0", " pending=0 pending=0"),
            text.replace("input=0 move=0 frame=0 pending=0", "input=0"),
        ] {
            assert!(CollectedTrace::parse(&bad).is_err(), "{bad}");
        }
    }

    /// REQ-LAT-004 (F7): a press stamped inside a stall but applied in a
    /// later tick of the catch-up batch is still marked `stall = 1`, with
    /// the stall's start and end as its drain columns, so its allowance is
    /// the stall; a press stamped after the stall ended is not marked, and
    /// the first on-time tick after the batch ends the stall. A press
    /// stamped before the stall's start, delivered by a batch tick, is not
    /// stalled either (it keeps the ordinary tick gap).
    #[test]
    fn req_lat_004_stall_kept_across_the_catch_up_batch() {
        let ms = TICK_NS;
        let (mut tx, rx) = pin_core::spsc::channel::<InputEvent>(64);
        let (w, _r) = TripleBuffer::new(FrameStateSnapshot::default());
        let mut l = SimLoop::new(Seen::default(), rx, w);
        let (c, sim, _fw) = latency_trace(FlipperBindings::new(), now);
        l.set_latency_trace(sim);
        // Ticks 1..=10 on time: tick k drains at k ms.
        for k in 1..=10u64 {
            set_now(k * ms);
            l.run_ticks(1);
        }
        // The sim thread wakes at 30 ms: 8 ticks run, 12 are dropped (the
        // epoch moves to 12 ms; tick 11 ends at 23 ms, tick 18 at 30 ms).
        l.skip_ticks(12);
        for k in 11..=18u64 {
            set_now(30 * ms + (k - 11) * 10_000);
            if k == 13 {
                // Stamped at 12 ms (inside the stall), it reaches the ring
                // only now: applied in tick 13 (ends 25 ms).
                tx.push(InputEvent::button(12 * ms, CabinetInput::LeftFlipper, true))
                    .unwrap();
            }
            if k == 14 {
                // Stamped at 25.5 ms, applied in tick 14 (ends 26 ms).
                tx.push(InputEvent::button(
                    25 * ms + 500_000,
                    CabinetInput::RightFlipper,
                    true,
                ))
                .unwrap();
            }
            if k == 15 {
                // Stamped at 9 ms, before the stall started (10 ms): it
                // reached the ring late and is not inside the stall, so it
                // gets the ordinary tick gap, not the stall length.
                tx.push(InputEvent::button(9 * ms, CabinetInput::LeftFlipper, false))
                    .unwrap();
            }
            l.run_ticks(1);
        }
        // Tick 19 on time (31 ms): the batch is over. A press stamped after
        // the stall ended is not stalled, nor is one stamped inside the
        // stall that reaches the ring only after the batch.
        tx.push(InputEvent::button(
            30 * ms + 20_000,
            CabinetInput::RightFlipper,
            false,
        ))
        .unwrap();
        tx.push(InputEvent::button(
            15 * ms,
            CabinetInput::LeftFlipper,
            false,
        ))
        .unwrap();
        set_now(31 * ms + 50_000);
        l.run_ticks(1);
        let t = c.collect(0);
        let r: Vec<(u64, bool, u64, u64)> = t
            .rows
            .iter()
            .map(|r| {
                let i = &r.input;
                (i.apply_tick, i.stall, i.prev_drain_sdl_ns, i.drain_sdl_ns)
            })
            .collect();
        assert_eq!(
            r,
            vec![
                (13, true, 10 * ms, 30 * ms),
                (14, true, 10 * ms, 30 * ms),
                (15, false, 30 * ms + 30_000, 30 * ms + 40_000),
                (19, false, 30 * ms + 70_000, 31 * ms + 50_000),
                (19, false, 30 * ms + 70_000, 31 * ms + 50_000),
            ]
        );
        // The applying tick's end and the lateness the verdict sees.
        assert_eq!(t.rows[0].input.apply_tick_end_sdl_ns, 25 * ms);
        assert_eq!(t.rows[1].input.apply_tick_end_sdl_ns, 26 * ms);
    }

    /// Drives a writer and a frame writer: a press of the left flipper
    /// (bound to flipper 0, sweep sign `neg`) applied in tick 1, the bat at
    /// `angles[0]` at the tick's start and at `angles[k]` at the end of tick
    /// `k`; a frame presented right after each tick in `frames_at`
    /// (drawing that tick's snapshot); a release applied in `release_at`.
    /// Returns the press's joined row.
    fn joined(neg: bool, angles: &[f32], frames_at: &[u64], release_at: Option<u64>) -> TraceRow {
        set_now(0);
        let mut b = FlipperBindings::new();
        b.bind(CabinetInput::LeftFlipper, 0);
        b.set_negative_sweep(0, neg);
        let (c, mut sim, mut fw) = latency_trace_with_capacity(b, now, 16, 16);
        let mut before = bats(0, &[(0, angles[0])]);
        for (k, &a) in angles.iter().enumerate().skip(1) {
            let k = k as u64;
            set_now(k * TICK_NS);
            sim.begin_tick(k * TICK_NS, &before);
            if k == 1 {
                sim.delivered(
                    1,
                    &InputEvent::button(500_000, CabinetInput::LeftFlipper, true),
                );
            }
            if release_at == Some(k) {
                sim.delivered(
                    k,
                    &InputEvent::button(k * TICK_NS - 1, CabinetInput::LeftFlipper, false),
                );
            }
            let after = bats(k, &[(0, a)]);
            sim.end_tick(k, &after);
            if frames_at.contains(&k) {
                set_now(k * TICK_NS + 100);
                assert!(fw.presented(true, &after));
            }
            before = after;
        }
        c.collect(0).rows[0]
    }

    /// The bat crosses 1 degree at tick 3, falls back (a rebound) to 0.3
    /// degree by tick 5 and is raised again (1.5 degree) at tick 8.
    const CROSS_AND_FALL: [f32; 9] = [
        0.0,
        0.5 * DEG,
        0.9 * DEG,
        1.2 * DEG,
        0.5 * DEG,
        0.3 * DEG,
        0.6 * DEG,
        0.8 * DEG,
        1.5 * DEG,
    ];

    /// REQ-LAT-001, REQ-LAT-003 (B3): a bat that crosses 1 degree at tick T
    /// (`move_tick = T`) and is back below it in the frame drawn at T+2 is
    /// matched to the frame at T+5 that shows it raised, not to the T+2
    /// frame; the raw count still starts from the press.
    #[test]
    fn req_lat_001_join_skips_a_frame_that_shows_the_bat_fallen_back() {
        let r = joined(false, &CROSS_AND_FALL, &[5, 8], None);
        assert_eq!(r.move_tick, Some(3));
        assert_eq!(r.frame, Some(1));
        assert_eq!(r.frame_snap_tick, Some(8));
        assert_eq!(r.present_ns, Some(8 * TICK_NS + 100));
        // No frame before the press at 0.5 ms (O1): frames 0 and 1 count.
        assert_eq!(r.frames_to_photon, Some(2));
    }

    /// REQ-LAT-001, REQ-LAT-003 (B3): without a later frame that shows the
    /// bat raised, the press has no frame (a failing check b).
    #[test]
    fn req_lat_001_no_frame_when_no_eligible_frame_shows_the_bat_raised() {
        let r = joined(false, &CROSS_AND_FALL, &[5], None);
        assert_eq!(r.move_tick, Some(3));
        assert_eq!(
            (r.frame, r.frame_snap_tick, r.present_ns, r.frames_to_photon),
            (None, None, None, None)
        );
    }

    /// REQ-LAT-001, REQ-LAT-003 (B3): a frame drawn at the release tick is
    /// eligible when its snapshot still shows the bat raised and is not
    /// when the bat is below 1 degree there; frames after the release never
    /// count.
    #[test]
    fn req_lat_001_frame_at_the_release_tick_needs_the_bat_raised() {
        let r = joined(false, &CROSS_AND_FALL, &[5, 8], Some(8));
        assert_eq!(r.frame_snap_tick, Some(8), "raised at the release tick");
        let mut low = CROSS_AND_FALL.to_vec();
        low[8] = 0.5 * DEG;
        low.push(1.5 * DEG);
        let r = joined(false, &low, &[5, 8, 9], Some(8));
        assert_eq!(r.move_tick, Some(3));
        assert_eq!(
            r.frame, None,
            "below 1 degree at the release, then released"
        );
    }

    /// REQ-LAT-001, REQ-LAT-003 (B3): the same sequences for a bat whose
    /// energised direction is negative (`raise_sign` -1).
    #[test]
    fn req_lat_001_join_needs_the_bat_raised_negative_direction() {
        let neg: Vec<f32> = CROSS_AND_FALL.iter().map(|a| -a).collect();
        let r = joined(true, &neg, &[5, 8], None);
        assert_eq!((r.move_tick, r.frame_snap_tick), (Some(3), Some(8)));
        let r = joined(true, &neg, &[5], None);
        assert_eq!((r.move_tick, r.frame), (Some(3), None));
        let r = joined(true, &neg, &[5, 8], Some(8));
        assert_eq!(r.frame_snap_tick, Some(8));
        // Positive angles are the wrong direction for this bat: no move.
        let r = joined(true, &CROSS_AND_FALL, &[5, 8], None);
        assert_eq!((r.move_tick, r.frame), (None, None));
    }

    /// REQ-LAT-003 (B3): `end_tick` records the press's mask and base with
    /// its move, and the CSV does not carry frame angles (parsed frames
    /// have zero angles; the format is unchanged).
    #[test]
    fn req_lat_003_move_carries_mask_and_base_and_csv_drops_angles() {
        set_now(0);
        let mut b = FlipperBindings::new();
        b.bind(CabinetInput::LeftFlipper, 0);
        let (c, mut sim, mut fw) = latency_trace_with_capacity(b, now, 16, 16);
        let before = bats(0, &[(0, 0.25)]);
        sim.begin_tick(TICK_NS, &before);
        sim.delivered(1, &InputEvent::button(1, CabinetInput::LeftFlipper, true));
        let after = bats(1, &[(0, 0.5)]);
        sim.end_tick(1, &after);
        assert!(fw.presented(true, &after));
        let t = c.collect(0);
        assert_eq!(t.frames[0].angles[0], 0.5);
        assert_eq!(t.rows[0].frame, Some(0));
        let text = t.to_csv();
        assert!(
            text.contains(&format!("\n{FRAME_HEADER}\n0,1,0\n")),
            "{text}"
        );
        let back = CollectedTrace::parse(&text).unwrap();
        assert_eq!(back.frames, vec![frame(0, 1, 0)]);
        assert_eq!(back.rows, t.rows);
    }

    static CLOCK_READS: AtomicU64 = AtomicU64::new(0);

    /// A trace clock that counts its reads and returns 999.
    fn counting_clock() -> u64 {
        CLOCK_READS.fetch_add(1, Ordering::SeqCst);
        999
    }

    /// REQ-LAT-003 (B2): `presented_at` records exactly the given stamp and
    /// reads no clock; the playfield hook stamps only on the first
    /// `Presented` playfield status (one clock read), and `presented` reads
    /// the clock once.
    #[test]
    fn req_lat_003_presented_at_records_the_given_stamp_without_the_clock() {
        use pin_render::gpu::SurfaceStatus as S;
        let (c, _sim, mut fw) = latency_trace(FlipperBindings::new(), counting_clock);
        let reads = || CLOCK_READS.load(Ordering::SeqCst);
        let r0 = reads();
        assert!(fw.presented_at(true, &at(7), 1234));
        assert!(!fw.presented_at(false, &at(8), 5678));
        assert_eq!(reads(), r0, "presented_at read the clock");
        let mut stamp = None;
        for st in [S::Skipped, S::Hidden, S::Busy, S::Occluded] {
            fw.stamp_playfield(&mut stamp, st);
        }
        assert_eq!((stamp, reads()), (None, r0), "no stamp without a present");
        fw.stamp_playfield(&mut stamp, S::Presented);
        assert_eq!((stamp, reads()), (Some(999), r0 + 1));
        fw.stamp_playfield(&mut stamp, S::Presented);
        assert_eq!((stamp, reads()), (Some(999), r0 + 1), "first present kept");
        assert_eq!(fw.now_ns(), 999);
        assert_eq!(reads(), r0 + 2);
        assert!(fw.presented(true, &at(9)));
        assert_eq!(reads(), r0 + 3);
        let t = c.collect(0);
        assert_eq!(t.frames, vec![frame(0, 7, 1234), frame(1, 9, 999)]);
    }
}
