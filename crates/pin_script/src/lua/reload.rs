//! Hot reload, I/O-worker side (§4 Subphase 4.2 deliverable 3).
//!
//! [`ReloadWatcher`] watches the script's directory with `notify` (so editors
//! that save by rename are seen), reads the file after a change, syntax-checks
//! it with [`syntax_check`] and returns the new source from
//! [`ReloadWatcher::poll`]. The sim thread then calls
//! `TableScriptBackend::apply_reload` at a tick boundary. File I/O happens
//! only here, never on the sim thread.

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use pin_core::ScriptError;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::Duration;

/// Compiles `src` (text mode only; bytecode is rejected) without running it.
pub fn syntax_check(src: &[u8]) -> Result<(), ScriptError> {
    super::sandbox::syntax_check(src)
}

/// Watches one script file. See the module docs.
pub struct ReloadWatcher {
    // Dropping the watcher stops the notify thread.
    _watcher: RecommendedWatcher,
    rx: Receiver<notify::Result<Event>>,
    path: PathBuf,
    file_name: OsString,
    dirty: bool,
    last: Option<Vec<u8>>,
    last_error: Option<ScriptError>,
}

impl std::fmt::Debug for ReloadWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReloadWatcher")
            .field("path", &self.path)
            .field("dirty", &self.dirty)
            .field("last_error", &self.last_error)
            .finish_non_exhaustive()
    }
}

fn io_err(path: &Path, e: impl std::fmt::Display) -> ScriptError {
    ScriptError::Load(format!("{}: {e}", path.display()))
}

impl ReloadWatcher {
    /// Starts watching `path` (its directory must exist). The current
    /// contents, if any, count as already delivered.
    pub fn new(path: impl AsRef<Path>) -> Result<Self, ScriptError> {
        let path = path.as_ref().to_path_buf();
        let file_name = path
            .file_name()
            .ok_or_else(|| io_err(&path, "not a file path"))?
            .to_os_string();
        let dir = match path.parent() {
            Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
            _ => PathBuf::from("."),
        };
        let (tx, rx) = channel();
        let mut watcher = notify::recommended_watcher(tx).map_err(|e| io_err(&path, e))?;
        watcher
            .watch(&dir, RecursiveMode::NonRecursive)
            .map_err(|e| io_err(&dir, e))?;
        let last = std::fs::read(&path).ok();
        Ok(Self {
            _watcher: watcher,
            rx,
            path,
            file_name,
            dirty: false,
            last,
            last_error: None,
        })
    }

    /// The watched file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The error from the most recent rejected change (syntax error or
    /// watcher error), cleared when a change is accepted.
    pub fn last_error(&self) -> Option<&ScriptError> {
        self.last_error.as_ref()
    }

    fn absorb(&mut self, ev: notify::Result<Event>) {
        match ev {
            Ok(ev) => {
                if ev
                    .paths
                    .iter()
                    .any(|p| p.file_name() == Some(self.file_name.as_os_str()))
                {
                    self.dirty = true;
                }
            }
            Err(e) => self.last_error = Some(io_err(&self.path, e)),
        }
    }

    /// Non-blocking: drains pending file events and, if the file changed to
    /// new contents that compile, returns them. Unchanged contents, an
    /// unreadable file (retried on the next poll) and syntax errors (kept in
    /// [`Self::last_error`]) return `None`.
    pub fn poll(&mut self) -> Option<Vec<u8>> {
        while let Ok(ev) = self.rx.try_recv() {
            self.absorb(ev);
        }
        self.take_change()
    }

    /// Like [`Self::poll`], but blocks for up to `timeout` until a change is
    /// accepted (for an I/O worker loop, or tests that must not sleep).
    pub fn wait(&mut self, timeout: Duration) -> Option<Vec<u8>> {
        if let Some(src) = self.poll() {
            return Some(src);
        }
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match self.rx.recv_timeout(left) {
                Ok(ev) => {
                    self.absorb(ev);
                    if let Some(src) = self.poll() {
                        return Some(src);
                    }
                }
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                    return self.poll();
                }
            }
        }
    }

    /// Re-reads the file now, as if a change event had arrived.
    pub fn force_check(&mut self) -> Option<Vec<u8>> {
        self.dirty = true;
        self.poll()
    }

    fn take_change(&mut self) -> Option<Vec<u8>> {
        if !self.dirty {
            return None;
        }
        let src = std::fs::read(&self.path).ok()?;
        self.dirty = false;
        if self.last.as_deref() == Some(src.as_slice()) {
            return None;
        }
        match syntax_check(&src) {
            Ok(()) => {
                self.last = Some(src.clone());
                self.last_error = None;
                Some(src)
            }
            Err(e) => {
                // Forget the last good text so reverting to it is delivered.
                self.last = None;
                self.last_error = Some(e);
                None
            }
        }
    }
}
