//! SHA-256 hex digest (hybrid design's allow-list key).

use sha2::{Digest, Sha256};

/// Lower-case hex SHA-256 of `b`.
#[must_use]
pub fn hex(b: &[u8]) -> String {
    let d = Sha256::digest(b);
    d.iter().map(|x| format!("{x:02x}")).collect()
}
