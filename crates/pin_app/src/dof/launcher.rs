//! Spawning and stopping the bridge process.

use super::map::{DofError, ResolvedDofMap};
use pin_core::schema::table::TableManifest;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// File name of the bridge executable.
pub const BRIDGE_EXE_NAME: &str = if cfg!(windows) {
    "KilohertzDofBridge.exe"
} else {
    "KilohertzDofBridge"
};

/// How long [`DofBridge::stop`] lets the bridge shut down cleanly (DOF
/// `Finish` turns the outputs off) before killing it.
pub const DEFAULT_STOP_GRACE: Duration = Duration::from_millis(1500);

/// How often the reaper thread polls the stopping bridge.
const REAP_POLL: Duration = Duration::from_millis(10);

/// Whether the engine starts the bridge on this OS (Windows only; DOF is a
/// Windows library).
pub fn platform_supported() -> bool {
    cfg!(windows)
}

/// How to launch the bridge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DofLaunchConfig {
    /// The bridge executable.
    pub bridge_exe: PathBuf,
    /// UDP port the engine's telemetry is sent to.
    pub udp_port: u16,
    /// DOF install directory or `DirectOutput.dll` (`--dof-path`); `None`
    /// lets the bridge search the default locations.
    pub dof_path: Option<PathBuf>,
    /// Directory for the resolved map file (default: the OS temp dir).
    pub map_dir: PathBuf,
    /// Grace period before a stopping bridge is killed.
    pub stop_grace: Duration,
    /// File the bridge's stderr is appended to (`None`: discarded).
    pub log_file: Option<PathBuf>,
    /// Extra arguments appended after the standard ones.
    pub extra_args: Vec<OsString>,
}

impl DofLaunchConfig {
    /// A config with the temp dir for the map file, [`DEFAULT_STOP_GRACE`],
    /// no DOF path, no log file and no extra arguments.
    pub fn new(bridge_exe: impl Into<PathBuf>, udp_port: u16) -> Self {
        Self {
            bridge_exe: bridge_exe.into(),
            udp_port,
            dof_path: None,
            map_dir: std::env::temp_dir(),
            stop_grace: DEFAULT_STOP_GRACE,
            log_file: None,
            extra_args: Vec::new(),
        }
    }

    /// The installed bridge next to the running engine:
    /// `<exe dir>/dof_bridge/KilohertzDofBridge.exe`, then
    /// `<exe dir>/KilohertzDofBridge.exe`; `None` when neither exists.
    pub fn default_bridge_exe() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        Self::find_bridge_exe(exe.parent()?)
    }

    /// [`default_bridge_exe`](Self::default_bridge_exe) for an explicit
    /// engine directory.
    pub fn find_bridge_exe(engine_dir: &Path) -> Option<PathBuf> {
        [
            engine_dir.join("dof_bridge").join(BRIDGE_EXE_NAME),
            engine_dir.join(BRIDGE_EXE_NAME),
        ]
        .into_iter()
        .find(|p| p.is_file())
    }

    /// The bridge's command-line arguments: `--map <file> --port <p> --rom
    /// <rom> --table <name> --parent-pid <engine pid> --exit-on-stdin-eof
    /// [--dof-path <dir>] <extra_args…>`.
    pub fn args(&self, map: &ResolvedDofMap, map_file: &Path) -> Vec<OsString> {
        let mut a: Vec<OsString> = vec![
            "--map".into(),
            map_file.into(),
            "--port".into(),
            self.udp_port.to_string().into(),
            "--rom".into(),
            map.rom_name.clone().into(),
            "--table".into(),
            map.table_name.clone().into(),
            "--parent-pid".into(),
            std::process::id().to_string().into(),
            "--exit-on-stdin-eof".into(),
        ];
        if let Some(p) = &self.dof_path {
            a.push("--dof-path".into());
            a.push(p.into());
        }
        a.extend(self.extra_args.iter().cloned());
        a
    }
}

/// A running bridge process. Dropping it stops it like [`DofBridge::stop`].
///
/// Each bridge gets its reaper thread when it is launched; the thread stays
/// parked until the stop is requested, so stopping (or dropping) never has
/// to create a thread and never blocks the caller.
#[derive(Debug)]
pub struct DofBridge {
    /// The child and its map file, shared with the reaper thread, which
    /// takes them once the stop is requested.
    slot: ReaperSlot,
    id: u32,
    map_file: PathBuf,
    /// Dropping it wakes the reaper thread (the stop request).
    stop_tx: Option<mpsc::Sender<()>>,
    reaper: Option<JoinHandle<Option<ExitStatus>>>,
}

/// The stop in progress of a [`DofBridge`]; dropping it detaches (the reaper
/// thread still finishes the stop).
#[derive(Debug)]
pub struct DofStopHandle {
    join: Option<JoinHandle<Option<ExitStatus>>>,
}

impl DofStopHandle {
    /// Waits for the bridge to have exited (at most about the grace period
    /// plus the kill); returns its exit status, `None` if it could not be
    /// determined.
    pub fn wait(mut self) -> Option<ExitStatus> {
        self.join.take().and_then(|j| j.join().ok().flatten())
    }

    /// Whether the stop has completed.
    pub fn is_finished(&self) -> bool {
        self.join.as_ref().is_none_or(JoinHandle::is_finished)
    }
}

impl DofBridge {
    /// The call 6.0 makes at table load: resolves `[dof]`, writes the map file
    /// and spawns the bridge. `Ok(None)` when the table has no `[dof]` or the
    /// OS is not Windows ([`platform_supported`]).
    pub fn start_for_table(
        table: &TableManifest,
        config: &DofLaunchConfig,
    ) -> Result<Option<Self>, DofError> {
        if !platform_supported() {
            return Ok(None);
        }
        match ResolvedDofMap::resolve(table)? {
            Some(map) => Self::spawn(&map, config).map(Some),
            None => Ok(None),
        }
    }

    /// Writes the map file and spawns `config.bridge_exe` on any OS (no
    /// platform check; [`start_for_table`](Self::start_for_table) adds it).
    /// stdin is a pipe the bridge watches (closing it is the stop signal),
    /// stdout is discarded, stderr goes to `config.log_file` or is discarded.
    /// On Windows the bridge gets no console window. Fails (without starting
    /// the bridge) if the bridge's reaper thread cannot be started.
    pub fn spawn(map: &ResolvedDofMap, config: &DofLaunchConfig) -> Result<Self, DofError> {
        let map_file = map.write_file(&config.map_dir)?;
        let stderr = match &config.log_file {
            Some(p) => match std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
            {
                Ok(f) => Stdio::from(f),
                Err(e) => {
                    let _ = std::fs::remove_file(&map_file);
                    return Err(DofError::Spawn(e));
                }
            },
            None => Stdio::null(),
        };
        let mut cmd = Command::new(&config.bridge_exe);
        cmd.args(config.args(map, &map_file))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(stderr);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            /// `CREATE_NO_WINDOW`: no console window for the bridge.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        Self::launch_with(cmd, map_file, config.stop_grace, |f| {
            std::thread::Builder::new()
                .name("dof-bridge-reaper".into())
                .spawn(f)
        })
    }

    /// Starts the reaper thread with `spawn_reaper` (tests inject a failing
    /// one), then the bridge from `cmd`. On any failure nothing is left
    /// running and `map_file` is deleted.
    fn launch_with(
        mut cmd: Command,
        map_file: PathBuf,
        grace: Duration,
        spawn_reaper: impl FnOnce(ReaperJob) -> std::io::Result<JoinHandle<Option<ExitStatus>>>,
    ) -> Result<Self, DofError> {
        let slot: ReaperSlot = Arc::new(Mutex::new(None));
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let job_slot = Arc::clone(&slot);
        let job: ReaperJob = Box::new(move || {
            // Parked until the bridge handle asks for the stop (drops the
            // sender); `Err` is that request.
            let _ = stop_rx.recv();
            let taken = lock(&job_slot).take();
            taken.and_then(|(mut child, map_file)| stop_child(&mut child, &map_file, grace))
        });
        // The reaper thread first: if it cannot be started, no bridge has
        // been started either, so nothing is left to clean up but the map.
        let reaper = match spawn_reaper(job) {
            Ok(j) => j,
            Err(e) => {
                let _ = std::fs::remove_file(&map_file);
                return Err(DofError::Spawn(e));
            }
        };
        match cmd.spawn() {
            Ok(child) => {
                let id = child.id();
                *lock(&slot) = Some((child, map_file.clone()));
                Ok(Self {
                    slot,
                    id,
                    map_file,
                    stop_tx: Some(stop_tx),
                    reaper: Some(reaper),
                })
            }
            Err(e) => {
                // The reaper wakes on the dropped sender, finds no child and
                // exits.
                drop(stop_tx);
                let _ = std::fs::remove_file(&map_file);
                Err(DofError::Spawn(e))
            }
        }
    }

    /// The bridge's process id.
    pub fn id(&self) -> u32 {
        self.id
    }

    /// The map file passed to the bridge (deleted once the bridge stopped).
    pub fn map_file(&self) -> &Path {
        &self.map_file
    }

    /// `Some(status)` once the bridge has exited (e.g. DOF not installed:
    /// exit code 4), without blocking.
    pub fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        match lock(&self.slot).as_mut() {
            Some((c, _)) => c.try_wait(),
            None => Ok(None),
        }
    }

    /// Stops the bridge without blocking: closes its stdin (it then calls
    /// DOF `Finish` and exits) and wakes its reaper thread, which kills it
    /// if it is still running after the grace period, then deletes the map
    /// file.
    pub fn stop(mut self) -> DofStopHandle {
        self.begin_stop()
    }

    fn begin_stop(&mut self) -> DofStopHandle {
        if let Some((child, _)) = lock(&self.slot).as_mut() {
            drop(child.stdin.take());
        }
        drop(self.stop_tx.take());
        DofStopHandle {
            join: self.reaper.take(),
        }
    }
}

/// The child and its map file, until the reaper thread takes them.
type ReaperSlot = Arc<Mutex<Option<(Child, PathBuf)>>>;

/// The reaper thread's work, handed to the thread spawner.
type ReaperJob = Box<dyn FnOnce() -> Option<ExitStatus> + Send + 'static>;

/// Locks `slot`, ignoring poisoning (the data stays valid).
fn lock(slot: &ReaperSlot) -> std::sync::MutexGuard<'_, Option<(Child, PathBuf)>> {
    slot.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Reaps `child` (see [`reap`]), then deletes its map file.
fn stop_child(child: &mut Child, map_file: &Path, grace: Duration) -> Option<ExitStatus> {
    let status = reap(child, grace);
    let _ = std::fs::remove_file(map_file);
    status
}

/// Waits up to `grace` for `child` to exit, then kills it.
fn reap(child: &mut Child, grace: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + grace;
    loop {
        match child.try_wait() {
            Ok(Some(s)) => return Some(s),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(REAP_POLL),
            Ok(None) => break,
            Err(_) => break,
        }
    }
    let _ = child.kill();
    child.wait().ok()
}

impl Drop for DofBridge {
    fn drop(&mut self) {
        let _ = self.begin_stop();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A fresh empty map file in the temp dir.
    fn scratch_map(tag: &str) -> PathBuf {
        let map_file = std::env::temp_dir().join(format!(
            "khz-dof-{tag}-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        // `create_new`: never write through a pre-existing path.
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&map_file)
            .unwrap();
        map_file
    }

    /// A bridge that ignores stdin EOF and SIGTERM.
    fn stubborn() -> Command {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "trap '' TERM; exec sleep 600"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cmd
    }

    fn real_spawner(f: ReaperJob) -> std::io::Result<JoinHandle<Option<ExitStatus>>> {
        std::thread::Builder::new().spawn(f)
    }

    /// Whether process `pid` exists (signal 0 probes existence).
    fn alive(pid: u32) -> bool {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    }

    /// Thread creation fails at launch: the launch fails, no bridge is
    /// started (so none can be left running) and the map file is deleted.
    #[test]
    fn launch_without_a_reaper_thread_fails_and_starts_nothing() {
        let map_file = scratch_map("reaper-fail");
        let marker = map_file.with_extension("started");
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg(format!("touch '{}'; exec sleep 600", marker.display()))
            .stdin(Stdio::piped());
        let r = DofBridge::launch_with(cmd, map_file.clone(), Duration::from_secs(8), |_job| {
            Err(std::io::Error::other("no threads"))
        });
        assert!(matches!(r, Err(DofError::Spawn(_))), "{r:?}");
        assert!(!map_file.exists());
        std::thread::sleep(Duration::from_millis(200));
        assert!(!marker.exists(), "the bridge was started");
    }

    /// stop() and drop() return at once even for a bridge that ignores stdin
    /// EOF (the whole grace period runs on the parked reaper thread), and
    /// the bridge is still killed, reaped and its map file deleted.
    #[test]
    fn stop_and_drop_never_block_the_caller() {
        let grace = Duration::from_secs(2);
        let map_file = scratch_map("stop-prompt");
        let bridge = DofBridge::launch_with(stubborn(), map_file.clone(), grace, real_spawner)
            .expect("launch");
        let pid = bridge.id();
        let t = Instant::now();
        let handle = bridge.stop();
        let took = t.elapsed();
        assert!(took < Duration::from_millis(500), "stop() blocked {took:?}");
        assert!(
            !handle.is_finished(),
            "stop finished before the grace period"
        );
        let status = handle.wait().expect("child reaped");
        assert!(t.elapsed() >= grace);
        assert!(!status.success());
        assert!(!map_file.exists());
        assert!(!alive(pid));

        let map_file = scratch_map("drop-prompt");
        let bridge = DofBridge::launch_with(stubborn(), map_file.clone(), grace, real_spawner)
            .expect("launch");
        let pid = bridge.id();
        let t = Instant::now();
        drop(bridge);
        let took = t.elapsed();
        assert!(took < Duration::from_millis(500), "drop blocked {took:?}");
        let deadline = Instant::now() + grace + Duration::from_secs(10);
        while (map_file.exists() || alive(pid)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!map_file.exists());
        assert!(!alive(pid));
    }
}
