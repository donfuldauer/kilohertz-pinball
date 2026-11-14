//! Private scratch directories for tests that write scripts and then run
//! them (CWE-377). The OS temp dir can be shared with other users, so a
//! directory is only used if this call created it (`create_dir`, which fails
//! on any existing entry, symlinks included), owner-only (`0o700`) on Unix.
//! A pre-existing path is never reused: another name is tried instead.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Creates `path` as a new, owner-only directory; `AlreadyExists` (or any
/// other error) if it cannot be created by this call.
pub fn create_exclusive(path: &Path) -> io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(path)
}

/// A fresh private directory `<temp>/<prefix>-<pid>-<n>-<nonce>`.
/// Panics if none can be created.
pub fn create(prefix: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let base = std::env::temp_dir();
    for _ in 0..64 {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let p = base.join(format!("{prefix}-{}-{n}-{nonce:x}", std::process::id()));
        match create_exclusive(&p) {
            Ok(()) => return p,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => panic!("create private temp dir {}: {e}", p.display()),
        }
    }
    panic!("no fresh private temp dir name for {prefix}");
}
