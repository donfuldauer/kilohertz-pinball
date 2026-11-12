//! Data-driven ball profiles (PLAN.md §3.2, D14, D15). All fields are SI.

use crate::materials::Material;
use serde::{Deserialize, Serialize};

/// Physical and visual description of a ball type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BallProfile {
    /// Profile name (e.g. `standard_steel`), interned to a `ProfileId` at load.
    pub name: String,
    /// Diameter, metres.
    pub diameter_m: f32,
    /// Mass, kilograms.
    pub mass_kg: f32,
    /// Fallback restitution; the material-pair table takes precedence.
    pub restitution: f32,
    /// Magnetic coupling `c ∈ [0, 1]`; 0 means magnets exert exactly no force.
    pub magnetic_coupling: f32,
    /// Remanence `ρ ∈ [0, 1]` (Subphase 2.7).
    pub remanence: f32,
    /// Whether eddy sensors detect this ball (D12).
    pub conductive: bool,
    /// Magnus coefficient `S_m`, kg (Subphase 2.7).
    pub magnus_coeff: f32,
    /// Ball surface material, for material-pair lookup.
    pub material: Material,
    /// Render shader name.
    pub shader: String,
}

impl BallProfile {
    /// Radius, metres.
    #[inline]
    pub fn radius_m(&self) -> f32 {
        self.diameter_m * 0.5
    }

    /// Moment of inertia of a solid sphere about its centre, `I = 2/5 · m · r²` (kg·m²).
    #[inline]
    pub fn moment_of_inertia(&self) -> f32 {
        let r = self.radius_m();
        0.4 * self.mass_kg * r * r
    }
}

/// Default Magnus coefficient of the UV Glo-Ball (Appendix C), kg.
pub const UV_GLOBALL_MAGNUS_COEFF: f32 = 1.0e-5;

/// Names of the seven built-in profiles, in the §3.2 table order.
pub const BUILTIN_PROFILE_NAMES: [&str; 7] = [
    "standard_steel",
    "low_rem_steel",
    "ceramic_powerball",
    "uv_globall",
    "mini_steel",
    "jumbo_steel",
    "jumbo_xl_resin",
];

#[allow(clippy::too_many_arguments)]
fn profile(
    name: &str,
    diameter_m: f32,
    mass_kg: f32,
    restitution: f32,
    magnetic_coupling: f32,
    remanence: f32,
    conductive: bool,
    magnus_coeff: f32,
    material: Material,
    shader: &str,
) -> BallProfile {
    BallProfile {
        name: name.to_owned(),
        diameter_m,
        mass_kg,
        restitution,
        magnetic_coupling,
        remanence,
        conductive,
        magnus_coeff,
        material,
        shader: shader.to_owned(),
    }
}

/// Returns the built-in profile called `name` (§3.2), or `None`.
pub fn builtin_profile(name: &str) -> Option<BallProfile> {
    use Material::*;
    let p = match name {
        "standard_steel" => profile(
            name,
            0.027,
            0.080,
            0.65,
            1.0,
            0.05,
            true,
            0.0,
            Steel,
            "pbr_chrome_steel",
        ),
        "low_rem_steel" => profile(
            name,
            0.027,
            0.080,
            0.65,
            1.0,
            0.0,
            true,
            0.0,
            Steel,
            "pbr_chrome_steel",
        ),
        "ceramic_powerball" => profile(
            name,
            0.027,
            0.065,
            0.78,
            0.0,
            0.0,
            false,
            0.0,
            Ceramic,
            "pbr_ceramic_pearl",
        ),
        "uv_globall" => profile(
            name,
            0.027,
            0.024,
            0.84,
            0.0,
            0.0,
            false,
            UV_GLOBALL_MAGNUS_COEFF,
            Plastic,
            "pbr_uv_emissive",
        ),
        "mini_steel" => profile(
            name,
            0.01588,
            0.0165,
            0.65,
            1.0,
            0.05,
            true,
            0.0,
            Steel,
            "pbr_chrome_steel",
        ),
        "jumbo_steel" => profile(
            name,
            0.03175,
            0.130,
            0.60,
            1.0,
            0.05,
            true,
            0.0,
            Steel,
            "pbr_chrome_steel",
        ),
        "jumbo_xl_resin" => profile(
            name,
            0.0794,
            0.445,
            0.80,
            0.0,
            0.0,
            false,
            0.0,
            Resin,
            "pbr_ceramic_pearl",
        ),
        _ => return None,
    };
    Some(p)
}

/// All seven built-in profiles in §3.2 order.
pub fn builtin_profiles() -> impl Iterator<Item = BallProfile> {
    BUILTIN_PROFILE_NAMES
        .into_iter()
        .filter_map(builtin_profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn all_builtins_exist_and_match_their_names() {
        let all: Vec<_> = builtin_profiles().collect();
        assert_eq!(all.len(), 7);
        for (p, n) in all.iter().zip(BUILTIN_PROFILE_NAMES) {
            assert_eq!(p.name, n);
        }
        assert!(builtin_profile("tungsten").is_none());
    }

    #[test]
    fn table_values() {
        let s = builtin_profile("standard_steel").unwrap();
        assert_eq!((s.diameter_m, s.mass_kg, s.remanence), (0.027, 0.08, 0.05));
        assert!(s.conductive);
        let g = builtin_profile("uv_globall").unwrap();
        assert_eq!(g.magnus_coeff, 1.0e-5);
        assert_eq!(g.magnetic_coupling, 0.0);
        let xl = builtin_profile("jumbo_xl_resin").unwrap();
        assert_eq!((xl.diameter_m, xl.mass_kg), (0.0794, 0.445));
        // Non-magnetic profiles are exactly zero, so magnet force is exactly zero.
        for p in builtin_profiles() {
            if !p.conductive {
                assert_eq!(p.magnetic_coupling, 0.0);
            }
        }
    }

    #[test]
    fn moment_of_inertia_of_standard_ball() {
        let s = builtin_profile("standard_steel").unwrap();
        let expected = 0.4 * 0.08 * 0.0135 * 0.0135;
        assert!((s.moment_of_inertia() - expected).abs() < 1e-12);
        assert_eq!(s.radius_m(), 0.0135);
    }

    proptest! {
        #[test]
        fn inertia_formula(d in 0.001f32..0.2, m in 0.001f32..5.0) {
            let p = BallProfile { diameter_m: d, mass_kg: m, ..builtin_profile("standard_steel").unwrap() };
            let r = d / 2.0;
            let expected = 2.0 / 5.0 * m * r * r;
            prop_assert!((p.moment_of_inertia() - expected).abs() <= 1e-6 * expected.max(1e-12));
            prop_assert!(p.moment_of_inertia() > 0.0);
        }
    }
}
