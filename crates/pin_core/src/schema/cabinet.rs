//! `cabinet.toml` (SPEC Appendix A.4, CR-012): the cabinet as data.
//!
//! REQ-BUILD-002, REQ-BUILD-005, REQ-CAB-003, REQ-CAB-004, REQ-CAB-005, REQ-CAB-007,
//! REQ-CAB-008.
//!
//! A table's `cabinet.toml` describes the cabinet body, its four legs and levelers, the
//! backbox, the playfield's mount in the body, the controls, the trim colours and art
//! overrides. It usually starts from a named preset shipped with the engine
//! ([`CABINET_PRESETS_TOML`], SPEC Appendix E) and states only what differs.
//!
//! # Types
//!
//! * [`CabinetManifest`] is the file: `schema_version`, an optional `preset` and partial
//!   sections whose fields are all optional ([`CabinetOverride`] and its nested
//!   `*Override` types).
//! * [`CabinetGeometry`] is the fully resolved cabinet: no optional field. It keeps the
//!   file's units (mm and degrees, `f32`) because the generator and the validation work in
//!   them; [`playfield_attitude`] returns radians (`f64`, SI per D17).
//!
//! # Frames
//!
//! * **Cabinet frame** (mm): `x` across the cabinet from the body centreline, positive to
//!   the player's right; `y` along the cabinet from the body's front face (player side,
//!   `y = 0`) toward the rear (backbox); `z` up. Parts fixed to the body (buttons, coin
//!   door, plunger, playfield mount, backbox) store `z` as the height above the **body
//!   bottom**; the height above the floor is that plus the body bottom's height above the
//!   floor, which the legs set ([`CabinetGeometry::floor_height_mm`]).
//! * **Playfield frame** (mm, Appendix A): `x` across the playfield from its left edge, `y`
//!   up the table from its front edge, `z` up from the playfield surface. Its origin, the
//!   front-left corner of the playfield surface, sits in the cabinet frame at
//!   `(mount.offset_x - playfield_width / 2, mount.inset, mount.height)`.
//!
//! # Presets and overrides
//!
//! `preset = "<id>"` starts from one of [`preset_ids`]; every field the file states
//! replaces the preset's value, field by field inside nested tables. `[legs.default]`
//! applies to all four legs and a per-leg table (`[legs.front_left]`, ...) beats it.
//! `[art]` entries are added to (or replace entries of) the preset's art map. Without
//! `preset` the file must state every field; the first missing one is an error naming it.
//! A table with no `cabinet.toml` uses [`DEFAULT_PRESET`] ([`CabinetGeometry::default_preset`]).

use super::{Manifest, SchemaError, manifest_io};
use crate::fmath::max_f64;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// File name of the optional cabinet manifest inside a table bundle.
pub const CABINET_FILE: &str = "cabinet.toml";

/// The preset a table gets without a `cabinet.toml` (REQ-CAB-004).
pub const DEFAULT_PRESET: &str = "classic_standard";

/// The cabinet presets shipped with the engine (SPEC Appendix E), as TOML text.
///
/// Layout: `schema_version = 1`, then one `[presets.<id>]` table per preset whose
/// sub-tables have the same shape as a `cabinet.toml` (every field given).
pub const CABINET_PRESETS_TOML: &str = include_str!("../../data/cabinet_presets.toml");

/// Most grilles a backbox may list.
pub const MAX_GRILLES: usize = 8;
/// Most `[art]` entries a cabinet may list.
pub const MAX_ART_ENTRIES: usize = 256;

/// Lower and upper bound of `mount.pitch_deg` and `mount.roll_deg`, degrees.
pub const MOUNT_ANGLE_LIMIT_DEG: f32 = 45.0;

/// Allowed disagreement between `meta.pitch_deg` and the derived pitch, degrees
/// (REQ-CAB-008).
pub const PITCH_AGREEMENT_DEG: f64 = 0.05;

/// Extra allowance on [`PITCH_AGREEMENT_DEG`] for the `f32` rounding of the file value
/// (`6.55f32` is `6.5500002`), degrees.
pub const PITCH_ROUNDING_SLACK_DEG: f64 = 1e-5;

// ---------------------------------------------------------------------------
// Layering: partial (file) values over a resolved base.
// ---------------------------------------------------------------------------

/// A resolved value and its partial (file) form.
trait Layer: Sized {
    /// The file form: every field optional.
    type Partial: Clone;
    /// `upper` over `lower`, both partial: a field of `upper` wins.
    fn overlay(
        lower: Option<&Self::Partial>,
        upper: Option<&Self::Partial>,
    ) -> Option<Self::Partial>;
    /// `p` over `base`; a field neither gives is an error naming `path`.
    fn resolve(
        p: Option<&Self::Partial>,
        base: Option<&Self>,
        path: &str,
    ) -> Result<Self, SchemaError>;
}

fn join(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_owned()
    } else {
        format!("{path}.{name}")
    }
}

fn invalid(msg: String) -> SchemaError {
    SchemaError::Invalid(msg)
}

macro_rules! leaf {
    ($($t:ty),* $(,)?) => {$(
        impl Layer for $t {
            type Partial = $t;
            fn overlay(lower: Option<&$t>, upper: Option<&$t>) -> Option<$t> {
                upper.or(lower).cloned()
            }
            fn resolve(p: Option<&$t>, base: Option<&$t>, path: &str) -> Result<$t, SchemaError> {
                p.or(base).cloned().ok_or_else(|| {
                    invalid(format!(
                        "`{path}` is missing: state it or start from a preset (`preset = \"<id>\"`)"
                    ))
                })
            }
        }
    )*};
}

impl Layer for BTreeMap<String, String> {
    type Partial = BTreeMap<String, String>;
    fn overlay(lower: Option<&Self>, upper: Option<&Self>) -> Option<Self> {
        match (lower, upper) {
            (None, None) => None,
            (l, u) => {
                let mut m = l.cloned().unwrap_or_default();
                if let Some(u) = u {
                    m.extend(u.iter().map(|(k, v)| (k.clone(), v.clone())));
                }
                Some(m)
            }
        }
    }
    fn resolve(p: Option<&Self>, base: Option<&Self>, _path: &str) -> Result<Self, SchemaError> {
        Ok(Self::overlay(base, p).unwrap_or_default())
    }
}

/// Defines a resolved struct `$full` and its partial file form `$part` (same fields, each
/// `Option`), plus the field-by-field [`Layer`] impl.
macro_rules! layered {
    (
        $(#[$sm:meta])*
        pub struct $full:ident / $part:ident {
            $( $(#[$fm:meta])* $f:ident : $ft:ty => $pt:ty ),* $(,)?
        }
    ) => {
        $(#[$sm])*
        #[derive(Clone, Debug, PartialEq)]
        pub struct $full {
            $( $(#[$fm])* pub $f: $ft, )*
        }

        #[doc = concat!("Partial (file) form of [`", stringify!($full),
            "`]: every field optional; a field not given keeps the base (preset) value.")]
        #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct $part {
            $(
                $(#[$fm])*
                #[serde(default, skip_serializing_if = "Option::is_none")]
                pub $f: Option<$pt>,
            )*
        }

        impl Layer for $full {
            type Partial = $part;
            fn overlay(lower: Option<&$part>, upper: Option<&$part>) -> Option<$part> {
                match (lower, upper) {
                    (None, None) => None,
                    (Some(l), None) => Some(l.clone()),
                    (None, Some(u)) => Some(u.clone()),
                    (Some(l), Some(u)) => Some($part {
                        $( $f: <$ft as Layer>::overlay(l.$f.as_ref(), u.$f.as_ref()), )*
                    }),
                }
            }
            fn resolve(p: Option<&$part>, base: Option<&$full>, path: &str)
                -> Result<$full, SchemaError>
            {
                Ok($full {
                    $( $f: <$ft as Layer>::resolve(
                        p.and_then(|p| p.$f.as_ref()),
                        base.map(|b| &b.$f),
                        &join(path, stringify!($f)),
                    )?, )*
                })
            }
        }
    };
}

// ---------------------------------------------------------------------------
// Enumerations (snake_case in the file).
// ---------------------------------------------------------------------------

macro_rules! file_enum {
    ($(#[$m:meta])* pub enum $name:ident { $( $(#[$vm:meta])* $v:ident ),* $(,)? }) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $( $(#[$vm])* $v, )* }
    };
}

file_enum! {
    /// Leg cross-section (REQ-CAB-007).
    pub enum CrossSectionShape {
        /// Square: `top` is the side length (`bottom` unused).
        Square,
        /// Round: `top` is the diameter (`bottom` unused).
        Round,
        /// Square, tapering from side `top` at the body to side `bottom` at the foot.
        Tapered,
    }
}

file_enum! {
    /// Leg material.
    pub enum LegMaterial {
        /// Steel.
        Steel,
        /// Aluminium.
        Aluminium,
        /// Wood.
        Wood,
    }
}

file_enum! {
    /// Metal finish of legs and trim.
    pub enum Finish {
        /// Polished chrome plating.
        Chrome,
        /// Brushed (satin) steel.
        BrushedSteel,
        /// Smooth powder coat in the part's colour.
        PowderCoat,
        /// Wrinkle-texture powder coat in the part's colour.
        Wrinkle,
        /// Painted in the part's colour.
        Painted,
    }
}

file_enum! {
    /// Leg surface pattern.
    pub enum LegSurface {
        /// Smooth faces.
        Plain,
        /// Lengthwise ribs.
        Ribbed,
    }
}

file_enum! {
    /// Leg bracket style (how the leg's top is held at the body corner).
    pub enum BracketStyle {
        /// Inside corner plate the leg bolts through.
        CornerPlate,
        /// Flush plate on the side wall.
        FlushPlate,
    }
}

file_enum! {
    /// Leg bolt head style.
    pub enum BoltStyle {
        /// Hex head.
        Hex,
        /// Domed carriage head.
        Carriage,
        /// Low button head.
        ButtonHead,
    }
}

file_enum! {
    /// Leveler foot shape.
    pub enum FootShape {
        /// Flat disc.
        Flat,
        /// Domed disc.
        Domed,
        /// Swivelling pad.
        Swivel,
    }
}

file_enum! {
    /// Lockdown bar cross-section.
    pub enum BarProfile {
        /// Flat top.
        Flat,
        /// Rounded top.
        Rounded,
    }
}

file_enum! {
    /// Side rail cross-section.
    pub enum RailProfile {
        /// Flat strip.
        Flat,
        /// Rounded strip.
        Rounded,
        /// Angled (bevelled) strip.
        Angled,
    }
}

file_enum! {
    /// Backbox front profile.
    pub enum BackboxFrontProfile {
        /// Vertical front.
        Flat,
        /// Front leaning back.
        Angled,
    }
}

file_enum! {
    /// Backbox top profile.
    pub enum BackboxTopProfile {
        /// Flat top.
        Flat,
        /// Top sloping toward the rear.
        Angled,
        /// Arched top.
        Arched,
    }
}

file_enum! {
    /// Score display technology behind the score-display window.
    pub enum ScoreDisplayKind {
        /// Dot-matrix display.
        Dmd,
        /// LCD panel.
        Lcd,
    }
}

/// One round speaker grille on the backbox speaker panel.
///
/// Coordinates on the backbox front: `u` across from the backbox centreline (mm, right
/// positive), `v` up from the backbox bottom (mm).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grille {
    /// Centre across the backbox front from its centreline, mm.
    pub u: f32,
    /// Centre above the backbox bottom, mm.
    pub v: f32,
    /// Diameter, mm.
    pub diameter: f32,
}

leaf!(
    f32,
    String,
    [f32; 2],
    [f32; 3],
    [u32; 2],
    Vec<Grille>,
    CrossSectionShape,
    LegMaterial,
    Finish,
    LegSurface,
    BracketStyle,
    BoltStyle,
    FootShape,
    BarProfile,
    RailProfile,
    BackboxFrontProfile,
    BackboxTopProfile,
    ScoreDisplayKind,
);

// ---------------------------------------------------------------------------
// Sections.
// ---------------------------------------------------------------------------

layered! {
    /// `[body.glass_channel]`: the slot in the side walls that holds the playfield glass.
    pub struct GlassChannel / GlassChannelOverride {
        /// Slot width, mm (at least the glass thickness).
        width: f32 => f32,
        /// Slot depth into each side wall, mm.
        depth: f32 => f32,
    }
}

layered! {
    /// `[body.lockdown_bar]`: the bar across the front of the body over the glass.
    pub struct LockdownBar / LockdownBarOverride {
        /// Cross-section.
        profile: BarProfile => BarProfile,
        /// Height of the bar's top above the front wall's top, mm.
        height: f32 => f32,
        /// Front-to-back depth, mm.
        depth: f32 => f32,
    }
}

layered! {
    /// `[body.side_rail]`: the trim rails along the top of the side walls.
    pub struct SideRail / SideRailOverride {
        /// Cross-section.
        profile: RailProfile => RailProfile,
        /// Width, mm.
        width: f32 => f32,
        /// Height, mm.
        height: f32 => f32,
    }
}

layered! {
    /// `[body]`: the cabinet body (REQ-CAB-003). The body's side walls run in a straight
    /// line from `front_height` at `y = 0` to `rear_height` at `y = length`.
    pub struct Body / BodyOverride {
        /// Outer width across the side walls, mm.
        width: f32 => f32,
        /// Outer length along the cabinet, plunger excluded, mm.
        length: f32 => f32,
        /// Side wall height at the front, above the body bottom, mm.
        front_height: f32 => f32,
        /// Side wall height at the rear, above the body bottom, mm.
        rear_height: f32 => f32,
        /// Wall thickness, mm.
        wall_thickness: f32 => f32,
        /// Height of each leg's top above the body bottom (the part of the leg bolted
        /// alongside the body), mm; the same for all four legs.
        leg_overlap: f32 => f32,
        /// Glass channel.
        glass_channel: GlassChannel => GlassChannelOverride,
        /// Lockdown bar.
        lockdown_bar: LockdownBar => LockdownBarOverride,
        /// Side rails.
        side_rail: SideRail => SideRailOverride,
    }
}

layered! {
    /// `[legs.<leg>.section]`: the leg's cross-section.
    pub struct LegSection / LegSectionOverride {
        /// Shape.
        shape: CrossSectionShape => CrossSectionShape,
        /// Side (square, tapered) or diameter (round) at the body end, mm.
        top: f32 => f32,
        /// Side at the foot end for `tapered`, mm (ignored otherwise).
        bottom: f32 => f32,
    }
}

layered! {
    /// `[legs.<leg>.leveler]`: the leveler screwed into the leg's foot.
    pub struct Leveler / LevelerOverride {
        /// Bolt thread length, mm.
        thread_length: f32 => f32,
        /// Bolt thread diameter, mm.
        thread_diameter: f32 => f32,
        /// Current extension: what the leveler adds to the leg's length, mm.
        extension: f32 => f32,
        /// Extension when fully screwed in, mm.
        extension_min: f32 => f32,
        /// Extension when fully screwed out, mm.
        extension_max: f32 => f32,
        /// Foot shape.
        foot_shape: FootShape => FootShape,
        /// Foot diameter, mm.
        foot_diameter: f32 => f32,
    }
}

layered! {
    /// One leg (REQ-CAB-007).
    pub struct Leg / LegOverride {
        /// Leg length without the leveler, mm.
        length: f32 => f32,
        /// Cross-section.
        section: LegSection => LegSectionOverride,
        /// Material.
        material: LegMaterial => LegMaterial,
        /// Finish.
        finish: Finish => Finish,
        /// Surface pattern.
        surface: LegSurface => LegSurface,
        /// Colour, `"#rrggbb"` sRGB (the tint for chrome and brushed finishes).
        colour: String => String,
        /// Bracket style.
        bracket: BracketStyle => BracketStyle,
        /// Bolt style.
        bolt: BoltStyle => BoltStyle,
        /// Leveler.
        leveler: Leveler => LevelerOverride,
        /// Leg centreline inset from its body corner, `[across, along]`, mm. The four
        /// positions give the leg spacing the attitude uses.
        corner_inset: [f32; 2] => [f32; 2],
    }
}

impl Leg {
    /// Effective length: leg length plus leveler extension, mm (REQ-CAB-008).
    pub fn effective_length_mm(&self) -> f64 {
        f64::from(self.length) + f64::from(self.leveler.extension)
    }
}

/// `[legs]`: the four legs, each set independently (REQ-CAB-007).
#[derive(Clone, Debug, PartialEq)]
pub struct Legs {
    /// Front left leg (player's left).
    pub front_left: Leg,
    /// Front right leg.
    pub front_right: Leg,
    /// Rear left leg.
    pub rear_left: Leg,
    /// Rear right leg.
    pub rear_right: Leg,
}

/// Partial (file) form of [`Legs`]: `default` applies to all four legs, a per-leg table
/// beats it, and the base (preset) leg supplies the rest.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegsOverride {
    /// Values for all four legs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<LegOverride>,
    /// Front left leg.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub front_left: Option<LegOverride>,
    /// Front right leg.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub front_right: Option<LegOverride>,
    /// Rear left leg.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rear_left: Option<LegOverride>,
    /// Rear right leg.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rear_right: Option<LegOverride>,
}

impl LegsOverride {
    /// Per-leg partials with `default` folded in, in [`Legs::all`] order.
    fn folded(&self) -> [Option<LegOverride>; 4] {
        let d = self.default.as_ref();
        [
            Leg::overlay(d, self.front_left.as_ref()),
            Leg::overlay(d, self.front_right.as_ref()),
            Leg::overlay(d, self.rear_left.as_ref()),
            Leg::overlay(d, self.rear_right.as_ref()),
        ]
    }
}

impl Layer for Legs {
    type Partial = LegsOverride;
    fn overlay(lower: Option<&LegsOverride>, upper: Option<&LegsOverride>) -> Option<LegsOverride> {
        match (lower, upper) {
            (None, None) => None,
            (Some(l), None) => Some(l.clone()),
            (None, Some(u)) => Some(u.clone()),
            (Some(l), Some(u)) => {
                let [lfl, lfr, lrl, lrr] = l.folded();
                let [ufl, ufr, url, urr] = u.folded();
                Some(LegsOverride {
                    default: None,
                    front_left: Leg::overlay(lfl.as_ref(), ufl.as_ref()),
                    front_right: Leg::overlay(lfr.as_ref(), ufr.as_ref()),
                    rear_left: Leg::overlay(lrl.as_ref(), url.as_ref()),
                    rear_right: Leg::overlay(lrr.as_ref(), urr.as_ref()),
                })
            }
        }
    }
    fn resolve(
        p: Option<&LegsOverride>,
        base: Option<&Legs>,
        path: &str,
    ) -> Result<Legs, SchemaError> {
        let [fl, fr, rl, rr] = p.map(LegsOverride::folded).unwrap_or_default();
        Ok(Legs {
            front_left: Leg::resolve(
                fl.as_ref(),
                base.map(|b| &b.front_left),
                &join(path, "front_left"),
            )?,
            front_right: Leg::resolve(
                fr.as_ref(),
                base.map(|b| &b.front_right),
                &join(path, "front_right"),
            )?,
            rear_left: Leg::resolve(
                rl.as_ref(),
                base.map(|b| &b.rear_left),
                &join(path, "rear_left"),
            )?,
            rear_right: Leg::resolve(
                rr.as_ref(),
                base.map(|b| &b.rear_right),
                &join(path, "rear_right"),
            )?,
        })
    }
}

impl Legs {
    /// The four legs with their file names, in the fixed order front_left, front_right,
    /// rear_left, rear_right.
    pub fn all(&self) -> [(&'static str, &Leg); 4] {
        [
            ("front_left", &self.front_left),
            ("front_right", &self.front_right),
            ("rear_left", &self.rear_left),
            ("rear_right", &self.rear_right),
        ]
    }
}

layered! {
    /// `[mount]`: the playfield's mount in the cabinet body (REQ-BUILD-002). The
    /// playfield frame's origin (front-left corner of the surface) is at cabinet
    /// `(offset_x - playfield_width / 2, inset, height)`.
    pub struct Mount / MountOverride {
        /// Cabinet-frame x of the playfield's centreline (0 = centred), mm.
        offset_x: f32 => f32,
        /// Playfield inset: cabinet-frame y of the playfield's front edge, from the body's
        /// front face, mm.
        inset: f32 => f32,
        /// Height of the playfield surface at its front edge above the body bottom, mm.
        height: f32 => f32,
        /// Playfield pitch relative to the body, degrees, rear up positive.
        pitch_deg: f32 => f32,
        /// Playfield roll relative to the body, degrees, right (+x) side up positive.
        roll_deg: f32 => f32,
    }
}

layered! {
    /// `[glass_panel]`: the playfield glass sheet (its height above the playfield is
    /// `glass.toml`'s `height`, Appendix A.3a).
    pub struct GlassPanel / GlassPanelOverride {
        /// Width across, mm.
        width: f32 => f32,
        /// Length along the playfield, mm.
        length: f32 => f32,
        /// Thickness, mm.
        thickness: f32 => f32,
    }
}

layered! {
    /// `[backbox.backglass]`: the backglass sheet.
    pub struct Backglass / BackglassOverride {
        /// Width, mm.
        width: f32 => f32,
        /// Height, mm.
        height: f32 => f32,
        /// Thickness, mm.
        thickness: f32 => f32,
    }
}

layered! {
    /// `[backbox.translite_opening]`: the visible translite opening in the backbox front.
    pub struct TransliteOpening / TransliteOpeningOverride {
        /// Width, mm (centred on the backbox).
        width: f32 => f32,
        /// Height, mm.
        height: f32 => f32,
        /// Bottom edge above the backbox bottom, mm.
        bottom: f32 => f32,
    }
}

layered! {
    /// `[backbox.score_display]`: the score-display window in the speaker panel.
    pub struct ScoreDisplay / ScoreDisplayOverride {
        /// Technology.
        kind: ScoreDisplayKind => ScoreDisplayKind,
        /// Resolution `[columns, rows]` in pixels (dots for a DMD).
        pixels: [u32; 2] => [u32; 2],
        /// Window width, mm (centred on the backbox).
        width: f32 => f32,
        /// Window height, mm.
        height: f32 => f32,
        /// Window centre above the backbox bottom, mm.
        centre_v: f32 => f32,
    }
}

layered! {
    /// `[backbox]`: the backbox on top of the body's rear (REQ-CAB-003). Front-face
    /// coordinates: `u` across from the backbox centreline, `v` up from its bottom.
    pub struct Backbox / BackboxOverride {
        /// Width, mm.
        width: f32 => f32,
        /// Height, mm.
        height: f32 => f32,
        /// Depth front to back, mm.
        depth: f32 => f32,
        /// Cabinet-frame y of the backbox's front face, mm.
        front_y: f32 => f32,
        /// Height of the backbox bottom above the body bottom, mm.
        bottom_z: f32 => f32,
        /// Front profile.
        front_profile: BackboxFrontProfile => BackboxFrontProfile,
        /// Top profile.
        top_profile: BackboxTopProfile => BackboxTopProfile,
        /// Height of the speaker panel at the bottom of the front, mm.
        speaker_panel_height: f32 => f32,
        /// Backglass sheet.
        backglass: Backglass => BackglassOverride,
        /// Translite opening.
        translite_opening: TransliteOpening => TransliteOpeningOverride,
        /// Score-display window.
        score_display: ScoreDisplay => ScoreDisplayOverride,
        /// Speaker grilles (at most [`MAX_GRILLES`]); an override replaces the list.
        grilles: Vec<Grille> => Vec<Grille>,
    }
}

layered! {
    /// `[controls.coin_door]`: the coin door in the body front.
    pub struct CoinDoor / CoinDoorOverride {
        /// Centre `[x, y, z]`, cabinet frame (z above the body bottom), mm.
        centre: [f32; 3] => [f32; 3],
        /// Width, mm.
        width: f32 => f32,
        /// Height, mm.
        height: f32 => f32,
    }
}

layered! {
    /// `[controls]`: button, coin door and plunger positions, cabinet frame
    /// (`[x, y, z]` mm, z above the body bottom).
    pub struct Controls / ControlsOverride {
        /// Left flipper button centre.
        left_flipper_button: [f32; 3] => [f32; 3],
        /// Right flipper button centre.
        right_flipper_button: [f32; 3] => [f32; 3],
        /// Start button centre.
        start_button: [f32; 3] => [f32; 3],
        /// Action button centre.
        action_button: [f32; 3] => [f32; 3],
        /// Plunger axis where it leaves the body front.
        plunger: [f32; 3] => [f32; 3],
        /// Coin door.
        coin_door: CoinDoor => CoinDoorOverride,
    }
}

layered! {
    /// `[trim]`: colours and trim (REQ-CAB-003). Leg colour and finish are per leg.
    pub struct Trim / TrimOverride {
        /// Finish of the lockdown bar, side rails and hinges.
        metal_finish: Finish => Finish,
        /// Side rail colour, `"#rrggbb"` sRGB.
        rail_colour: String => String,
    }
}

layered! {
    /// A fully resolved cabinet: every field present (REQ-CAB-003). Units are the file's
    /// (mm, degrees). See the module docs for the frames.
    pub struct CabinetGeometry / CabinetOverride {
        /// Body.
        body: Body => BodyOverride,
        /// Legs.
        legs: Legs => LegsOverride,
        /// Playfield mount.
        mount: Mount => MountOverride,
        /// Playfield glass sheet.
        glass_panel: GlassPanel => GlassPanelOverride,
        /// Backbox.
        backbox: Backbox => BackboxOverride,
        /// Controls.
        controls: Controls => ControlsOverride,
        /// Trim.
        trim: Trim => TrimOverride,
        /// Art overrides: stable asset ID (D32) to a bundle-relative file path.
        art: BTreeMap<String, String> => BTreeMap<String, String>,
    }
}

// ---------------------------------------------------------------------------
// The file.
// ---------------------------------------------------------------------------

/// `cabinet.toml`: a preset plus per-field overrides (REQ-CAB-003, REQ-CAB-004).
///
/// Parsing ([`Self::from_toml_str`]) rejects unknown keys, a wrong `schema_version`,
/// non-finite floats, an unknown `preset` and a file that does not resolve to a
/// self-consistent [`CabinetGeometry`] ([`Self::resolve`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CabinetManifest {
    /// Must be 1.
    pub schema_version: u32,
    /// Preset to start from (one of [`preset_ids`]); `None` means every field is given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// `[body]` overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<BodyOverride>,
    /// `[legs]` overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legs: Option<LegsOverride>,
    /// `[mount]` overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount: Option<MountOverride>,
    /// `[glass_panel]` overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glass_panel: Option<GlassPanelOverride>,
    /// `[backbox]` overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backbox: Option<BackboxOverride>,
    /// `[controls]` overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controls: Option<ControlsOverride>,
    /// `[trim]` overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trim: Option<TrimOverride>,
    /// `[art]` entries added to the preset's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art: Option<BTreeMap<String, String>>,
}

impl CabinetManifest {
    /// The file's sections as one [`CabinetOverride`].
    pub fn overrides(&self) -> CabinetOverride {
        CabinetOverride {
            body: self.body.clone(),
            legs: self.legs.clone(),
            mount: self.mount.clone(),
            glass_panel: self.glass_panel.clone(),
            backbox: self.backbox.clone(),
            controls: self.controls.clone(),
            trim: self.trim.clone(),
            art: self.art.clone(),
        }
    }

    /// Resolves the file against its preset (or alone, without one) and checks the result
    /// with [`CabinetGeometry::check_consistency`].
    ///
    /// Errors: an unknown preset names `preset`; without a preset, the first field not
    /// given is named (`body.width`, ...); inconsistent values name their field.
    pub fn resolve(&self) -> Result<CabinetGeometry, SchemaError> {
        let base = match &self.preset {
            Some(id) => Some(CabinetGeometry::preset(id)?),
            None => None,
        };
        let geom = CabinetGeometry::resolve(Some(&self.overrides()), base.as_ref(), "")?;
        geom.check_consistency()?;
        Ok(geom)
    }
}

impl Manifest for CabinetManifest {
    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn validate(&self) -> Result<(), SchemaError> {
        self.resolve().map(|_| ())
    }
}

manifest_io!(CabinetManifest);

// ---------------------------------------------------------------------------
// Presets.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PresetFile {
    schema_version: u32,
    presets: BTreeMap<String, CabinetOverride>,
}

type Presets = Result<BTreeMap<String, CabinetGeometry>, SchemaError>;

fn load_presets(text: &str) -> Presets {
    let file: PresetFile = toml::from_str(text)
        .map_err(|e| invalid(format!("built-in cabinet presets do not parse: {e}")))?;
    if file.schema_version != super::SCHEMA_VERSION {
        return Err(SchemaError::UnsupportedVersion(file.schema_version));
    }
    let mut out = BTreeMap::new();
    for (id, p) in &file.presets {
        let g = CabinetGeometry::resolve(Some(p), None, "")
            .and_then(|g| g.check_consistency().map(|()| g))
            .map_err(|e| invalid(format!("built-in cabinet preset `{id}`: {e}")))?;
        out.insert(id.clone(), g);
    }
    Ok(out)
}

fn presets() -> &'static Presets {
    static PRESETS: OnceLock<Presets> = OnceLock::new();
    PRESETS.get_or_init(|| load_presets(CABINET_PRESETS_TOML))
}

/// The preset ids shipped with the engine, sorted (`classic_standard`,
/// `classic_widebody`, `modern_standard`). Empty only if the built-in data were broken.
pub fn preset_ids() -> &'static [&'static str] {
    static IDS: OnceLock<Vec<&'static str>> = OnceLock::new();
    IDS.get_or_init(|| match presets() {
        Ok(m) => m.keys().map(String::as_str).collect(),
        Err(_) => Vec::new(),
    })
}

// ---------------------------------------------------------------------------
// Attitude.
// ---------------------------------------------------------------------------

/// The playfield's attitude on a level floor (REQ-CAB-008), radians, `f64`.
///
/// * `pitch_rad` > 0: the playfield's rear (far from the player, +y of the playfield
///   frame) is higher, so gravity along the playfield points toward the front (−y), as
///   the engine's current gravity "tilted by `pitch_deg` down −y".
/// * `roll_rad` > 0: the playfield's right side (+x) is higher, so gravity pulls toward −x.
///
/// The two are independent angles (small-angle composition): pitch about the cabinet's
/// x axis and roll about its y axis, not a composed rotation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Attitude {
    /// Pitch along the table, radians, rear up positive.
    pub pitch_rad: f64,
    /// Roll across the table, radians, right side up positive.
    pub roll_rad: f64,
}

impl Attitude {
    /// Pitch in degrees.
    pub fn pitch_deg(&self) -> f64 {
        rad_to_deg(self.pitch_rad)
    }

    /// Roll in degrees.
    pub fn roll_deg(&self) -> f64 {
        rad_to_deg(self.roll_rad)
    }
}

/// Radians to degrees (a multiplication by a constant).
pub fn rad_to_deg(rad: f64) -> f64 {
    rad * (180.0 / core::f64::consts::PI)
}

/// Degrees to radians (a multiplication by a constant).
pub fn deg_to_rad(deg: f64) -> f64 {
    deg * (core::f64::consts::PI / 180.0)
}

/// Cabinet-frame `(x, y)` of a leg's centreline, mm, from its corner inset.
fn leg_xy(body: &Body, leg: &Leg, right: bool, rear: bool) -> (f64, f64) {
    let half_w = f64::from(body.width) * 0.5;
    let ix = f64::from(leg.corner_inset[0]);
    let iy = f64::from(leg.corner_inset[1]);
    let x = if right { half_w - ix } else { ix - half_w };
    let y = if rear {
        f64::from(body.length) - iy
    } else {
        iy
    };
    (x, y)
}

/// Leg spacing `(sx, sy)`, mm: distance between the mean left and mean right leg
/// centrelines, and between the mean front and mean rear leg centrelines.
fn leg_spacing(g: &CabinetGeometry) -> (f64, f64) {
    let b = &g.body;
    let l = &g.legs;
    let (flx, fly) = leg_xy(b, &l.front_left, false, false);
    let (frx, fry) = leg_xy(b, &l.front_right, true, false);
    let (rlx, rly) = leg_xy(b, &l.rear_left, false, true);
    let (rrx, rry) = leg_xy(b, &l.rear_right, true, true);
    let sx = (frx + rrx) * 0.5 - (flx + rlx) * 0.5;
    let sy = (rly + rry) * 0.5 - (fly + fry) * 0.5;
    (sx, sy)
}

/// The playfield's pitch and roll on a level floor (REQ-CAB-008).
///
/// Model (SPEC Appendix A.4): the body is rigid and stands on its four legs; a leg's
/// effective length is `length + leveler.extension`. With `F`, `R`, `L`, `Rt` the mean
/// effective lengths of the front, rear, left and right leg pairs and `Sx`, `Sy` the
/// left-right and front-rear leg spacings (mean centrelines):
///
/// * body pitch = `atan2(R - F, Sy)`, body roll = `atan2(Rt - L, Sx)`;
/// * `pitch_rad = body pitch + radians(mount.pitch_deg)`,
///   `roll_rad = body roll + radians(mount.roll_deg)`.
///
/// Pure and deterministic (`libm::atan2`, no FMA), no allocation or I/O: callable at
/// load and from sim setup. The body heights do not enter the tilt.
pub fn playfield_attitude(geom: &CabinetGeometry) -> Attitude {
    let l = &geom.legs;
    let fl = l.front_left.effective_length_mm();
    let fr = l.front_right.effective_length_mm();
    let rl = l.rear_left.effective_length_mm();
    let rr = l.rear_right.effective_length_mm();
    let front = (fl + fr) * 0.5;
    let rear = (rl + rr) * 0.5;
    let left = (fl + rl) * 0.5;
    let right = (fr + rr) * 0.5;
    let (sx, sy) = leg_spacing(geom);
    let body_pitch = libm::atan2(rear - front, sy);
    let body_roll = libm::atan2(right - left, sx);
    Attitude {
        pitch_rad: body_pitch + deg_to_rad(f64::from(geom.mount.pitch_deg)),
        roll_rad: body_roll + deg_to_rad(f64::from(geom.mount.roll_deg)),
    }
}

// ---------------------------------------------------------------------------
// Validation.
// ---------------------------------------------------------------------------

/// What the cabinet must contain: the table's playfield, glass and tallest part
/// (REQ-CAB-005) and its stated pitch (REQ-CAB-008).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TableEnvelope {
    /// Playfield width (playfield-frame x extent), mm.
    pub playfield_width_mm: f32,
    /// Playfield length (playfield-frame y extent), mm.
    pub playfield_length_mm: f32,
    /// Glass underside above the playfield surface (`glass.toml` `height`), mm.
    pub glass_height_mm: f32,
    /// Tallest ramp or toy above the playfield surface, mm.
    pub max_part_height_mm: f32,
    /// `meta.pitch_deg`, when given, degrees.
    pub pitch_deg: Option<f32>,
}

/// One playfield corner in the cabinet (body) frame, for the containment checks
/// (REQ-CAB-005).
#[derive(Copy, Clone, Debug, PartialEq)]
struct PlayfieldCorner {
    /// Cabinet-frame `y` of the corner, mm.
    y: f64,
    /// Playfield surface height above the body bottom, mm.
    surface_z: f64,
    /// Glass sheet top above the body bottom, mm.
    glass_z: f64,
    /// `"front"` or `"rear"`.
    edge: &'static str,
    /// `"left"` or `"right"`.
    side: &'static str,
}

/// The four playfield corners (front-left, front-right, rear-left, rear-right) in the
/// cabinet frame (REQ-BUILD-002, REQ-CAB-005, REQ-CAB-008).
///
/// The playfield origin (front-left surface corner) is at `y = inset`, `z = height`. Pitch
/// (rear up positive) raises the rear edge by `length * sin(pitch)` and moves it to
/// `y = inset + length * cos(pitch)`; roll (right side up positive) turns the playfield about
/// its left edge through the origin, so the right side rises by `width * sin(roll)` (falls
/// for negative roll) while the left side stays put. The glass top sits `glass` (glass
/// height plus sheet thickness) along the playfield normal, whose vertical component is
/// `cos(pitch) * cos(roll)` (small-angle composition). At roll 0 the numbers equal the
/// pitch-only edge heights bit for bit. Heights are unclamped: a corner may come out
/// negative (below the body bottom) or above the wall top; `validate` rejects both.
fn playfield_corners(
    inset: f64,
    height: f64,
    width: f64,
    length: f64,
    pitch: f64,
    roll: f64,
    glass: f64,
) -> [PlayfieldCorner; 4] {
    let (sp, cp) = (libm::sin(pitch), libm::cos(pitch));
    let (sr, cr) = (libm::sin(roll), libm::cos(roll));
    let glass_up = glass * cp * cr;
    let right_rise = width * sr;
    let rear_rise = length * sp;
    let rear_y = inset + length * cp;
    let corner = |y: f64, rise: f64, edge, side| {
        let surface_z = height + rise;
        PlayfieldCorner {
            y,
            surface_z,
            glass_z: surface_z + glass_up,
            edge,
            side,
        }
    };
    [
        corner(inset, 0.0, "front", "left"),
        corner(inset, right_rise, "front", "right"),
        corner(rear_y, rear_rise, "rear", "left"),
        corner(rear_y, rear_rise + right_rise, "rear", "right"),
    ]
}

fn require(ok: bool, msg: impl FnOnce() -> String) -> Result<(), SchemaError> {
    if ok { Ok(()) } else { Err(invalid(msg())) }
}

fn positive(path: &str, v: f32) -> Result<(), SchemaError> {
    require(v.is_finite() && v > 0.0, || {
        format!("`{path}` must be finite and > 0, got {v}")
    })
}

fn non_negative(path: &str, v: f32) -> Result<(), SchemaError> {
    require(v.is_finite() && v >= 0.0, || {
        format!("`{path}` must be finite and >= 0, got {v}")
    })
}

fn finite_all(path: &str, v: &[f32]) -> Result<(), SchemaError> {
    require(v.iter().all(|x| x.is_finite()), || {
        format!("`{path}` must be finite, got {v:?}")
    })
}

fn colour(path: &str, s: &str) -> Result<(), SchemaError> {
    let b = s.as_bytes();
    let ok =
        b.len() == 7 && b.first() == Some(&b'#') && b.iter().skip(1).all(u8::is_ascii_hexdigit);
    require(ok, || format!("`{path}` must be \"#rrggbb\", got {s:?}"))
}

fn art_entry(id: &str, file: &str) -> Result<(), SchemaError> {
    let id_ok = !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_.-".contains(&c));
    require(id_ok, || {
        format!("`art` asset ID {id:?} must be 1-64 characters of a-z, 0-9, `_`, `.`, `-`")
    })?;
    let file_ok = !file.is_empty()
        && file.len() <= 256
        && !file.starts_with('/')
        && !file.contains('\\')
        && !file.contains(':')
        && file.split('/').all(|s| !s.is_empty() && s != "..");
    require(file_ok, || {
        format!("`art.{id}` must be a relative bundle path without `..`, got {file:?}")
    })
}

impl Body {
    fn check(&self) -> Result<(), SchemaError> {
        positive("body.width", self.width)?;
        positive("body.length", self.length)?;
        positive("body.front_height", self.front_height)?;
        positive("body.rear_height", self.rear_height)?;
        positive("body.wall_thickness", self.wall_thickness)?;
        require(
            2.0 * self.wall_thickness < self.width && 2.0 * self.wall_thickness < self.length,
            || {
                format!(
                    "`body.wall_thickness` {} leaves no interior in a {} x {} body",
                    self.wall_thickness, self.width, self.length
                )
            },
        )?;
        non_negative("body.leg_overlap", self.leg_overlap)?;
        let lowest = if self.front_height < self.rear_height {
            self.front_height
        } else {
            self.rear_height
        };
        require(self.leg_overlap <= lowest, || {
            format!(
                "`body.leg_overlap` {} is above the lower side wall height {lowest}",
                self.leg_overlap
            )
        })?;
        positive("body.glass_channel.width", self.glass_channel.width)?;
        positive("body.glass_channel.depth", self.glass_channel.depth)?;
        require(self.glass_channel.depth < self.wall_thickness, || {
            format!(
                "`body.glass_channel.depth` {} must be less than `body.wall_thickness` {}",
                self.glass_channel.depth, self.wall_thickness
            )
        })?;
        non_negative("body.lockdown_bar.height", self.lockdown_bar.height)?;
        positive("body.lockdown_bar.depth", self.lockdown_bar.depth)?;
        positive("body.side_rail.width", self.side_rail.width)?;
        positive("body.side_rail.height", self.side_rail.height)
    }
}

impl Leg {
    fn check(&self, name: &str, body: &Body) -> Result<(), SchemaError> {
        let p = |f: &str| format!("legs.{name}.{f}");
        positive(&p("length"), self.length)?;
        positive(&p("section.top"), self.section.top)?;
        positive(&p("section.bottom"), self.section.bottom)?;
        colour(&p("colour"), &self.colour)?;
        let lv = &self.leveler;
        positive(&p("leveler.thread_length"), lv.thread_length)?;
        positive(&p("leveler.thread_diameter"), lv.thread_diameter)?;
        positive(&p("leveler.foot_diameter"), lv.foot_diameter)?;
        non_negative(&p("leveler.extension_min"), lv.extension_min)?;
        let range_ok = lv.extension_max.is_finite()
            && lv.extension_max >= lv.extension_min
            && lv.extension_max - lv.extension_min <= lv.thread_length;
        require(range_ok, || {
            format!(
                "`{}` {} must be in [extension_min {}, extension_min + thread_length {}]",
                p("leveler.extension_max"),
                lv.extension_max,
                lv.extension_min,
                lv.extension_min + lv.thread_length
            )
        })?;
        let ext_ok = lv.extension.is_finite()
            && lv.extension >= lv.extension_min
            && lv.extension <= lv.extension_max;
        require(ext_ok, || {
            format!(
                "`{}` {} is outside [{}, {}]",
                p("leveler.extension"),
                lv.extension,
                lv.extension_min,
                lv.extension_max
            )
        })?;
        require(self.length + lv.extension_min > body.leg_overlap, || {
            format!(
                "`{}` {} with the leveler fully in does not reach below the body \
                 (`body.leg_overlap` {})",
                p("length"),
                self.length,
                body.leg_overlap
            )
        })?;
        let [ix, iy] = self.corner_inset;
        let inset_ok = ix.is_finite()
            && iy.is_finite()
            && ix >= 0.0
            && iy >= 0.0
            && ix < body.width * 0.5
            && iy < body.length * 0.5;
        require(inset_ok, || {
            format!(
                "`{}` {:?} must be >= 0 and less than half the body width and length",
                p("corner_inset"),
                self.corner_inset
            )
        })
    }
}

impl Backbox {
    fn check(&self, body: &Body) -> Result<(), SchemaError> {
        positive("backbox.width", self.width)?;
        positive("backbox.height", self.height)?;
        positive("backbox.depth", self.depth)?;
        finite_all("backbox.front_y", &[self.front_y])?;
        require(self.front_y >= 0.0 && self.front_y < body.length, || {
            format!(
                "`backbox.front_y` {} must be in [0, body.length {})",
                self.front_y, body.length
            )
        })?;
        require(
            self.bottom_z.is_finite() && self.bottom_z >= body.rear_height,
            || {
                format!(
                    "`backbox.bottom_z` {} must be at least `body.rear_height` {}",
                    self.bottom_z, body.rear_height
                )
            },
        )?;
        positive("backbox.speaker_panel_height", self.speaker_panel_height)?;
        require(self.speaker_panel_height < self.height, || {
            format!(
                "`backbox.speaker_panel_height` {} must be less than `backbox.height` {}",
                self.speaker_panel_height, self.height
            )
        })?;
        let bg = &self.backglass;
        positive("backbox.backglass.width", bg.width)?;
        positive("backbox.backglass.height", bg.height)?;
        positive("backbox.backglass.thickness", bg.thickness)?;
        require(bg.width <= self.width, || {
            format!(
                "`backbox.backglass.width` {} exceeds `backbox.width` {}",
                bg.width, self.width
            )
        })?;
        require(bg.height <= self.height, || {
            format!(
                "`backbox.backglass.height` {} exceeds `backbox.height` {}",
                bg.height, self.height
            )
        })?;
        let t = &self.translite_opening;
        positive("backbox.translite_opening.width", t.width)?;
        positive("backbox.translite_opening.height", t.height)?;
        require(t.width <= bg.width, || {
            format!(
                "`backbox.translite_opening.width` {} exceeds the backglass width {}",
                t.width, bg.width
            )
        })?;
        require(t.height <= bg.height, || {
            format!(
                "`backbox.translite_opening.height` {} exceeds the backglass height {}",
                t.height, bg.height
            )
        })?;
        require(
            t.bottom.is_finite()
                && t.bottom >= self.speaker_panel_height
                && t.bottom + t.height <= self.height,
            || {
                format!(
                    "`backbox.translite_opening.bottom` {} must put the opening between the \
                     speaker panel ({}) and the backbox top ({})",
                    t.bottom, self.speaker_panel_height, self.height
                )
            },
        )?;
        let s = &self.score_display;
        positive("backbox.score_display.width", s.width)?;
        positive("backbox.score_display.height", s.height)?;
        require(s.pixels[0] > 0 && s.pixels[1] > 0, || {
            format!("`backbox.score_display.pixels` {:?} must be > 0", s.pixels)
        })?;
        require(
            s.width <= self.width
                && s.centre_v.is_finite()
                && s.centre_v - s.height * 0.5 >= 0.0
                && s.centre_v + s.height * 0.5 <= self.speaker_panel_height,
            || {
                format!(
                    "`backbox.score_display` window {} x {} at v {} must lie in the speaker \
                     panel ({} x {})",
                    s.width, s.height, s.centre_v, self.width, self.speaker_panel_height
                )
            },
        )?;
        require(self.grilles.len() <= MAX_GRILLES, || {
            format!(
                "`backbox.grilles` lists {} grilles, at most {MAX_GRILLES}",
                self.grilles.len()
            )
        })?;
        for (i, g) in self.grilles.iter().enumerate() {
            let path = format!("backbox.grilles[{i}]");
            positive(&format!("{path}.diameter"), g.diameter)?;
            let r = g.diameter * 0.5;
            let inside = g.u.is_finite()
                && g.v.is_finite()
                && g.u - r >= -self.width * 0.5
                && g.u + r <= self.width * 0.5
                && g.v - r >= 0.0
                && g.v + r <= self.speaker_panel_height;
            require(inside, || {
                format!(
                    "`{path}` (u {}, v {}, diameter {}) must lie in the speaker panel",
                    g.u, g.v, g.diameter
                )
            })?;
        }
        Ok(())
    }
}

impl CabinetGeometry {
    /// The built-in preset `id` (SPEC Appendix E), resolved.
    ///
    /// An unknown id is [`SchemaError::Invalid`] naming `preset`.
    pub fn preset(id: &str) -> Result<CabinetGeometry, SchemaError> {
        let map = presets().as_ref().map_err(Clone::clone)?;
        map.get(id).cloned().ok_or_else(|| {
            invalid(format!(
                "`preset` = {id:?} is not a cabinet preset (known: {})",
                preset_ids().join(", ")
            ))
        })
    }

    /// The cabinet a table gets without a `cabinet.toml`: [`DEFAULT_PRESET`].
    pub fn default_preset() -> Result<CabinetGeometry, SchemaError> {
        Self::preset(DEFAULT_PRESET)
    }

    /// Self-consistency (REQ-CAB-005): dimensions finite and > 0 where physically
    /// required, leveler extensions within their range, legs inside the body footprint,
    /// mount angles within ±[`MOUNT_ANGLE_LIMIT_DEG`], backbox parts inside the backbox,
    /// valid colours and art paths. Each error names its field.
    pub fn check_consistency(&self) -> Result<(), SchemaError> {
        let b = &self.body;
        b.check()?;
        for (name, leg) in self.legs.all() {
            leg.check(name, b)?;
        }
        let m = &self.mount;
        finite_all("mount.offset_x", &[m.offset_x])?;
        require(m.inset.is_finite() && m.inset >= b.wall_thickness, || {
            format!(
                "`mount.inset` {} must be at least `body.wall_thickness` {}",
                m.inset, b.wall_thickness
            )
        })?;
        positive("mount.height", m.height)?;
        for (path, v) in [
            ("mount.pitch_deg", m.pitch_deg),
            ("mount.roll_deg", m.roll_deg),
        ] {
            require(v.is_finite() && v.abs() <= MOUNT_ANGLE_LIMIT_DEG, || {
                format!("`{path}` {v} must be within ±{MOUNT_ANGLE_LIMIT_DEG}°")
            })?;
        }
        let gp = &self.glass_panel;
        positive("glass_panel.width", gp.width)?;
        positive("glass_panel.length", gp.length)?;
        positive("glass_panel.thickness", gp.thickness)?;
        require(gp.thickness <= b.glass_channel.width, || {
            format!(
                "`glass_panel.thickness` {} does not fit `body.glass_channel.width` {}",
                gp.thickness, b.glass_channel.width
            )
        })?;
        let span = b.width - 2.0 * b.wall_thickness + 2.0 * b.glass_channel.depth;
        require(gp.width <= span, || {
            format!(
                "`glass_panel.width` {} exceeds the glass channel span {span} (interior + 2 x channel depth)",
                gp.width
            )
        })?;
        self.backbox.check(b)?;
        let c = &self.controls;
        for (path, v) in [
            ("controls.left_flipper_button", &c.left_flipper_button),
            ("controls.right_flipper_button", &c.right_flipper_button),
            ("controls.start_button", &c.start_button),
            ("controls.action_button", &c.action_button),
            ("controls.plunger", &c.plunger),
            ("controls.coin_door.centre", &c.coin_door.centre),
        ] {
            finite_all(path, v)?;
        }
        positive("controls.coin_door.width", c.coin_door.width)?;
        positive("controls.coin_door.height", c.coin_door.height)?;
        colour("trim.rail_colour", &self.trim.rail_colour)?;
        require(self.art.len() <= MAX_ART_ENTRIES, || {
            format!(
                "`art` lists {} entries, at most {MAX_ART_ENTRIES}",
                self.art.len()
            )
        })?;
        for (id, file) in &self.art {
            art_entry(id, file)?;
        }
        Ok(())
    }

    /// Height above the floor of the body bottom at cabinet-frame `y` (mm), from the
    /// front and rear leg pairs' mean effective lengths (linear between the leg
    /// centrelines) less `body.leg_overlap`.
    pub fn body_bottom_height_mm(&self, y: f64) -> f64 {
        let l = &self.legs;
        let front =
            (l.front_left.effective_length_mm() + l.front_right.effective_length_mm()) * 0.5;
        let rear = (l.rear_left.effective_length_mm() + l.rear_right.effective_length_mm()) * 0.5;
        let (_, sy) = leg_spacing(self);
        let (_, fly) = leg_xy(&self.body, &l.front_left, false, false);
        let (_, fry) = leg_xy(&self.body, &l.front_right, true, false);
        let y_front = (fly + fry) * 0.5;
        let at = if sy > 0.0 {
            front + (rear - front) * (y - y_front) / sy
        } else {
            front
        };
        at - f64::from(self.body.leg_overlap)
    }

    /// Height above the floor (mm) of a body-fixed point at cabinet-frame `y` with `z`
    /// above the body bottom.
    pub fn floor_height_mm(&self, y: f64, z: f64) -> f64 {
        self.body_bottom_height_mm(y) + z
    }

    /// Floor to the top of the lockdown bar, mm.
    pub fn lockdown_bar_top_height_mm(&self) -> f64 {
        let b = &self.body;
        self.floor_height_mm(
            0.0,
            f64::from(b.front_height) + f64::from(b.lockdown_bar.height),
        )
    }

    /// Floor to the top of the backbox, mm.
    pub fn backbox_top_height_mm(&self) -> f64 {
        let bb = &self.backbox;
        self.floor_height_mm(
            f64::from(bb.front_y),
            f64::from(bb.bottom_z) + f64::from(bb.height),
        )
    }

    /// Side wall top above the body bottom at cabinet-frame `y`, mm.
    fn wall_top(&self, y: f64) -> f64 {
        let b = &self.body;
        let f = f64::from(b.front_height);
        f + (f64::from(b.rear_height) - f) * y / f64::from(b.length)
    }

    /// Checks that the cabinet contains the table (REQ-CAB-005) and agrees with its
    /// `meta.pitch_deg` (REQ-CAB-008), after [`Self::check_consistency`]. In order:
    ///
    /// 1. the envelope's values are finite (> 0 for sizes);
    /// 2. playfield outline: interior width (`body.width` − 2 × wall) ≥ playfield width
    ///    (`body.width`); the playfield lies between the side walls (`mount.offset_x`);
    ///    `body.length` − wall ≥ `mount.inset` + playfield length (`body.length`); the
    ///    glass sheet covers the playfield (`glass_panel.width`, `glass_panel.length`);
    /// 3. glass ≥ tallest ramp or toy (`glass.height`); the playfield surface and the glass
    ///    sheet's top at the playfield's front and rear edges stay at or above the body
    ///    bottom and at or below the side wall top (`mount.height`), measured at all four
    ///    corners from the playfield origin (the front-left surface corner), so a rolled
    ///    playfield is checked at its raised side and at its lowered side;
    /// 4. backbox clear of the playfield: either its front face is at or behind the
    ///    playfield's rear edge, or its bottom is above the highest glass-top corner
    ///    (`backbox.front_y` / `backbox.bottom_z`);
    /// 5. `meta.pitch_deg`, when given, within [`PITCH_AGREEMENT_DEG`] (plus
    ///    [`PITCH_ROUNDING_SLACK_DEG`]) of the derived pitch (`meta.pitch_deg`).
    ///
    /// Containment uses the mount angles relative to the body (the body frame).
    pub fn validate(&self, table: &TableEnvelope) -> Result<(), SchemaError> {
        self.check_consistency()?;
        positive("table.playfield_width_mm", table.playfield_width_mm)?;
        positive("table.playfield_length_mm", table.playfield_length_mm)?;
        positive("glass.height", table.glass_height_mm)?;
        non_negative("table.max_part_height_mm", table.max_part_height_mm)?;
        if let Some(p) = table.pitch_deg {
            finite_all("meta.pitch_deg", &[p])?;
        }
        let b = &self.body;
        let m = &self.mount;
        let pw = f64::from(table.playfield_width_mm);
        let pl = f64::from(table.playfield_length_mm);
        let wall = f64::from(b.wall_thickness);
        let interior = f64::from(b.width) - 2.0 * wall;

        // 2. Playfield outline.
        require(interior >= pw, || {
            format!(
                "`body.width` {} gives an interior of {interior} mm (minus 2 x wall {wall}), \
                 narrower than the playfield width {pw} mm",
                b.width
            )
        })?;
        let ox = f64::from(m.offset_x);
        require(
            ox - pw * 0.5 >= -interior * 0.5 && ox + pw * 0.5 <= interior * 0.5,
            || {
                format!(
                    "`mount.offset_x` {ox} puts the {pw} mm playfield outside the {interior} mm interior"
                )
            },
        )?;
        let inset = f64::from(m.inset);
        require(f64::from(b.length) - wall >= inset + pl, || {
            format!(
                "`body.length` {} minus the rear wall {wall} is shorter than `mount.inset` {inset} \
                 plus the playfield length {pl} mm",
                b.length
            )
        })?;
        let gp = &self.glass_panel;
        require(f64::from(gp.width) >= pw, || {
            format!(
                "`glass_panel.width` {} does not cover the playfield width {pw} mm",
                gp.width
            )
        })?;
        require(f64::from(gp.length) >= pl, || {
            format!(
                "`glass_panel.length` {} does not cover the playfield length {pl} mm",
                gp.length
            )
        })?;

        // 3. Glass above every part, and inside the body.
        require(table.glass_height_mm >= table.max_part_height_mm, || {
            format!(
                "`glass.height` {} mm is below the tallest ramp or toy ({} mm above the playfield)",
                table.glass_height_mm, table.max_part_height_mm
            )
        })?;
        let pitch = deg_to_rad(f64::from(m.pitch_deg));
        let roll = deg_to_rad(f64::from(m.roll_deg));
        let h = f64::from(m.height);
        let corners = playfield_corners(
            inset,
            h,
            pw,
            pl,
            pitch,
            roll,
            f64::from(table.glass_height_mm) + f64::from(gp.thickness),
        );
        for c in &corners {
            let wall_top = self.wall_top(c.y);
            let (z, edge, side) = (c.glass_z, c.edge, c.side);
            let low = c.surface_z;
            require(low >= 0.0 && z >= 0.0, || {
                let bad = if low < 0.0 { low } else { z };
                format!(
                    "`mount.height` {h}: the playfield's {edge} edge, {side} corner \
                     ({bad:.1} mm above the body bottom) is below the body bottom"
                )
            })?;
            require(low <= wall_top && z <= wall_top, || {
                format!(
                    "`mount.height` {h}: the glass top at the playfield's {edge} edge, {side} \
                     corner ({z:.1} mm above the body bottom) is above the side wall top \
                     ({wall_top:.1} mm)"
                )
            })?;
        }

        // 4. Backbox clear of the playfield.
        let bb = &self.backbox;
        let rear_y = corners[2].y;
        let highest = corners
            .iter()
            .fold(f64::NEG_INFINITY, |acc, c| max_f64(acc, c.glass_z));
        let behind = f64::from(bb.front_y) >= rear_y;
        let above = f64::from(bb.bottom_z) >= highest;
        require(behind || above, || {
            format!(
                "`backbox.front_y` {} is in front of the playfield's rear edge ({rear_y:.1} mm) and \
                 `backbox.bottom_z` {} is below the glass top ({highest:.1} mm): the backbox \
                 overlaps the playfield",
                bb.front_y, bb.bottom_z
            )
        })?;

        // 5. meta.pitch_deg agreement.
        if let Some(p) = table.pitch_deg {
            let derived = playfield_attitude(self).pitch_deg();
            let diff = libm::fabs(f64::from(p) - derived);
            require(
                diff <= PITCH_AGREEMENT_DEG + PITCH_ROUNDING_SLACK_DEG,
                || {
                    format!(
                        "`meta.pitch_deg` {p}° disagrees with the cabinet's derived pitch {derived:.4}° \
                     by {diff:.4}° (allowed {PITCH_AGREEMENT_DEG}°)"
                    )
                },
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn std_geom() -> CabinetGeometry {
        CabinetGeometry::default_preset().unwrap()
    }

    /// REQ-CAB-005, REQ-BUILD-002: corner heights from the front-left origin. Roll raises
    /// only the right corners by `width * sin(roll)`; at roll 0 the corners equal the
    /// pitch-only edge heights bit for bit.
    #[test]
    fn playfield_corners_from_origin_req_cab_005() {
        let (w, l, g) = (514.0, 1070.0, 116.8);
        let c = playfield_corners(30.0, 80.0, w, l, 0.0, deg_to_rad(45.0), g);
        let s45 = libm::sin(deg_to_rad(45.0));
        assert_eq!(c[0].surface_z, 80.0);
        assert!((c[1].surface_z - (80.0 + w * s45)).abs() < 1e-9);
        assert!((c[1].surface_z - 443.45).abs() < 0.1, "{}", c[1].surface_z);
        assert!((c[0].glass_z - (80.0 + g * s45)).abs() < 1e-9);
        assert_eq!(c[2].surface_z, 80.0);
        assert_eq!((c[0].y, c[2].y), (30.0, 30.0 + l));
        assert_eq!((c[1].edge, c[1].side), ("front", "right"));
        assert_eq!((c[3].edge, c[3].side), ("rear", "right"));
        // Negative roll lowers the right side; the left stays at the origin height.
        let n = playfield_corners(30.0, 80.0, w, l, 0.0, deg_to_rad(-10.0), g);
        assert_eq!(n[0].surface_z, 80.0);
        assert!(n[1].surface_z < 80.0);
        // Roll 0: bit-identical to the pitch-only formulas.
        let p = deg_to_rad(6.5);
        let (sp, cp) = (libm::sin(p), libm::cos(p));
        let z = playfield_corners(30.0, 283.0, w, l, p, 0.0, g);
        for k in [0, 1] {
            assert_eq!(z[k].glass_z.to_bits(), (283.0 + g * cp).to_bits());
        }
        for k in [2, 3] {
            assert_eq!(z[k].glass_z.to_bits(), (283.0 + l * sp + g * cp).to_bits());
            assert_eq!(z[k].y.to_bits(), (30.0 + l * cp).to_bits());
        }
    }

    /// REQ-CAB-005: the glass sheet's top is never below the surface corner it sits on
    /// (the glass offset `glass * cos(pitch) * cos(roll)` is >= 0 for a non-negative glass
    /// height), so for the rolled and pitched cases the surface-below-bottom check
    /// subsumes a glass-below-bottom one; `validate` still tests both.
    #[test]
    fn glass_never_below_surface_req_cab_005() {
        for (pitch, roll) in [
            (0.0, -45.0),
            (0.0, -8.0),
            (-6.5, 0.0),
            (6.5, -9.1),
            (-6.5, 45.0),
        ] {
            let c = playfield_corners(
                30.0,
                20.0,
                514.0,
                1070.0,
                deg_to_rad(pitch),
                deg_to_rad(roll),
                116.8,
            );
            for k in &c {
                assert!(k.glass_z >= k.surface_z, "{pitch} {roll} {k:?}");
            }
        }
    }

    #[test]
    fn presets_load_and_ids_are_sorted_appendix_e() {
        assert_eq!(
            preset_ids(),
            ["classic_standard", "classic_widebody", "modern_standard"]
        );
        assert_eq!(CABINET_FILE, "cabinet.toml");
    }

    /// REQ-CAB-008: same input, same bits, input not mutated; signature by reference
    /// in, value out.
    #[test]
    fn attitude_is_a_pure_function_req_cab_008() {
        let f: fn(&CabinetGeometry) -> Attitude = playfield_attitude;
        let g = std_geom();
        let before = g.clone();
        let a = f(&g);
        let b = f(&g);
        assert_eq!(a.pitch_rad.to_bits(), b.pitch_rad.to_bits());
        assert_eq!(a.roll_rad.to_bits(), b.roll_rad.to_bits());
        assert_eq!(g, before);
    }

    #[test]
    fn deg_rad_helpers() {
        assert!((rad_to_deg(core::f64::consts::PI) - 180.0).abs() < 1e-12);
        assert!((deg_to_rad(180.0) - core::f64::consts::PI).abs() < 1e-12);
        let a = Attitude {
            pitch_rad: deg_to_rad(6.5),
            roll_rad: deg_to_rad(-1.0),
        };
        assert!((a.pitch_deg() - 6.5).abs() < 1e-12);
        assert!((a.roll_deg() + 1.0).abs() < 1e-12);
    }

    #[test]
    fn broken_preset_data_is_an_error_not_a_panic() {
        assert!(matches!(load_presets("x = "), Err(SchemaError::Invalid(_))));
        assert!(matches!(
            load_presets("schema_version = 2\n[presets]\n"),
            Err(SchemaError::UnsupportedVersion(2))
        ));
        let e = load_presets("schema_version = 1\n[presets.p.body]\nwidth = 1.0\n").unwrap_err();
        assert!(e.to_string().contains("body.length"), "{e}");
    }

    /// REQ-BUILD-003: partial over partial keeps the upper value and the lower's rest,
    /// and an upper `[legs.default]` beats a lower per-leg value.
    #[test]
    fn overlay_of_partials_req_build_003() {
        let lower = CabinetOverride {
            body: Some(BodyOverride {
                width: Some(1.0),
                length: Some(2.0),
                ..Default::default()
            }),
            legs: Some(LegsOverride {
                front_left: Some(LegOverride {
                    length: Some(700.0),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            art: Some(BTreeMap::from([("a".to_owned(), "x.png".to_owned())])),
            ..Default::default()
        };
        let upper = CabinetOverride {
            body: Some(BodyOverride {
                width: Some(3.0),
                ..Default::default()
            }),
            legs: Some(LegsOverride {
                default: Some(LegOverride {
                    length: Some(800.0),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            art: Some(BTreeMap::from([("b".to_owned(), "y.png".to_owned())])),
            ..Default::default()
        };
        let o = CabinetGeometry::overlay(Some(&lower), Some(&upper)).unwrap();
        let body = o.body.unwrap();
        assert_eq!((body.width, body.length), (Some(3.0), Some(2.0)));
        let legs = o.legs.unwrap();
        assert_eq!(legs.front_left.unwrap().length, Some(800.0));
        assert_eq!(legs.rear_right.unwrap().length, Some(800.0));
        assert_eq!(o.art.unwrap().len(), 2);
        assert_eq!(CabinetGeometry::overlay(None, None), None);
        assert_eq!(
            CabinetGeometry::overlay(Some(&lower), None).as_ref(),
            Some(&lower)
        );
        assert_eq!(
            CabinetGeometry::overlay(None, Some(&upper)).as_ref(),
            Some(&upper)
        );
        assert_eq!(Legs::overlay(None, None), None);
        assert!(Legs::overlay(lower.legs.as_ref(), None).is_some());
        assert!(Legs::overlay(None, upper.legs.as_ref()).is_some());
        assert_eq!(<BTreeMap<String, String>>::overlay(None, None), None);
    }

    /// REQ-CAB-005: each self-consistency rule names its field.
    #[test]
    fn consistency_errors_name_the_field_req_cab_005() {
        type Edit = fn(&mut CabinetGeometry);
        let cases: &[(&str, Edit)] = &[
            ("body.width", |g| g.body.width = 0.0),
            ("body.wall_thickness", |g| g.body.wall_thickness = 400.0),
            ("body.leg_overlap", |g| g.body.leg_overlap = 1000.0),
            ("body.glass_channel.depth", |g| {
                g.body.glass_channel.depth = 30.0
            }),
            ("body.lockdown_bar.height", |g| {
                g.body.lockdown_bar.height = -1.0
            }),
            ("legs.front_right.colour", |g| {
                g.legs.front_right.colour = "red".into()
            }),
            ("legs.rear_left.leveler.extension_max", |g| {
                g.legs.rear_left.leveler.extension_max = 500.0
            }),
            ("legs.rear_right.leveler.extension", |g| {
                g.legs.rear_right.leveler.extension = 45.0
            }),
            ("legs.front_left.length", |g| {
                g.legs.front_left.length = 100.0
            }),
            ("legs.front_left.corner_inset", |g| {
                g.legs.front_left.corner_inset = [300.0, 10.0]
            }),
            ("mount.inset", |g| g.mount.inset = 5.0),
            ("mount.height", |g| g.mount.height = 0.0),
            ("mount.pitch_deg", |g| g.mount.pitch_deg = 46.0),
            ("mount.roll_deg", |g| g.mount.roll_deg = -46.0),
            ("mount.offset_x", |g| g.mount.offset_x = f32::NAN),
            ("glass_panel.thickness", |g| g.glass_panel.thickness = 10.0),
            ("glass_panel.width", |g| g.glass_panel.width = 600.0),
            ("backbox.front_y", |g| g.backbox.front_y = -1.0),
            ("backbox.bottom_z", |g| g.backbox.bottom_z = 10.0),
            ("backbox.speaker_panel_height", |g| {
                g.backbox.speaker_panel_height = 800.0
            }),
            ("backbox.backglass.width", |g| {
                g.backbox.backglass.width = 800.0
            }),
            ("backbox.backglass.height", |g| {
                g.backbox.backglass.height = 800.0
            }),
            ("backbox.translite_opening.width", |g| {
                g.backbox.translite_opening.width = 700.0
            }),
            ("backbox.translite_opening.height", |g| {
                g.backbox.translite_opening.height = 480.0
            }),
            ("backbox.translite_opening.bottom", |g| {
                g.backbox.translite_opening.bottom = 100.0
            }),
            ("backbox.score_display.pixels", |g| {
                g.backbox.score_display.pixels = [0, 32]
            }),
            ("backbox.score_display", |g| {
                g.backbox.score_display.centre_v = 190.0
            }),
            ("backbox.grilles[1]", |g| g.backbox.grilles[1].u = 400.0),
            ("backbox.grilles[0].diameter", |g| {
                g.backbox.grilles[0].diameter = 0.0
            }),
            ("backbox.grilles", |g| {
                g.backbox.grilles = vec![g.backbox.grilles[0].clone(); MAX_GRILLES + 1]
            }),
            ("controls.plunger", |g| {
                g.controls.plunger[2] = f32::INFINITY
            }),
            ("controls.coin_door.width", |g| {
                g.controls.coin_door.width = 0.0
            }),
            ("trim.rail_colour", |g| g.trim.rail_colour = "#12345".into()),
            ("art", |g| {
                g.art.insert("Bad ID".into(), "x.png".into());
            }),
            ("art.side", |g| {
                g.art.insert("side".into(), "../x.png".into());
            }),
            ("art", |g| {
                for i in 0..=MAX_ART_ENTRIES {
                    g.art.insert(format!("a{i}"), "x.png".into());
                }
            }),
        ];
        assert!(std_geom().check_consistency().is_ok());
        for (field, edit) in cases {
            let mut g = std_geom();
            edit(&mut g);
            match g.check_consistency() {
                Err(SchemaError::Invalid(m)) => {
                    assert!(m.contains(&format!("`{field}")), "{field}: {m}")
                }
                other => panic!("{field}: expected Invalid, got {other:?}"),
            }
        }
    }

    #[test]
    fn spacing_degenerate_body_bottom_is_finite() {
        let mut g = std_geom();
        g.legs.front_left.corner_inset[1] = g.body.length;
        g.legs.front_right.corner_inset[1] = g.body.length;
        g.legs.rear_left.corner_inset[1] = 0.0;
        g.legs.rear_right.corner_inset[1] = 0.0;
        assert!(g.body_bottom_height_mm(100.0).is_finite());
        let a = playfield_attitude(&g);
        assert!(a.pitch_rad.is_finite() && a.roll_rad.is_finite());
    }
}
