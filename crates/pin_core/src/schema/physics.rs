//! `physics_defaults.toml` (PLAN.md Appendix C) and per-table `[physics]` overrides.
//!
//! Units: tunables are SI except `magnet_remanence_tau_ms` (milliseconds), so
//! the values match the Appendix C table literally.

use super::{Manifest, SchemaError, manifest_io};
use crate::materials::{Material, MaterialPair, lookup_pair};
use serde::{Deserialize, Serialize};

/// A tunable: its default and the bounds Gate A1 may search within.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tunable {
    /// Default value.
    pub default: f32,
    /// Lower search bound.
    pub min: f32,
    /// Upper search bound.
    pub max: f32,
}

/// A feel window checked by Gate A1.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeelWindow {
    /// Lower limit.
    pub min: f32,
    /// Upper limit.
    pub max: f32,
}

impl FeelWindow {
    /// True when `v` lies within `[min, max]`.
    pub fn contains(&self, v: f32) -> bool {
        v >= self.min && v <= self.max
    }
}

/// The Appendix C tunables.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tunables {
    /// Flipper inertia (76 mm bat), kg·m².
    pub flipper_inertia: Tunable,
    /// Flipper power torque, N·m.
    pub flipper_power_torque: Tunable,
    /// Flipper hold torque, N·m.
    pub flipper_hold_torque: Tunable,
    /// Flipper return-spring torque, N·m.
    pub flipper_return_spring_torque: Tunable,
    /// Bat end-stop restitution.
    pub flipper_stop_restitution: Tunable,
    /// Flipper rubber damping.
    pub flipper_rubber_damping: Tunable,
    /// Rolling resistance coefficient.
    pub rolling_resistance: Tunable,
    /// UV Glo-Ball Magnus coefficient, kg.
    pub globall_magnus_coeff: Tunable,
    /// Magnet remanence time constant τ, ms.
    pub magnet_remanence_tau_ms: Tunable,
}

/// The Appendix C feel windows.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeelWindows {
    /// Unloaded flipper stroke time (rest to EOS), ms.
    pub flipper_stroke_time_ms: FeelWindow,
    /// Exit speed of a full shot from a cradled standard ball, m/s.
    pub flipper_exit_speed_m_s: FeelWindow,
    /// Bumper kick exit speed, m/s.
    pub bumper_exit_speed_m_s: FeelWindow,
    /// Slingshot kick exit speed, m/s.
    pub sling_exit_speed_m_s: FeelWindow,
}

/// `physics_defaults.toml`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhysicsDefaults {
    /// Must be 1.
    pub schema_version: u32,
    /// Restitution and friction per unordered material pair.
    pub material_pairs: Vec<MaterialPair>,
    /// Tunables and search bounds.
    pub tunables: Tunables,
    /// Feel windows.
    pub feel_windows: FeelWindows,
}

impl Manifest for PhysicsDefaults {
    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn validate(&self) -> Result<(), SchemaError> {
        for (i, p) in self.material_pairs.iter().enumerate() {
            if self.material_pairs[..i]
                .iter()
                .any(|q| q.matches(p.materials[0], p.materials[1]))
            {
                return Err(SchemaError::Invalid(format!(
                    "duplicate material pair {}–{}",
                    p.materials[0].name(),
                    p.materials[1].name()
                )));
            }
        }
        Ok(())
    }
}

manifest_io!(PhysicsDefaults);

/// The `physics_defaults.toml` shipped at the repository root.
pub const DEFAULT_PHYSICS_TOML: &str = include_str!("../../../../physics_defaults.toml");

impl PhysicsDefaults {
    /// Parses the built-in [`DEFAULT_PHYSICS_TOML`].
    pub fn builtin() -> PhysicsDefaults {
        // The file is part of the build and covered by tests.
        PhysicsDefaults::from_toml_str(DEFAULT_PHYSICS_TOML)
            .expect("physics_defaults.toml shipped with the engine must parse")
    }

    /// The pair for `a`/`b` (either order).
    pub fn material_pair(&self, a: Material, b: Material) -> Option<&MaterialPair> {
        lookup_pair(&self.material_pairs, a, b)
    }

    /// Resolves the default parameters, applying a table's `[physics]` overrides.
    pub fn resolve(&self, overrides: Option<&PhysicsOverrides>) -> PhysicsParams {
        let o = overrides.copied().unwrap_or_default();
        let t = &self.tunables;
        PhysicsParams {
            flipper_inertia: o.flipper_inertia.unwrap_or(t.flipper_inertia.default),
            flipper_power_torque: o
                .flipper_power_torque
                .unwrap_or(t.flipper_power_torque.default),
            flipper_hold_torque: o
                .flipper_hold_torque
                .unwrap_or(t.flipper_hold_torque.default),
            flipper_return_spring_torque: o
                .flipper_return_spring_torque
                .unwrap_or(t.flipper_return_spring_torque.default),
            flipper_stop_restitution: o
                .flipper_stop_restitution
                .unwrap_or(t.flipper_stop_restitution.default),
            flipper_rubber_damping: o
                .flipper_rubber_damping
                .unwrap_or(t.flipper_rubber_damping.default),
            flipper_rubber_restitution: o.flipper_rubber_restitution.unwrap_or_else(|| {
                self.material_pair(Material::Steel, Material::Rubber)
                    .map_or(FALLBACK_RUBBER_RESTITUTION, |p| p.restitution)
            }),
            rolling_resistance: o
                .rolling_resistance
                .unwrap_or(t.rolling_resistance.default),
            globall_magnus_coeff: o
                .globall_magnus_coeff
                .unwrap_or(t.globall_magnus_coeff.default),
            magnet_remanence_tau_ms: o
                .magnet_remanence_tau_ms
                .unwrap_or(t.magnet_remanence_tau_ms.default),
        }
    }
}

/// Steel–rubber restitution used when the pair table lacks that pair (Appendix C).
pub const FALLBACK_RUBBER_RESTITUTION: f32 = 0.65;

/// A table's optional `[physics]` section: any tunable may be overridden.
#[derive(Copy, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhysicsOverrides {
    /// See [`Tunables::flipper_inertia`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flipper_inertia: Option<f32>,
    /// See [`Tunables::flipper_power_torque`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flipper_power_torque: Option<f32>,
    /// See [`Tunables::flipper_hold_torque`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flipper_hold_torque: Option<f32>,
    /// See [`Tunables::flipper_return_spring_torque`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flipper_return_spring_torque: Option<f32>,
    /// See [`Tunables::flipper_stop_restitution`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flipper_stop_restitution: Option<f32>,
    /// See [`Tunables::flipper_rubber_damping`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flipper_rubber_damping: Option<f32>,
    /// Flipper rubber restitution (default: the steel–rubber pair).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flipper_rubber_restitution: Option<f32>,
    /// See [`Tunables::rolling_resistance`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rolling_resistance: Option<f32>,
    /// See [`Tunables::globall_magnus_coeff`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub globall_magnus_coeff: Option<f32>,
    /// See [`Tunables::magnet_remanence_tau_ms`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub magnet_remanence_tau_ms: Option<f32>,
}

/// Resolved physics parameters (defaults with overrides applied).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PhysicsParams {
    /// kg·m².
    pub flipper_inertia: f32,
    /// N·m.
    pub flipper_power_torque: f32,
    /// N·m.
    pub flipper_hold_torque: f32,
    /// N·m.
    pub flipper_return_spring_torque: f32,
    /// Bat end-stop restitution.
    pub flipper_stop_restitution: f32,
    /// Rubber damping.
    pub flipper_rubber_damping: f32,
    /// Rubber restitution.
    pub flipper_rubber_restitution: f32,
    /// Rolling resistance coefficient.
    pub rolling_resistance: f32,
    /// kg.
    pub globall_magnus_coeff: f32,
    /// ms.
    pub magnet_remanence_tau_ms: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_matches_appendix_c() {
        let d = PhysicsDefaults::builtin();
        assert_eq!(d.material_pairs.len(), 9);
        let p = d.material_pair(Material::Rubber, Material::Steel).unwrap();
        assert_eq!((p.restitution, p.friction), (0.65, 0.80));
        let p = d.material_pair(Material::Resin, Material::Wood).unwrap();
        assert_eq!((p.restitution, p.friction), (0.30, 0.22));
        assert!(d.material_pair(Material::Acrylic, Material::Resin).is_none());
        let t = d.tunables;
        assert_eq!(
            (t.flipper_inertia.default, t.flipper_inertia.min, t.flipper_inertia.max),
            (1.0e-4, 0.5e-4, 3.0e-4)
        );
        assert_eq!(t.flipper_power_torque.default, 1.2);
        assert_eq!(t.flipper_hold_torque.default, 0.15);
        assert_eq!(t.flipper_return_spring_torque.default, 0.03);
        assert_eq!(t.flipper_stop_restitution.default, 0.15);
        assert_eq!(t.flipper_rubber_damping.default, 0.35);
        assert_eq!(t.rolling_resistance.default, 0.002);
        assert_eq!(t.globall_magnus_coeff.default, 1.0e-5);
        assert_eq!(t.magnet_remanence_tau_ms.default, 60.0);
        for tun in [
            t.flipper_inertia,
            t.flipper_power_torque,
            t.flipper_hold_torque,
            t.flipper_return_spring_torque,
            t.flipper_stop_restitution,
            t.flipper_rubber_damping,
            t.rolling_resistance,
            t.globall_magnus_coeff,
            t.magnet_remanence_tau_ms,
        ] {
            assert!(tun.min <= tun.default && tun.default <= tun.max, "{tun:?}");
        }
        assert!(d.feel_windows.flipper_stroke_time_ms.contains(25.0));
        assert!(!d.feel_windows.flipper_stroke_time_ms.contains(36.0));
        assert_eq!(d.feel_windows.bumper_exit_speed_m_s, FeelWindow { min: 1.5, max: 4.0 });
        // The builtin profile constant agrees with the defaults file.
        assert_eq!(
            t.globall_magnus_coeff.default,
            crate::profiles::UV_GLOBALL_MAGNUS_COEFF
        );
    }

    #[test]
    fn resolve_with_and_without_overrides() {
        let d = PhysicsDefaults::builtin();
        let base = d.resolve(None);
        assert_eq!(base.flipper_rubber_damping, 0.35);
        assert_eq!(base.flipper_rubber_restitution, 0.65);
        let o = PhysicsOverrides {
            flipper_rubber_damping: Some(0.5),
            flipper_power_torque: Some(2.0),
            ..Default::default()
        };
        let r = d.resolve(Some(&o));
        assert_eq!(r.flipper_rubber_damping, 0.5);
        assert_eq!(r.flipper_power_torque, 2.0);
        assert_eq!(r.flipper_hold_torque, base.flipper_hold_torque);
        let mut no_pair = d.clone();
        no_pair.material_pairs.clear();
        assert_eq!(
            no_pair.resolve(None).flipper_rubber_restitution,
            FALLBACK_RUBBER_RESTITUTION
        );
    }

    #[test]
    fn duplicate_pairs_rejected() {
        let mut s = DEFAULT_PHYSICS_TOML.to_owned();
        s.push_str(
            "\n[[material_pairs]]\nmaterials = [\"rubber\", \"steel\"]\nrestitution = 0.1\nfriction = 0.1\n",
        );
        assert!(matches!(
            PhysicsDefaults::from_toml_str(&s),
            Err(SchemaError::Invalid(_))
        ));
    }
}
