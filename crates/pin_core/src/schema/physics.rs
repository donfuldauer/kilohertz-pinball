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
    /// Kind-specific mech defaults (CR-004; optional section, defaults to
    /// [`MechDefaults::default`]).
    #[serde(default)]
    pub mechs: MechDefaults,
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
            rolling_resistance: o.rolling_resistance.unwrap_or(t.rolling_resistance.default),
            globall_magnus_coeff: o
                .globall_magnus_coeff
                .unwrap_or(t.globall_magnus_coeff.default),
            magnet_remanence_tau_ms: o
                .magnet_remanence_tau_ms
                .unwrap_or(t.magnet_remanence_tau_ms.default),
        }
    }
}

/// Default kind-specific mech parameters (CR-004): the `[mechs]` tables of
/// `physics_defaults.toml`, all SI. A table overrides them per mech through
/// the optional [`MechDef`](super::table::MechDef) fields. A file without
/// `[mechs]` gets [`MechDefaults::default`], which equals the shipped values.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MechDefaults {
    /// Pop bumpers.
    pub bumper: BumperDefaults,
    /// Slingshots.
    pub sling: KickerDefaults,
    /// Kickbacks.
    pub kickback: KickerDefaults,
    /// Drop targets.
    pub drop_target: DropTargetDefaults,
    /// Spinners.
    pub spinner: SpinnerDefaults,
    /// VUK scoops.
    pub vuk: VukDefaults,
    /// Plungers.
    pub plunger: PlungerDefaults,
    /// The cabinet tilt bob.
    pub tilt_bob: TiltBobDefaults,
}

impl Default for MechDefaults {
    fn default() -> Self {
        Self {
            bumper: BumperDefaults {
                radius: 0.030,
                height: 0.032,
                kick_speed: 1.7,
                kick_reach: 0.008,
            },
            sling: KickerDefaults {
                length: 0.10,
                height: 0.025,
                thickness: 0.010,
                mass: 0.06,
                force: 40.0,
                stroke: 0.010,
                spring_k: 200.0,
            },
            kickback: KickerDefaults {
                length: 0.025,
                height: 0.025,
                thickness: 0.012,
                mass: 0.06,
                force: 60.0,
                stroke: 0.020,
                spring_k: 200.0,
            },
            drop_target: DropTargetDefaults {
                length: 0.025,
                height: 0.032,
                thickness: 0.005,
                threshold_speed: 0.5,
                kick_speed: 0.5,
            },
            spinner: SpinnerDefaults {
                length: 0.040,
                radius: 0.028,
                height: 0.042,
                thickness: 0.002,
                inertia: 8.0e-7,
                damping: 0.8,
            },
            vuk: VukDefaults {
                radius: 0.012,
                eject_speed: 1.2,
                kick_speed: 0.3,
                exit_layer: 1,
                exit_offset: [0.0, 0.0, 0.06],
            },
            plunger: PlungerDefaults {
                length: 0.024,
                height: 0.024,
                thickness: 0.012,
                mass: 0.05,
                spring_k: 1000.0,
                stroke: 0.05,
                auto_pull: 0.8,
            },
            tilt_bob: TiltBobDefaults {
                length: 0.12,
                gap: 0.012,
                damping: 0.5,
                restitution: 0.3,
            },
        }
    }
}

/// Pop bumper (`[mechs.bumper]`).
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BumperDefaults {
    /// Ring radius (kinematic cylinder), m.
    pub radius: f32,
    /// Ring height above the playfield, m.
    pub height: f32,
    /// Ring speed while the coil is on, m/s.
    pub kick_speed: f32,
    /// Skirt: largest ball–ring gap the kick still catches, m.
    pub kick_reach: f32,
}

/// Coil-driven slider kicker (`[mechs.sling]`, `[mechs.kickback]`).
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KickerDefaults {
    /// Face width, m.
    pub length: f32,
    /// Height above the playfield, m.
    pub height: f32,
    /// Thickness along the kick axis, m.
    pub thickness: f32,
    /// Moving mass, kg.
    pub mass: f32,
    /// Coil force while the coil is on, N.
    pub force: f32,
    /// Travel from rest to the end stop, m.
    pub stroke: f32,
    /// Return spring, N/m.
    pub spring_k: f32,
}

/// Drop target (`[mechs.drop_target]`).
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DropTargetDefaults {
    /// Face width, m.
    pub length: f32,
    /// Height above the playfield when up, m.
    pub height: f32,
    /// Thickness, m.
    pub thickness: f32,
    /// Smallest normal impact speed that drops the target, m/s.
    pub threshold_speed: f32,
    /// Rise speed while the reset coil raises it, m/s.
    pub kick_speed: f32,
}

/// Spinner flap (`[mechs.spinner]`).
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpinnerDefaults {
    /// Flap width along the pivot axis, m.
    pub length: f32,
    /// Flap length from the pivot to its free edge, m.
    pub radius: f32,
    /// Pivot height above the playfield, m.
    pub height: f32,
    /// Flap thickness, m.
    pub thickness: f32,
    /// Moment of inertia about the pivot, kg·m².
    pub inertia: f32,
    /// Spin-down rate d (dω/dt = −d·ω), 1/s.
    pub damping: f32,
}

/// Vertical up-kicker scoop (`[mechs.vuk]`).
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VukDefaults {
    /// Capture radius around the scoop position, m.
    pub radius: f32,
    /// Vertical eject speed, m/s.
    pub eject_speed: f32,
    /// Horizontal eject speed along the mech angle, m/s.
    pub kick_speed: f32,
    /// Layer the ejected ball ends on.
    pub exit_layer: u8,
    /// Exit point relative to the scoop position, m.
    pub exit_offset: [f32; 3],
}

/// Spring plunger (`[mechs.plunger]`).
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlungerDefaults {
    /// Tip width, m.
    pub length: f32,
    /// Tip height above the playfield, m.
    pub height: f32,
    /// Tip thickness along the launch axis, m.
    pub thickness: f32,
    /// Rod and tip mass, kg.
    pub mass: f32,
    /// Main spring constant (F = −k·x), N/m.
    pub spring_k: f32,
    /// Pull travel, m.
    pub stroke: f32,
    /// Pull depth (0..1) the auto-plunger coil releases from.
    pub auto_pull: f32,
}

/// Cabinet tilt bob (`[mechs.tilt_bob]`).
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TiltBobDefaults {
    /// Pendulum length (pivot to bob centre), m.
    pub length: f32,
    /// Horizontal bob travel before it touches the contact ring, m.
    pub gap: f32,
    /// Viscous damping rate of the swing, 1/s.
    pub damping: f32,
    /// Bob–ring restitution.
    pub restitution: f32,
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
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn feel_window_contains_matches_bounds(min in -1.0e3f32..1.0e3, max in -1.0e3f32..1.0e3,
                                               v in -2.0e3f32..2.0e3) {
            let w = FeelWindow { min, max };
            prop_assert_eq!(w.contains(v), min <= v && v <= max);
            prop_assert_eq!(w.contains(min), min <= max);
            prop_assert_eq!(w.contains(max), min <= max);
            prop_assert!(!w.contains(f32::NAN));
        }
    }

    #[test]
    fn builtin_matches_appendix_c() {
        let d = PhysicsDefaults::builtin();
        assert_eq!(d.material_pairs.len(), 9);
        let p = d.material_pair(Material::Rubber, Material::Steel).unwrap();
        assert_eq!((p.restitution, p.friction), (0.65, 0.80));
        let p = d.material_pair(Material::Resin, Material::Wood).unwrap();
        assert_eq!((p.restitution, p.friction), (0.30, 0.22));
        assert!(
            d.material_pair(Material::Acrylic, Material::Resin)
                .is_none()
        );
        let t = d.tunables;
        assert_eq!(
            (
                t.flipper_inertia.default,
                t.flipper_inertia.min,
                t.flipper_inertia.max
            ),
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
        assert_eq!(
            d.feel_windows.bumper_exit_speed_m_s,
            FeelWindow { min: 1.5, max: 4.0 }
        );
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
    fn mech_defaults_match_the_shipped_section() {
        let d = PhysicsDefaults::builtin();
        assert_eq!(d.mechs, MechDefaults::default());
        assert_eq!(d.mechs.bumper.kick_speed, 1.7);
        assert_eq!(d.mechs.vuk.exit_layer, 1);
        // The section is optional: a file without it gets the same defaults.
        let i = DEFAULT_PHYSICS_TOML.find("[mechs.").unwrap();
        let without = PhysicsDefaults::from_toml_str(&DEFAULT_PHYSICS_TOML[..i]).unwrap();
        assert_eq!(without.mechs, MechDefaults::default());
        assert_eq!(without.material_pairs, d.material_pairs);
        // Unknown keys inside a kind table are errors.
        let bad = DEFAULT_PHYSICS_TOML.replacen("kick_reach ", "kick_reachy ", 1);
        assert!(PhysicsDefaults::from_toml_str(&bad).is_err());
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
