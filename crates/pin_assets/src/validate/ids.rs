//! Name-to-id interning tables.

use pin_core::schema::table::NameResolver;
use pin_core::{
    CoilId, DispenserId, DiverterId, FlipperId, LampId, MagnetId, PersistKey, ProfileId, SceneId,
    SoundId, SwitchId,
};

/// The kinds of interned names.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IdKind {
    /// Switches (declared `number`; cabinet switches included).
    Switch,
    /// Coils (declared `number`).
    Coil,
    /// Lamps (declared `number`).
    Lamp,
    /// Magnets (declared `number`).
    Magnet,
    /// Diverters: mechs of kind `diverter`, in declaration order.
    Diverter,
    /// Flippers, in declaration order.
    Flipper,
    /// Dispensers (declared `number`).
    Dispenser,
    /// Ball profiles: index in `BUILTIN_PROFILE_NAMES`.
    Profile,
    /// Sounds, in declaration order.
    Sound,
    /// Scenes, in declaration order.
    Scene,
    /// Persistent keys, in declaration order.
    PersistKey,
}

impl IdKind {
    /// Every kind.
    pub const ALL: [IdKind; 11] = [
        IdKind::Switch,
        IdKind::Coil,
        IdKind::Lamp,
        IdKind::Magnet,
        IdKind::Diverter,
        IdKind::Flipper,
        IdKind::Dispenser,
        IdKind::Profile,
        IdKind::Sound,
        IdKind::Scene,
        IdKind::PersistKey,
    ];

    /// Human-readable name used in error messages.
    pub const fn label(self) -> &'static str {
        match self {
            IdKind::Switch => "switch",
            IdKind::Coil => "coil",
            IdKind::Lamp => "lamp",
            IdKind::Magnet => "magnet",
            IdKind::Diverter => "diverter",
            IdKind::Flipper => "flipper",
            IdKind::Dispenser => "dispenser",
            IdKind::Profile => "profile",
            IdKind::Sound => "sound",
            IdKind::Scene => "scene",
            IdKind::PersistKey => "persist key",
        }
    }

    const fn slot(self) -> usize {
        self as usize
    }
}

/// Name → id for every kind, in declaration order (cabinet switches follow
/// the declared switches; a cabinet input whose number is also a declared
/// switch is an extra name for that switch).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableIds {
    tables: [Vec<(String, u16)>; 11],
}

macro_rules! lookup {
    ($($(#[$doc:meta])* $fn:ident => $kind:ident, $ty:ident;)*) => {$(
        $(#[$doc])*
        pub fn $fn(&self, name: &str) -> Option<$ty> {
            self.get(IdKind::$kind, name).map($ty)
        }
    )*};
}

impl TableIds {
    /// The `(name, id)` pairs of `kind`, in interning order.
    pub fn entries(&self, kind: IdKind) -> &[(String, u16)] {
        &self.tables[kind.slot()]
    }

    /// Raw id of `name` in `kind`.
    pub fn get(&self, kind: IdKind, name: &str) -> Option<u16> {
        self.entries(kind)
            .iter()
            .find(|(n, _)| n == name)
            .map(|&(_, id)| id)
    }

    /// The first name interned with `id` in `kind`.
    pub fn name(&self, kind: IdKind, id: u16) -> Option<&str> {
        self.entries(kind)
            .iter()
            .find(|&&(_, i)| i == id)
            .map(|(n, _)| n.as_str())
    }

    pub(crate) fn push(&mut self, kind: IdKind, name: &str, id: u16) {
        self.tables[kind.slot()].push((name.to_owned(), id));
    }

    lookup! {
        /// Switch id of `name`.
        switch => Switch, SwitchId;
        /// Coil id of `name`.
        coil => Coil, CoilId;
        /// Lamp id of `name`.
        lamp => Lamp, LampId;
        /// Magnet id of `name`.
        magnet => Magnet, MagnetId;
        /// Diverter id of `name`.
        diverter => Diverter, DiverterId;
        /// Flipper id of `name`.
        flipper => Flipper, FlipperId;
        /// Dispenser id of `name`.
        dispenser => Dispenser, DispenserId;
        /// Profile id of `name`.
        profile => Profile, ProfileId;
        /// Sound id of `name`.
        sound => Sound, SoundId;
        /// Scene id of `name`.
        scene => Scene, SceneId;
        /// Persistent key id of `name`.
        persist_key => PersistKey, PersistKey;
    }
}

impl NameResolver for TableIds {
    fn coil(&self, name: &str) -> Option<CoilId> {
        TableIds::coil(self, name)
    }

    fn switch(&self, name: &str) -> Option<SwitchId> {
        TableIds::switch(self, name)
    }
}

/// Levenshtein distance between `a` and `b`, by characters.
pub(crate) fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.chars().enumerate() {
        cur[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let substitute = prev[j] + usize::from(ca != cb);
            cur[j + 1] = substitute.min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        core::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// The candidate closest to `name` within edit distance 2, if any (ties go to
/// the earliest candidate).
pub(crate) fn suggest<'a>(name: &str, candidates: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    // Skip pathological inputs: the distance is quadratic in length.
    if name.len() > 256 {
        return None;
    }
    let mut best: Option<(usize, &str)> = None;
    for c in candidates {
        if c.len() > 256 || c.len().abs_diff(name.len()) > 2 {
            continue;
        }
        let d = edit_distance(name, c);
        if d <= 2 && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, c));
        }
    }
    best.map(|(_, c)| c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distances() {
        assert_eq!(edit_distance("", ""), 0);
        assert_eq!(edit_distance("abc", ""), 3);
        assert_eq!(edit_distance("", "ab"), 2);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("left_flipper_coi", "left_flipper_coil"), 1);
    }

    #[test]
    fn suggestions() {
        let c = ["alpha", "beta", "betta"];
        assert_eq!(suggest("bet", c.into_iter()), Some("beta"));
        assert_eq!(suggest("alpah", c.into_iter()), Some("alpha"));
        assert_eq!(suggest("zzzzz", c.into_iter()), None);
        let long = "x".repeat(300);
        assert_eq!(suggest(&long, c.into_iter()), None);
    }

    #[test]
    fn lookups() {
        let mut ids = TableIds::default();
        ids.push(IdKind::Switch, "a", 7);
        ids.push(IdKind::Switch, "b", 7);
        assert_eq!(ids.switch("a"), Some(SwitchId(7)));
        assert_eq!(ids.name(IdKind::Switch, 7), Some("a"));
        assert_eq!(ids.coil("a"), None);
        assert_eq!(NameResolver::switch(&ids, "b"), Some(SwitchId(7)));
        assert_eq!(NameResolver::coil(&ids, "b"), None);
        assert_eq!(IdKind::ALL.len(), 11);
        for k in IdKind::ALL {
            assert!(!k.label().is_empty());
            assert!(ids.entries(k).len() <= 2);
        }
    }
}
