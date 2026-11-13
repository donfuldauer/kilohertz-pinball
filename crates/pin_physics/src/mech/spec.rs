//! Mech input ([`MechSpec`]), errors and the resolved SI parameters of each
//! kind (defaults from `physics_defaults.toml` `[mechs.*]`, CR-004, with the
//! per-mech `[[mechs]]` overrides applied).

use core::fmt;

use glam::Vec3;
use pin_core::ids::{CoilId, SwitchId};
use pin_core::schema::MechParams;
use pin_core::schema::table::{MechDef, MechKind};
use pin_core::{Material, PhysicsDefaults};

/// One mech given to [`MechSet::new`](super::MechSet::new): a `[[mechs]]`
/// entry with its names resolved to ids and its position in metres.
///
/// * `kind`, `coil`, `switch`: as in the manifest (`pin_assets`'
///   `MechSi` carries exactly these; [`MechSpec::from_def`] builds one from
///   a raw [`MechDef`]).
/// * `position`: world position in metres, in the frame the ball solver
///   runs in, whose `+z` is the playfield normal (the playfield is the
///   `z = 0` plane of the mech; what the point is depends on the kind, see
///   the [module documentation](super)).
/// * `params`: the kind-specific overrides exactly as written in the
///   `[[mechs]]` entry (table units: lengths mm, angles degrees, masses g,
///   the rest SI with the unit in the field name). `None` fields take the
///   `[mechs.<kind>]` default of the [`PhysicsDefaults`] given to
///   [`MechSet::new`](super::MechSet::new).
///
/// ```
/// use glam::Vec3;
/// use pin_core::ids::{CoilId, SwitchId};
/// use pin_core::schema::table::MechKind;
/// use pin_physics::mech::MechSpec;
///
/// let mut bumper = MechSpec::new(MechKind::Bumper, Vec3::new(0.1, 0.3, 0.0))
///     .with_coil(CoilId(5))
///     .with_switch(SwitchId(40));
/// bumper.params.radius = Some(28.0); // mm
/// ```
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MechSpec {
    /// Kind.
    pub kind: MechKind,
    /// Driving coil (bumper/sling/kickback kick, drop-target bank reset,
    /// VUK eject, auto-plunger).
    pub coil: Option<CoilId>,
    /// Associated switch (ring, sling leaf, target, spinner, scoop, shooter
    /// lane).
    pub switch: Option<SwitchId>,
    /// Position, metres (see the kind's table in the module docs).
    pub position: Vec3,
    /// Per-mech overrides in table units.
    pub params: MechParams,
}

impl MechSpec {
    /// A mech of `kind` at `position` (m) with no coil, no switch and all
    /// parameters at their defaults.
    #[must_use]
    pub fn new(kind: MechKind, position: Vec3) -> Self {
        Self {
            kind,
            coil: None,
            switch: None,
            position,
            params: MechParams::default(),
        }
    }

    /// Sets the coil.
    #[must_use]
    pub fn with_coil(mut self, coil: CoilId) -> Self {
        self.coil = Some(coil);
        self
    }

    /// Sets the switch.
    #[must_use]
    pub fn with_switch(mut self, switch: SwitchId) -> Self {
        self.switch = Some(switch);
        self
    }

    /// Sets the overrides.
    #[must_use]
    pub fn with_params(mut self, params: MechParams) -> Self {
        self.params = params;
        self
    }

    /// From a raw manifest entry: `coil`/`switch` are its resolved names,
    /// the position is converted from mm (missing: the origin), the
    /// overrides are [`MechDef::params`].
    #[must_use]
    pub fn from_def(def: &MechDef, coil: Option<CoilId>, switch: Option<SwitchId>) -> Self {
        let p = def.position.unwrap_or([0.0; 3]);
        Self {
            kind: def.kind,
            coil,
            switch,
            position: Vec3::new(p[0], p[1], p[2]) * MM,
            params: def.params(),
        }
    }
}

/// Metres per millimetre.
pub(crate) const MM: f32 = 1.0e-3;

/// Why [`MechSet::new`](super::MechSet::new) rejected a spec.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MechError {
    /// Mech `mech` (index in the input) has a non-finite position.
    BadPosition {
        /// Index in the input slice.
        mech: usize,
    },
    /// Parameter `name` of mech `mech` is out of range (non-finite, not
    /// positive where a size or mass is needed, negative where a speed,
    /// force, spring or damping is needed, a layer above 7, or a pull depth
    /// outside 0..1).
    BadParameter {
        /// Index in the input slice.
        mech: usize,
        /// Parameter key (as in `[[mechs]]`).
        name: &'static str,
    },
    /// More than [`MAX_MECHS`](super::MAX_MECHS) mechs.
    TooMany,
}

impl fmt::Display for MechError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadPosition { mech } => write!(f, "mech {mech}: non-finite position"),
            Self::BadParameter { mech, name } => write!(f, "mech {mech}: bad parameter `{name}`"),
            Self::TooMany => write!(f, "too many mechs (max {})", super::MAX_MECHS),
        }
    }
}

impl std::error::Error for MechError {}

/// Parameters common to every kind.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct Common {
    /// Unit direction in the playfield plane (from `angle_deg`, default 90°).
    pub dir: Vec3,
    /// Collision layer index.
    pub layer: u8,
}

/// Resolved bumper parameters (SI).
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct BumperP {
    pub radius: f32,
    pub height: f32,
    pub kick_speed: f32,
    pub kick_reach: f32,
}

/// Resolved slider-kicker parameters (sling, kickback, plunger), SI.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct SliderP {
    pub length: f32,
    pub height: f32,
    pub thickness: f32,
    pub mass: f32,
    pub force: f32,
    pub stroke: f32,
    pub spring_k: f32,
    pub auto_pull: f32,
}

/// Resolved drop-target parameters (SI).
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct DropP {
    pub length: f32,
    pub height: f32,
    pub thickness: f32,
    pub threshold: f32,
    pub rise_speed: f32,
}

/// Resolved spinner parameters (SI).
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct SpinnerP {
    pub width: f32,
    pub flap: f32,
    pub pivot_height: f32,
    pub thickness: f32,
    pub inertia: f32,
    pub damping: f32,
}

/// Resolved VUK parameters (SI).
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct VukP {
    pub radius: f32,
    pub eject_speed: f32,
    pub kick_speed: f32,
    pub exit_layer: u8,
    pub exit_offset: Vec3,
}

/// Resolved parameters of one mech.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) enum KindP {
    Bumper(BumperP),
    Sling(SliderP),
    Kickback(SliderP),
    Plunger(SliderP),
    Drop(DropP),
    Spinner(SpinnerP),
    Vuk(VukP),
    /// Diverters, gates and Newton balls: not simulated by 2.4.
    Inert,
}

/// Body material of each kind (for the material-pair table).
pub(crate) fn material(kind: MechKind) -> Material {
    match kind {
        MechKind::Bumper | MechKind::Spinner => Material::Steel,
        MechKind::DropTarget => Material::Plastic,
        _ => Material::Rubber,
    }
}

struct Check {
    mech: usize,
}

impl Check {
    fn positive(&self, name: &'static str, v: f32) -> Result<f32, MechError> {
        if v.is_finite() && v > 0.0 {
            Ok(v)
        } else {
            Err(MechError::BadParameter {
                mech: self.mech,
                name,
            })
        }
    }

    fn non_negative(&self, name: &'static str, v: f32) -> Result<f32, MechError> {
        if v.is_finite() && v >= 0.0 {
            Ok(v)
        } else {
            Err(MechError::BadParameter {
                mech: self.mech,
                name,
            })
        }
    }

    fn layer(&self, name: &'static str, v: u8) -> Result<u8, MechError> {
        if v <= 7 {
            Ok(v)
        } else {
            Err(MechError::BadParameter {
                mech: self.mech,
                name,
            })
        }
    }
}

/// `o` (table units, scaled by `scale` to SI) or the default `d`.
fn or(o: Option<f32>, scale: f32, d: f32) -> f32 {
    o.map_or(d, |v| v * scale)
}

/// Resolves and validates the parameters of `spec` (input index `mech`).
pub(crate) fn resolve(
    mech: usize,
    spec: &MechSpec,
    d: &PhysicsDefaults,
) -> Result<(Common, KindP), MechError> {
    let c = Check { mech };
    if !spec.position.is_finite() {
        return Err(MechError::BadPosition { mech });
    }
    let o = &spec.params;
    let deg = o.angle_deg.unwrap_or(90.0);
    if !deg.is_finite() {
        return Err(MechError::BadParameter {
            mech,
            name: "angle_deg",
        });
    }
    let a = deg.to_radians();
    let dir = Vec3::new(libm::cosf(a), libm::sinf(a), 0.0);
    let common = Common {
        dir,
        layer: c.layer("layer", o.layer.unwrap_or(0))?,
    };
    let m = &d.mechs;
    let slider = |k: &pin_core::schema::physics::KickerDefaults| -> Result<SliderP, MechError> {
        Ok(SliderP {
            length: c.positive("length", or(o.length, MM, k.length))?,
            height: c.positive("height", or(o.height, MM, k.height))?,
            thickness: c.positive("thickness", or(o.thickness, MM, k.thickness))?,
            mass: c.positive("mass_g", or(o.mass_g, 1.0e-3, k.mass))?,
            force: c.non_negative("force_n", or(o.force_n, 1.0, k.force))?,
            stroke: c.positive("stroke", or(o.stroke, MM, k.stroke))?,
            spring_k: c.non_negative("spring_k_n_m", or(o.spring_k_n_m, 1.0, k.spring_k))?,
            auto_pull: 0.0,
        })
    };
    let kind = match spec.kind {
        MechKind::Bumper => {
            let b = &m.bumper;
            KindP::Bumper(BumperP {
                radius: c.positive("radius", or(o.radius, MM, b.radius))?,
                height: c.positive("height", or(o.height, MM, b.height))?,
                kick_speed: c.non_negative("kick_speed_m_s", or(o.kick_speed_m_s, 1.0, b.kick_speed))?,
                kick_reach: c.non_negative("kick_reach", or(o.kick_reach, MM, b.kick_reach))?,
            })
        }
        MechKind::Sling => KindP::Sling(slider(&m.sling)?),
        MechKind::Kickback => KindP::Kickback(slider(&m.kickback)?),
        MechKind::Plunger => {
            let p = &m.plunger;
            let auto_pull = or(o.auto_pull, 1.0, p.auto_pull);
            if !(0.0..=1.0).contains(&auto_pull) {
                return Err(MechError::BadParameter {
                    mech,
                    name: "auto_pull",
                });
            }
            KindP::Plunger(SliderP {
                length: c.positive("length", or(o.length, MM, p.length))?,
                height: c.positive("height", or(o.height, MM, p.height))?,
                thickness: c.positive("thickness", or(o.thickness, MM, p.thickness))?,
                mass: c.positive("mass_g", or(o.mass_g, 1.0e-3, p.mass))?,
                force: 0.0,
                stroke: c.positive("stroke", or(o.stroke, MM, p.stroke))?,
                spring_k: c.non_negative("spring_k_n_m", or(o.spring_k_n_m, 1.0, p.spring_k))?,
                auto_pull,
            })
        }
        MechKind::DropTarget => {
            let t = &m.drop_target;
            KindP::Drop(DropP {
                length: c.positive("length", or(o.length, MM, t.length))?,
                height: c.positive("height", or(o.height, MM, t.height))?,
                thickness: c.positive("thickness", or(o.thickness, MM, t.thickness))?,
                threshold: c.non_negative(
                    "threshold_speed_m_s",
                    or(o.threshold_speed_m_s, 1.0, t.threshold_speed),
                )?,
                rise_speed: c.positive("kick_speed_m_s", or(o.kick_speed_m_s, 1.0, t.kick_speed))?,
            })
        }
        MechKind::Spinner => {
            let s = &m.spinner;
            let p = SpinnerP {
                width: c.positive("length", or(o.length, MM, s.length))?,
                flap: c.positive("radius", or(o.radius, MM, s.radius))?,
                pivot_height: c.positive("height", or(o.height, MM, s.height))?,
                thickness: c.positive("thickness", or(o.thickness, MM, s.thickness))?,
                inertia: c.positive("inertia_kg_m2", or(o.inertia_kg_m2, 1.0, s.inertia))?,
                damping: c.non_negative("damping_per_s", or(o.damping_per_s, 1.0, s.damping))?,
            };
            KindP::Spinner(p)
        }
        MechKind::Vuk => {
            let v = &m.vuk;
            let off = o.exit_offset.map_or(Vec3::from_array(v.exit_offset), |a| {
                Vec3::from_array(a) * MM
            });
            if !off.is_finite() {
                return Err(MechError::BadParameter {
                    mech,
                    name: "exit_offset",
                });
            }
            KindP::Vuk(VukP {
                radius: c.positive("radius", or(o.radius, MM, v.radius))?,
                eject_speed: c.non_negative(
                    "eject_speed_m_s",
                    or(o.eject_speed_m_s, 1.0, v.eject_speed),
                )?,
                kick_speed: c.non_negative("kick_speed_m_s", or(o.kick_speed_m_s, 1.0, v.kick_speed))?,
                exit_layer: c.layer("exit_layer", o.exit_layer.unwrap_or(v.exit_layer))?,
                exit_offset: off,
            })
        }
        MechKind::Diverter | MechKind::NewtonBall | MechKind::Gate => KindP::Inert,
    };
    Ok((common, kind))
}
