//! Black-box spec tests for `pin_core::profiles` (PLAN.md §3.2, Appendix C).

use pin_core::profiles::{BallProfile, builtin_profile};

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-5 * a.abs().max(b.abs()).max(1e-12)
}

struct Row {
    name: &'static str,
    diameter_mm: f32,
    mass_g: f32,
    restitution: f32,
    coupling: f32,
    remanence: f32,
    conductive: bool,
    magnus: f32,
}

/// The table of §3.2, with uv_globall's magnus_coeff from Appendix C.
const TABLE: [Row; 7] = [
    Row {
        name: "standard_steel",
        diameter_mm: 27.0,
        mass_g: 80.0,
        restitution: 0.65,
        coupling: 1.0,
        remanence: 0.05,
        conductive: true,
        magnus: 0.0,
    },
    Row {
        name: "low_rem_steel",
        diameter_mm: 27.0,
        mass_g: 80.0,
        restitution: 0.65,
        coupling: 1.0,
        remanence: 0.0,
        conductive: true,
        magnus: 0.0,
    },
    Row {
        name: "ceramic_powerball",
        diameter_mm: 27.0,
        mass_g: 65.0,
        restitution: 0.78,
        coupling: 0.0,
        remanence: 0.0,
        conductive: false,
        magnus: 0.0,
    },
    Row {
        name: "uv_globall",
        diameter_mm: 27.0,
        mass_g: 24.0,
        restitution: 0.84,
        coupling: 0.0,
        remanence: 0.0,
        conductive: false,
        magnus: 1.0e-5,
    },
    Row {
        name: "mini_steel",
        diameter_mm: 15.88,
        mass_g: 16.5,
        restitution: 0.65,
        coupling: 1.0,
        remanence: 0.05,
        conductive: true,
        magnus: 0.0,
    },
    Row {
        name: "jumbo_steel",
        diameter_mm: 31.75,
        mass_g: 130.0,
        restitution: 0.60,
        coupling: 1.0,
        remanence: 0.05,
        conductive: true,
        magnus: 0.0,
    },
    Row {
        name: "jumbo_xl_resin",
        diameter_mm: 79.4,
        mass_g: 445.0,
        restitution: 0.80,
        coupling: 0.0,
        remanence: 0.0,
        conductive: false,
        magnus: 0.0,
    },
];

fn get(name: &str) -> BallProfile {
    builtin_profile(name).unwrap_or_else(|| panic!("builtin profile {name:?} missing"))
}

#[test]
fn all_seven_profiles_match_the_spec_table_in_si() {
    for row in &TABLE {
        let p = get(row.name);
        assert!(
            close(p.diameter_m, row.diameter_mm / 1000.0),
            "{}: diameter_m = {}",
            row.name,
            p.diameter_m
        );
        assert!(
            close(p.mass_kg, row.mass_g / 1000.0),
            "{}: mass_kg = {}",
            row.name,
            p.mass_kg
        );
        assert!(
            close(p.restitution, row.restitution),
            "{}: restitution = {}",
            row.name,
            p.restitution
        );
        assert_eq!(p.magnetic_coupling, row.coupling, "{}: coupling", row.name);
        assert!(
            close(p.remanence, row.remanence) || (row.remanence == 0.0 && p.remanence == 0.0),
            "{}: remanence = {}",
            row.name,
            p.remanence
        );
        assert_eq!(p.conductive, row.conductive, "{}: conductive", row.name);
        if row.magnus == 0.0 {
            assert_eq!(p.magnus_coeff, 0.0, "{}: magnus_coeff", row.name);
        } else {
            assert!(
                close(p.magnus_coeff, row.magnus),
                "{}: magnus_coeff = {}",
                row.name,
                p.magnus_coeff
            );
        }
    }
}

#[test]
fn headline_values() {
    assert!(close(get("standard_steel").diameter_m, 0.027));
    assert!(close(get("standard_steel").mass_kg, 0.080));
    assert!(close(get("standard_steel").remanence, 0.05));
    assert_eq!(get("low_rem_steel").remanence, 0.0);
    assert!(close(get("uv_globall").magnus_coeff, 1.0e-5));
    assert!(close(get("jumbo_xl_resin").diameter_m, 0.0794));
    assert!(close(get("jumbo_xl_resin").mass_kg, 0.445));
}

#[test]
fn zero_coupling_profiles_have_exactly_zero_coupling_and_remanence() {
    // §3.2: "When c = 0 the force is exactly zero" relies on exact zeros.
    for name in ["ceramic_powerball", "uv_globall", "jumbo_xl_resin"] {
        let p = get(name);
        assert_eq!(p.magnetic_coupling.to_bits(), 0.0f32.to_bits(), "{name}");
        assert_eq!(p.remanence, 0.0, "{name}");
        assert!(!p.conductive, "{name}");
    }
}

#[test]
fn only_uv_globall_has_magnus() {
    for row in &TABLE {
        let p = get(row.name);
        assert_eq!(
            p.magnus_coeff != 0.0,
            row.name == "uv_globall",
            "{}",
            row.name
        );
    }
}

#[test]
fn moment_of_inertia_is_two_fifths_m_r_squared() {
    for row in &TABLE {
        let p = get(row.name);
        let r = f64::from(p.diameter_m) / 2.0;
        let expect = 0.4 * f64::from(p.mass_kg) * r * r;
        let got = f64::from(p.moment_of_inertia());
        assert!(
            (got - expect).abs() <= 1e-5 * expect,
            "{}: I = {got}, expected {expect}",
            row.name
        );
        assert!(got > 0.0);
    }
    // Standard ball: 2/5 * 0.08 kg * (0.0135 m)^2 = 5.832e-6 kg m^2.
    let i = get("standard_steel").moment_of_inertia();
    assert!(close(i, 5.832e-6), "I = {i}");
}

#[test]
fn moment_of_inertia_follows_the_fields() {
    // The method must compute from the profile's own fields, not from a lookup.
    let mut p = get("standard_steel");
    p.mass_kg = 2.0;
    p.diameter_m = 1.0;
    assert!(close(p.moment_of_inertia(), 0.2));
    p.mass_kg = 4.0;
    assert!(close(p.moment_of_inertia(), 0.4));
    p.diameter_m = 2.0;
    assert!(close(p.moment_of_inertia(), 1.6));
}

#[test]
fn unknown_names_yield_none() {
    for name in [
        "",
        "steel",
        "Standard_Steel",
        "STANDARD_STEEL",
        "standard_steel ",
        " standard_steel",
        "standard-steel",
        "pbr_chrome_steel",
        "jumbo_xl_steel",
        "no_such_profile",
    ] {
        assert!(builtin_profile(name).is_none(), "{name:?} must be unknown");
    }
}

#[test]
fn lookup_is_deterministic() {
    for row in &TABLE {
        let a = get(row.name);
        let b = get(row.name);
        assert_eq!(a.diameter_m, b.diameter_m);
        assert_eq!(a.mass_kg, b.mass_kg);
        assert_eq!(a.restitution, b.restitution);
        assert_eq!(a.magnus_coeff, b.magnus_coeff);
    }
}
