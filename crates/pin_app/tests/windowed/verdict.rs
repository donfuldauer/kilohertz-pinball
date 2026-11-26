//! The windowed harness's verdict (KP-002): a pure function of the latency
//! trace text (`pin_app --latency-trace`, format version 1, documented in
//! `pin_app::orchestrator::trace`) and the app's stderr (the shutdown line
//! `pin_app: N sim ticks (D dropped), F frames, ...`). No display, clock or
//! file is touched here, so every rule is unit-tested in plain `cargo test`.
//!
//! The parser is written independently of the app's own
//! (`CollectedTrace::parse`) so a formatting bug in the app cannot hide
//! behind a shared parser.
//!
//! # Checks (DECISIONS "#82 — Windowed latency check definitions")
//!
//! Only presses are checked: down events of the four flipper inputs.
//! `move_tick` (the app's trace) is the first tick at whose end a bound bat
//! is raised at least 1 degree (0.01745 rad) from its angle at the applying
//! tick's start, in its energised direction.
//!
//! * **a. Key to coil (REQ-LAT-004, REQ-LAT-005).** For every press:
//!   `apply_tick_end_sdl_ns >= sdl_ts_ns` (never applied before its
//!   timestamp); the lateness `apply_tick_end_sdl_ns - sdl_ts_ns` is at most
//!   [`TICK_NS`] + [`Params::tolerance_ns`] (the main thread's forwarding
//!   delay and the sim thread's wake-up latency), plus, for a press with
//!   `stall = 1`, the observed stall `drain_sdl_ns - prev_drain_sdl_ns` and
//!   no more (a press marked `stall = 1` whose `sdl_ts_ns` is not in the
//!   stall window `[prev_drain_sdl_ns, drain_sdl_ns)` fails and gets no
//!   allowance: the press predates or follows the stall); and the press took effect in the applying tick: its bat was
//!   raised at most [`Params::coil_ticks`] ticks after `apply_tick`. A press
//!   whose bat never moved fails, unless its applying tick ended within
//!   `coil_ticks + 2` ticks of the run's end (`run_end_ns`) and the press is
//!   stamped at or before `run_end_ns` (see check d).
//! * **b. Key to photon in frames (REQ-LAT-001, CI form).** For every press
//!   except those stamped within [`Params::end_exclusion_ns`] of `run_end_ns`
//!   (excluded and counted): `frame` is the first presented frame whose
//!   snapshot tick is in `[move_tick, release_tick]` (the next release of the
//!   same input; unbounded without one) and whose snapshot shows the bat
//!   displaced by at least 1 degree in the energised direction from the
//!   press's applying-tick base (the app's join; the angles are not in the
//!   file, so the verdict checks that the trace's frame exists, matches the
//!   frame table and is such a candidate, not before the first one); a
//!   press without such a frame fails, and so does one whose frame was
//!   presented before `sdl_ts_ns`, or before `drain_sdl_ns` (the sim had
//!   not drained the press yet, so the frame cannot show its snapshot; for
//!   a stalled row the stall's end, which is at or before the applying
//!   tick's drain), fails once and is not counted (equal times pass);
//!   a press stamped after `run_end_ns` is never excluded;
//!   `n_raw = frame - F0`, `F0` the last
//!   frame presented at or before `sdl_ts_ns` (the raw count starts from
//!   the press; `move_tick` only selects the frame); pass when `n_raw <= 2`. One allowance: when
//!   the frame in flight at the press (the first frame presented after
//!   `sdl_ts_ns`) was presented at or before `sdl_ts_ns + bound_a +
//!   coil_ticks` ticks (it could not show the press), the count may start
//!   after it, `n_adj = frame - in_flight`, and the press passes when
//!   `n_adj <= 2` (counted as "in-flight shifted"). Never more than one
//!   frame, and never for a press that failed check a.
//! * **c. Real time (REQ-LAT-002, CI form).** From the shutdown line:
//!   `D / (N + D) <= 0.001`, and `N` within 1 % of the expected run length
//!   (so a run that died early cannot pass).
//! * **d. Completeness.** No trace overflow, at least
//!   [`Params::min_presses`] presses, no input event dropped by the main
//!   thread, and (when the driver passes it) as many presses traced as were
//!   injected. Without overflows, the frame table has exactly as many rows
//!   as the shutdown line's `F frames` (a missing last row has no other
//!   trace); this holds for the windowed run only, which drives one
//!   playfield surface, so `F` (render iterations that presented any
//!   surface) counts exactly the playfield presents the table records. No
//!   press may be stamped after `run_end_ns`: the marker is read after the
//!   last SDL pump, so such a press shows the marker is wrong, and the row
//!   gets neither end-of-run exception (checks a and b).

use std::fmt::Write as _;

/// One tick, ns.
pub const TICK_NS: u64 = 1_000_000;

/// The flipper inputs whose presses are checked.
pub const FLIPPER_INPUTS: [&str; 4] =
    ["left_flipper", "right_flipper", "left_upper", "right_upper"];

/// Event-table header (format version 1).
pub const EVENT_HEADER: &str = "event_seq,input,down,sdl_ts_ns,apply_tick,apply_tick_end_sdl_ns,\
prev_tick_end_sdl_ns,drain_sdl_ns,prev_drain_sdl_ns,dropped_ticks_at_apply,stall,move_tick,\
frame,frame_snap_tick,present_ns,frames_to_photon";
/// Frame-table header.
pub const FRAME_HEADER: &str = "frame,snap_tick,present_ns";
/// Prefix of the trailer's overflow line.
pub const OVERFLOW_PREFIX: &str = "# overflows:";
/// Prefix of the trailer's run-end line.
pub const RUN_END_PREFIX: &str = "# run_end_ns=";

/// One event row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    pub seq: u64,
    pub input: String,
    pub down: bool,
    pub sdl_ts_ns: u64,
    pub apply_tick: u64,
    pub apply_tick_end_sdl_ns: u64,
    pub prev_tick_end_sdl_ns: u64,
    pub drain_sdl_ns: u64,
    pub prev_drain_sdl_ns: u64,
    pub dropped_ticks_at_apply: u64,
    pub stall: bool,
    pub move_tick: Option<u64>,
    pub frame: Option<u64>,
    pub frame_snap_tick: Option<u64>,
    pub present_ns: Option<u64>,
    pub frames_to_photon: Option<u64>,
}

impl Row {
    /// A checked press: a down event of a flipper input.
    pub fn is_flipper_press(&self) -> bool {
        self.down && FLIPPER_INPUTS.contains(&self.input.as_str())
    }
}

/// One frame row.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    pub frame: u64,
    pub snap_tick: u64,
    pub present_ns: u64,
}

/// A parsed trace.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Trace {
    pub rows: Vec<Row>,
    pub frames: Vec<Frame>,
    /// Sum of the trailer's overflow counters.
    pub overflows: u64,
    /// The run loop's end on the trace clock (`# run_end_ns=`).
    pub run_end_ns: u64,
}

/// `r` with the trace line number in its error.
fn at<T>(n: usize, r: Result<T, String>) -> Result<T, String> {
    r.map_err(|e| format!("trace line {n}: {e}"))
}

/// Largest timing value (nanoseconds) a trace may carry: 2^60 ns (about 36
/// years). Larger values are a malformed trace, so no bound, deadline or
/// slack computed from them can wrap.
pub const MAX_TIMING_NS: u64 = 1 << 60;

/// A timing field: an integer of at most [`MAX_TIMING_NS`].
fn ns(name: &str, s: &str) -> Result<u64, String> {
    let v = int(s)?;
    if v > MAX_TIMING_NS {
        return Err(format!("{name} {v} is out of range (limit 2^60 ns)"));
    }
    Ok(v)
}

/// An optional timing field (see [`ns`]).
fn opt_ns(name: &str, s: &str) -> Result<Option<u64>, String> {
    if s.is_empty() {
        Ok(None)
    } else {
        ns(name, s).map(Some)
    }
}

fn int(s: &str) -> Result<u64, String> {
    s.parse::<u64>().map_err(|e| format!("{s:?}: {e}"))
}

fn opt(s: &str) -> Result<Option<u64>, String> {
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
        _ => Err(format!("{s:?} is not 0 or 1")),
    }
}

/// Parses the overflow line's counters (after [`OVERFLOW_PREFIX`]): each
/// of `input`, `move`, `frame`, `pending` exactly once, nothing else.
fn parse_overflows(rest: &str) -> Result<u64, String> {
    let mut seen = [false; 4];
    let mut sum = 0u64;
    for kv in rest.split_whitespace() {
        let (k, v) = kv
            .split_once('=')
            .ok_or_else(|| format!("trailer field {kv:?}"))?;
        let i = ["input", "move", "frame", "pending"]
            .iter()
            .position(|n| *n == k)
            .ok_or_else(|| format!("unknown trailer counter {k:?}"))?;
        if seen[i] {
            return Err(format!("trailer counter {k:?} twice"));
        }
        seen[i] = true;
        sum = sum.saturating_add(int(v)?);
    }
    if seen.contains(&false) {
        return Err("trailer needs input, move, frame and pending".into());
    }
    Ok(sum)
}

/// Parses a trace file's text. Never panics.
///
/// # Errors
/// Missing headers or trailer lines, a frame row or trailer before the
/// frame header, a missing, duplicate or unknown overflow counter, a
/// missing `run_end_ns` line, wrong field counts, non-integer fields, text
/// after the trailer, frame ids that are not consecutive from 0 (a gap, a
/// duplicate, a decrease, or a start other than 0).
pub fn parse_trace(text: &str) -> Result<Trace, String> {
    #[derive(PartialEq)]
    enum Part {
        Events,
        FrameHeader,
        Frames,
        RunEnd,
        Done,
    }
    let mut lines = text.lines();
    if lines.next() != Some(EVENT_HEADER) {
        return Err("trace: missing event header".into());
    }
    let mut t = Trace::default();
    let mut part = Part::Events;
    for (i, line) in lines.enumerate() {
        let n = i + 2;
        match part {
            Part::Events => {
                if line.is_empty() {
                    part = Part::FrameHeader;
                    continue;
                }
                let f: Vec<&str> = line.split(',').collect();
                if f.len() != 16 {
                    return Err(format!("trace line {n}: {} fields, want 16", f.len()));
                }
                t.rows.push(Row {
                    seq: at(n, int(f[0]))?,
                    input: f[1].to_owned(),
                    down: at(n, flag(f[2]))?,
                    sdl_ts_ns: at(n, ns("sdl_ts_ns", f[3]))?,
                    apply_tick: at(n, int(f[4]))?,
                    apply_tick_end_sdl_ns: at(n, ns("apply_tick_end_sdl_ns", f[5]))?,
                    prev_tick_end_sdl_ns: at(n, ns("prev_tick_end_sdl_ns", f[6]))?,
                    drain_sdl_ns: at(n, ns("drain_sdl_ns", f[7]))?,
                    prev_drain_sdl_ns: at(n, ns("prev_drain_sdl_ns", f[8]))?,
                    dropped_ticks_at_apply: at(n, int(f[9]))?,
                    stall: at(n, flag(f[10]))?,
                    move_tick: at(n, opt(f[11]))?,
                    frame: at(n, opt(f[12]))?,
                    frame_snap_tick: at(n, opt(f[13]))?,
                    present_ns: at(n, opt_ns("present_ns", f[14]))?,
                    frames_to_photon: at(n, opt(f[15]))?,
                });
            }
            Part::FrameHeader => {
                if line.trim().is_empty() {
                    continue;
                }
                if line != FRAME_HEADER {
                    return Err(format!("trace line {n}: frame header missing"));
                }
                part = Part::Frames;
            }
            Part::Frames => {
                if let Some(rest) = line.strip_prefix(OVERFLOW_PREFIX) {
                    t.overflows = at(n, parse_overflows(rest))?;
                    part = Part::RunEnd;
                    continue;
                }
                let f: Vec<&str> = line.split(',').collect();
                if f.len() != 3 {
                    return Err(format!("trace line {n}: frame row needs 3 fields"));
                }
                let frame = at(n, int(f[0]))?;
                let want = t.frames.len() as u64;
                if frame != want {
                    return Err(format!(
                        "trace line {n}: frame id {frame}, expected {want} (frame ids must be \
                         consecutive from 0)"
                    ));
                }
                t.frames.push(Frame {
                    frame,
                    snap_tick: at(n, int(f[1]))?,
                    present_ns: at(n, ns("present_ns", f[2]))?,
                });
            }
            Part::RunEnd => {
                let v = line
                    .strip_prefix(RUN_END_PREFIX)
                    .ok_or_else(|| format!("trace line {n}: run_end_ns line missing"))?;
                t.run_end_ns = at(n, ns("run_end_ns", v))?;
                part = Part::Done;
            }
            Part::Done => {
                if !line.trim().is_empty() {
                    return Err(format!("trace line {n}: text after the trailer"));
                }
            }
        }
    }
    match part {
        Part::Done => Ok(t),
        Part::Events | Part::FrameHeader => Err("trace: missing frame table".into()),
        Part::Frames => Err("trace: missing trailer (truncated file?)".into()),
        Part::RunEnd => Err("trace: missing run_end_ns line (malformed trace)".into()),
    }
}

/// The shutdown counters.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Shutdown {
    /// `N sim ticks`.
    pub ticks: u64,
    /// `(D dropped)`.
    pub dropped: u64,
    /// `F frames`.
    pub frames: u64,
    /// `input dropped N`: input events the main thread could not forward
    /// (each a press or release missing from the trace).
    pub input_dropped: u64,
}

/// Finds and parses `pin_app: N sim ticks (D dropped), F frames, input
/// dropped I, ...` in the app's stderr. Never panics.
///
/// # Errors
/// No such line, or a malformed one (also one without the `input dropped`
/// field, which the app always prints).
pub fn parse_shutdown(stderr: &str) -> Result<Shutdown, String> {
    for line in stderr.lines() {
        let Some(rest) = line.strip_prefix("pin_app: ") else {
            continue;
        };
        let Some((n, rest)) = rest.split_once(" sim ticks (") else {
            continue;
        };
        let bad = || format!("malformed shutdown line {line:?}");
        let (d, rest) = rest.split_once(" dropped), ").ok_or_else(bad)?;
        let (f, rest) = rest.split_once(" frames, input dropped ").ok_or_else(bad)?;
        let i = rest.split_once(',').map_or(rest, |(i, _)| i);
        return Ok(Shutdown {
            ticks: int(n).map_err(|_| bad())?,
            dropped: int(d).map_err(|_| bad())?,
            frames: int(f).map_err(|_| bad())?,
            input_dropped: int(i).map_err(|_| bad())?,
        });
    }
    Err("no shutdown line (`pin_app: N sim ticks (D dropped), F frames, ...`)".into())
}

/// Thresholds of the checks.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Params {
    /// Forwarding plus wake-up tolerance on top of one tick (check a).
    pub tolerance_ns: u64,
    /// Ticks after the applying tick within which the bat must be raised
    /// 1 degree (`move_tick - apply_tick`).
    pub coil_ticks: u64,
    /// Most presented frames from the press to the frame showing it.
    pub max_frames: u64,
    /// Presses stamped this close to `run_end_ns` are excluded from check b.
    pub end_exclusion_ns: u64,
    /// The run's expected sim ticks (60 s: 60 000).
    pub expected_ticks: u64,
    /// Fewest presses a run must have.
    pub min_presses: usize,
    /// Presses the driver injected (`None`: not compared).
    pub injected_presses: Option<usize>,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            tolerance_ns: 2_000_000,
            // Measured max 6 ticks (1 degree raise) over 500 presses of
            // three 60 s head runs, plus 2 ticks of margin (DECISIONS "#82 —
            // Windowed latency check definitions").
            coil_ticks: 8,
            max_frames: 2,
            end_exclusion_ns: 500_000_000,
            expected_ticks: 60_000,
            min_presses: 100,
            injected_presses: None,
        }
    }
}

/// One check's outcome: failures (empty = pass) and an info line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Check {
    pub failures: Vec<String>,
    pub info: String,
}

impl Check {
    pub fn passed(&self) -> bool {
        self.failures.is_empty()
    }
}

/// The summary numbers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub presses: usize,
    pub stalled_presses: usize,
    pub excluded_presses: usize,
    pub late_arrivals: usize,
    pub max_lateness_ns: u64,
    pub min_slack_ns: i64,
    /// `n_raw` (frames from the frame presented at or before the press).
    pub max_frames_raw: u64,
    pub p99_frames_raw: u64,
    /// The gated count: `n_adj` for a shifted press, else `n_raw`.
    pub max_frames_adjusted: u64,
    pub p99_frames_adjusted: u64,
    /// Presses that needed the one-frame in-flight allowance.
    pub in_flight_shifted: usize,
    pub p50_photon_ms: f64,
    pub p99_photon_ms: f64,
    pub max_coil_ticks: u64,
    pub max_drain_gap_ns: u64,
    pub p99_drain_gap_ns: u64,
    pub sim_ticks: u64,
    pub dropped_ticks: u64,
    pub dropped_ratio: f64,
    pub sim_to_wall: f64,
    pub frames: usize,
}

/// The four checks and the summary.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Verdict {
    pub a: Check,
    pub b: Check,
    pub c: Check,
    pub d: Check,
    pub summary: Summary,
}

/// `p`-th percentile (nearest rank) of sorted `v`; 0 for an empty slice.
pub fn percentile(sorted: &[u64], p: u64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (sorted.len() as u64 * p).div_ceil(100).max(1);
    sorted[(rank - 1) as usize]
}

/// Whether `r`'s timestamp lies in its stall window
/// `[prev_drain_sdl_ns, drain_sdl_ns)` (half-open). The trace writer marks
/// `stall = 1` only for such a press, also for one delivered in a later
/// catch-up tick of the same batch (same window); the verdict checks it
/// instead of trusting the flag.
pub fn in_stall_window(r: &Row) -> bool {
    r.prev_drain_sdl_ns <= r.sdl_ts_ns && r.sdl_ts_ns < r.drain_sdl_ns
}

/// Check a's bound for `r`: one tick + tolerance, plus the observed stall
/// when the row is marked as stalled and its timestamp lies in the stall
/// window ([`in_stall_window`]); a flagged press outside it gets the
/// ordinary bound.
pub fn coil_bound_ns(r: &Row, p: &Params) -> u64 {
    let stall = if r.stall && in_stall_window(r) {
        r.drain_sdl_ns.saturating_sub(r.prev_drain_sdl_ns)
    } else {
        0
    };
    TICK_NS.saturating_add(p.tolerance_ns).saturating_add(stall)
}

/// Index of the last frame presented at or before `t` (`None` when no frame
/// was).
pub fn last_frame_at_or_before(frames: &[Frame], t: u64) -> Option<u64> {
    frames
        .iter()
        .take_while(|f| f.present_ns <= t)
        .last()
        .map(|f| f.frame)
}

/// The raw count of presented frames from a press at `ts` to `frame`:
/// `frame - F0` ([`last_frame_at_or_before`]), or `frame + 1` when no frame
/// was presented at or before the press (frames 0..=frame all count).
pub fn raw_frames(frames: &[Frame], ts: u64, frame: u64) -> u64 {
    match last_frame_at_or_before(frames, ts) {
        Some(f0) => frame.saturating_sub(f0),
        None => frame.saturating_add(1),
    }
}

/// For each row, the `apply_tick` of the next release (`down = 0`) of the
/// same input after it (`None` when there is none).
pub fn release_ticks(rows: &[Row]) -> Vec<Option<u64>> {
    let mut out = vec![None; rows.len()];
    for (i, r) in rows.iter().enumerate() {
        out[i] = rows[i + 1..]
            .iter()
            .find(|x| !x.down && x.input == r.input)
            .map(|x| x.apply_tick);
    }
    out
}

/// The first frame whose snapshot tick is in `[move_tick, release]`
/// (`release` `None`: unbounded): the first candidate for the frame that
/// showed the bat raised while the key was held (the app takes the first
/// candidate whose snapshot shows the bat raised; the angles are not in the
/// file, so the verdict checks the app's choice is a candidate at or after
/// this one).
pub fn frame_showing(frames: &[Frame], move_tick: u64, release: Option<u64>) -> Option<&Frame> {
    let until = release.unwrap_or(u64::MAX);
    frames
        .iter()
        .find(|f| f.snap_tick >= move_tick && f.snap_tick <= until)
}

/// Check b's frame count for one press: `(n_raw, gated, shifted)`.
/// `n_raw = frame - F0` (`F0` the last frame presented at or before `ts`;
/// `frame + 1` for a press before the first presented frame, [`raw_frames`]).
/// When `n_raw > max_frames`, `allow_shift`, the frame in flight at the
/// press (the first presented after `ts`) was presented at or before
/// `deadline`, and counting after it gives `frame - in_flight <=
/// max_frames`, the press needed the shift: `gated = frame - in_flight`,
/// `shifted = true`. Otherwise `gated = n_raw` (at most one frame of
/// allowance, never a second).
pub fn frame_count(
    frames: &[Frame],
    ts: u64,
    frame: u64,
    deadline: u64,
    max_frames: u64,
    allow_shift: bool,
) -> (u64, u64, bool) {
    let raw = raw_frames(frames, ts, frame);
    if raw <= max_frames || !allow_shift {
        return (raw, raw, false);
    }
    match frames.iter().find(|f| f.present_ns > ts) {
        Some(in_flight)
            if in_flight.present_ns <= deadline
                && in_flight.frame < frame
                && frame - in_flight.frame <= max_frames =>
        {
            (raw, frame - in_flight.frame, true)
        }
        _ => (raw, raw, false),
    }
}

const MAX_LISTED: usize = 10;

fn fail(c: &mut Check, msg: String) {
    if c.failures.len() < MAX_LISTED {
        c.failures.push(msg);
    } else if c.failures.len() == MAX_LISTED {
        c.failures.push("... (more failures not listed)".into());
    }
}

/// Evaluates a parsed trace and shutdown counters.
pub fn evaluate(trace: &Trace, shutdown: &Shutdown, p: &Params) -> Verdict {
    let mut v = Verdict::default();
    let s = &mut v.summary;
    s.frames = trace.frames.len();
    let releases = release_ticks(&trace.rows);
    let presses: Vec<(&Row, Option<u64>)> = trace
        .rows
        .iter()
        .zip(releases)
        .filter(|(r, _)| r.is_flipper_press())
        .collect();
    s.presses = presses.len();
    s.min_slack_ns = i64::MAX;
    let run_end = trace.run_end_ns;
    let mut raw = Vec::new();
    let mut adjusted = Vec::new();
    let mut photon = Vec::new();
    let mut gaps: Vec<u64> = trace
        .rows
        .iter()
        .filter(|r| !r.stall)
        .map(|r| r.drain_sdl_ns.saturating_sub(r.prev_drain_sdl_ns))
        .collect();
    gaps.sort_unstable();
    s.max_drain_gap_ns = gaps.last().copied().unwrap_or(0);
    s.p99_drain_gap_ns = percentile(&gaps, 99);
    for &(r, release) in &presses {
        let tag = format!("press #{} ({} at {} ns)", r.seq, r.input, r.sdl_ts_ns);
        let mut a_failed = false;
        // The run-end marker is read after the last pump polled SDL, so
        // every delivered press is stamped at or before it. A press stamped
        // after it means the marker contradicts the app's own order: the
        // row fails check d and gets neither end-of-run exception (the
        // check-a tail excuse, the check-b end exclusion).
        let after_end = r.sdl_ts_ns > run_end;
        if after_end {
            fail(
                &mut v.d,
                format!(
                    "{tag}: stamped after the run-end marker {run_end} ns (the marker predates a \
                     delivered press; no end-of-run exception applies)"
                ),
            );
        }
        // a. Key to coil.
        if r.stall {
            s.stalled_presses += 1;
        }
        if r.prev_tick_end_sdl_ns >= r.sdl_ts_ns {
            s.late_arrivals += 1;
        }
        let bound = coil_bound_ns(r, p);
        // A press marked as stalled must lie in the stall window; one that
        // does not fails here once (with its timing against the ordinary
        // bound) and its timing failures below are not listed again.
        let bad_stall = r.stall && !in_stall_window(r);
        if bad_stall {
            a_failed = true;
            fail(
                &mut v.a,
                format!(
                    "{tag}: marked as stalled but its timestamp is outside the stall window \
                     [{}, {}) ns; no stall allowance (applied in tick {} ending at {} ns, \
                     bound {bound} ns)",
                    r.prev_drain_sdl_ns, r.drain_sdl_ns, r.apply_tick, r.apply_tick_end_sdl_ns
                ),
            );
        }
        if r.apply_tick_end_sdl_ns < r.sdl_ts_ns {
            a_failed = true;
            if !bad_stall {
                fail(
                    &mut v.a,
                    format!(
                        "{tag}: applied in tick {} ending at {} ns, before its timestamp",
                        r.apply_tick, r.apply_tick_end_sdl_ns
                    ),
                );
            }
        } else {
            let late = r.apply_tick_end_sdl_ns - r.sdl_ts_ns;
            s.max_lateness_ns = s.max_lateness_ns.max(late);
            let slack = i128::from(bound) - i128::from(late);
            s.min_slack_ns = s
                .min_slack_ns
                .min(i64::try_from(slack).unwrap_or(if slack < 0 { i64::MIN } else { i64::MAX }));
            if late > bound {
                a_failed = true;
            }
            if late > bound && !bad_stall {
                fail(
                    &mut v.a,
                    format!(
                        "{tag}: applied {late} ns after its timestamp (tick {}), bound {bound} ns \
                         (stall {})",
                        r.apply_tick,
                        u8::from(r.stall)
                    ),
                );
            }
        }
        match r.move_tick {
            Some(m) => {
                let coil = m.saturating_sub(r.apply_tick);
                s.max_coil_ticks = s.max_coil_ticks.max(coil);
                if m < r.apply_tick || coil > p.coil_ticks {
                    a_failed = true;
                    fail(
                        &mut v.a,
                        format!(
                            "{tag}: delivered in tick {} but the bat was first raised 1 degree in \
                             tick {m} ({coil} ticks later; at most {} allowed)",
                            r.apply_tick, p.coil_ticks
                        ),
                    );
                }
            }
            None => {
                // Only a press applied right before the run ended may have
                // had no time to move its bat.
                let tail = p.coil_ticks.saturating_add(2).saturating_mul(TICK_NS);
                if after_end || run_end.saturating_sub(r.apply_tick_end_sdl_ns) > tail {
                    a_failed = true;
                    fail(
                        &mut v.a,
                        format!(
                            "{tag}: the bat never moved after the press was applied (tick {})",
                            r.apply_tick
                        ),
                    );
                }
            }
        }
        // b. Key to photon in frames.
        if !after_end && run_end - r.sdl_ts_ns < p.end_exclusion_ns {
            s.excluded_presses += 1;
            continue;
        }
        let Some(m) = r.move_tick else {
            fail(&mut v.b, format!("{tag}: the bat never moved"));
            continue;
        };
        // The app picks the frame: the first candidate whose snapshot shows
        // the bat raised (the angles are not in the file). Checked here:
        // the frame exists, matches its frame-table row, is a candidate
        // (snapshot tick in `[move_tick, release_tick]`) and is not before
        // the first candidate.
        let Some(fi) = r.frame else {
            fail(
                &mut v.b,
                format!(
                    "{tag}: no presented frame showed the bat raised while held (move tick {m}, \
                     release tick {release:?})"
                ),
            );
            continue;
        };
        let Some(f) = trace.frames.iter().find(|f| f.frame == fi) else {
            fail(
                &mut v.b,
                format!("{tag}: trace says frame {fi}, the frame table has none"),
            );
            continue;
        };
        if r.frame_snap_tick != Some(f.snap_tick) || r.present_ns != Some(f.present_ns) {
            fail(
                &mut v.b,
                format!(
                    "{tag}: trace says frame {fi} drew tick {:?} at {:?}, the frame table says \
                     tick {} at {}",
                    r.frame_snap_tick, r.present_ns, f.snap_tick, f.present_ns
                ),
            );
        }
        let first = frame_showing(&trace.frames, m, release).map(|c| c.frame);
        if f.snap_tick < m
            || f.snap_tick > release.unwrap_or(u64::MAX)
            || first.is_none_or(|c| fi < c)
        {
            fail(
                &mut v.b,
                format!(
                    "{tag}: trace says frame {fi} (snapshot tick {}), not a frame drawn while \
                     held at or after the move (move tick {m}, release tick {release:?}, first \
                     candidate {first:?})",
                    f.snap_tick
                ),
            );
        }
        // A frame cannot show a press it was presented before; counting it
        // would record a zero-latency photon sample. Equal times pass.
        if f.present_ns < r.sdl_ts_ns {
            fail(
                &mut v.b,
                format!(
                    "{tag}: trace says frame {fi} (snapshot tick {}) showed the press, but it \
                     was presented at {} ns, before the press's timestamp {} ns",
                    f.snap_tick, f.present_ns, r.sdl_ts_ns
                ),
            );
            continue;
        }
        // Nor can it show a snapshot of input the sim had not drained yet:
        // `drain_sdl_ns` is the applying tick's drain (a stall's end for a
        // stalled row, which is at or before it), and the snapshot the frame
        // drew was published after that. Equal times pass.
        if f.present_ns < r.drain_sdl_ns {
            fail(
                &mut v.b,
                format!(
                    "{tag}: trace says frame {fi} (snapshot tick {}) showed the press, but it \
                     was presented at {} ns, before the sim drained the press (drained at {} \
                     ns)",
                    f.snap_tick, f.present_ns, r.drain_sdl_ns
                ),
            );
            continue;
        }
        let deadline = r
            .sdl_ts_ns
            .saturating_add(bound)
            .saturating_add(p.coil_ticks.saturating_mul(TICK_NS));
        let (n_raw, n, shifted) = frame_count(
            &trace.frames,
            r.sdl_ts_ns,
            f.frame,
            deadline,
            p.max_frames,
            !a_failed,
        );
        if r.frames_to_photon != Some(n_raw) {
            fail(
                &mut v.b,
                format!(
                    "{tag}: trace says frames_to_photon {:?}, the frame table says {n_raw}",
                    r.frames_to_photon
                ),
            );
        }
        if shifted {
            s.in_flight_shifted += 1;
        }
        if n > p.max_frames {
            fail(
                &mut v.b,
                format!(
                    "{tag}: bat raised in tick {m}, first shown by frame {} = {n_raw} presented \
                     frames after the press (at most {}{})",
                    f.frame,
                    p.max_frames,
                    if a_failed {
                        "; no in-flight allowance for a press that failed check a"
                    } else {
                        ", or one more when the frame in flight at the press presented before \
                         the coil deadline"
                    }
                ),
            );
        }
        raw.push(n_raw);
        adjusted.push(n);
        photon.push(f.present_ns.saturating_sub(r.sdl_ts_ns));
    }
    if raw.is_empty() && s.presses > s.excluded_presses {
        fail(&mut v.b, "no press could be checked".into());
    } else if raw.is_empty() && s.presses > 0 {
        fail(
            &mut v.b,
            "every press fell in the final window (no frames?)".into(),
        );
    }
    if s.min_slack_ns == i64::MAX {
        s.min_slack_ns = 0;
    }
    raw.sort_unstable();
    adjusted.sort_unstable();
    photon.sort_unstable();
    s.max_frames_raw = raw.last().copied().unwrap_or(0);
    s.p99_frames_raw = percentile(&raw, 99);
    s.max_frames_adjusted = adjusted.last().copied().unwrap_or(0);
    s.p99_frames_adjusted = percentile(&adjusted, 99);
    s.p50_photon_ms = percentile(&photon, 50) as f64 / 1e6;
    s.p99_photon_ms = percentile(&photon, 99) as f64 / 1e6;
    v.a.info = format!(
        "{} presses, {} stalled, {} arrived after their tick ran; max lateness {} ns, min slack {} \
         ns, max coil delay {} ticks",
        s.presses,
        s.stalled_presses,
        s.late_arrivals,
        s.max_lateness_ns,
        s.min_slack_ns,
        s.max_coil_ticks
    );
    v.b.info = format!(
        "{} checked, {} excluded (final {} ms before run end); frames raw max {} p99 {}, adjusted \
         max {} p99 {}, in-flight shifted {}; key to photon p50 {:.1} ms p99 {:.1} ms \
         (information)",
        raw.len(),
        s.excluded_presses,
        p.end_exclusion_ns / 1_000_000,
        s.max_frames_raw,
        s.p99_frames_raw,
        s.max_frames_adjusted,
        s.p99_frames_adjusted,
        s.in_flight_shifted,
        s.p50_photon_ms,
        s.p99_photon_ms
    );
    // c. Real time.
    s.sim_ticks = shutdown.ticks;
    s.dropped_ticks = shutdown.dropped;
    // u128: counters parsed from untrusted output never overflow or wrap.
    let total = u128::from(shutdown.ticks) + u128::from(shutdown.dropped);
    s.dropped_ratio = if total == 0 {
        0.0
    } else {
        shutdown.dropped as f64 / total as f64
    };
    s.sim_to_wall = if total == 0 {
        0.0
    } else {
        shutdown.ticks as f64 / total as f64
    };
    if u128::from(shutdown.dropped) * 1000 > total {
        fail(
            &mut v.c,
            format!(
                "{} of {} ticks dropped ({:.4} %), more than 0.1 %",
                shutdown.dropped,
                total,
                s.dropped_ratio * 100.0
            ),
        );
    }
    if u128::from(shutdown.ticks.abs_diff(p.expected_ticks)) * 100 > u128::from(p.expected_ticks) {
        fail(
            &mut v.c,
            format!(
                "{} sim ticks, not within 1 % of {}",
                shutdown.ticks, p.expected_ticks
            ),
        );
    }
    v.c.info = format!(
        "{} sim ticks, {} dropped ({:.4} %), simulated/wall {:.5}",
        shutdown.ticks,
        shutdown.dropped,
        s.dropped_ratio * 100.0,
        s.sim_to_wall
    );
    // d. Completeness.
    if trace.overflows > 0 {
        fail(
            &mut v.d,
            format!("{} trace rows lost to overflows", trace.overflows),
        );
    } else if u64::try_from(trace.frames.len()).ok() != Some(shutdown.frames) {
        // Valid for the windowed run only: it drives one playfield surface,
        // so the shutdown line's `F frames` (render-loop iterations that
        // presented any surface) counts exactly the playfield presents the
        // frame table records. A multi-surface run would count more. With
        // overflows the lost rows are already reported above.
        fail(
            &mut v.d,
            format!(
                "{} frame rows in the trace, but the shutdown line reports {} frames: frame rows \
                 missing or extra",
                trace.frames.len(),
                shutdown.frames
            ),
        );
    }
    if s.presses < p.min_presses {
        fail(
            &mut v.d,
            format!(
                "{} presses traced, at least {} needed",
                s.presses, p.min_presses
            ),
        );
    }
    if shutdown.input_dropped > 0 {
        fail(
            &mut v.d,
            format!(
                "{} input events dropped by the main thread (each a missing trace row)",
                shutdown.input_dropped
            ),
        );
    }
    if let Some(n) = p.injected_presses
        && n != s.presses
    {
        fail(
            &mut v.d,
            format!("{n} presses injected but {} traced", s.presses),
        );
    }
    v.d.info = format!(
        "{} presses traced (injected {:?}), {} frames, {} overflows, {} input events dropped",
        s.presses, p.injected_presses, s.frames, trace.overflows, shutdown.input_dropped
    );
    v
}

/// The overall summary text: `report` unchanged when the app exited
/// successfully (`exit_failure` is `None`); otherwise the report's own
/// verdict lines are kept as `measurements verdict: …` and the overall line
/// is `verdict: FAIL (app exit status <status>)`, so the artifact and the
/// job agree.
pub fn overall_report(report: &str, exit_failure: Option<&str>) -> String {
    let Some(status) = exit_failure else {
        return report.to_owned();
    };
    let mut out = String::with_capacity(report.len() + 64);
    for line in report.lines() {
        if line.starts_with("verdict: ") {
            out.push_str("measurements ");
        }
        out.push_str(line);
        out.push('\n');
    }
    let _ = writeln!(out, "verdict: FAIL (app exit status {status})");
    out
}

/// Parses both inputs and evaluates them.
///
/// # Errors
/// A trace or shutdown line that does not parse.
pub fn evaluate_text(trace: &str, stderr: &str, p: &Params) -> Result<Verdict, String> {
    Ok(evaluate(&parse_trace(trace)?, &parse_shutdown(stderr)?, p))
}

impl Verdict {
    /// Every check passed.
    pub fn passed(&self) -> bool {
        self.a.passed() && self.b.passed() && self.c.passed() && self.d.passed()
    }

    /// Names of the failed checks.
    pub fn failed_checks(&self) -> Vec<&'static str> {
        [
            ("a", &self.a),
            ("b", &self.b),
            ("c", &self.c),
            ("d", &self.d),
        ]
        .into_iter()
        .filter(|(_, c)| !c.passed())
        .map(|(n, _)| n)
        .collect()
    }

    /// The per-check verdicts and the summary table.
    pub fn report(&self) -> String {
        let mut out = String::new();
        for (name, title, c) in [
            ("a", "key to coil (REQ-LAT-004)", &self.a),
            ("b", "key to photon in frames (REQ-LAT-001)", &self.b),
            ("c", "real time (REQ-LAT-002)", &self.c),
            ("d", "trace complete", &self.d),
        ] {
            let _ = writeln!(
                out,
                "check {name} {title}: {} — {}",
                if c.passed() { "PASS" } else { "FAIL" },
                c.info
            );
            for f in &c.failures {
                let _ = writeln!(out, "    {f}");
            }
        }
        let s = &self.summary;
        let rows: [(&str, String); 14] = [
            ("presses", s.presses.to_string()),
            ("stalled presses", s.stalled_presses.to_string()),
            (
                "excluded presses (final 500 ms before run end)",
                s.excluded_presses.to_string(),
            ),
            (
                "frames_to_photon max / p99 (raw, gated)",
                format!("{} / {}", s.max_frames_raw, s.p99_frames_raw),
            ),
            (
                "frames_to_photon max / p99 (adjusted)",
                format!("{} / {}", s.max_frames_adjusted, s.p99_frames_adjusted),
            ),
            (
                "presses in-flight shifted (one frame)",
                s.in_flight_shifted.to_string(),
            ),
            ("max apply lateness (ns)", s.max_lateness_ns.to_string()),
            ("min apply slack (ns)", s.min_slack_ns.to_string()),
            ("max coil delay (ticks)", s.max_coil_ticks.to_string()),
            (
                "drain gap max / p99 (ns, not stalled)",
                format!("{} / {}", s.max_drain_gap_ns, s.p99_drain_gap_ns),
            ),
            (
                "key to photon p50 / p99 (ms, info)",
                format!("{:.1} / {:.1}", s.p50_photon_ms, s.p99_photon_ms),
            ),
            (
                "sim ticks / dropped",
                format!("{} / {}", s.sim_ticks, s.dropped_ticks),
            ),
            ("dropped ratio", format!("{:.5} %", s.dropped_ratio * 100.0)),
            ("simulated / wall", format!("{:.5}", s.sim_to_wall)),
        ];
        let _ = writeln!(out, "| metric | value |\n|---|---|");
        for (k, val) in rows {
            let _ = writeln!(out, "| {k} | {val} |");
        }
        let _ = writeln!(out, "| frames presented | {} |", s.frames);
        let _ = writeln!(
            out,
            "verdict: {}",
            if self.passed() {
                "PASS".to_owned()
            } else {
                format!("FAIL (checks {})", self.failed_checks().join(", "))
            }
        );
        out
    }
}
