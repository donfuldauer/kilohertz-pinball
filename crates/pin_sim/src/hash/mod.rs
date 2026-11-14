//! FNV-1a 64: the fixed, documented hash behind [`Simulation::state_hash`],
//! [`Simulation::table_hash`] and the replay file checksum.
//!
//! Offset basis `0xcbf29ce484222325`, prime `0x100000001b3`, one byte at a
//! time. Integers are fed little-endian, `f32` as its exact bit pattern, so
//! the hash of the same state is the same on every platform.
//!
//! [`Simulation::state_hash`]: crate::Simulation::state_hash
//! [`Simulation::table_hash`]: crate::Simulation::table_hash

/// FNV-1a 64 offset basis.
pub const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
/// FNV-1a 64 prime.
pub const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// An incremental FNV-1a 64 hasher (no allocation).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Fnv64(u64);

impl Default for Fnv64 {
    fn default() -> Self {
        Self::new()
    }
}

impl Fnv64 {
    /// A hasher at the offset basis.
    #[must_use]
    pub const fn new() -> Self {
        Self(FNV_OFFSET)
    }

    /// Feeds bytes.
    pub fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(FNV_PRIME);
        }
    }

    /// Feeds one byte.
    pub fn write_u8(&mut self, v: u8) {
        self.write(&[v]);
    }

    /// Feeds a `u32`, little-endian.
    pub fn write_u32(&mut self, v: u32) {
        self.write(&v.to_le_bytes());
    }

    /// Feeds a `u64`, little-endian.
    pub fn write_u64(&mut self, v: u64) {
        self.write(&v.to_le_bytes());
    }

    /// Feeds an `i64`, little-endian.
    pub fn write_i64(&mut self, v: i64) {
        self.write(&v.to_le_bytes());
    }

    /// Feeds the exact bits of an `f32`.
    pub fn write_f32(&mut self, v: f32) {
        self.write_u32(v.to_bits());
    }

    /// The hash so far.
    #[must_use]
    pub const fn finish(&self) -> u64 {
        self.0
    }
}

/// FNV-1a 64 of `bytes`.
#[must_use]
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h = Fnv64::new();
    h.write(bytes);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_vectors() {
        // Published FNV-1a 64 test vectors.
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn typed_writes_are_little_endian_bytes() {
        let mut a = Fnv64::default();
        a.write_u64(0x0102_0304_0506_0708);
        a.write_u32(7);
        a.write_i64(-1);
        a.write_f32(-0.0);
        a.write_u8(9);
        let mut b = Fnv64::new();
        b.write(&[8, 7, 6, 5, 4, 3, 2, 1, 7, 0, 0, 0]);
        b.write(&[0xff; 8]);
        b.write(&[0, 0, 0, 0x80, 9]);
        assert_eq!(a.finish(), b.finish());
        assert_ne!(fnv1a64(&0.0f32.to_bits().to_le_bytes()), {
            let mut h = Fnv64::new();
            h.write_f32(-0.0);
            h.finish()
        });
    }
}
