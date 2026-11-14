//! Black-box spec tests for `pin_core::ids` (PLAN.md §3.1).

use pin_core::ids::{
    CoilId, DispenserId, DiverterId, FlipperId, LampId, MagnetId, PersistKey, ProfileId, SceneId,
    SoundId, SwitchId,
};
use std::collections::{BTreeSet, HashSet};
use std::fmt::Debug;
use std::hash::Hash;

fn assert_id_traits<T: Copy + Eq + Ord + Hash + Debug + Send + Sync + 'static>() {}

#[test]
fn all_eleven_ids_have_the_required_traits() {
    assert_id_traits::<SwitchId>();
    assert_id_traits::<CoilId>();
    assert_id_traits::<LampId>();
    assert_id_traits::<MagnetId>();
    assert_id_traits::<DiverterId>();
    assert_id_traits::<FlipperId>();
    assert_id_traits::<DispenserId>();
    assert_id_traits::<ProfileId>();
    assert_id_traits::<SoundId>();
    assert_id_traits::<SceneId>();
    assert_id_traits::<PersistKey>();
}

#[test]
fn ids_are_exactly_u16_sized() {
    assert_eq!(size_of::<SwitchId>(), 2);
    assert_eq!(size_of::<CoilId>(), 2);
    assert_eq!(size_of::<LampId>(), 2);
    assert_eq!(size_of::<MagnetId>(), 2);
    assert_eq!(size_of::<DiverterId>(), 2);
    assert_eq!(size_of::<FlipperId>(), 2);
    assert_eq!(size_of::<DispenserId>(), 2);
    assert_eq!(size_of::<ProfileId>(), 2);
    assert_eq!(size_of::<SoundId>(), 2);
    assert_eq!(size_of::<SceneId>(), 2);
    assert_eq!(size_of::<PersistKey>(), 2);
}

/// Exercises the public `u16` field, Copy, Eq, Ord, Hash and Debug for one type.
macro_rules! id_behaviour {
    ($name:ident, $ty:ident) => {
        #[test]
        fn $name() {
            let raw: u16 = 513;
            let a = $ty(raw);
            let b = a; // Copy
            assert_eq!(a.0, raw);
            assert_eq!(a, b);
            assert_ne!($ty(1), $ty(2));
            assert_eq!($ty(u16::MAX).0, u16::MAX);
            assert_eq!($ty(0).0, 0);

            // Ord follows the wrapped number.
            assert!($ty(1) < $ty(2));
            assert!($ty(300) > $ty(299));
            assert!($ty(0) < $ty(u16::MAX));
            assert_eq!($ty(7).cmp(&$ty(7)), std::cmp::Ordering::Equal);
            let sorted: Vec<u16> = [$ty(9), $ty(256), $ty(3), $ty(255)]
                .into_iter()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .map(|id| id.0)
                .collect();
            assert_eq!(sorted, vec![3, 9, 255, 256]);

            // Hash is consistent with Eq.
            let mut set = HashSet::new();
            assert!(set.insert($ty(5)));
            assert!(!set.insert($ty(5)));
            assert!(set.insert($ty(6)));
            assert!(set.contains(&$ty(6)));
            assert_eq!(set.len(), 2);

            // The field is public and writable.
            let mut c = a;
            c.0 += 1;
            assert_eq!(c.0, raw + 1);
            assert_ne!(c, a);

            assert!(!format!("{a:?}").is_empty());
        }
    };
}

id_behaviour!(switch_id, SwitchId);
id_behaviour!(coil_id, CoilId);
id_behaviour!(lamp_id, LampId);
id_behaviour!(magnet_id, MagnetId);
id_behaviour!(diverter_id, DiverterId);
id_behaviour!(flipper_id, FlipperId);
id_behaviour!(dispenser_id, DispenserId);
id_behaviour!(profile_id, ProfileId);
id_behaviour!(sound_id, SoundId);
id_behaviour!(scene_id, SceneId);
id_behaviour!(persist_key, PersistKey);
