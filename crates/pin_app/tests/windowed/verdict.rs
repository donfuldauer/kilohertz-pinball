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
//! # Checks (DECISIONS "#82 — windowed latency check definitions")
//!
//! Only presses are checked: down events of the four flipper inputs.
//!
//! * **a. Key to coil (REQ-LAT-004, REQ-LAT-005).** For every press:
//!   `apply_tick_end_sdl_ns >= sdl_ts_ns` (never applied before its
//!   timestamp); the lateness `apply_tick_end_sdl_ns - sdl_ts_ns` is at most
//!   [`TICK_NS`] + [`Params::tolerance_ns`] (the main thread's forwarding
//!   delay and the sim thread's wake-up latency), plus, for a press with
//!   `stall = 1`, the observed stall `drain_sdl_ns - prev_drain_sdl_ns` and
//!   no more; and the press took effect in the applying tick: its bat moved
//!   at most [`Params::coil_ticks`] ticks after `apply_tick` (the coil is
//!   energised in the applying tick, REQ-LAT-005).
//! * **b. Key to photon in frames (REQ-LAT-001, CI form).** For every press
//!   except those in the final [`Params::end_exclusion_ns`] of the run
//!   (excluded and counted): the bat moved and a frame drew it, and
//!   `frame - F0 <= 2`, where `frame` is the first presented frame whose
//!   snapshot tick is `>= move_tick` and `F0` the last presented frame whose
//!   present time is at or before the moment REQ-LAT-004 lets the bat start
//!   moving: `sdl_ts_ns` + the press's check-a bound + `coil_ticks` ticks
//!   (0 when none). The trace's raw `frames_to_photon` (F0 at `sdl_ts_ns`)
//!   is reported as information.
//! * **c. Real time (REQ-LAT-002, CI form).** From the shutdown line:
//!   `D / (N + D) <= 0.001`, and `N` within 1 % of the expected run length
//!   (so a run that died early cannot pass).
//! * **d. Completeness.** No trace overflow, at least
//!   [`Params::min_presses`] presses, and (when the driver passes it) as
//!   many presses traced as were injected.

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
}

/// `r` with the trace line number in its error.
fn at<T>(n: usize, r: Result<T, String>) -> Result<T, String> {
    r.map_err(|e| format!("trace line {n}: {e}"))
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

/// Parses a trace file's text. Never panics.
///
/// # Errors
/// Missing headers or trailer, wrong field counts, non-integer fields.
pub fn parse_trace(text: &str) -> Result<Trace, String> {
    let mut lines = text.lines();
    if lines.next() != Some(EVENT_HEADER) {
        return Err("trace: missing event header".into());
    }
    let mut t = Trace::default();
    let mut in_events = true;
    let mut trailer = false;
    for (i, line) in lines.enumerate() {
        let n = i + 2;
        if trailer {
            if !line.trim().is_empty() {
                return Err(format!("trace line {n}: text after the trailer"));
            }
            continue;
        }
        if in_events {
            if line.is_empty() {
                in_events = false;
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
                sdl_ts_ns: at(n, int(f[3]))?,
                apply_tick: at(n, int(f[4]))?,
                apply_tick_end_sdl_ns: at(n, int(f[5]))?,
                prev_tick_end_sdl_ns: at(n, int(f[6]))?,
                drain_sdl_ns: at(n, int(f[7]))?,
                prev_drain_sdl_ns: at(n, int(f[8]))?,
                dropped_ticks_at_apply: at(n, int(f[9]))?,
                stall: at(n, flag(f[10]))?,
                move_tick: at(n, opt(f[11]))?,
                frame: at(n, opt(f[12]))?,
                frame_snap_tick: at(n, opt(f[13]))?,
                present_ns: at(n, opt(f[14]))?,
                frames_to_photon: at(n, opt(f[15]))?,
            });
            continue;
        }
        if line == FRAME_HEADER && t.frames.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("# overflows:") {
            for kv in rest.split_whitespace() {
                let (_, v) = kv
                    .split_once('=')
                    .ok_or_else(|| format!("trace line {n}: trailer field {kv:?}"))?;
                t.overflows = t
                    .overflows
                    .saturating_add(int(v).map_err(|e| format!("trace line {n}: {e}"))?);
            }
            trailer = true;
            continue;
        }
        let f: Vec<&str> = line.split(',').collect();
        if f.len() != 3 {
            return Err(format!("trace line {n}: frame row needs 3 fields"));
        }
        let e = |r: Result<u64, String>| r.map_err(|e| format!("trace line {n}: {e}"));
        t.frames.push(Frame {
            frame: e(int(f[0]))?,
            snap_tick: e(int(f[1]))?,
            present_ns: e(int(f[2]))?,
        });
    }
    if in_events {
        return Err("trace: missing frame table".into());
    }
    if !trailer {
        return Err("trace: missing trailer (truncated file?)".into());
    }
    Ok(t)
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
}

/// Finds and parses `pin_app: N sim ticks (D dropped), F frames, ...` in the
/// app's stderr. Never panics.
///
/// # Errors
/// No such line, or a malformed one.
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
        let (f, _) = rest.split_once(" frames").ok_or_else(bad)?;
        return Ok(Shutdown {
            ticks: int(n).map_err(|_| bad())?,
            dropped: int(d).map_err(|_| bad())?,
            frames: int(f).map_err(|_| bad())?,
        });
    }
    Err("no shutdown line (`pin_app: N sim ticks (D dropped), F frames, ...`)".into())
}

/// Thresholds of the checks.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Params {
    /// Forwarding plus wake-up tolerance on top of one tick (check a).
    pub tolerance_ns: u64,
    /// Ticks after the applying tick within which the bat must move.
    pub coil_ticks: u64,
    /// Most presented frames from the press to the frame showing it.
    pub max_frames: u64,
    /// Presses this close to the end of the run are excluded from check b.
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
            coil_ticks: 2,
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
    pub max_frames_checked: u64,
    pub p99_frames_checked: u64,
    pub max_frames_raw: u64,
    pub p99_frames_raw: u64,
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

/// Check a's bound for `r`: one tick + tolerance, plus the observed stall
/// when the row is marked as stalled.
pub fn coil_bound_ns(r: &Row, p: &Params) -> u64 {
    let stall = if r.stall {
        r.drain_sdl_ns.saturating_sub(r.prev_drain_sdl_ns)
    } else {
        0
    };
    TICK_NS + p.tolerance_ns + stall
}

/// Index of the last frame presented at or before `t` (0 when none).
pub fn last_frame_at_or_before(frames: &[Frame], t: u64) -> u64 {
    frames
        .iter()
        .take_while(|f| f.present_ns <= t)
        .last()
        .map_or(0, |f| f.frame)
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
    let presses: Vec<&Row> = trace.rows.iter().filter(|r| r.is_flipper_press()).collect();
    s.presses = presses.len();
    s.min_slack_ns = i64::MAX;
    let run_end = trace.frames.last().map_or(0, |f| f.present_ns);
    let mut checked = Vec::new();
    let mut raw = Vec::new();
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
    for r in &presses {
        let tag = format!("press #{} ({} at {} ns)", r.seq, r.input, r.sdl_ts_ns);
        // a. Key to coil.
        if r.stall {
            s.stalled_presses += 1;
        }
        if r.prev_tick_end_sdl_ns >= r.sdl_ts_ns {
            s.late_arrivals += 1;
        }
        let bound = coil_bound_ns(r, p);
        if r.apply_tick_end_sdl_ns < r.sdl_ts_ns {
            fail(
                &mut v.a,
                format!(
                    "{tag}: applied in tick {} ending at {} ns, before its timestamp",
                    r.apply_tick, r.apply_tick_end_sdl_ns
                ),
            );
        } else {
            let late = r.apply_tick_end_sdl_ns - r.sdl_ts_ns;
            s.max_lateness_ns = s.max_lateness_ns.max(late);
            s.min_slack_ns = s.min_slack_ns.min(bound as i64 - late as i64);
            if late > bound {
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
        if let Some(m) = r.move_tick {
            let coil = m.saturating_sub(r.apply_tick);
            s.max_coil_ticks = s.max_coil_ticks.max(coil);
            if m < r.apply_tick || coil > p.coil_ticks {
                fail(
                    &mut v.a,
                    format!(
                        "{tag}: delivered in tick {} but the bat first moved in tick {m} ({coil} \
                         ticks later; at most {} allowed)",
                        r.apply_tick, p.coil_ticks
                    ),
                );
            }
        }
        // b. Key to photon in frames.
        if run_end.saturating_sub(r.sdl_ts_ns) < p.end_exclusion_ns {
            s.excluded_presses += 1;
            continue;
        }
        let Some(m) = r.move_tick else {
            fail(&mut v.b, format!("{tag}: the bat never moved"));
            continue;
        };
        let Some(f) = trace.frames.iter().find(|f| f.snap_tick >= m) else {
            fail(&mut v.b, format!("{tag}: no presented frame drew tick {m}"));
            continue;
        };
        if r.frame != Some(f.frame) {
            fail(
                &mut v.b,
                format!(
                    "{tag}: trace says frame {:?}, the frame table says {}",
                    r.frame, f.frame
                ),
            );
        }
        let f_raw = f
            .frame
            .saturating_sub(last_frame_at_or_before(&trace.frames, r.sdl_ts_ns));
        if r.frames_to_photon != Some(f_raw) {
            fail(
                &mut v.b,
                format!(
                    "{tag}: trace says frames_to_photon {:?}, the frame table says {f_raw}",
                    r.frames_to_photon
                ),
            );
        }
        let due = r.sdl_ts_ns + bound + p.coil_ticks * TICK_NS;
        let n = f
            .frame
            .saturating_sub(last_frame_at_or_before(&trace.frames, due));
        if n > p.max_frames {
            fail(
                &mut v.b,
                format!(
                    "{tag}: bat moved in tick {m}, first drawn by frame {} = {n} presented frames \
                     after the press (at most {})",
                    f.frame, p.max_frames
                ),
            );
        }
        checked.push(n);
        raw.push(f_raw);
        photon.push(f.present_ns.saturating_sub(r.sdl_ts_ns));
    }
    if checked.is_empty() && s.presses > s.excluded_presses {
        fail(&mut v.b, "no press could be checked".into());
    } else if checked.is_empty() && s.presses > 0 {
        fail(
            &mut v.b,
            "every press fell in the final window (no frames?)".into(),
        );
    }
    if s.min_slack_ns == i64::MAX {
        s.min_slack_ns = 0;
    }
    checked.sort_unstable();
    raw.sort_unstable();
    photon.sort_unstable();
    s.max_frames_checked = checked.last().copied().unwrap_or(0);
    s.p99_frames_checked = percentile(&checked, 99);
    s.max_frames_raw = raw.last().copied().unwrap_or(0);
    s.p99_frames_raw = percentile(&raw, 99);
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
        "{} checked, {} excluded (final {} ms); frames max {} p99 {} (raw, F0 at the timestamp: \
         max {} p99 {}); key to photon p50 {:.1} ms p99 {:.1} ms (information)",
        checked.len(),
        s.excluded_presses,
        p.end_exclusion_ns / 1_000_000,
        s.max_frames_checked,
        s.p99_frames_checked,
        s.max_frames_raw,
        s.p99_frames_raw,
        s.p50_photon_ms,
        s.p99_photon_ms
    );
    // c. Real time.
    s.sim_ticks = shutdown.ticks;
    s.dropped_ticks = shutdown.dropped;
    let total = shutdown.ticks + shutdown.dropped;
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
    if shutdown.dropped * 1000 > total {
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
    if shutdown.ticks.abs_diff(p.expected_ticks) * 100 > p.expected_ticks {
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
    if let Some(n) = p.injected_presses
        && n != s.presses
    {
        fail(
            &mut v.d,
            format!("{n} presses injected but {} traced", s.presses),
        );
    }
    v.d.info = format!(
        "{} presses traced (injected {:?}), {} frames, {} overflows",
        s.presses, p.injected_presses, s.frames, trace.overflows
    );
    v
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
                "excluded presses (final 500 ms)",
                s.excluded_presses.to_string(),
            ),
            (
                "frames_to_photon max (checked)",
                s.max_frames_checked.to_string(),
            ),
            (
                "frames_to_photon p99 (checked)",
                s.p99_frames_checked.to_string(),
            ),
            (
                "frames_to_photon max / p99 (raw)",
                format!("{} / {}", s.max_frames_raw, s.p99_frames_raw),
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
