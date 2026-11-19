//! Hot-swap state blob: every rule-state local as little-endian `i64`s in a
//! fixed order (the Wasm counterpart of the Lua `on_save_state` table).
//! Booleans are stored as 0 / 1.
//!
//! Slot 0 holds the layout fingerprint ([`fnv1a_extend`] over every saved
//! field's name and slot count, in field order), so the blob describes its
//! own layout. A blob of another length or another fingerprint is ignored on
//! restore, as the Lua rules ignore a non-table value (Lua carries a keyed
//! table, so a reordered or renamed local cannot be restored into the wrong
//! one; the fingerprint gives the positional Wasm blob the same guarantee).

/// A value that round-trips through the state blob.
pub trait Saved {
    /// Number of `i64` slots the value takes.
    const SLOTS: usize;
    /// Writes the value's slots starting at slot `at`; returns the next slot.
    fn put(&self, buf: &mut [u8], at: usize) -> usize;
    /// Reads the value's slots starting at slot `at`; returns the next slot.
    fn get(&mut self, buf: &[u8], at: usize) -> usize;
}

fn write_slot(buf: &mut [u8], at: usize, v: i64) {
    buf[at * 8..at * 8 + 8].copy_from_slice(&v.to_le_bytes());
}

fn read_slot(buf: &[u8], at: usize) -> i64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&buf[at * 8..at * 8 + 8]);
    i64::from_le_bytes(b)
}

impl Saved for i64 {
    const SLOTS: usize = 1;
    fn put(&self, buf: &mut [u8], at: usize) -> usize {
        write_slot(buf, at, *self);
        at + 1
    }
    fn get(&mut self, buf: &[u8], at: usize) -> usize {
        *self = read_slot(buf, at);
        at + 1
    }
}

impl Saved for bool {
    const SLOTS: usize = 1;
    fn put(&self, buf: &mut [u8], at: usize) -> usize {
        write_slot(buf, at, i64::from(*self));
        at + 1
    }
    fn get(&mut self, buf: &[u8], at: usize) -> usize {
        *self = read_slot(buf, at) != 0;
        at + 1
    }
}

impl<T: Saved, const N: usize> Saved for [T; N] {
    const SLOTS: usize = T::SLOTS * N;
    fn put(&self, buf: &mut [u8], mut at: usize) -> usize {
        for v in self {
            at = v.put(buf, at);
        }
        at
    }
    fn get(&mut self, buf: &[u8], mut at: usize) -> usize {
        for v in self.iter_mut() {
            at = v.get(buf, at);
        }
        at
    }
}

/// Declares the saved fields of a rules struct: `saved_fields!(Rules; a, b, …)`
/// defines `Rules::LAYOUT_FINGERPRINT`, `Rules::SAVED_BYTES`,
/// `Rules::encode_state(&self, buf) -> len` and
/// `Rules::decode_state(&mut self, blob) -> bool`.
///
/// The blob begins with the layout fingerprint (slot 0, the `u64` as an
/// `i64`), followed by the fields in declaration order from slot 1.
/// `decode_state` returns false, and changes nothing, unless
/// `blob.len() == SAVED_BYTES` and slot 0 equals `LAYOUT_FINGERPRINT`.
macro_rules! saved_fields {
    ($ty:ty; $($f:ident),* $(,)?) => {
        impl $ty {
            /// 64-bit FNV-1a over each saved field's name and slot count
            /// (8 little-endian bytes), in field order; computed at compile
            /// time. It changes when fields are reordered, renamed, or change
            /// their slot count.
            pub const LAYOUT_FINGERPRINT: u64 = {
                let h = $crate::saved::FNV_OFFSET_BASIS;
                $(
                    let h = $crate::saved::fnv1a_extend(h, stringify!($f).as_bytes());
                    let h = $crate::saved::fnv1a_extend(
                        h,
                        &($crate::saved::slots_of(|s: &Self| &s.$f) as u64).to_le_bytes(),
                    );
                )*
                h
            };

            /// Length of the state blob, bytes (fingerprint slot included).
            pub const SAVED_BYTES: usize =
                8 * (1 $(+ $crate::saved::slots_of(|s: &Self| &s.$f))*);

            /// Writes the state blob (fingerprint first) into `buf`; returns
            /// its length.
            pub fn encode_state(&self, buf: &mut [u8]) -> usize {
                let at = $crate::saved::Saved::put(
                    &(Self::LAYOUT_FINGERPRINT as i64),
                    buf,
                    0,
                );
                $(let at = $crate::saved::Saved::put(&self.$f, buf, at);)*
                at * 8
            }

            /// Restores the fields from `blob`; false, and nothing changed,
            /// if its length or layout fingerprint is wrong.
            pub fn decode_state(&mut self, blob: &[u8]) -> bool {
                if blob.len() != Self::SAVED_BYTES {
                    return false;
                }
                let mut fingerprint = 0i64;
                let at = $crate::saved::Saved::get(&mut fingerprint, blob, 0);
                if fingerprint != Self::LAYOUT_FINGERPRINT as i64 {
                    return false;
                }
                $(let at = $crate::saved::Saved::get(&mut self.$f, blob, at);)*
                let _ = at;
                true
            }
        }
    };
}

pub(crate) use saved_fields;

/// `Saved::SLOTS` of the field a projection selects (used by
/// [`saved_fields!`] to size the blob at compile time).
pub const fn slots_of<S, T: Saved>(_field: fn(&S) -> &T) -> usize {
    T::SLOTS
}

/// 64-bit FNV-1a offset basis (start value of a layout fingerprint).
pub const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;

/// 64-bit FNV-1a prime.
pub const FNV_PRIME: u64 = 0x0100_0000_01b3;

/// Folds `bytes` into the 64-bit FNV-1a hash `h` (xor each byte, then
/// multiply by [`FNV_PRIME`], wrapping). A `const fn` so
/// [`saved_fields!`] computes the layout fingerprint at compile time.
pub const fn fnv1a_extend(mut h: u64, bytes: &[u8]) -> u64 {
    let mut i = 0;
    while i < bytes.len() {
        h ^= bytes[i] as u64;
        h = h.wrapping_mul(FNV_PRIME);
        i += 1;
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    struct A {
        x: i64,
        y: bool,
    }
    struct B {
        y: bool,
        x: i64,
    }
    struct C {
        x: i64,
        z: bool,
    }
    struct D {
        x: [i64; 2],
    }
    struct E {
        x: i64,
        w: i64,
    }
    saved_fields!(A; x, y);
    saved_fields!(B; y, x);
    saved_fields!(C; x, z);
    saved_fields!(D; x);
    saved_fields!(E; x, w);

    #[test]
    fn fnv1a_matches_the_reference_vectors() {
        assert_eq!(fnv1a_extend(FNV_OFFSET_BASIS, b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a_extend(FNV_OFFSET_BASIS, b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(
            fnv1a_extend(FNV_OFFSET_BASIS, b"foobar"),
            0x8594_4171_f739_67e8
        );
        // Folding in pieces equals hashing the concatenation.
        let h = fnv1a_extend(fnv1a_extend(FNV_OFFSET_BASIS, b"foo"), b"bar");
        assert_eq!(h, fnv1a_extend(FNV_OFFSET_BASIS, b"foobar"));
    }

    #[test]
    fn fingerprint_is_fnv1a_of_names_and_slot_counts() {
        let mut h = FNV_OFFSET_BASIS;
        h = fnv1a_extend(h, b"x");
        h = fnv1a_extend(h, &1u64.to_le_bytes());
        h = fnv1a_extend(h, b"y");
        h = fnv1a_extend(h, &1u64.to_le_bytes());
        assert_eq!(A::LAYOUT_FINGERPRINT, h);
    }

    #[test]
    fn fingerprint_changes_with_order_name_and_slot_count() {
        assert_eq!(A::SAVED_BYTES, B::SAVED_BYTES);
        assert_eq!(A::SAVED_BYTES, C::SAVED_BYTES);
        assert_eq!(D::SAVED_BYTES, E::SAVED_BYTES);
        assert_ne!(A::LAYOUT_FINGERPRINT, B::LAYOUT_FINGERPRINT, "swapped");
        assert_ne!(A::LAYOUT_FINGERPRINT, C::LAYOUT_FINGERPRINT, "renamed");
        assert_ne!(D::LAYOUT_FINGERPRINT, E::LAYOUT_FINGERPRINT, "slots");
    }

    #[test]
    fn a_blob_of_a_renamed_or_resized_layout_is_rejected() {
        let mut buf = [0u8; 64];
        let n = A { x: 7, y: true }.encode_state(&mut buf);
        let mut c = C { x: 3, z: false };
        assert!(!c.decode_state(&buf[..n]));
        assert_eq!((c.x, c.z), (3, false));
        let n = D { x: [8, 9] }.encode_state(&mut buf);
        let mut e = E { x: 1, w: 2 };
        assert!(!e.decode_state(&buf[..n]));
        assert_eq!((e.x, e.w), (1, 2));
    }

    #[test]
    fn blob_starts_with_the_fingerprint_and_round_trips() {
        let a = A { x: -5, y: true };
        let mut buf = [0u8; 64];
        let n = a.encode_state(&mut buf);
        assert_eq!(n, A::SAVED_BYTES);
        assert_eq!(n, 3 * 8);
        assert_eq!(buf[..8], A::LAYOUT_FINGERPRINT.to_le_bytes());
        let mut back = A { x: 0, y: false };
        assert!(back.decode_state(&buf[..n]));
        assert_eq!((back.x, back.y), (-5, true));
    }

    #[test]
    fn a_blob_of_a_reordered_layout_is_rejected() {
        let a = A { x: 7, y: true };
        let mut buf = [0u8; 64];
        let n = a.encode_state(&mut buf);
        assert_eq!(n, B::SAVED_BYTES);
        let mut b = B { y: false, x: 3 };
        assert!(!b.decode_state(&buf[..n]));
        assert_eq!((b.x, b.y), (3, false));
    }

    #[test]
    fn a_blob_with_a_flipped_fingerprint_bit_or_wrong_length_is_rejected() {
        let a = A { x: 7, y: true };
        let mut buf = [0u8; 64];
        let n = a.encode_state(&mut buf);
        let mut back = A { x: 1, y: false };
        for bit in 0..64 {
            let mut bad = buf;
            bad[bit / 8] ^= 1 << (bit % 8);
            assert!(!back.decode_state(&bad[..n]), "bit {bit}");
            assert_eq!((back.x, back.y), (1, false));
        }
        assert!(!back.decode_state(&buf[..n - 8]));
        assert!(!back.decode_state(&buf[..n + 8]));
        assert!(!back.decode_state(&[]));
        assert_eq!((back.x, back.y), (1, false));
    }

    #[test]
    fn every_test_layout_round_trips_into_itself() {
        let mut buf = [0u8; 64];
        let n = B { y: true, x: 4 }.encode_state(&mut buf);
        let mut b = B { y: false, x: 0 };
        assert!(b.decode_state(&buf[..n]));
        assert_eq!((b.x, b.y), (4, true));
        let n = C { x: 5, z: true }.encode_state(&mut buf);
        let mut c = C { x: 0, z: false };
        assert!(c.decode_state(&buf[..n]));
        assert_eq!((c.x, c.z), (5, true));
        let n = D { x: [6, 7] }.encode_state(&mut buf);
        let mut d = D { x: [0, 0] };
        assert!(d.decode_state(&buf[..n]));
        assert_eq!(d.x, [6, 7]);
        let n = E { x: 8, w: 9 }.encode_state(&mut buf);
        let mut e = E { x: 0, w: 0 };
        assert!(e.decode_state(&buf[..n]));
        assert_eq!((e.x, e.w), (8, 9));
    }
}
