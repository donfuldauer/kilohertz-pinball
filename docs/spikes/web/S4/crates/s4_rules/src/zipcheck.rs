//! Bundle hardening through the real loader (`Vfs::from_zip_bytes`, the
//! web path) on bytes, with the allocator high-water mark: the same
//! function runs natively and in the wasm module (fed from a file input).

use crate::peak;
use pin_assets::{Vfs, ZipLimits};
use std::fmt::Write as _;

/// The proposed web limits (RESULTS §zip): 64 MiB total, 32 MiB per file.
#[must_use]
pub fn web_limits() -> ZipLimits {
    ZipLimits {
        max_total_uncompressed: 64 << 20,
        max_file: 32 << 20,
        ..ZipLimits::default()
    }
}

/// Opens `bytes`, then reads every entry; returns one line:
/// `open=<ok|err> entries=<n> read_ok=<n> read_err=[...] peak_alloc=<bytes>`
/// (`limits`: `default` or `web`).
#[must_use]
pub fn check(bytes: &[u8], web: bool) -> String {
    let limits = if web {
        web_limits()
    } else {
        ZipLimits::default()
    };
    let base = peak::current();
    peak::reset();
    let mut out = String::new();
    match Vfs::from_zip_bytes(bytes.to_vec(), limits) {
        Err(e) => {
            let _ = write!(out, "open=err({e})");
        }
        Ok(vfs) => {
            let list = vfs.list();
            let (mut ok, mut errs, mut total) = (0usize, Vec::new(), 0u64);
            for p in &list {
                match vfs.read(p) {
                    Ok(b) => {
                        ok += 1;
                        total += b.len() as u64;
                    }
                    Err(e) => errs.push(format!("{p}: {e}")),
                }
            }
            let _ = write!(
                out,
                "open=ok entries={} read_ok={ok} read_bytes={total} read_err={errs:?}",
                list.len()
            );
        }
    }
    let _ = write!(
        out,
        " peak_alloc_above_base={}",
        peak::peak().saturating_sub(base)
    );
    out
}
