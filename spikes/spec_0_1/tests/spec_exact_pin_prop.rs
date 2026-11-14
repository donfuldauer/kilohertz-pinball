//! Property tests for the exact-pin validator used by the versions.toml checks.

use proptest::prelude::*;
use spec_0_1::{is_exact_pin, pin_triple};

proptest! {
    #[test]
    fn arbitrary_strings_never_panic(s in any::<String>()) {
        let accepted = is_exact_pin(&s);
        // Accepted strings always decompose unless a component overflows u64.
        if !accepted {
            prop_assert!(pin_triple(&s).is_none());
        }
    }

    #[test]
    fn version_shaped_noise_never_panics(s in r"[=^~<>*0-9a-z.+\- ]{0,40}") {
        let _ = is_exact_pin(&s);
        let _ = pin_triple(&s);
    }

    #[test]
    fn plain_exact_pins_are_accepted(maj in any::<u64>(), min in any::<u64>(), patch in any::<u64>()) {
        let pin = format!("={maj}.{min}.{patch}");
        prop_assert!(is_exact_pin(&pin), "{pin} rejected");
        prop_assert_eq!(pin_triple(&pin), Some((maj, min, patch)));
    }

    #[test]
    fn prerelease_exact_pins_are_accepted(
        maj in 0u32..1000, min in 0u32..1000, patch in 0u32..1000,
        pre in r"[a-z][a-z0-9]{0,6}(\.(0|[1-9][0-9]{0,3}|[0-9]{0,2}[a-z][0-9a-z]{0,2})){0,2}",
    ) {
        let pin = format!("={maj}.{min}.{patch}-{pre}");
        prop_assert!(is_exact_pin(&pin), "{pin} rejected");
    }

    #[test]
    fn prerelease_numeric_leading_zeros_are_rejected(
        maj in 0u32..1000, min in 0u32..1000, patch in 0u32..1000,
        lead in r"([a-z][a-z0-9]{0,6}\.)?", digits in r"0[0-9]{1,4}",
    ) {
        let pin = format!("={maj}.{min}.{patch}-{lead}{digits}");
        prop_assert!(!is_exact_pin(&pin), "{pin} accepted");
    }

    #[test]
    fn strings_without_equals_are_rejected(s in "[^=]*") {
        prop_assert!(!is_exact_pin(&s), "{s:?} accepted without '='");
    }

    #[test]
    fn bare_versions_and_other_operators_are_rejected(
        op in prop::sample::select(vec!["", "^", "~", ">=", "<=", ">", "<", "v", " =", "=="]),
        maj in any::<u32>(), min in any::<u32>(), patch in any::<u32>(),
    ) {
        let req = format!("{op}{maj}.{min}.{patch}");
        prop_assert!(!is_exact_pin(&req), "{req:?} accepted");
    }

    #[test]
    fn partial_versions_are_rejected(maj in any::<u32>(), min in any::<u32>()) {
        let major_only = format!("={maj}");
        let major_minor = format!("={maj}.{min}");
        let wildcard = format!("={maj}.{min}.*");
        prop_assert!(!is_exact_pin(&major_only));
        prop_assert!(!is_exact_pin(&major_minor));
        prop_assert!(!is_exact_pin(&wildcard));
    }

    #[test]
    fn surrounding_text_is_rejected(
        maj in any::<u16>(), min in any::<u16>(), patch in any::<u16>(),
        junk in r"[ \t\n,]{1,3}",
    ) {
        let pin = format!("={maj}.{min}.{patch}");
        let trailing = format!("{pin}{junk}");
        let leading = format!("{junk}{pin}");
        prop_assert!(!is_exact_pin(&trailing));
        prop_assert!(!is_exact_pin(&leading));
    }
}
