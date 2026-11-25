//! FNV-1a 64: the fixed, documented hash behind [`Simulation::state_hash`],
//! [`Simulation::table_hash`] and the replay file checksum.
//!
//! Offset basis `0xcbf29ce484222325`, prime `0x100000001b3`, one byte at a
//! time. Integers are fed little-endian; `f32` and `f64` as their exact bit
//! pattern, except NaN, which is fed as one canonical pattern (`0x7fc0_0000`
//! for `f32`, `0x7ff8_0000_0000_0000` for `f64`; D9, REQ-WEB-002), so the hash
//! of the same state is the same on every platform even though targets produce
//! different NaN bits for the same invalid operation.
//!
//! [`Simulation::state_hash`]: crate::Simulation::state_hash
//! [`Simulation::table_hash`]: crate::Simulation::table_hash

/// FNV-1a 64 offset basis.
pub const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
/// FNV-1a 64 prime.
pub const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// The one bit pattern [`Fnv64::write_f32`] feeds for every `f32` NaN (D9).
const CANONICAL_NAN_F32: u32 = 0x7fc0_0000;
/// The one bit pattern [`Fnv64::write_f64`] feeds for every `f64` NaN (D9).
const CANONICAL_NAN_F64: u64 = 0x7ff8_0000_0000_0000;

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

    /// Feeds an `f32`: its exact bits, little-endian, or the canonical
    /// pattern `0x7fc0_0000` if it is any NaN (D9, REQ-WEB-002).
    ///
    /// Every other value, including `-0.0`, infinities and subnormals, is fed
    /// bit for bit, so `-0.0` and `+0.0` hash differently.
    pub fn write_f32(&mut self, v: f32) {
        if v.is_nan() {
            self.write_u32(CANONICAL_NAN_F32);
        } else {
            self.write_u32(v.to_bits());
        }
    }

    /// Feeds an `f64`: its exact bits, little-endian, or the canonical
    /// pattern `0x7ff8_0000_0000_0000` if it is any NaN (D9, REQ-WEB-002).
    ///
    /// Every other value, including `-0.0`, infinities and subnormals, is fed
    /// bit for bit, so `-0.0` and `+0.0` hash differently.
    pub fn write_f64(&mut self, v: f64) {
        if v.is_nan() {
            self.write_u64(CANONICAL_NAN_F64);
        } else {
            self.write_u64(v.to_bits());
        }
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

    fn f32_digest(v: f32) -> u64 {
        let mut h = Fnv64::new();
        h.write_f32(v);
        h.finish()
    }

    fn f64_digest(v: f64) -> u64 {
        let mut h = Fnv64::new();
        h.write_f64(v);
        h.finish()
    }

    /// D9, REQ-WEB-002: f32 NaN payload classes covered by the canonical-NaN tests.
    fn f32_nans() -> Vec<f32> {
        let (z1, z2) = (std::hint::black_box(0.0f32), std::hint::black_box(0.0f32));
        vec![
            f32::NAN,
            -f32::NAN,
            z1 / z2,
            f32::from_bits(0x7fc0_0000), // positive quiet, canonical
            f32::from_bits(0xffc0_0000), // negative quiet (x86_64 default NaN)
            f32::from_bits(0x7fc0_0001), // positive quiet with payload
            f32::from_bits(0xffc1_2345), // negative quiet with payload
            f32::from_bits(0x7f80_0001), // positive signalling, smallest payload
            f32::from_bits(0xff80_0001), // negative signalling
            f32::from_bits(0x7fbf_ffff), // positive signalling, full payload
            f32::from_bits(0x7fff_ffff), // positive quiet, all payload bits
            f32::from_bits(0xffff_ffff), // negative quiet, all payload bits
        ]
    }

    /// D9, REQ-WEB-002: f64 NaN payload classes covered by the canonical-NaN tests.
    fn f64_nans() -> Vec<f64> {
        let (z1, z2) = (std::hint::black_box(0.0f64), std::hint::black_box(0.0f64));
        vec![
            f64::NAN,
            -f64::NAN,
            z1 / z2,
            f64::from_bits(0x7ff8_0000_0000_0000),
            f64::from_bits(0xfff8_0000_0000_0000),
            f64::from_bits(0x7ff8_0000_0000_0001),
            f64::from_bits(0xfff8_dead_beef_0001),
            f64::from_bits(0x7ff0_0000_0000_0001),
            f64::from_bits(0xfff0_0000_0000_0001),
            f64::from_bits(0x7ff7_ffff_ffff_ffff),
            f64::from_bits(0x7fff_ffff_ffff_ffff),
            f64::from_bits(0xffff_ffff_ffff_ffff),
        ]
    }

    /// D9, REQ-WEB-002: every f32 NaN hashes as the canonical pattern `0x7fc0_0000`.
    #[test]
    fn d9_req_web_002_f32_nan_hashes_canonically() {
        let canonical = fnv1a64(&0x7fc0_0000u32.to_le_bytes());
        for v in f32_nans() {
            assert!(v.is_nan());
            assert_eq!(f32_digest(v), canonical, "NaN bits {:#010x}", v.to_bits());
        }
        assert_eq!(CANONICAL_NAN_F32, 0x7fc0_0000);
    }

    /// D9, REQ-WEB-002: every f64 NaN hashes as the canonical pattern `0x7ff8_0000_0000_0000`.
    #[test]
    fn d9_req_web_002_f64_nan_hashes_canonically() {
        let canonical = fnv1a64(&0x7ff8_0000_0000_0000u64.to_le_bytes());
        for v in f64_nans() {
            assert!(v.is_nan());
            assert_eq!(f64_digest(v), canonical, "NaN bits {:#018x}", v.to_bits());
        }
        assert_eq!(CANONICAL_NAN_F64, 0x7ff8_0000_0000_0000);
    }

    /// D9: non-NaN f32 values (zeros, infinities, subnormals, normals) hash their exact
    /// little-endian bits, as before the canonical-NaN change; -0.0 and +0.0 differ.
    #[test]
    fn d9_f32_non_nan_hash_unchanged() {
        for bits in [
            0x0000_0000u32,
            0x8000_0000,
            0x7f80_0000,
            0xff80_0000,
            0x0000_0001,
            0x807f_ffff,
            0x3f80_0000,
            0xc2f6_e979,
            0x7f7f_ffff,
        ] {
            let v = f32::from_bits(bits);
            assert!(!v.is_nan());
            assert_eq!(
                f32_digest(v),
                fnv1a64(&bits.to_le_bytes()),
                "bits {bits:#010x}"
            );
        }
        assert_ne!(f32_digest(0.0), f32_digest(-0.0));
    }

    /// D9: non-NaN f64 values hash their exact little-endian bits; -0.0 and +0.0 differ;
    /// `write_f64` feeds the little-endian bytes of `to_bits`.
    #[test]
    fn d9_f64_non_nan_hash_is_le_bits() {
        for bits in [
            0x0000_0000_0000_0000u64,
            0x8000_0000_0000_0000,
            0x7ff0_0000_0000_0000,
            0xfff0_0000_0000_0000,
            0x0000_0000_0000_0001,
            0x800f_ffff_ffff_ffff,
            0x3ff0_0000_0000_0000,
            0x0102_0304_0506_0708,
            0x7fef_ffff_ffff_ffff,
        ] {
            let v = f64::from_bits(bits);
            assert!(!v.is_nan());
            assert_eq!(
                f64_digest(v),
                fnv1a64(&bits.to_le_bytes()),
                "bits {bits:#018x}"
            );
            let mut by_u64 = Fnv64::new();
            by_u64.write_u64(bits);
            assert_eq!(f64_digest(v), by_u64.finish());
        }
        assert_ne!(f64_digest(0.0), f64_digest(-0.0));
        let mut a = Fnv64::new();
        a.write_f64(f64::from_bits(0x0102_0304_0506_0708));
        assert_eq!(a.finish(), fnv1a64(&[8, 7, 6, 5, 4, 3, 2, 1]));
    }
}
