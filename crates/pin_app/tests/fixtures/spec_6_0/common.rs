//! Shared helpers of the Subphase 6.0 spec tests (Independent Tester).
//!
//! The including test file declares `mod support;` (the Implementer's
//! `tests/support`, whose `build_greybox_bundle(out_dir)` is the brief's
//! documented way to build `tables/greybox` into a bundle) next to this
//! module.

#![allow(dead_code)]

use pin_assets::{LoadedTable, Vfs, load_table};
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Profile used for injected balls.
pub const STEEL: &str = "standard_steel";

/// Radius of `standard_steel`, m.
pub fn steel_radius_m() -> f32 {
    pin_core::builtin_profile(STEEL)
        .expect("built-in profile")
        .radius_m()
}

/// What `build_greybox_bundle` may return: the bundle path, or nothing when
/// the bundle is `out_dir` itself (either possibly wrapped in a `Result`).
pub trait BundleOut {
    /// The bundle directory.
    fn bundle_path(self, out_dir: &Path) -> PathBuf;
}

impl BundleOut for PathBuf {
    fn bundle_path(self, _out_dir: &Path) -> PathBuf {
        self
    }
}

impl BundleOut for () {
    fn bundle_path(self, out_dir: &Path) -> PathBuf {
        out_dir.to_path_buf()
    }
}

impl<E: Debug> BundleOut for Result<PathBuf, E> {
    fn bundle_path(self, _out_dir: &Path) -> PathBuf {
        self.expect("build_greybox_bundle")
    }
}

impl<E: Debug> BundleOut for Result<(), E> {
    fn bundle_path(self, out_dir: &Path) -> PathBuf {
        self.expect("build_greybox_bundle");
        out_dir.to_path_buf()
    }
}

/// Scratch directory under the test target dir, unique to this process.
pub fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("spec_6_0_{name}_{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clear scratch dir");
    }
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// The grey-box table built once per test binary.
pub fn greybox_bundle() -> &'static Path {
    static BUNDLE: OnceLock<PathBuf> = OnceLock::new();
    BUNDLE.get_or_init(|| {
        let out = scratch("greybox");
        crate::support::build_greybox_bundle(&out).bundle_path(&out)
    })
}

/// `pin_assets::load_table` on the grey-box bundle.
pub fn greybox_table() -> &'static LoadedTable {
    static TABLE: OnceLock<LoadedTable> = OnceLock::new();
    TABLE.get_or_init(|| {
        let vfs = Vfs::open(greybox_bundle()).expect("open grey-box bundle");
        load_table(&vfs).expect("grey-box bundle passes load_table")
    })
}

/// The hand-written fixtures of this subphase.
pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/spec_6_0")
        .join(name)
}
