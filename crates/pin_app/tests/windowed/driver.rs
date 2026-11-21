//! The windowed driver (KP-002): starts the release `pin_app` with the
//! grey-box table under Xvfb on lavapipe, waits for the first presented
//! frame, injects flipper presses through XTest (`xdotool keydown/keyup`
//! with the X keycodes of Shift_L and Shift_R), takes a few screenshots of
//! the root window, and returns the app's stderr and its latency trace.
//! Every wait is bounded; a hung app is killed and reported.
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
//! * `LP_NUM_THREADS`: lavapipe's worker threads (default
//!   [`DEFAULT_LP_NUM_THREADS`]);
//! * `DISPLAY`: an X server to use; without it the driver starts its own
//!   Xvfb (1280x720x24).

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
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

/// What to run.
#[derive(Clone, Debug)]
pub struct DriverConfig {
    pub app: PathBuf,
    pub bundle: PathBuf,
    pub out_dir: PathBuf,
    pub run_ms: u64,
    pub lp_num_threads: String,
    pub seed: u64,
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

    /// Left (`true`) or right.
    pub fn left(&mut self) -> bool {
        self.next_u64() >> 63 == 0
    }
}

/// Kills and reaps a child on drop.
struct Reaper(Option<Child>);

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

/// Our own Xvfb on a free display number; returns the display name.
fn start_xvfb() -> Result<(String, Reaper), String> {
    for n in 90..200u32 {
        let sock = PathBuf::from(format!("/tmp/.X11-unix/X{n}"));
        let lock = PathBuf::from(format!("/tmp/.X{n}-lock"));
        if sock.exists() || lock.exists() {
            continue;
        }
        // Niced (`nice` execs Xvfb, so the pid is Xvfb's): the display server
        // yields to the app under test on a busy runner.
        let child = Command::new("nice")
            .args([
                "-n",
                "10",
                "Xvfb",
                &format!(":{n}"),
                "-screen",
                "0",
                "1280x720x24",
                "-nolisten",
                "tcp",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("starting Xvfb: {e}"))?;
        let mut reaper = Reaper(Some(child));
        for _ in 0..100 {
            if sock.exists() {
                return Ok((format!(":{n}"), reaper));
            }
            if let Some(c) = reaper.0.as_mut()
                && let Ok(Some(_)) = c.try_wait()
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        drop(reaper);
    }
    Err("no Xvfb display could be started".into())
}

fn xdotool(display: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new("xdotool")
        .args(args)
        .env("DISPLAY", display)
        .output()
        .map_err(|e| format!("xdotool {args:?}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!(
            "xdotool {args:?}: {} {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

fn screenshot(display: &str, path: &Path) -> Result<(), String> {
    let cmd = format!(
        "xwd -root -silent -display '{display}' | convert xwd:- png:'{}'",
        path.display()
    );
    // Niced and single-threaded, so a capture does not take the sim's core.
    let st = Command::new("nice")
        .args(["-n", "10", "sh", "-c", &cmd])
        .env("MAGICK_THREAD_LIMIT", "1")
        .status()
        .map_err(|e| format!("screenshot: {e}"))?;
    if st.success() && path.is_file() {
        Ok(())
    } else {
        Err(format!("screenshot {} failed ({st})", path.display()))
    }
}

/// The last `n` lines of `text`.
pub fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// Runs the app once (see the module docs).
///
/// # Errors
/// A missing tool, bundle or binary, no first frame, no window, a failed
/// key injection, or an app that did not exit in time (killed; the error
/// carries the stderr tail).
pub fn run(cfg: &DriverConfig) -> Result<RunOutput, String> {
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
    std::fs::create_dir_all(&cfg.out_dir).map_err(|e| format!("{}: {e}", cfg.out_dir.display()))?;
    let (display, _xvfb) = match std::env::var("DISPLAY").ok().filter(|d| !d.is_empty()) {
        Some(d) => (d, Reaper(None)),
        None => start_xvfb()?,
    };
    eprintln!(
        "windowed: display {display}, LP_NUM_THREADS={}, app {}, run {} ms",
        cfg.lp_num_threads,
        cfg.app.display(),
        cfg.run_ms
    );
    let trace_path = cfg.trace_path();
    let _ = std::fs::remove_file(&trace_path);
    let started = Instant::now();
    let mut child = Command::new(&cfg.app)
        .arg("--table")
        .arg(&cfg.bundle)
        .arg("--latency-trace")
        .arg(&trace_path)
        .args(["--run-ms", &cfg.run_ms.to_string()])
        .env("DISPLAY", &display)
        .env("LP_NUM_THREADS", &cfg.lp_num_threads)
        .env("WGPU_BACKEND", "vulkan")
        .env("SDL_VIDEODRIVER", "x11")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("starting {}: {e}", cfg.app.display()))?;
    let pid = child.id();
    let stderr_pipe = child.stderr.take().ok_or("no stderr pipe")?;
    let mut app = Reaper(Some(child));
    let log = Arc::new(Mutex::new(String::new()));
    let reader = {
        let log = Arc::clone(&log);
        std::thread::spawn(move || {
            for line in BufReader::new(stderr_pipe).lines().map_while(Result::ok) {
                if line.starts_with("pin_app: render on") {
                    eprintln!("windowed: {line}");
                }
                let mut l = log
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                l.push_str(&line);
                l.push('\n');
            }
        })
    };
    let stderr_now = || log.lock().map(|l| l.clone()).unwrap_or_default();
    let fail = |msg: String, app: &mut Reaper| -> String {
        if let Some(c) = app.0.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
        app.0 = None;
        format!(
            "{msg}\n--- pin_app stderr (tail) ---\n{}",
            tail(&stderr_now(), 40)
        )
    };

    // 1. The first presented frame (polled from stderr, bounded).
    let t0 = Instant::now();
    loop {
        if stderr_now().contains("pin_app: latency trace: first frame presented") {
            break;
        }
        let exited = app.0.as_mut().and_then(|c| c.try_wait().ok().flatten());
        if let Some(st) = exited {
            return Err(fail(
                format!("pin_app exited before its first frame ({st})"),
                &mut app,
            ));
        }
        if t0.elapsed() > FIRST_FRAME_TIMEOUT {
            return Err(fail(
                format!("no frame presented within {FIRST_FRAME_TIMEOUT:?}"),
                &mut app,
            ));
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
    for _ in 0..100 {
        if let Ok(out) = xdotool(&display, &["search", "--pid", &pid_s]) {
            window = out
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .map(str::to_owned);
            if window.is_some() {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let Some(window) = window else {
        return Err(fail("no X window found for pin_app".into(), &mut app));
    };
    for args in [
        &["mousemove", "--window", &window, "200", "200"][..],
        &["windowraise", &window],
        &["windowfocus", &window],
    ] {
        if let Err(e) = xdotool(&display, args) {
            return Err(fail(e, &mut app));
        }
    }

    // 3. Presses until shortly before the run ends, with screenshots.
    let press_end = started + Duration::from_millis(cfg.run_ms) - PRESS_END_MARGIN;
    let mut rng = Schedule::new(cfg.seed);
    let mut shots = Vec::new();
    let shot = |name: &str, shots: &mut Vec<PathBuf>| {
        let p = cfg.out_dir.join(name);
        match screenshot(&display, &p) {
            Ok(()) => shots.push(p),
            Err(e) => eprintln!("windowed: {e}"),
        }
    };
    shot("frame_0_rest.png", &mut shots);
    let mut injected = 0usize;
    while Instant::now() + Duration::from_millis(1000) < press_end {
        let left = rng.left();
        // Two long holds with a screenshot each: left at press 10, right at 20.
        let held_shot = match injected {
            10 => Some((true, "frame_1_left_held.png")),
            20 => Some((false, "frame_2_right_held.png")),
            _ => None,
        };
        let left = held_shot.map_or(left, |(l, _)| l);
        let key = if left { KEY_LEFT } else { KEY_RIGHT };
        let hold = rng.duration(30, 200);
        let gap = rng.duration(100, 350);
        if let Err(e) = xdotool(&display, &["keydown", key]) {
            return Err(fail(e, &mut app));
        }
        injected += 1;
        match held_shot {
            Some((_, name)) => {
                // Let a few frames show the raised bat, then capture.
                std::thread::sleep(Duration::from_millis(800));
                shot(name, &mut shots);
            }
            None => std::thread::sleep(hold),
        }
        if let Err(e) = xdotool(&display, &["keyup", key]) {
            return Err(fail(e, &mut app));
        }
        std::thread::sleep(gap);
        if injected == 30 {
            std::thread::sleep(Duration::from_millis(800));
            shot("frame_3_rest.png", &mut shots);
        }
    }
    eprintln!("windowed: injected {injected} presses");

    // 4. The app ends by itself (`--run-ms`); bounded.
    let deadline = started + Duration::from_millis(cfg.run_ms) + EXIT_MARGIN;
    let exit = loop {
        let st = app.0.as_mut().and_then(|c| c.try_wait().ok().flatten());
        if let Some(st) = st {
            app.0 = None;
            break st;
        }
        if Instant::now() > deadline {
            return Err(fail(
                format!(
                    "pin_app still running {:?} after its run time; killed",
                    EXIT_MARGIN
                ),
                &mut app,
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let _ = reader.join();
    let stderr = stderr_now();
    let _ = std::fs::write(cfg.out_dir.join("pin_app_stderr.log"), &stderr);
    let trace = std::fs::read_to_string(&trace_path).map_err(|e| {
        format!(
            "reading {}: {e}\n--- pin_app stderr (tail) ---\n{}",
            trace_path.display(),
            tail(&stderr, 40)
        )
    })?;
    Ok(RunOutput {
        stderr,
        trace,
        injected_presses: injected,
        exit,
        screenshots: shots,
    })
}
