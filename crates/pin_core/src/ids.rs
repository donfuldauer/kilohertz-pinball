//! Interned runtime identifiers (PLAN.md §3.1).
//!
//! Every name in a table manifest is interned to one of these `u16` newtypes at
//! load time; the sim thread only ever sees the numbers.

use serde::{Deserialize, Serialize};

macro_rules! id_newtype {
    ($($(#[$doc:meta])* $name:ident),* $(,)?) => {$(
        $(#[$doc])*
        #[derive(
            Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub u16);

        impl $name {
            /// The raw interned index.
            #[inline]
            pub const fn get(self) -> u16 {
                self.0
            }

            /// The raw index as a `usize`, for indexing fixed-size tables.
            #[inline]
            pub const fn index(self) -> usize {
                self.0 as usize
            }
        }

        impl From<u16> for $name {
            #[inline]
            fn from(v: u16) -> Self {
                Self(v)
            }
        }

        impl From<$name> for u16 {
            #[inline]
            fn from(v: $name) -> u16 {
                v.0
            }
        }
    )*};
}

id_newtype!(
    /// A playfield or cabinet switch.
    SwitchId,
    /// A coil (solenoid) driver.
    CoilId,
    /// A lamp (insert, GI string or flasher).
    LampId,
    /// An electromagnet.
    MagnetId,
    /// A diverter.
    DiverterId,
    /// A flipper.
    FlipperId,
    /// A ball dispenser (trough, power chamber, ...).
    DispenserId,
    /// A ball profile.
    ProfileId,
    /// A sound.
    SoundId,
    /// A display scene.
    SceneId,
    /// A persistent value slot (high scores, audits, settings).
    PersistKey,
);
