//! Surface materials and the material-pair table (PLAN.md §3.2, Appendix C).
//!
//! Restitution and sliding friction for a contact come from the unordered pair
//! of materials involved; the pair table lives in `physics_defaults.toml` and is
//! looked up through [`PhysicsDefaults::material_pair`](crate::PhysicsDefaults::material_pair).

use serde::{Deserialize, Serialize};

/// A contact material. Ball materials (steel, ceramic, plastic, resin) and
/// table materials (rubber, wood, plastic, steel, acrylic).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Material {
    /// Steel balls, wire ramps, metal guides.
    Steel,
    /// Ceramic Powerball.
    Ceramic,
    /// Plastic (UV Glo-Ball, plastic ramps and guides).
    Plastic,
    /// Phenolic resin (jumbo XL ball).
    Resin,
    /// Rubber rings, flipper rubbers, posts.
    Rubber,
    /// Playfield wood.
    Wood,
    /// Clear acrylic decks.
    Acrylic,
}

impl Material {
    /// Every material, in declaration order.
    pub const ALL: [Material; 7] = [
        Material::Steel,
        Material::Ceramic,
        Material::Plastic,
        Material::Resin,
        Material::Rubber,
        Material::Wood,
        Material::Acrylic,
    ];

    /// The lower-case name used in TOML files.
    pub const fn name(self) -> &'static str {
        match self {
            Material::Steel => "steel",
            Material::Ceramic => "ceramic",
            Material::Plastic => "plastic",
            Material::Resin => "resin",
            Material::Rubber => "rubber",
            Material::Wood => "wood",
            Material::Acrylic => "acrylic",
        }
    }

    /// Parses a TOML material name.
    pub fn from_name(name: &str) -> Option<Material> {
        Material::ALL.into_iter().find(|m| m.name() == name)
    }
}

/// Contact coefficients for an unordered pair of materials.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaterialPair {
    /// The two materials; order is irrelevant.
    pub materials: [Material; 2],
    /// Coefficient of restitution.
    pub restitution: f32,
    /// Sliding friction coefficient μ.
    pub friction: f32,
}

impl MaterialPair {
    /// True when this pair describes contact between `a` and `b` (in either order).
    pub fn matches(&self, a: Material, b: Material) -> bool {
        (self.materials[0] == a && self.materials[1] == b)
            || (self.materials[0] == b && self.materials[1] == a)
    }
}

/// Finds the pair for `a`/`b` in `pairs`, in either order.
pub fn lookup_pair(pairs: &[MaterialPair], a: Material, b: Material) -> Option<&MaterialPair> {
    pairs.iter().find(|p| p.matches(a, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for m in Material::ALL {
            assert_eq!(Material::from_name(m.name()), Some(m));
        }
        assert_eq!(Material::from_name("granite"), None);
    }

    #[test]
    fn lookup_is_symmetric() {
        let pairs = [MaterialPair {
            materials: [Material::Steel, Material::Rubber],
            restitution: 0.65,
            friction: 0.8,
        }];
        assert_eq!(
            lookup_pair(&pairs, Material::Rubber, Material::Steel).map(|p| p.restitution),
            Some(0.65)
        );
        assert!(lookup_pair(&pairs, Material::Steel, Material::Wood).is_none());
    }
}
