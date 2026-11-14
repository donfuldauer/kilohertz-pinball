//! A temporary directory that this test created exclusively and that is
//! removed when the guard drops (also when the test panics).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// See the module docs.
pub struct TempDir(PathBuf);

impl TempDir {
    /// Creates a fresh directory under the system temp dir. `create_dir`
    /// (not `create_dir_all`) fails if the path exists, so the directory is
    /// never one left behind by another run or owned by someone else.
    pub fn new(tag: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let base = std::env::temp_dir();
        for _ in 0..1000 {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.subsec_nanos());
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = base.join(format!(
                "pin_script_{tag}_{}_{nanos:08x}_{n}",
                std::process::id()
            ));
            match std::fs::create_dir(&dir) {
                Ok(()) => return Self(dir),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => panic!("cannot create {}: {e}", dir.display()),
            }
        }
        panic!("no free temporary directory name for {tag}");
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
