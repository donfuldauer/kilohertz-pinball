//! [`HeldInputs`]: which raw inputs hold each cabinet input (issue #9b).
//!
//! `pin_input::ProfileMapper` is stateless: every raw edge becomes a cabinet
//! edge on its own, so with two raw inputs bound to one cabinet input (Space
//! and Return both launch; a keyboard key and a gamepad button both flip)
//! releasing one would release the cabinet input while the other is still
//! down. `HeldInputs` keeps, per cabinet input, the set of raw-input
//! *sources* holding it and reports a cabinet edge only when the first
//! source presses it or the last one releases it (the OR of the sources).
//!
//! A source is one raw input of one device: a key, a button or an axis used
//! as a button, qualified by the device ([`source_id`]). Up to
//! [`MAX_HELD_SOURCES`] sources can hold inputs at the same time; a source
//! takes a slot while it holds at least one input and frees it on its last
//! release. Nothing allocates after construction.
//!
//! The UI's input monitor (`pin_app::ui::live::InputMonitor`) is built on
//! the same type.

use arrayvec::ArrayVec;
use pin_core::input::CabinetInput;

/// Sources that can hold cabinet inputs at the same time (distinct
/// [`source_id`]s with at least one input held).
pub const MAX_HELD_SOURCES: usize = 256;

const WORDS: usize = MAX_HELD_SOURCES / 64;

/// Bits of a source id that name the raw input; the device number sits
/// above them.
pub const RAW_BITS: u32 = 16;

/// Raw-input code space of keyboard keys (`SDL_Scancode`, below `0x1000`).
pub const RAW_KEY: u32 = 0x0000;
/// Raw-input code space of gamepad buttons (`0x1000 + SDL button index`).
pub const RAW_BUTTON: u32 = 0x1000;
/// Raw-input code space of gamepad axes used as buttons (`0x2000 + axis`).
pub const RAW_AXIS: u32 = 0x2000;

/// The source id of raw input `raw` on device `device`:
/// `(device << 16) | raw`, both truncated to 16 bits.
///
/// The platform layer numbers connected devices below 65 536 (0 = the
/// keyboard, gamepads get the lowest free number from 1 while connected) and
/// encodes raw inputs below 65 536 ([`RAW_KEY`] + scancode, [`RAW_BUTTON`] +
/// button, [`RAW_AXIS`] + axis), so two raw inputs, or the same raw input on
/// two devices, never share an id.
pub const fn source_id(device: u32, raw: u32) -> u32 {
    ((device & 0xFFFF) << RAW_BITS) | (raw & 0xFFFF)
}

/// The device number of a [`source_id`].
pub const fn source_device(source: u32) -> u32 {
    source >> RAW_BITS
}

/// Per cabinet input, the raw-input sources holding it. Fixed size; never
/// allocates after [`new`](Self::new).
///
/// Equality compares what is held (which source holds which input), not
/// the internal slot layout or the overflow counter.
#[derive(Clone, Debug)]
pub struct HeldInputs {
    /// Source id of each slot (meaningful where `used` is set).
    slots: [u32; MAX_HELD_SOURCES],
    /// Allocated slots.
    used: [u64; WORDS],
    /// Per cabinet input (`CabinetInput::index`), the slots holding it.
    held: [[u64; WORDS]; CabinetInput::COUNT],
    /// Presses ignored because every slot was taken.
    overflow: u64,
}

impl Default for HeldInputs {
    fn default() -> Self {
        Self::new()
    }
}

fn bit(words: &[u64; WORDS], slot: usize) -> bool {
    words[slot / 64] & (1u64 << (slot % 64)) != 0
}

fn set_bit(words: &mut [u64; WORDS], slot: usize, on: bool) {
    let b = 1u64 << (slot % 64);
    if on {
        words[slot / 64] |= b;
    } else {
        words[slot / 64] &= !b;
    }
}

fn any(words: &[u64; WORDS]) -> bool {
    words.iter().any(|w| *w != 0)
}

impl PartialEq for HeldInputs {
    fn eq(&self, other: &Self) -> bool {
        let covered = |a: &HeldInputs, b: &HeldInputs| {
            (0..MAX_HELD_SOURCES).filter(|&s| bit(&a.used, s)).all(|s| {
                let src = a.slots[s];
                CabinetInput::ALL
                    .iter()
                    .all(|i| bit(&a.held[i.index()], s) == b.is_held_by(src, *i))
            })
        };
        covered(self, other) && covered(other, self)
    }
}

impl Eq for HeldInputs {}

impl HeldInputs {
    /// Nothing held.
    pub const fn new() -> Self {
        Self {
            slots: [0; MAX_HELD_SOURCES],
            used: [0; WORDS],
            held: [[0; WORDS]; CabinetInput::COUNT],
            overflow: 0,
        }
    }

    fn slot_of(&self, source: u32) -> Option<usize> {
        (0..MAX_HELD_SOURCES).find(|&s| bit(&self.used, s) && self.slots[s] == source)
    }

    fn free_slot(&self) -> Option<usize> {
        (0..MAX_HELD_SOURCES).find(|&s| !bit(&self.used, s))
    }

    /// Frees `slot` when it holds no input any more.
    fn release_slot_if_idle(&mut self, slot: usize) {
        if !self.held.iter().any(|h| bit(h, slot)) {
            set_bit(&mut self.used, slot, false);
        }
    }

    /// Records that `source` pressed (`pressed = true`) or released `input`.
    ///
    /// Returns `Some(true)` only when `input` goes from not held to held,
    /// `Some(false)` only when its last holder releases it, `None` otherwise:
    /// a repeated press by a source that already holds it, a press while
    /// another source holds it, a release while another source still holds
    /// it, and a release by a source that does not hold it are all `None`.
    /// A press when [`MAX_HELD_SOURCES`] other sources already hold inputs is
    /// ignored (`None`, counted in [`overflow`](Self::overflow)).
    pub fn set(&mut self, source: u32, input: CabinetInput, pressed: bool) -> Option<bool> {
        let i = input.index();
        if pressed {
            let slot = match self.slot_of(source) {
                Some(s) => s,
                None => {
                    let Some(s) = self.free_slot() else {
                        self.overflow += 1;
                        return None;
                    };
                    self.slots[s] = source;
                    set_bit(&mut self.used, s, true);
                    s
                }
            };
            if bit(&self.held[i], slot) {
                return None;
            }
            let was_held = any(&self.held[i]);
            set_bit(&mut self.held[i], slot, true);
            (!was_held).then_some(true)
        } else {
            let slot = self.slot_of(source)?;
            if !bit(&self.held[i], slot) {
                return None;
            }
            set_bit(&mut self.held[i], slot, false);
            self.release_slot_if_idle(slot);
            (!any(&self.held[i])).then_some(false)
        }
    }

    /// Whether at least one source holds `input`.
    pub fn is_held(&self, input: CabinetInput) -> bool {
        any(&self.held[input.index()])
    }

    /// Whether `source` holds `input`.
    pub fn is_held_by(&self, source: u32, input: CabinetInput) -> bool {
        self.slot_of(source)
            .is_some_and(|s| bit(&self.held[input.index()], s))
    }

    /// Bit `CabinetInput::index` set for every held input (the
    /// `CabinetFrame::buttons` layout).
    pub fn pressed_mask(&self) -> u8 {
        CabinetInput::ALL
            .iter()
            .filter(|i| self.is_held(**i))
            .fold(0u8, |m, i| m | (1 << i.index()))
    }

    /// Sources currently holding at least one input.
    pub fn active_sources(&self) -> usize {
        self.used.iter().map(|w| w.count_ones() as usize).sum()
    }

    /// Presses ignored because all [`MAX_HELD_SOURCES`] slots were taken.
    pub fn overflow(&self) -> u64 {
        self.overflow
    }

    /// Releases everything `source` holds. Returns the inputs that became
    /// released (no other source holds them), in `CabinetInput::ALL` order.
    pub fn release_source(
        &mut self,
        source: u32,
    ) -> ArrayVec<CabinetInput, { CabinetInput::COUNT }> {
        self.release_where(|s| s == source)
    }

    /// Releases everything held by any source of `device` (the device was
    /// disconnected). Returns the inputs that became released, in
    /// `CabinetInput::ALL` order.
    pub fn release_all_for_device(
        &mut self,
        device: u32,
    ) -> ArrayVec<CabinetInput, { CabinetInput::COUNT }> {
        self.release_where(|s| source_device(s) == device)
    }

    /// Releases every source (focus lost, profile switch). Returns the
    /// inputs that were held, in `CabinetInput::ALL` order.
    pub fn release_all(&mut self) -> ArrayVec<CabinetInput, { CabinetInput::COUNT }> {
        self.release_where(|_| true)
    }

    fn release_where(
        &mut self,
        matches: impl Fn(u32) -> bool,
    ) -> ArrayVec<CabinetInput, { CabinetInput::COUNT }> {
        let before = CabinetInput::ALL.map(|i| self.is_held(i));
        for slot in 0..MAX_HELD_SOURCES {
            if bit(&self.used, slot) && matches(self.slots[slot]) {
                for h in &mut self.held {
                    set_bit(h, slot, false);
                }
                set_bit(&mut self.used, slot, false);
            }
        }
        let mut out = ArrayVec::new();
        for (k, input) in CabinetInput::ALL.iter().enumerate() {
            if before[k] && !self.is_held(*input) {
                out.push(*input);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use CabinetInput::*;

    #[test]
    fn aliases_hold_until_last_release() {
        let mut h = HeldInputs::new();
        let a = source_id(0, RAW_KEY + 44);
        let b = source_id(0, RAW_KEY + 40);
        assert_eq!(h.set(a, Launch, true), Some(true));
        assert_eq!(h.set(b, Launch, true), None);
        assert_eq!(h.set(a, Launch, false), None);
        assert!(h.is_held(Launch));
        assert_eq!(h.set(b, Launch, false), Some(false));
        assert!(!h.is_held(Launch));
        assert_eq!(h.active_sources(), 0);
    }

    #[test]
    fn source_ids_are_device_qualified() {
        assert_ne!(source_id(0, 5), source_id(1, 5));
        assert_ne!(source_id(1, RAW_KEY + 5), source_id(1, RAW_BUTTON + 5));
        assert_eq!(source_device(source_id(7, 0x2003)), 7);
        assert_eq!(source_id(0x1_0001, 0x1_0002), source_id(1, 2));
    }

    #[test]
    fn equality_ignores_slot_order() {
        let mut a = HeldInputs::new();
        let mut b = HeldInputs::new();
        a.set(1, Start, true);
        a.set(2, Start, true);
        b.set(2, Start, true);
        b.set(1, Start, true);
        assert_eq!(a, b);
        b.set(1, Start, false);
        assert_ne!(a, b);
        assert_ne!(b, a);
        assert_eq!(HeldInputs::default(), HeldInputs::new());
    }

    #[test]
    fn overflow_is_counted_and_ignored() {
        let mut h = HeldInputs::new();
        for s in 0..MAX_HELD_SOURCES as u32 {
            h.set(s, Tilt, true);
        }
        assert_eq!(h.active_sources(), MAX_HELD_SOURCES);
        assert_eq!(h.set(9999, Start, true), None);
        assert_eq!(h.overflow(), 1);
        assert!(!h.is_held(Start));
        assert_eq!(h.release_all().as_slice(), &[Tilt]);
        assert_eq!(h.set(9999, Start, true), Some(true));
    }
}
