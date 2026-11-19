//! Hot-swap state blob: every rule-state local as little-endian `i64`s in a
//! fixed order (the Wasm counterpart of the Lua `on_save_state` table).
//! Booleans are stored as 0 / 1. A blob of any other length is ignored on
//! restore, as the Lua rules ignore a non-table value.

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
/// defines `Rules::SAVED_BYTES`, `Rules::encode_state(&self, buf) -> len`
/// and `Rules::decode_state(&mut self, blob) -> bool` (false, and nothing
/// changed, unless `blob.len() == SAVED_BYTES`).
macro_rules! saved_fields {
    ($ty:ty; $($f:ident),* $(,)?) => {
        impl $ty {
            /// Length of the state blob, bytes.
            pub const SAVED_BYTES: usize =
                8 * (0 $(+ $crate::saved::slots_of(|s: &Self| &s.$f))*);

            /// Writes the state blob into `buf`; returns its length.
            pub fn encode_state(&self, buf: &mut [u8]) -> usize {
                let at = 0;
                $(let at = $crate::saved::Saved::put(&self.$f, buf, at);)*
                at * 8
            }

            /// Restores the fields from `blob`; false if its length is wrong.
            pub fn decode_state(&mut self, blob: &[u8]) -> bool {
                if blob.len() != Self::SAVED_BYTES {
                    return false;
                }
                let at = 0;
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
