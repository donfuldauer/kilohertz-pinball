//! Spawning and stopping the bridge process.

use super::map::{DofError, ResolvedDofMap};
use pin_core::schema::table::TableManifest;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
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
#[derive(Debug)]
pub struct DofBridge {
    child: Option<Child>,
    id: u32,
    map_file: PathBuf,
    grace: Duration,
}

/// The stop in progress of a [`DofBridge`]; dropping it detaches (the reaper
/// thread still finishes the stop).
#[derive(Debug)]
pub struct DofStopHandle {
    join: Option<JoinHandle<Option<ExitStatus>>>,
    /// Exit status of a stop completed on the caller's thread (no reaper
    /// thread could be started).
    status: Option<ExitStatus>,
}

impl DofStopHandle {
    /// Waits for the bridge to have exited (at most about the grace period
    /// plus the kill); returns its exit status, `None` if it could not be
    /// determined.
    pub fn wait(mut self) -> Option<ExitStatus> {
        match self.join.take() {
            Some(j) => j.join().ok().flatten(),
            None => self.status,
        }
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
    /// On Windows the bridge gets no console window.
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
        match cmd.spawn() {
            Ok(child) => Ok(Self {
                id: child.id(),
                child: Some(child),
                map_file,
                grace: config.stop_grace,
            }),
            Err(e) => {
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
        match &mut self.child {
            Some(c) => c.try_wait(),
            None => Ok(None),
        }
    }

    /// Stops the bridge without blocking: closes its stdin (it then calls
    /// DOF `Finish` and exits) and hands it to a reaper thread that kills it
    /// if it is still running after the grace period, then deletes the map
    /// file.
    pub fn stop(mut self) -> DofStopHandle {
        self.begin_stop()
    }

    fn begin_stop(&mut self) -> DofStopHandle {
        self.begin_stop_with(|f| {
            std::thread::Builder::new()
                .name("dof-bridge-reaper".into())
                .spawn(f)
        })
    }

    /// [`begin_stop`](Self::begin_stop) with the reaper thread started by
    /// `spawn` (tests inject a failing one).
    fn begin_stop_with(
        &mut self,
        spawn: impl FnOnce(ReaperJob) -> std::io::Result<JoinHandle<Option<ExitStatus>>>,
    ) -> DofStopHandle {
        let Some(mut child) = self.child.take() else {
            return DofStopHandle {
                join: None,
                status: None,
            };
        };
        drop(child.stdin.take());
        let map_file = std::mem::take(&mut self.map_file);
        let grace = self.grace;
        // The child and map file stay reachable from here until the reaper
        // has taken them: if no thread can be started, the closure is
        // dropped unrun and the stop is completed below.
        let slot = Arc::new(Mutex::new(Some((child, map_file))));
        let job_slot = Arc::clone(&slot);
        let job: ReaperJob = Box::new(move || {
            let taken = job_slot.lock().ok().and_then(|mut s| s.take());
            taken.and_then(|(mut child, map_file)| stop_child(&mut child, &map_file, grace))
        });
        match spawn(job) {
            Ok(j) => DofStopHandle {
                join: Some(j),
                status: None,
            },
            // No thread could be created: finish the stop on this thread
            // (blocks for at most the grace period plus the kill) rather than
            // drop an unreaped child and leave the map file behind.
            Err(_) => {
                let taken = slot.lock().ok().and_then(|mut s| s.take());
                let status = taken
                    .and_then(|(mut child, map_file)| stop_child(&mut child, &map_file, grace));
                DofStopHandle { join: None, status }
            }
        }
    }
}

/// The reaper thread's work, handed to the thread spawner.
type ReaperJob = Box<dyn FnOnce() -> Option<ExitStatus> + Send + 'static>;

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

    /// Thread creation fails: the stop still kills and reaps the bridge
    /// (one that ignores stdin EOF and SIGTERM) and deletes the map file.
    #[test]
    fn stop_without_a_reaper_thread_still_reaps_and_cleans_up() {
        let map_file = std::env::temp_dir().join(format!(
            "khz-dof-reaper-fallback-{}-{}.json",
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
        let child = Command::new("/bin/sh")
            .args(["-c", "trap '' TERM; exec sleep 600"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let mut bridge = DofBridge {
            id: pid,
            child: Some(child),
            map_file: map_file.clone(),
            grace: Duration::from_millis(20),
        };
        let handle = bridge.begin_stop_with(|_job| Err(std::io::Error::other("no threads")));
        assert!(handle.is_finished());
        let status = handle.wait().expect("child reaped");
        assert!(!status.success());
        assert!(!map_file.exists());
        // Reaped: the pid is gone (signal 0 probes existence).
        let alive = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(!alive.success());
        drop(bridge);
    }
}
