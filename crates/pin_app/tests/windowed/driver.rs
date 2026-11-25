//! The windowed driver (KP-002): starts the release `pin_app` with the
//! grey-box table under Xvfb on lavapipe (`PIN_FORCE_FALLBACK_ADAPTER=1`:
//! the software adapter or nothing), waits for the first presented frame,
//! injects flipper presses through XTest (`xdotool keydown/keyup` with the X
//! keycodes of Shift_L and Shift_R) on a seeded plan ([`press_plan`]), takes
//! screenshots of the root window, and returns the app's stderr and its
//! latency trace. Every helper process runs bounded ([`run_bounded`]: killed
//! and reaped at its deadline) and shell-free; a hung app is killed and
//! reported. A run starts by creating the artifact directory and removing
//! the previous run's artifacts ([`ARTIFACTS`]) before any check
//! ([`preflight`]); every exit path after the app started writes a fresh
//! `pin_app_stderr.log` (the stderr drain is bounded by
//! [`STDERR_DRAIN_TIMEOUT`]), and every failure, the preflight's included,
//! a `summary.txt` ([`failure_summary`]).
//!
//! Environment (all optional):
//! * `KP_PIN_APP`: the app binary (default: this build's `pin_app`;
//!   `cargo test --release` makes it the release binary);
//! * `KP_GREYBOX_BUNDLE`: the grey-box bundle (default
//!   `<target>/greybox`, built by `tables/greybox/build.sh`);
//! * `KP_WINDOWED_OUT`: the artifact directory (default
//!   `<target>/windowed`);
//! * `KP_WINDOWED_RUN_MS`: the run length (default 60 000 ms);
//! * `KP_WINDOWED_SEED`: the press schedule's seed;
//! * `KP_WINDOWED_FRAME_MS`: the frame interval the press holds are sized
//!   for (default [`DEFAULT_FRAME_INTERVAL_MS`]);
//! * `LP_NUM_THREADS`: lavapipe's worker threads (default
//!   [`DEFAULT_LP_NUM_THREADS`]);
//! * `DISPLAY`: an X server to use; without it the driver starts its own
//!   Xvfb (1280x720x24, `-displayfd`: Xvfb picks the display and reports it
//!   when it accepts connections).

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

/// lavapipe worker threads when `LP_NUM_THREADS` is not set (DECISIONS
/// "#82 — LP_NUM_THREADS cap").
pub const DEFAULT_LP_NUM_THREADS: &str = "2";
/// Default press schedule seed.
pub const DEFAULT_SEED: u64 = 0x4b50_3030_3220_4c41;
/// X keycode of Shift_L (left flipper) in Xvfb's default keymap. The
/// keysym `Shift_R` makes xdotool also press Shift_L, so keycodes are used.
pub const KEY_LEFT: &str = "50";
/// X keycode of Shift_R (right flipper).
pub const KEY_RIGHT: &str = "62";
/// Stop pressing this long before the run's end.
pub const PRESS_END_MARGIN: Duration = Duration::from_millis(1500);
/// Longest wait for the first presented frame.
pub const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(90);
/// Grace after the run time before a still-running app is killed.
pub const EXIT_MARGIN: Duration = Duration::from_secs(60);
/// Deadline of one helper process (xdotool, xwd, convert).
pub const HELPER_TIMEOUT: Duration = Duration::from_secs(5);
/// Longest wait for Xvfb to report its display.
pub const XVFB_TIMEOUT: Duration = Duration::from_secs(10);
/// The frame interval press holds are sized for when
/// `KP_WINDOWED_FRAME_MS` is not set: a conservative 8 frames per second
/// (lavapipe draws the grey-box table at about 8.8 with `LP_NUM_THREADS=2`).
pub const DEFAULT_FRAME_INTERVAL_MS: u64 = 125;
/// The environment variable that restricts wgpu to the fallback (software)
/// adapter (`pin_render::gpu::FORCE_FALLBACK_ENV`).
pub const FORCE_FALLBACK_ENV: &str = "PIN_FORCE_FALLBACK_ADAPTER";
/// Files of a previous run removed from the artifact directory first
/// (plus every `frame_*.png`).
pub const ARTIFACTS: [&str; 3] = ["latency_trace.csv", "pin_app_stderr.log", "summary.txt"];
/// How long a screenshot hold keeps its key down before the capture.
pub const SHOT_HOLD: Duration = Duration::from_millis(800);

/// What to run.
#[derive(Clone, Debug)]
pub struct DriverConfig {
    pub app: PathBuf,
    pub bundle: PathBuf,
    pub out_dir: PathBuf,
    pub run_ms: u64,
    pub lp_num_threads: String,
    pub seed: u64,
    pub frame_interval_ms: u64,
}

fn target_dir() -> PathBuf {
    // CARGO_TARGET_TMPDIR is <target>/tmp.
    Path::new(env!("CARGO_TARGET_TMPDIR"))
        .parent()
        .map_or_else(|| PathBuf::from("target"), Path::to_path_buf)
}

impl DriverConfig {
    /// The configuration from the environment (see the module docs).
    pub fn from_env() -> Self {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        Self {
            app: var("KP_PIN_APP").map_or_else(
                || PathBuf::from(env!("CARGO_BIN_EXE_pin_app")),
                PathBuf::from,
            ),
            bundle: var("KP_GREYBOX_BUNDLE")
                .map_or_else(|| target_dir().join("greybox"), PathBuf::from),
            out_dir: var("KP_WINDOWED_OUT")
                .map_or_else(|| target_dir().join("windowed"), PathBuf::from),
            run_ms: var("KP_WINDOWED_RUN_MS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(60_000),
            lp_num_threads: var("LP_NUM_THREADS")
                .unwrap_or_else(|| DEFAULT_LP_NUM_THREADS.to_owned()),
            seed: var("KP_WINDOWED_SEED")
                .and_then(|v| v.parse().ok())
                .unwrap_or(DEFAULT_SEED),
            frame_interval_ms: var("KP_WINDOWED_FRAME_MS")
                .and_then(|v| v.parse().ok())
                .filter(|v| *v > 0)
                .unwrap_or(DEFAULT_FRAME_INTERVAL_MS),
        }
    }

    /// The trace file.
    pub fn trace_path(&self) -> PathBuf {
        self.out_dir.join("latency_trace.csv")
    }
}

/// What a run produced.
#[derive(Debug)]
pub struct RunOutput {
    pub stderr: String,
    pub trace: String,
    pub injected_presses: usize,
    pub exit: ExitStatus,
    pub screenshots: Vec<PathBuf>,
}

/// A small deterministic generator for the press schedule (xorshift64*).
#[derive(Clone, Debug)]
pub struct Schedule(u64);

impl Schedule {
    pub fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    /// The next raw value.
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// A duration in `[lo_ms, hi_ms)` with microsecond resolution (never
    /// a whole number of ticks by construction).
    pub fn duration(&mut self, lo_ms: u64, hi_ms: u64) -> Duration {
        let span_us = (hi_ms - lo_ms) * 1000;
        Duration::from_micros(lo_ms * 1000 + self.next_u64() % span_us)
    }
}

/// A flipper key.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Left,
    Right,
}

impl Key {
    /// The X keycode xdotool sends.
    pub fn keycode(self) -> &'static str {
        match self {
            Key::Left => KEY_LEFT,
            Key::Right => KEY_RIGHT,
        }
    }
}

/// One step of the press plan.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Down(Key),
    Up(Key),
    /// A screenshot into the named file.
    Shot(&'static str),
}

/// A step at `at_us` microseconds after the plan's start.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Action {
    pub at_us: u64,
    pub step: Step,
}

/// The base hold of every press: three frame intervals, capped at 1 s, so a
/// presented frame reads its snapshot while the bat is still raised
/// (check b counts only frames that showed the bat raised while held).
pub fn base_hold_us(frame_interval_ms: u64) -> u64 {
    (3 * frame_interval_ms).min(1000) * 1000
}

/// The press plan over `span_us` microseconds (DECISIONS "#82 — Windowed
/// press schedule"): the left and right keys each press on their own
/// timeline (hold [`base_hold_us`] plus 0 to 170 ms of jitter, gap 100 to
/// 250 ms, both in microseconds so never aligned to ticks), the right one
/// phase-shifted, so the keys alternate and overlap. A rest screenshot
/// comes first; after 10 presses the plan waits for both keys to be up and
/// holds the left key alone for [`SHOT_HOLD`] with a screenshot, after 20
/// the right key, after 30 a rest screenshot after [`SHOT_HOLD`] of quiet.
/// Seeded and deterministic; every down has its up before the plan ends.
pub fn press_plan(seed: u64, span_us: u64, frame_interval_ms: u64) -> Vec<Action> {
    let mut rng = Schedule::new(seed);
    let base = base_hold_us(frame_interval_ms);
    let shot_us = SHOT_HOLD.as_micros() as u64;
    let mut out = vec![Action {
        at_us: 0,
        step: Step::Shot("frame_0_rest.png"),
    }];
    let mut presses = 0usize;
    // (presses before it, the key held alone with its screenshot, or a
    // rest screenshot).
    let barriers = [
        (10usize, Some((Key::Left, "frame_1_left_held.png"))),
        (20, Some((Key::Right, "frame_2_right_held.png"))),
        (30, None),
    ];
    let mut barrier = 0;
    let phase = base / 2 + 60_000;
    // Next down time of each key.
    let mut next = [500_137u64, 500_137 + phase];
    let keys = [Key::Left, Key::Right];
    loop {
        if let Some(&(at, solo)) = barriers.get(barrier)
            && presses >= at
        {
            barrier += 1;
            // Both keys are up once every planned step has happened.
            let quiet = out.iter().map(|a| a.at_us).max().unwrap_or(0) + 150_000;
            if quiet + shot_us + 1_000_000 > span_us {
                break;
            }
            match solo {
                Some((key, name)) => {
                    out.push(Action {
                        at_us: quiet,
                        step: Step::Down(key),
                    });
                    out.push(Action {
                        at_us: quiet + shot_us,
                        step: Step::Shot(name),
                    });
                    // After the capture (a slow capture delays it).
                    out.push(Action {
                        at_us: quiet + shot_us + 1,
                        step: Step::Up(key),
                    });
                    presses += 1;
                }
                None => out.push(Action {
                    at_us: quiet + shot_us,
                    step: Step::Shot("frame_3_rest.png"),
                }),
            }
            let resume = quiet + shot_us + 200_137;
            next = [resume, resume + phase];
            continue;
        }
        // The key whose next press comes first.
        let k = usize::from(next[1] < next[0]);
        let down = next[k];
        let hold = base + rng.duration(0, 170).as_micros() as u64;
        let gap = rng.duration(100, 250).as_micros() as u64;
        if down + hold + 100_000 > span_us {
            break;
        }
        out.push(Action {
            at_us: down,
            step: Step::Down(keys[k]),
        });
        out.push(Action {
            at_us: down + hold,
            step: Step::Up(keys[k]),
        });
        presses += 1;
        next[k] = down + hold + gap;
    }
    out.sort_by_key(|a| a.at_us);
    out
}

/// Presses (downs) in a plan.
pub fn planned_presses(plan: &[Action]) -> usize {
    plan.iter()
        .filter(|a| matches!(a.step, Step::Down(_)))
        .count()
}

/// Kills and reaps a child on drop.
pub struct Reaper(pub Option<Child>);

impl Drop for Reaper {
    fn drop(&mut self) {
        if let Some(c) = self.0.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn have(tool: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(tool).is_file()))
}

/// Reads `r` to its end on a thread; the bytes arrive on the receiver.
fn read_all_async(mut r: impl Read + Send + 'static) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = r.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    rx
}

/// What [`wait_with`] needs from a child process: a seam so the
/// kill-at-deadline decision runs under a controlled clock in the headless
/// tests (PROCESS.md R6).
pub trait Waitable {
    /// Non-blocking poll for the exit status.
    ///
    /// # Errors
    /// The poll failed.
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>>;
    /// Kills the process (errors ignored).
    fn kill(&mut self);
    /// Reaps the process (errors ignored).
    fn reap(&mut self);
}

impl Waitable for Child {
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        Child::try_wait(self)
    }
    fn kill(&mut self) {
        let _ = Child::kill(self);
    }
    fn reap(&mut self) {
        let _ = Child::wait(self);
    }
}

/// Polls `w` until it exits or `now()` (a monotonic time since a fixed
/// start) reaches `deadline`; between polls calls `pause`. At the deadline,
/// or when a poll fails, kills and reaps `w` once and returns an error
/// naming `what`.
///
/// # Errors
/// The poll failed, or the deadline passed (the error contains "killed").
pub fn wait_with<W: Waitable>(
    w: &mut W,
    deadline: Duration,
    now: &mut dyn FnMut() -> Duration,
    pause: &mut dyn FnMut(),
    what: &str,
) -> Result<ExitStatus, String> {
    loop {
        match w.try_wait() {
            Ok(Some(st)) => return Ok(st),
            Ok(None) => {}
            Err(e) => {
                w.kill();
                w.reap();
                return Err(format!("{what}: {e}; killed"));
            }
        }
        if now() >= deadline {
            w.kill();
            w.reap();
            return Err(format!("{what}: no exit within its deadline; killed"));
        }
        pause();
    }
}

/// Polls `child` until it exits or `deadline`; at the deadline kills and
/// reaps it and returns an error naming `what` (the real clock over
/// [`wait_with`]).
fn wait_bounded(child: &mut Child, deadline: Instant, what: &str) -> Result<ExitStatus, String> {
    let start = Instant::now();
    let limit = deadline.saturating_duration_since(start);
    wait_with(
        child,
        limit,
        &mut || start.elapsed(),
        &mut || std::thread::sleep(Duration::from_millis(5)),
        what,
    )
}

/// Real-process evidence for the bounded runner (windowed run only: it
/// runs real helper processes): a helper that never exits is killed and
/// reaped at its deadline (the error says "killed"), and a short one
/// returns its output. No elapsed-time assertion: wall time is not a SPEC
/// 7.5 measurement (PROCESS.md R6).
///
/// # Errors
/// Any of those checks failed.
pub fn self_check_bounded_runner() -> Result<(), String> {
    let e = run_bounded(Command::new("sleep").arg("30"), Duration::from_millis(300))
        .err()
        .ok_or("bounded runner: `sleep 30` was not killed")?;
    if !e.contains("killed") {
        return Err(format!("bounded runner: error without \"killed\": {e}"));
    }
    let out = run_bounded(Command::new("echo").arg("hi"), Duration::from_secs(5))?;
    if !out.status.success() || out.stdout != "hi\n" {
        return Err(format!("bounded runner: `echo hi` gave {out:?}"));
    }
    Ok(())
}

/// What a bounded helper printed.
#[derive(Debug)]
pub struct HelperOutput {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

/// Runs `cmd` (stdin null, stdout and stderr captured) with a deadline of
/// `timeout`: polled with `try_wait`; at the deadline the child is killed
/// and reaped and an error returned. Never blocks on an unbounded child.
///
/// # Errors
/// The spawn failed, or the deadline passed.
pub fn run_bounded(cmd: &mut Command, timeout: Duration) -> Result<HelperOutput, String> {
    let what = format!("{:?}", cmd.get_program());
    let deadline = Instant::now() + timeout;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{what}: {e}"))?;
    let out = child.stdout.take().map(read_all_async);
    let err = child.stderr.take().map(read_all_async);
    let status = wait_bounded(&mut child, deadline, &what)?;
    // The pipes close with the child; a grandchild holding them open is
    // not waited for beyond the deadline.
    let left = deadline
        .saturating_duration_since(Instant::now())
        .max(Duration::from_millis(100));
    let text = |rx: Option<mpsc::Receiver<Vec<u8>>>| {
        rx.and_then(|rx| rx.recv_timeout(left).ok())
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default()
    };
    Ok(HelperOutput {
        status,
        stdout: text(out),
        stderr: text(err),
    })
}

/// Our own Xvfb (niced; `nice` execs Xvfb, so the pid is Xvfb's): Xvfb
/// chooses a free display and writes its number to its stdout
/// (`-displayfd 1`) once it accepts connections; the number is read with a
/// bounded wait. Returns the display name (`:N`) and the reaper.
fn start_xvfb() -> Result<(String, Reaper), String> {
    let child = Command::new("nice")
        .args(xvfb_args())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("starting Xvfb: {e}"))?;
    let mut reaper = Reaper(Some(child));
    let stdout = reaper
        .0
        .as_mut()
        .and_then(|c| c.stdout.take())
        .ok_or("Xvfb: no stdout pipe")?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(stdout).read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = rx
        .recv_timeout(XVFB_TIMEOUT)
        .map_err(|_| format!("Xvfb reported no display within {XVFB_TIMEOUT:?}"))?;
    let n: u32 = line
        .trim()
        .parse()
        .map_err(|_| format!("Xvfb -displayfd wrote {line:?}, not a display number"))?;
    if let Some(c) = reaper.0.as_mut()
        && let Ok(Some(st)) = c.try_wait()
    {
        return Err(format!("Xvfb exited ({st}) after reporting display {n}"));
    }
    Ok((format!(":{n}"), reaper))
}

/// `nice`'s arguments that start Xvfb with `-displayfd 1`.
pub fn xvfb_args() -> Vec<&'static str> {
    vec![
        "-n",
        "10",
        "Xvfb",
        "-displayfd",
        "1",
        "-screen",
        "0",
        "1280x720x24",
        "-nolisten",
        "tcp",
    ]
}

fn xdotool(display: &str, args: &[&str]) -> Result<String, String> {
    xdotool_within(display, args, HELPER_TIMEOUT)
}

/// [`xdotool`] bounded by `timeout` instead of [`HELPER_TIMEOUT`].
fn xdotool_within(display: &str, args: &[&str], timeout: Duration) -> Result<String, String> {
    let out = run_bounded(
        Command::new("xdotool").args(args).env("DISPLAY", display),
        timeout,
    )
    .map_err(|e| format!("xdotool {args:?}: {e}"))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(format!("xdotool {args:?}: {} {}", out.status, out.stderr))
    }
}

/// The screenshot's two processes, shell-free: `nice -n 10 xwd -root
/// -silent -display <display>` piped into `nice -n 10 convert xwd:-
/// png:<path>` (the display and the path are arguments, never shell
/// source). Returns both argument vectors (program `nice`).
pub fn screenshot_argv(display: &str, path: &Path) -> [Vec<OsString>; 2] {
    let mut png = OsString::from("png:");
    png.push(path.as_os_str());
    [
        ["-n", "10", "xwd", "-root", "-silent", "-display", display]
            .map(OsString::from)
            .to_vec(),
        vec![
            "-n".into(),
            "10".into(),
            "convert".into(),
            "xwd:-".into(),
            png,
        ],
    ]
}

fn screenshot(display: &str, path: &Path) -> Result<(), String> {
    let [xwd_args, convert_args] = screenshot_argv(display, path);
    let deadline = Instant::now() + HELPER_TIMEOUT;
    // Niced and single-threaded, so a capture does not take the sim's core.
    let mut xwd = Command::new("nice")
        .args(&xwd_args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("screenshot: xwd: {e}"))?;
    let pipe = xwd.stdout.take().ok_or("screenshot: no xwd pipe")?;
    let mut xwd = Reaper(Some(xwd));
    let mut convert = Reaper(Some(
        Command::new("nice")
            .args(&convert_args)
            .env("MAGICK_THREAD_LIMIT", "1")
            .stdin(Stdio::from(pipe))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("screenshot: convert: {e}"))?,
    ));
    let a = xwd
        .0
        .as_mut()
        .map(|c| wait_bounded(c, deadline, "xwd"))
        .ok_or("xwd")??;
    xwd.0 = None;
    let b = convert
        .0
        .as_mut()
        .map(|c| wait_bounded(c, deadline, "convert"))
        .ok_or("convert")??;
    convert.0 = None;
    if a.success() && b.success() && path.is_file() {
        Ok(())
    } else {
        Err(format!(
            "screenshot {} failed (xwd {a}, convert {b})",
            path.display()
        ))
    }
}

/// The last `n` lines of `text`.
pub fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// The `summary.txt` of a failed run: what failed and the app's stderr
/// tail.
pub fn failure_summary(what: &str, stderr: &str) -> String {
    format!(
        "verdict: FAIL (windowed driver: {what})\n--- pin_app stderr (tail) ---\n{}\n",
        tail(stderr, 40)
    )
}

/// Why the screenshots are not enough (at least 3, one with a flipper
/// held), or `None`.
pub fn missing_captures(shots: &[PathBuf]) -> Option<String> {
    let names: Vec<String> = shots
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    let held = names.iter().any(|n| n.contains("_held"));
    if names.len() >= 3 && held {
        return None;
    }
    let all = [
        "frame_0_rest.png",
        "frame_1_left_held.png",
        "frame_2_right_held.png",
        "frame_3_rest.png",
    ];
    let missing: Vec<&str> = all
        .iter()
        .copied()
        .filter(|a| !names.iter().any(|n| n == a))
        .collect();
    Some(format!(
        "{} screenshots captured (at least 3 needed, one with a flipper held{}); missing: {}",
        names.len(),
        if held { "" } else { ": none" },
        missing.join(", ")
    ))
}

/// Removes a previous run's artifacts from `dir`.
fn clean_artifacts(dir: &Path) {
    for f in ARTIFACTS {
        let _ = std::fs::remove_file(dir.join(f));
    }
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.starts_with("frame_") && n.ends_with(".png") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// What [`drive`] did before the app exited.
struct Driven {
    injected: usize,
    shots: Vec<PathBuf>,
    exit: ExitStatus,
}

/// Marks `k` held BEFORE `send` runs its key-down, so a key-down that
/// fails after reaching the X server is still released.
///
/// # Errors
/// What `send` returned.
pub fn key_down(
    held: &mut [bool; 2],
    k: Key,
    mut send: impl FnMut(Key) -> Result<(), String>,
) -> Result<(), String> {
    held[usize::from(k == Key::Right)] = true;
    send(k)
}

/// Sends `k`'s key-up; `k` counts as released once it succeeds (a failed
/// key-up stays held, for [`release_held`] to retry).
///
/// # Errors
/// What `send` returned.
pub fn key_up(
    held: &mut [bool; 2],
    k: Key,
    mut send: impl FnMut(Key) -> Result<(), String>,
) -> Result<(), String> {
    send(k)?;
    held[usize::from(k == Key::Right)] = false;
    Ok(())
}

/// Total time [`release_held`] may spend on the key-ups at the end of a
/// run: one shared deadline for every attempt and retry.
pub const KEY_RELEASE_BUDGET: Duration = Duration::from_millis(400);
/// The shortest bound a key's first key-up attempt gets, even when the
/// shared budget is spent (so the second key's first attempt still runs).
/// A retry of a key that already had an attempt gets no such floor: it is
/// bounded by the budget left ([`key_up_retry_timeout`]) and skipped once
/// the budget is spent.
pub const KEY_RELEASE_MIN_TIMEOUT: Duration = Duration::from_millis(50);
/// Pause between two rounds of [`release_held`] retries.
pub const KEY_RELEASE_PAUSE: Duration = Duration::from_millis(50);
/// Most rounds of key-ups [`release_held`] sends, whatever the clock says.
pub const KEY_RELEASE_MAX_ROUNDS: u32 = 16;

/// The bound of one key-up attempt at `now` within the shared `budget`:
/// what is left of it, never above [`HELPER_TIMEOUT`] and never below
/// [`KEY_RELEASE_MIN_TIMEOUT`]. Only a key's first attempt uses it
/// (retries use [`key_up_retry_timeout`]).
pub fn key_up_timeout(budget: Duration, now: Duration) -> Duration {
    budget
        .saturating_sub(now)
        .clamp(KEY_RELEASE_MIN_TIMEOUT, HELPER_TIMEOUT)
}

/// The bound of a key-up retry (a key that already had an attempt) at `now`
/// within the shared `budget`: what is left of it, never above
/// [`HELPER_TIMEOUT`], with no minimum, so a retry never runs past the
/// shared deadline.
pub fn key_up_retry_timeout(budget: Duration, now: Duration) -> Duration {
    budget.saturating_sub(now).min(HELPER_TIMEOUT)
}

/// Attempts a key-up for every key in `held` (never stopping at the first
/// failure; [`key_up`]: a key is cleared only when its key-up succeeded).
/// One shared deadline: `now()` (a monotonic time since the release
/// started) against `budget`. A key's first `send(key, timeout)` is bounded
/// by [`key_up_timeout`] (the budget left, floored at
/// [`KEY_RELEASE_MIN_TIMEOUT`]), so every held key gets a first attempt.
/// While a key is still held and budget remains, retries it after `pause`,
/// at most [`KEY_RELEASE_MAX_ROUNDS`] rounds, each retry bounded by
/// [`key_up_retry_timeout`] (the budget left, no floor); a key that already
/// had an attempt is not sent again once `now() >= budget` (it stays in the
/// failed list with its last error). The whole release therefore never
/// exceeds the budget by more than the first attempts' floor. Returns the releases that still
/// failed after the last round (their keys stay marked held).
pub fn release_held(
    held: &mut [bool; 2],
    mut send: impl FnMut(Key, Duration) -> Result<(), String>,
    budget: Duration,
    now: &mut dyn FnMut() -> Duration,
    pause: &mut dyn FnMut(),
) -> Vec<(Key, String)> {
    let mut round = 0;
    let mut last_err: [Option<String>; 2] = [None, None];
    loop {
        round += 1;
        let mut failed = Vec::new();
        for k in [Key::Left, Key::Right] {
            let i = usize::from(k == Key::Right);
            if !held[i] {
                continue;
            }
            let t = now();
            let t = match &last_err[i] {
                Some(prev) if t >= budget => {
                    failed.push((k, prev.clone()));
                    continue;
                }
                Some(_) => key_up_retry_timeout(budget, t),
                None => key_up_timeout(budget, t),
            };
            if let Err(e) = key_up(held, k, |k| send(k, t)) {
                last_err[i] = Some(e.clone());
                failed.push((k, e));
            }
        }
        if failed.is_empty() || round >= KEY_RELEASE_MAX_ROUNDS || now() >= budget {
            return failed;
        }
        pause();
        if now() >= budget {
            return failed;
        }
    }
}

/// Combines the injection result with the failed releases: the original
/// error is returned unchanged (release failures are logged); a release
/// failure after a successful injection becomes `key release: ...`.
///
/// # Errors
/// The injection error, or the release failures.
pub fn finish_injection(
    injection: Result<(), String>,
    failed: &[(Key, String)],
) -> Result<(), String> {
    for (k, e) in failed {
        eprintln!("windowed: key release failed for {k:?}: {e}");
    }
    injection?;
    if failed.is_empty() {
        Ok(())
    } else {
        let list: Vec<String> = failed.iter().map(|(k, e)| format!("{k:?}: {e}")).collect();
        Err(format!("key release: {}", list.join("; ")))
    }
}

/// Steps 1 to 4 of a run, after the app started: first frame, window,
/// presses, exit. Errors leave the app to the caller (killed there).
fn drive(
    cfg: &DriverConfig,
    display: &str,
    pid: u32,
    started: Instant,
    app: &mut Reaper,
    stderr_now: &dyn Fn() -> String,
) -> Result<Driven, String> {
    // 1. The first presented frame (polled from stderr, bounded).
    let t0 = Instant::now();
    loop {
        if stderr_now().contains("pin_app: latency trace: first frame presented") {
            break;
        }
        if let Some(st) = app.0.as_mut().and_then(|c| c.try_wait().ok().flatten()) {
            return Err(format!("pin_app exited before its first frame ({st})"));
        }
        if t0.elapsed() > FIRST_FRAME_TIMEOUT {
            return Err(format!("no frame presented within {FIRST_FRAME_TIMEOUT:?}"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    eprintln!(
        "windowed: first frame after {} ms",
        started.elapsed().as_millis()
    );

    // 2. The window: pointer over it, keyboard focus on it.
    let pid_s = pid.to_string();
    let mut window = None;
    let search_deadline = Instant::now() + Duration::from_secs(10);
    while window.is_none() && Instant::now() < search_deadline {
        if let Ok(out) = xdotool(display, &["search", "--pid", &pid_s]) {
            window = out
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .map(str::to_owned);
        }
        if window.is_none() {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    let window = window.ok_or("no X window found for pin_app (window focus)")?;
    for args in [
        &["mousemove", "--window", &window, "200", "200"][..],
        &["windowraise", &window],
        &["windowfocus", &window],
    ] {
        xdotool(display, args).map_err(|e| format!("window focus: {e}"))?;
    }

    // 3. The press plan until shortly before the run ends.
    let press_end = started + Duration::from_millis(cfg.run_ms) - PRESS_END_MARGIN;
    let span = press_end.saturating_duration_since(Instant::now());
    let plan = press_plan(cfg.seed, span.as_micros() as u64, cfg.frame_interval_ms);
    eprintln!(
        "windowed: plan of {} presses over {} ms (holds from {} ms)",
        planned_presses(&plan),
        span.as_millis(),
        base_hold_us(cfg.frame_interval_ms) / 1000
    );
    let mut shots = Vec::new();
    let mut injected = 0usize;
    let mut held = [false; 2];
    let plan_start = Instant::now();
    // A late step (a slow capture) shifts every later step by its delay,
    // so holds and gaps keep their planned lengths.
    let mut slip = Duration::ZERO;
    let send = |verb: &str, k: Key| xdotool(display, &[verb, k.keycode()]).map(drop);
    // Whatever happens in the loop (done, press_end, an error), every key
    // that may be down is released before `drive` returns.
    let injection = (|| -> Result<(), String> {
        for a in &plan {
            let due = plan_start + Duration::from_micros(a.at_us) + slip;
            let now = Instant::now();
            if now < due {
                std::thread::sleep(due - now);
            } else {
                slip += now - due;
            }
            match a.step {
                Step::Down(k) => {
                    if Instant::now() >= press_end {
                        break;
                    }
                    key_down(&mut held, k, |k| send("keydown", k))
                        .map_err(|e| format!("key injection: {e}"))?;
                    injected += 1;
                }
                Step::Up(k) => {
                    key_up(&mut held, k, |k| send("keyup", k))
                        .map_err(|e| format!("key injection: {e}"))?;
                }
                Step::Shot(name) => {
                    let p = cfg.out_dir.join(name);
                    match screenshot(display, &p) {
                        Ok(()) => shots.push(p),
                        Err(e) => eprintln!("windowed: {e}"),
                    }
                }
            }
        }
        Ok(())
    })();
    let release_start = Instant::now();
    let failed = release_held(
        &mut held,
        |k, t| xdotool_within(display, &["keyup", k.keycode()], t).map(drop),
        KEY_RELEASE_BUDGET,
        &mut || release_start.elapsed(),
        &mut || std::thread::sleep(KEY_RELEASE_PAUSE),
    );
    if failed.is_empty() {
        eprintln!("windowed: key release: every key up");
    }
    finish_injection(injection, &failed)?;
    eprintln!("windowed: injected {injected} presses");

    // 4. The app ends by itself (`--run-ms`); bounded.
    let deadline = started + Duration::from_millis(cfg.run_ms) + EXIT_MARGIN;
    loop {
        if let Some(st) = app.0.as_mut().and_then(|c| c.try_wait().ok().flatten()) {
            app.0 = None;
            return Ok(Driven {
                injected,
                shots,
                exit: st,
            });
        }
        if Instant::now() > deadline {
            return Err(format!(
                "pin_app still running {EXIT_MARGIN:?} after its run time (hang); killed"
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Longest wait for the app's stderr to reach its end after the app was
/// reaped (a process the app started may still hold the pipe).
pub const STDERR_DRAIN_TIMEOUT: Duration = HELPER_TIMEOUT;

/// Waits until `finished()` or `now()` (a monotonic time since a fixed
/// start) reaches `limit`, calling `pause` between polls. Returns whether it
/// finished (a seam so the bounded stderr drain runs under a controlled
/// clock in the headless tests, PROCESS.md R6).
pub fn wait_until(
    finished: &mut dyn FnMut() -> bool,
    limit: Duration,
    now: &mut dyn FnMut() -> Duration,
    pause: &mut dyn FnMut(),
) -> bool {
    loop {
        if finished() {
            return true;
        }
        if now() >= limit {
            return false;
        }
        pause();
    }
}

/// Reads `r` line by line into `log` on a thread (each line ends `\n`);
/// lines starting `pin_app: render on` are also echoed to stderr. Lines are
/// read as bytes and decoded lossily (a non-UTF-8 line from a launcher or a
/// native dependency never stops the capture); a trailing `\n` and a
/// preceding `\r` are stripped. EOF ends the capture silently; a read error
/// appends one `windowed: stderr read failed: <error>` line and ends it.
pub fn capture_lines(
    r: impl Read + Send + 'static,
    log: Arc<Mutex<String>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(r);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            let res = reader.read_until(b'\n', &mut buf);
            let eof = matches!(res, Ok(0));
            if !eof && !buf.is_empty() {
                if buf.last() == Some(&b'\n') {
                    buf.pop();
                    if buf.last() == Some(&b'\r') {
                        buf.pop();
                    }
                }
                let line = String::from_utf8_lossy(&buf);
                if line.starts_with("pin_app: render on") {
                    eprintln!("windowed: {line}");
                }
                let mut l = log
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                l.push_str(&line);
                l.push('\n');
            }
            match res {
                Ok(0) => return,
                Ok(_) => {}
                Err(e) => {
                    let mut l = log
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    l.push_str(&format!("windowed: stderr read failed: {e}\n"));
                    return;
                }
            }
        }
    })
}

/// The note added to the captured stderr when the drain timed out.
pub fn drain_timeout_note(limit: Duration) -> String {
    format!(
        "windowed: stderr drain timed out after {} s (a process still holds the app's stderr; \
         the log ends here)\n",
        limit.as_secs()
    )
}

/// Waits at most `limit` for the capture thread `reader` to reach the end
/// of the stream; on time it is joined (the log is complete), otherwise it
/// is left detached and [`drain_timeout_note`] is appended to `log`, which
/// keeps every line captured so far. Returns whether the drain completed.
pub fn finish_capture(
    reader: std::thread::JoinHandle<()>,
    log: &Mutex<String>,
    limit: Duration,
    now: &mut dyn FnMut() -> Duration,
    pause: &mut dyn FnMut(),
) -> bool {
    if wait_until(&mut || reader.is_finished(), limit, now, pause) {
        let _ = reader.join();
        return true;
    }
    log.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push_str(&drain_timeout_note(limit));
    false
}

/// The run's preflight: creates `cfg.out_dir`, removes the previous run's
/// artifacts ([`ARTIFACTS`], `frame_*.png`), then checks the tools (`have`),
/// the app binary and the grey-box bundle. A failed check writes a fresh
/// `summary.txt` ([`failure_summary`]), so a stale summary of an earlier run
/// never survives a run that could not start.
///
/// # Errors
/// The artifact directory cannot be created, or a check failed.
pub fn preflight(cfg: &DriverConfig, have: &dyn Fn(&str) -> bool) -> Result<(), String> {
    std::fs::create_dir_all(&cfg.out_dir).map_err(|e| format!("{}: {e}", cfg.out_dir.display()))?;
    clean_artifacts(&cfg.out_dir);
    let check = || -> Result<(), String> {
        for tool in ["Xvfb", "xdotool", "xwd", "convert", "nice"] {
            if !have(tool) {
                return Err(format!(
                    "{tool} not found (apt-get install xvfb xdotool x11-apps imagemagick)"
                ));
            }
        }
        if !cfg.app.is_file() {
            return Err(format!("no app binary at {}", cfg.app.display()));
        }
        if !cfg.bundle.join("table.toml").is_file() {
            return Err(format!(
                "no grey-box bundle at {} (run tables/greybox/build.sh {})",
                cfg.bundle.display(),
                cfg.bundle.display()
            ));
        }
        Ok(())
    };
    check().inspect_err(|what| {
        let _ = std::fs::write(cfg.out_dir.join("summary.txt"), failure_summary(what, ""));
    })
}

/// Records a failure after the preflight but before the app ran (Xvfb or the
/// app did not start): writes the failure `summary.txt`
/// (`verdict: FAIL (windowed driver: <what>)`) and returns `what`. No app
/// stderr log is written, as the app never started.
pub fn startup_failed(cfg: &DriverConfig, what: &str) -> String {
    let _ = std::fs::write(cfg.out_dir.join("summary.txt"), failure_summary(what, ""));
    what.to_string()
}

/// Starts the app under `display`, writing its trace to `trace_path`, with
/// stderr piped.
///
/// # Errors
/// `starting <app>: <io error>` when the process cannot be spawned.
pub fn spawn_app(
    cfg: &DriverConfig,
    display: &str,
    trace_path: &Path,
) -> Result<std::process::Child, String> {
    Command::new(&cfg.app)
        .arg("--table")
        .arg(&cfg.bundle)
        .arg("--latency-trace")
        .arg(trace_path)
        .args(["--run-ms", &cfg.run_ms.to_string()])
        .env("DISPLAY", display)
        .env("LP_NUM_THREADS", &cfg.lp_num_threads)
        .env("WGPU_BACKEND", "vulkan")
        .env(FORCE_FALLBACK_ENV, "1")
        .env("SDL_VIDEODRIVER", "x11")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("starting {}: {e}", cfg.app.display()))
}

/// Runs the app once (see the module docs).
///
/// # Errors
/// A missing tool, bundle or binary, no first frame, no window, a failed
/// key injection, an app that did not exit in time (killed), or too few
/// screenshots. Every error has written a failure `summary.txt` (the
/// preflight's after removing the previous run's artifacts, or
/// [`startup_failed`] when Xvfb or the app did not start); every error after
/// the app started has also written `pin_app_stderr.log` and carries the
/// stderr tail.
pub fn run(cfg: &DriverConfig) -> Result<RunOutput, String> {
    preflight(cfg, &have)?;
    let (display, _xvfb) = match std::env::var("DISPLAY").ok().filter(|d| !d.is_empty()) {
        Some(d) => (d, Reaper(None)),
        None => start_xvfb().map_err(|e| startup_failed(cfg, &e))?,
    };
    eprintln!(
        "windowed: display {display}, LP_NUM_THREADS={}, app {}, run {} ms",
        cfg.lp_num_threads,
        cfg.app.display(),
        cfg.run_ms
    );
    let trace_path = cfg.trace_path();
    let started = Instant::now();
    let mut child = spawn_app(cfg, &display, &trace_path).map_err(|e| startup_failed(cfg, &e))?;
    let pid = child.id();
    let stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| startup_failed(cfg, "no stderr pipe"))?;
    let mut app = Reaper(Some(child));
    let log = Arc::new(Mutex::new(String::new()));
    let reader = capture_lines(stderr_pipe, Arc::clone(&log));
    let stderr_now = || {
        log.lock()
            .map(|l| l.clone())
            .unwrap_or_else(|p| p.into_inner().clone())
    };
    let driven = drive(cfg, &display, pid, started, &mut app, &stderr_now);
    // Every path: the app is gone (killed if still running), the stderr
    // capture is complete or bounded (a process the app started may hold
    // the pipe open: after STDERR_DRAIN_TIMEOUT the reader is left
    // detached and the log keeps what it captured, with a note), and the
    // log is written fresh.
    if let Some(c) = app.0.as_mut() {
        let _ = c.kill();
        let _ = c.wait();
    }
    app.0 = None;
    let drain_start = Instant::now();
    finish_capture(
        reader,
        &log,
        STDERR_DRAIN_TIMEOUT,
        &mut || drain_start.elapsed(),
        &mut || std::thread::sleep(Duration::from_millis(5)),
    );
    let stderr = stderr_now();
    let _ = std::fs::write(cfg.out_dir.join("pin_app_stderr.log"), &stderr);
    let failed = |what: String| -> String {
        let _ = std::fs::write(
            cfg.out_dir.join("summary.txt"),
            failure_summary(&what, &stderr),
        );
        format!(
            "{what}\n--- pin_app stderr (tail) ---\n{}",
            tail(&stderr, 40)
        )
    };
    let d = driven.map_err(&failed)?;
    let trace = std::fs::read_to_string(&trace_path)
        .map_err(|e| failed(format!("reading {}: {e}", trace_path.display())))?;
    if let Some(why) = missing_captures(&d.shots) {
        return Err(failed(why));
    }
    Ok(RunOutput {
        stderr,
        trace,
        injected_presses: d.injected,
        exit: d.exit,
        screenshots: d.shots,
    })
}
