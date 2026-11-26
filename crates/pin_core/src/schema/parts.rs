//! Part styles, the material library, part placement and the table theme
//! (SPEC Appendix A.5; REQ-BUILD-001 to REQ-BUILD-005).
//!
//! * The **material library** ([`MaterialDef`]) names how a part looks (render parameters)
//!   and, for a part the ball can touch, how it plays: `physical` names a [`Material`] of
//!   the Appendix C material-pair table. The built-in library ships as data
//!   ([`MATERIALS_TOML`]); a table's `[[materials]]` extends it, and an entry whose `id`
//!   equals a built-in replaces that entry whole.
//! * **Part styles** ([`PartStyleDef`]) supply defaults for a [`PartKind`]. Built-in styles
//!   ship as data ([`PART_STYLES_TOML`]); the style whose id is the kind's name is the
//!   kind default and the root of every chain of that kind. A table's `[[part_styles]]`
//!   adds styles (from `base` or the kind default) or overrides a built-in field by field.
//! * **Parts** ([`PartDef`], `table.toml` `[[parts]]`) place a styled part in the playfield
//!   frame or `bind` a style to an existing element (a mech, flipper, lamp, spline or switch
//!   of `table.toml`, or a `layout.toml` element by array path such as `posts[3]`).
//!   Precedence for every field: part instance > table style (and its `base` chain) >
//!   built-in style > kind default. For `layout.toml` elements the element's own keys
//!   (`radius`, `rubber`, `material`, `width`, `shape`, `size`, positions and points) are
//!   instance values and win.
//! * **Theme** ([`ThemeDef`], `table.toml` `[theme]`) drives the generated art;
//!   [`TableManifest::effective_theme`] falls back to `meta.name` and `meta.palette`.
//!
//! Everything here is pure: no I/O, no panics on any input, bounded collections and
//! `base` chains (R11), and fixed iteration order (`BTreeMap`, R3).

use super::layout::{InsertShape, LayoutManifest};
use super::table::{LampKind, MechKind, SplineKind, SwitchKind, TableManifest};
use super::{PhysicsDefaults, SchemaError};
use crate::materials::Material;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// The built-in material library, as TOML text (`schema_version = 1`, then `[[materials]]`).
pub const MATERIALS_TOML: &str = include_str!("../../data/materials.toml");

/// The built-in part styles and rubber-ring sizes, as TOML text (`schema_version = 1`,
/// then `[[ring_sizes]]` and `[[styles]]`).
pub const PART_STYLES_TOML: &str = include_str!("../../data/part_styles.toml");

/// Longest id (material, style, part, ring size, motif, font role), characters.
pub const MAX_ID_LEN: usize = 64;
/// Most `[[materials]]` entries in one table.
pub const MAX_MATERIALS: usize = 256;
/// Most `[[part_styles]]` entries in one table.
pub const MAX_PART_STYLES: usize = 1024;
/// Most `[[parts]]` entries in one table.
pub const MAX_PARTS: usize = 4096;
/// Most styles in one `base` chain, the style itself and the kind default included.
pub const MAX_STYLE_DEPTH: usize = 16;
/// Largest coordinate magnitude of a position or outline point, mm (the generator's limit).
pub const MAX_COORD_MM: f32 = 5000.0;
/// Largest dimension, mm (the generator's limit).
pub const MAX_SIZE_MM: f32 = 1000.0;
/// Smallest dimension, mm (the generator's limit).
pub const MIN_SIZE_MM: f32 = 0.1;
/// Largest rotation angle magnitude, degrees.
pub const MAX_ANGLE_DEG: f32 = 360.0;
/// Most points in one `outline` (the generator's per-element limit).
pub const MAX_POINTS: usize = 1024;
/// Largest `emissive_strength`.
pub const MAX_EMISSIVE_STRENGTH: f32 = 1000.0;
/// Most `theme.palette` colours.
pub const MAX_PALETTE: usize = 16;
/// Most `theme.fonts` entries.
pub const MAX_FONTS: usize = 8;
/// Longest `theme.title`, characters.
pub const MAX_TITLE_LEN: usize = 128;
/// Longest `theme.motif`, characters.
pub const MAX_MOTIF_LEN: usize = 32;
/// The motif of a table without `theme.motif`.
pub const DEFAULT_MOTIF: &str = "geometric";

fn invalid(msg: String) -> SchemaError {
    SchemaError::Invalid(msg)
}

/// True for a stable id: 1 to [`MAX_ID_LEN`] characters of `a-z`, `0-9`, `_`, `.`, `-` (D32).
pub fn is_valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_ID_LEN
        && s.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'.' | b'-')
        })
}

/// Parses an sRGB colour `#rrggbb` (either case).
pub fn parse_color(s: &str) -> Option<[u8; 3]> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

fn check_id(what: &str, s: &str) -> Result<(), SchemaError> {
    if is_valid_id(s) {
        Ok(())
    } else {
        Err(invalid(format!(
            "{what}: `{}` is not a valid id (1 to {MAX_ID_LEN} of a-z 0-9 _ . -)",
            truncate(s)
        )))
    }
}

fn check_color(what: &str, s: &str) -> Result<(), SchemaError> {
    match parse_color(s) {
        Some(_) => Ok(()),
        None => Err(invalid(format!(
            "{what}: `{}` is not a colour \"#rrggbb\"",
            truncate(s)
        ))),
    }
}

fn check_range(what: &str, v: f32, lo: f32, hi: f32) -> Result<(), SchemaError> {
    if v.is_finite() && v >= lo && v <= hi {
        Ok(())
    } else {
        Err(invalid(format!("{what} = {v} is outside {lo}..={hi}")))
    }
}

/// Shortens untrusted text quoted in an error message.
fn truncate(s: &str) -> String {
    s.chars().take(MAX_ID_LEN).collect()
}

// ---------------------------------------------------------------------------
// Materials.
// ---------------------------------------------------------------------------

fn default_roughness() -> f32 {
    0.5
}

fn default_alpha() -> f32 {
    1.0
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A `[[materials]]` entry: how a part looks and, when the ball can touch it, how it plays.
///
/// Render parameters follow Subphase 5.1 (metallic-roughness PBR plus clear coat, emission,
/// translucency and UV response). `physical` names a [`Material`] of the Appendix C
/// material-pair table (`steel`, `ceramic`, `plastic`, `resin`, `rubber`, `wood`,
/// `acrylic`); it is required for a material used by a part the ball can touch and absent
/// for purely visual materials. It is checked when a part uses the material.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaterialDef {
    /// Library id (D32 characters).
    pub id: String,
    /// Base colour, sRGB `"#rrggbb"`.
    pub base_color: String,
    /// Metalness, 0..=1 (default 0).
    #[serde(default)]
    pub metallic: f32,
    /// Perceptual roughness, 0..=1 (default 0.5).
    #[serde(default = "default_roughness")]
    pub roughness: f32,
    /// Clear-coat strength, 0..=1 (absent: none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clearcoat: Option<f32>,
    /// Emitted colour, sRGB `"#rrggbb"` (absent: no emission).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emissive: Option<String>,
    /// Emission multiplier, 0..=1000 (absent: 1 when `emissive` is given).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emissive_strength: Option<f32>,
    /// Opacity, 0..=1 (default 1).
    #[serde(default = "default_alpha")]
    pub alpha: f32,
    /// Light passes through (acrylic, translucent plastics, insert lenses).
    #[serde(default, skip_serializing_if = "is_false")]
    pub translucent: bool,
    /// Glows under the GI tint (UV-reactive plastics and paint).
    #[serde(default, skip_serializing_if = "is_false")]
    pub uv_reactive: bool,
    /// Physical material name for the material-pair table (Appendix C).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub physical: Option<String>,
}

impl MaterialDef {
    /// Checks the id, colours and ranges; errors start with `what` (for example
    /// ``material `chrome` (`materials[0]`)``).
    pub fn validate(&self, what: &str) -> Result<(), SchemaError> {
        check_id(what, &self.id)?;
        check_color(&format!("{what} `base_color`"), &self.base_color)?;
        check_range(&format!("{what} `metallic`"), self.metallic, 0.0, 1.0)?;
        check_range(&format!("{what} `roughness`"), self.roughness, 0.0, 1.0)?;
        if let Some(c) = self.clearcoat {
            check_range(&format!("{what} `clearcoat`"), c, 0.0, 1.0)?;
        }
        if let Some(e) = &self.emissive {
            check_color(&format!("{what} `emissive`"), e)?;
        }
        if let Some(s) = self.emissive_strength {
            check_range(
                &format!("{what} `emissive_strength`"),
                s,
                0.0,
                MAX_EMISSIVE_STRENGTH,
            )?;
        }
        check_range(&format!("{what} `alpha`"), self.alpha, 0.0, 1.0)?;
        if let Some(p) = &self.physical
            && p.len() > MAX_ID_LEN
        {
            return Err(invalid(format!("{what} `physical` is too long")));
        }
        Ok(())
    }

    /// The base colour as sRGB bytes, or `None` if it is not `#rrggbb`.
    pub fn base_rgb(&self) -> Option<[u8; 3]> {
        parse_color(&self.base_color)
    }
}

/// A library material as a part uses it.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedMaterial {
    /// The library entry (its `id` is the name the part used).
    pub def: MaterialDef,
    /// The physical material, `None` for a purely visual material.
    pub physical: Option<Material>,
}

// ---------------------------------------------------------------------------
// Part kinds and shapes.
// ---------------------------------------------------------------------------

macro_rules! kinds {
    ($( $(#[$m:meta])* $v:ident = $name:literal, touchable = $t:literal ; )*) => {
        /// The closed set of part kinds (`kind = "…"`, snake_case).
        #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum PartKind { $( $(#[$m])* $v, )* }

        impl PartKind {
            /// Every kind, in declaration order.
            pub const ALL: &'static [PartKind] = &[ $( PartKind::$v, )* ];

            /// The TOML name, which is also the id of the kind's default built-in style.
            pub const fn name(self) -> &'static str {
                match self { $( PartKind::$v => $name, )* }
            }

            /// True when the ball can touch a part of this kind; its material must then
            /// name a physical material.
            pub const fn is_touchable(self) -> bool {
                match self { $( PartKind::$v => $t, )* }
            }
        }
    };
}

kinds! {
    /// Metal or rubber-ringed post.
    Post = "post", touchable = true;
    /// Star post (plastic spacer with points).
    StarPost = "star_post", touchable = true;
    /// Rubber ring stretched around posts.
    RubberRing = "rubber_ring", touchable = true;
    /// Pop bumper.
    PopBumper = "pop_bumper", touchable = true;
    /// Slingshot (rubber band and plastic cover).
    Slingshot = "slingshot", touchable = true;
    /// Flipper bat.
    FlipperBat = "flipper_bat", touchable = true;
    /// Flipper rubber.
    FlipperRubber = "flipper_rubber", touchable = true;
    /// Drop target.
    DropTarget = "drop_target", touchable = true;
    /// Stand-up target.
    StandupTarget = "standup_target", touchable = true;
    /// Spinner flap.
    Spinner = "spinner", touchable = true;
    /// Gate flap.
    Gate = "gate", touchable = true;
    /// Lane guide rail.
    LaneGuide = "lane_guide", touchable = true;
    /// Wall.
    Wall = "wall", touchable = true;
    /// Wire ramp (wireform).
    Wireform = "wireform", touchable = true;
    /// Plastic ramp.
    PlasticRamp = "plastic_ramp", touchable = true;
    /// Raised deck (a `layout.toml` `[[decks]]` level).
    Deck = "deck", touchable = true;
    /// Playfield plastic (cut, printed sheet above ball height).
    Plastic = "plastic", touchable = false;
    /// General-illumination bulb.
    GiBulb = "gi_bulb", touchable = false;
    /// Flasher dome.
    FlasherDome = "flasher_dome", touchable = false;
    /// Playfield insert.
    Insert = "insert", touchable = false;
    /// Toy or decoration the ball does not touch.
    Decoration = "decoration", touchable = false;
}

impl PartKind {
    /// Parses a TOML kind name.
    pub fn from_name(name: &str) -> Option<PartKind> {
        PartKind::ALL.iter().copied().find(|k| k.name() == name)
    }

    /// The style fields that apply to this kind (SPEC A.5 table). Any other field set
    /// on a style or part of this kind is an error naming the part and the field.
    pub fn fields(self) -> &'static [StyleField] {
        use StyleField as F;
        match self {
            PartKind::Post => &[
                F::Shape,
                F::Diameter,
                F::Height,
                F::Material,
                F::TrimMaterial,
                F::Color,
            ],
            PartKind::StarPost => &[F::Diameter, F::Height, F::StarPoints, F::Material, F::Color],
            PartKind::RubberRing => &[F::RingSize, F::InnerDiameter, F::Section, F::Material],
            PartKind::PopBumper => &[
                F::Diameter,
                F::Height,
                F::Material,
                F::TrimMaterial,
                F::Color,
            ],
            PartKind::Slingshot => &[
                F::Length,
                F::Height,
                F::Thickness,
                F::Material,
                F::TrimMaterial,
                F::Color,
            ],
            PartKind::FlipperBat => &[F::Height, F::Material, F::Color],
            PartKind::FlipperRubber => &[F::Section, F::Material, F::Color],
            PartKind::DropTarget | PartKind::Spinner | PartKind::Gate | PartKind::PlasticRamp => {
                &[F::Width, F::Height, F::Thickness, F::Material, F::Color]
            }
            PartKind::StandupTarget => &[
                F::Shape,
                F::Width,
                F::Height,
                F::Thickness,
                F::Material,
                F::Color,
            ],
            PartKind::LaneGuide | PartKind::Wall => {
                &[F::Height, F::Thickness, F::Material, F::Color]
            }
            PartKind::Wireform => &[F::TubeRadius, F::TubeSides, F::Material, F::Color],
            PartKind::Deck => &[F::Thickness, F::Material, F::Color],
            PartKind::Plastic => &[
                F::Shape,
                F::Outline,
                F::Width,
                F::Length,
                F::Thickness,
                F::Material,
                F::Color,
            ],
            PartKind::GiBulb | PartKind::FlasherDome => {
                &[F::Diameter, F::Height, F::Material, F::Color]
            }
            PartKind::Insert => &[F::Shape, F::Size, F::Lift, F::Material, F::Color],
            PartKind::Decoration => &[
                F::Shape,
                F::Outline,
                F::Size,
                F::Diameter,
                F::Width,
                F::Length,
                F::Height,
                F::Thickness,
                F::Material,
                F::TrimMaterial,
                F::Color,
            ],
        }
    }

    /// True when `field` applies to this kind.
    pub fn allows(self, field: StyleField) -> bool {
        self.fields().contains(&field)
    }
}

/// Part outline shapes (`shape = "…"`); dimensions are the kind's fields, in mm.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    /// Circle of `diameter` (or `size`).
    Round,
    /// Rectangle `width` × `length` (or `size` square).
    Rectangle,
    /// The `outline` polygon (at least 3 points, part-local mm).
    Polygon,
    /// Stadium of `width` × `length`.
    Capsule,
    /// Star of `star_points` points.
    Star,
    /// Half-sphere of `diameter`.
    Dome,
    /// Arrow (inserts).
    Arrow,
    /// Triangle (inserts).
    Triangle,
}

impl From<InsertShape> for Shape {
    fn from(s: InsertShape) -> Shape {
        match s {
            InsertShape::Circle => Shape::Round,
            InsertShape::Arrow => Shape::Arrow,
            InsertShape::Triangle => Shape::Triangle,
            InsertShape::Rect => Shape::Rectangle,
        }
    }
}

// ---------------------------------------------------------------------------
// Style fields.
// ---------------------------------------------------------------------------

macro_rules! style_fields {
    ( $( $(#[$m:meta])* $f:ident : $t:ty => $variant:ident ),* $(,)? ) => {
        /// The overridable style fields, every one optional: the `overrides` table of a
        /// part, and (with `id`, `kind`, `base`) a style definition. Lengths mm.
        #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct StyleFields {
            $(
                $(#[$m])*
                #[serde(default, skip_serializing_if = "Option::is_none")]
                pub $f: Option<$t>,
            )*
        }

        /// A `[[part_styles]]` entry (`table.toml`) or a built-in style: defaults for one
        /// [`PartKind`]. A style without `base` starts from the kind default (the style
        /// whose id is the kind's name).
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct PartStyleDef {
            /// Style id (D32 characters).
            pub id: String,
            /// Kind the style is for.
            pub kind: PartKind,
            /// Style of the same kind this one starts from.
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub base: Option<String>,
            $(
                $(#[$m])*
                #[serde(default, skip_serializing_if = "Option::is_none")]
                pub $f: Option<$t>,
            )*
        }

        /// Names one style field.
        #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum StyleField {
            $( $(#[$m])* $variant, )*
        }

        impl StyleField {
            /// Every field, in declaration order.
            pub const ALL: &'static [StyleField] = &[ $( StyleField::$variant, )* ];

            /// The TOML key.
            pub const fn name(self) -> &'static str {
                match self { $( StyleField::$variant => stringify!($f), )* }
            }
        }

        impl StyleFields {
            /// True when `field` has a value.
            pub fn is_set(&self, field: StyleField) -> bool {
                match field { $( StyleField::$variant => self.$f.is_some(), )* }
            }

            /// Clears `field`.
            pub fn clear(&mut self, field: StyleField) {
                match field { $( StyleField::$variant => self.$f = None, )* }
            }

            /// Every field `upper` sets replaces this one's.
            pub fn overlay(&mut self, upper: &StyleFields) {
                $( if let Some(v) = &upper.$f { self.$f = Some(v.clone()); } )*
            }
        }

        impl PartStyleDef {
            /// The style's field values (without `id`, `kind`, `base`).
            pub fn fields(&self) -> StyleFields {
                StyleFields { $( $f: self.$f.clone(), )* }
            }
        }
    };
}

style_fields! {
    /// Outline shape.
    shape: Shape => Shape,
    /// Overall size of a one-number shape (inserts), mm.
    size: f32 => Size,
    /// Diameter, mm.
    diameter: f32 => Diameter,
    /// Width, mm.
    width: f32 => Width,
    /// Length, mm.
    length: f32 => Length,
    /// Height above the part's base, mm (posts, walls, guides: unset means
    /// `playfield.wall_height`).
    height: f32 => Height,
    /// Material thickness, mm.
    thickness: f32 => Thickness,
    /// Number of star points, 3..=24.
    star_points: u32 => StarPoints,
    /// Polygon outline, part-local `[x, y]` mm, at most 1024 points.
    outline: Vec<[f32; 2]> => Outline,
    /// Named rubber-ring size (built-in `[[ring_sizes]]` id).
    ring_size: String => RingSize,
    /// Rubber-ring inner diameter, mm (overrides the ring size's).
    inner_diameter: f32 => InnerDiameter,
    /// Rubber cross-section diameter, mm.
    section: f32 => Section,
    /// Primary material (library id); the one the ball touches.
    material: String => Material,
    /// Secondary material (caps, covers, skirts; library id).
    trim_material: String => TrimMaterial,
    /// Tint or decal colour, sRGB `"#rrggbb"`.
    color: String => Color,
    /// Wire tube radius, mm.
    tube_radius: f32 => TubeRadius,
    /// Sides of the wire tube polygon, 3..=64.
    tube_sides: u32 => TubeSides,
    /// Lift above the surface, mm, 0..=1000 (inserts).
    lift: f32 => Lift,
}

impl StyleFields {
    /// Checks every set field: it applies to `kind`, and its value is in range. Errors start
    /// with `what` and name the field.
    pub fn validate(&self, kind: PartKind, what: &str) -> Result<(), SchemaError> {
        for &f in StyleField::ALL {
            if self.is_set(f) && !kind.allows(f) {
                return Err(invalid(format!(
                    "{what}: field `{}` does not apply to kind `{}`",
                    f.name(),
                    kind.name()
                )));
            }
        }
        let size = |name: &str, v: Option<f32>| match v {
            Some(v) => check_range(&format!("{what} `{name}`"), v, MIN_SIZE_MM, MAX_SIZE_MM),
            None => Ok(()),
        };
        size("size", self.size)?;
        size("diameter", self.diameter)?;
        size("width", self.width)?;
        size("length", self.length)?;
        size("height", self.height)?;
        size("thickness", self.thickness)?;
        size("inner_diameter", self.inner_diameter)?;
        size("section", self.section)?;
        size("tube_radius", self.tube_radius)?;
        if let Some(l) = self.lift {
            check_range(&format!("{what} `lift`"), l, 0.0, MAX_SIZE_MM)?;
        }
        if let Some(n) = self.star_points
            && !(3..=24).contains(&n)
        {
            return Err(invalid(format!(
                "{what} `star_points` = {n} is outside 3..=24"
            )));
        }
        if let Some(n) = self.tube_sides
            && !(3..=64).contains(&n)
        {
            return Err(invalid(format!(
                "{what} `tube_sides` = {n} is outside 3..=64"
            )));
        }
        if let Some(o) = &self.outline {
            if o.len() > MAX_POINTS {
                return Err(invalid(format!(
                    "{what} `outline` has {} points (at most {MAX_POINTS})",
                    o.len()
                )));
            }
            for (i, p) in o.iter().enumerate() {
                for &c in p {
                    check_range(
                        &format!("{what} `outline[{i}]`"),
                        c,
                        -MAX_COORD_MM,
                        MAX_COORD_MM,
                    )?;
                }
            }
        }
        if let Some(m) = &self.material {
            check_id(&format!("{what} `material`"), m)?;
        }
        if let Some(m) = &self.trim_material {
            check_id(&format!("{what} `trim_material`"), m)?;
        }
        if let Some(r) = &self.ring_size {
            check_id(&format!("{what} `ring_size`"), r)?;
        }
        if let Some(c) = &self.color {
            check_color(&format!("{what} `color`"), c)?;
        }
        Ok(())
    }
}

/// A named rubber-ring size (built-in `[[ring_sizes]]`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RingSize {
    /// Size id.
    pub id: String,
    /// Inner diameter (relaxed), mm.
    pub inner_diameter: f32,
    /// Cross-section diameter, mm.
    pub section: f32,
}

// ---------------------------------------------------------------------------
// Placement.
// ---------------------------------------------------------------------------

/// A part's placement in the playfield frame (SPEC Appendix A.5, REQ-BUILD-002).
///
/// Frame: `x` across from the playfield's left edge, `y` up the table from its front edge,
/// `z` up from the playfield surface; right-handed, mm. `rotation_deg = [rx, ry, rz]` are
/// rotations about the fixed playfield axes `x`, `y`, `z`, applied in that order (`rz`
/// last), each counter-clockwise when looking from the positive axis toward the origin.
/// The part-local point `p` maps to `position + Rz(rz) · Ry(ry) · Rx(rx) · p`.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Placement {
    /// Position, mm.
    pub position: [f32; 3],
    /// Rotation about x, y, z, degrees.
    pub rotation_deg: [f32; 3],
}

impl Placement {
    /// The rotation matrix `Rz · Ry · Rx` (row-major, `f64`, `libm`).
    pub fn rotation_matrix(&self) -> [[f64; 3]; 3] {
        let rad = |d: f32| f64::from(d) * (core::f64::consts::PI / 180.0);
        let (sx, cx) = (
            libm::sin(rad(self.rotation_deg[0])),
            libm::cos(rad(self.rotation_deg[0])),
        );
        let (sy, cy) = (
            libm::sin(rad(self.rotation_deg[1])),
            libm::cos(rad(self.rotation_deg[1])),
        );
        let (sz, cz) = (
            libm::sin(rad(self.rotation_deg[2])),
            libm::cos(rad(self.rotation_deg[2])),
        );
        [
            [cz * cy, cz * sy * sx - sz * cx, cz * sy * cx + sz * sx],
            [sz * cy, sz * sy * sx + cz * cx, sz * sy * cx - cz * sx],
            [-sy, cy * sx, cy * cx],
        ]
    }

    /// Maps a part-local point (mm) to the playfield frame (mm).
    pub fn to_playfield(&self, local: [f64; 3]) -> [f64; 3] {
        let r = self.rotation_matrix();
        let mut out = [0.0; 3];
        for (i, row) in r.iter().enumerate() {
            out[i] = f64::from(self.position[i])
                + row[0] * local[0]
                + row[1] * local[1]
                + row[2] * local[2];
        }
        out
    }

    fn validate(&self, what: &str) -> Result<(), SchemaError> {
        for (i, &c) in self.position.iter().enumerate() {
            check_range(
                &format!("{what} `position[{i}]`"),
                c,
                -MAX_COORD_MM,
                MAX_COORD_MM,
            )?;
        }
        for (i, &a) in self.rotation_deg.iter().enumerate() {
            check_range(
                &format!("{what} `rotation_deg[{i}]`"),
                a,
                -MAX_ANGLE_DEG,
                MAX_ANGLE_DEG,
            )?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Parts.
// ---------------------------------------------------------------------------

fn is_zero3(v: &[f32; 3]) -> bool {
    *v == [0.0; 3]
}

/// A `table.toml` `[[parts]]` entry: one styled part.
///
/// Without `bind` the part stands alone and needs `position`. With `bind` it gives a style
/// to an existing element: `"<id>"` (or `"<section>:<id>"`, section `mechs`, `flippers`,
/// `lamps`, `splines` or `switches`) of the same `table.toml`, or `"<section>[<n>]"`
/// (section `walls`, `posts`, `lanes`, `ramps`, `decks` or `inserts`) of the table's
/// `layout.toml`. A bound part takes its position from the element and may give
/// `position` only when the element has none.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartDef {
    /// Part id (D32 characters), unique among `[[parts]]`.
    pub id: String,
    /// Kind.
    pub kind: PartKind,
    /// Style id (default: the kind default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    /// Element this part styles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind: Option<String>,
    /// Position `[x, y, z]`, mm, playfield frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[f32; 3]>,
    /// Rotation `[rx, ry, rz]`, degrees (see [`Placement`]); default `[0, 0, 0]`.
    #[serde(default, skip_serializing_if = "is_zero3")]
    pub rotation_deg: [f32; 3],
    /// Material (library id); the same as `overrides.material`, which may not also be given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material: Option<String>,
    /// Per-instance field overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overrides: Option<StyleFields>,
}

impl PartDef {
    /// The instance's field values: `overrides` plus `material`.
    pub fn instance_fields(&self) -> StyleFields {
        let mut f = self.overrides.clone().unwrap_or_default();
        if let Some(m) = &self.material {
            f.material = Some(m.clone());
        }
        f
    }

    /// Checks what the part states on its own; errors name `parts[index]` and the id.
    pub fn validate(&self, index: usize) -> Result<(), SchemaError> {
        let what = part_label(&self.id, index);
        check_id(&what, &self.id)?;
        if let Some(s) = &self.style {
            check_id(&format!("{what} `style`"), s)?;
        }
        if let Some(b) = &self.bind
            && (b.is_empty() || b.len() > 2 * MAX_ID_LEN)
        {
            return Err(invalid(format!("{what} `bind` is empty or too long")));
        }
        if let Some(m) = &self.material {
            check_id(&format!("{what} `material`"), m)?;
            if self
                .overrides
                .as_ref()
                .is_some_and(|o| o.material.is_some())
            {
                return Err(invalid(format!(
                    "{what}: give `material` or `overrides.material`, not both"
                )));
            }
        }
        if let Some(o) = &self.overrides {
            o.validate(self.kind, &what)?;
        }
        Placement {
            position: self.position.unwrap_or_default(),
            rotation_deg: self.rotation_deg,
        }
        .validate(&what)
    }
}

fn part_label(id: &str, index: usize) -> String {
    format!("part `{}` (`parts[{index}]`)", truncate(id))
}

/// A part with every field resolved (SPEC A.5 precedence).
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedPart {
    /// The part's name in messages: its id, or its `layout.toml` array path (`posts[3]`).
    pub name: String,
    /// Kind.
    pub kind: PartKind,
    /// Style used (the kind default when none was named).
    pub style: String,
    /// The bound element, if any.
    pub bind: Option<String>,
    /// Every resolved field value (fields not given anywhere stay `None`).
    pub fields: StyleFields,
    /// Resolved `material`.
    pub material: Option<ResolvedMaterial>,
    /// Resolved `trim_material`.
    pub trim_material: Option<ResolvedMaterial>,
    /// Placement in the playfield frame.
    pub placement: Placement,
}

impl ResolvedPart {
    /// The physical material of the primary material, if any.
    pub fn physical(&self) -> Option<Material> {
        self.material.as_ref().and_then(|m| m.physical)
    }
}

/// Checks that every ball-touchable part's physical material has at least one pair with a
/// ball material (steel, ceramic, plastic, resin) in `physics.material_pairs`
/// (REQ-BUILD-004). The error names the part and its material.
pub fn check_physical_pairs(
    parts: &[ResolvedPart],
    physics: &PhysicsDefaults,
) -> Result<(), SchemaError> {
    for p in parts.iter().filter(|p| p.kind.is_touchable()) {
        let Some(m) = &p.material else {
            return Err(invalid(format!(
                "part `{}`: kind `{}` is ball-touchable but has no material",
                p.name,
                p.kind.name()
            )));
        };
        let Some(phys) = m.physical else {
            return Err(invalid(format!(
                "part `{}`: material `{}` has no `physical` material",
                p.name, m.def.id
            )));
        };
        if !Material::BALL
            .iter()
            .any(|&b| physics.material_pair(b, phys).is_some())
        {
            return Err(invalid(format!(
                "part `{}`: material `{}` (physical `{}`) has no material pair with any ball material",
                p.name,
                m.def.id,
                phys.name()
            )));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Library.
// ---------------------------------------------------------------------------

/// The material library and part styles a table sees: the built-ins plus the table's
/// `[[materials]]` and `[[part_styles]]` (SPEC A.5). Lookups by name; iteration sorted.
#[derive(Clone, Debug, PartialEq)]
pub struct PartLibrary {
    materials: BTreeMap<String, MaterialDef>,
    builtin_styles: BTreeMap<String, PartStyleDef>,
    table_styles: BTreeMap<String, PartStyleDef>,
    ring_sizes: BTreeMap<String, RingSize>,
}

/// A style with its chain applied.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedStyle {
    /// Style id.
    pub id: String,
    /// Kind.
    pub kind: PartKind,
    /// Field values after the whole `base` chain.
    pub fields: StyleFields,
}

/// One layer of a style chain, root first.
struct ChainLayer {
    fields: StyleFields,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MaterialsFile {
    schema_version: u32,
    materials: Vec<MaterialDef>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StylesFile {
    schema_version: u32,
    ring_sizes: Vec<RingSize>,
    styles: Vec<PartStyleDef>,
}

fn load_builtin() -> Result<PartLibrary, SchemaError> {
    let bad = |what: &str, e: String| invalid(format!("built-in {what}: {e}"));
    let mf: MaterialsFile =
        toml::from_str(MATERIALS_TOML).map_err(|e| bad("materials", e.to_string()))?;
    let sf: StylesFile =
        toml::from_str(PART_STYLES_TOML).map_err(|e| bad("part styles", e.to_string()))?;
    if mf.schema_version != super::SCHEMA_VERSION {
        return Err(SchemaError::UnsupportedVersion(mf.schema_version));
    }
    if sf.schema_version != super::SCHEMA_VERSION {
        return Err(SchemaError::UnsupportedVersion(sf.schema_version));
    }
    let mut lib = PartLibrary {
        materials: BTreeMap::new(),
        builtin_styles: BTreeMap::new(),
        table_styles: BTreeMap::new(),
        ring_sizes: BTreeMap::new(),
    };
    for (i, m) in mf.materials.iter().enumerate() {
        let what = format!("built-in material `{}` (`materials[{i}]`)", truncate(&m.id));
        m.validate(&what)?;
        if let Some(p) = &m.physical
            && Material::from_name(p).is_none()
        {
            return Err(invalid(format!("{what}: unknown physical material `{p}`")));
        }
        if lib.materials.insert(m.id.clone(), m.clone()).is_some() {
            return Err(invalid(format!("{what}: duplicate id")));
        }
    }
    for r in &sf.ring_sizes {
        let what = format!("built-in ring size `{}`", truncate(&r.id));
        check_id(&what, &r.id)?;
        check_range(
            &format!("{what} `inner_diameter`"),
            r.inner_diameter,
            MIN_SIZE_MM,
            MAX_SIZE_MM,
        )?;
        check_range(
            &format!("{what} `section`"),
            r.section,
            MIN_SIZE_MM,
            MAX_SIZE_MM,
        )?;
        if lib.ring_sizes.insert(r.id.clone(), r.clone()).is_some() {
            return Err(invalid(format!("{what}: duplicate id")));
        }
    }
    for (i, s) in sf.styles.iter().enumerate() {
        let what = format!("built-in part style `{}` (`styles[{i}]`)", truncate(&s.id));
        check_id(&what, &s.id)?;
        s.fields().validate(s.kind, &what)?;
        if lib.builtin_styles.insert(s.id.clone(), s.clone()).is_some() {
            return Err(invalid(format!("{what}: duplicate id")));
        }
    }
    for &k in PartKind::ALL {
        match lib.builtin_styles.get(k.name()) {
            Some(s) if s.kind == k && s.base.is_none() => {}
            _ => {
                return Err(invalid(format!(
                    "built-in part styles: kind `{}` needs a default style `{}` without `base`",
                    k.name(),
                    k.name()
                )));
            }
        }
    }
    let ids: Vec<String> = lib.builtin_styles.keys().cloned().collect();
    for id in &ids {
        let r = lib.resolve_style(id)?;
        lib.check_style_materials(&r.fields, &format!("built-in part style `{id}`"))?;
    }
    Ok(lib)
}

fn builtin() -> &'static Result<PartLibrary, SchemaError> {
    static LIB: OnceLock<Result<PartLibrary, SchemaError>> = OnceLock::new();
    LIB.get_or_init(load_builtin)
}

impl PartLibrary {
    /// The built-in library ([`MATERIALS_TOML`], [`PART_STYLES_TOML`]), loaded once.
    /// An error only if the shipped data were broken.
    pub fn builtin() -> Result<PartLibrary, SchemaError> {
        builtin().clone()
    }

    /// The built-ins extended by a table's `[[materials]]` and `[[part_styles]]`.
    ///
    /// A material whose id equals a built-in replaces it whole; a style whose id equals a
    /// built-in overrides it field by field (it must keep the kind and give no `base`).
    /// Errors name the entry: duplicate ids, bad values, fields that do not apply to the
    /// kind, an unknown `base`, a `base` of another kind, `base` cycles and chains deeper
    /// than [`MAX_STYLE_DEPTH`], and style materials that are not in the library.
    pub fn with_table(
        materials: &[MaterialDef],
        styles: &[PartStyleDef],
    ) -> Result<PartLibrary, SchemaError> {
        let mut lib = PartLibrary::builtin()?;
        if materials.len() > MAX_MATERIALS {
            return Err(invalid(format!(
                "`materials` has {} entries (at most {MAX_MATERIALS})",
                materials.len()
            )));
        }
        if styles.len() > MAX_PART_STYLES {
            return Err(invalid(format!(
                "`part_styles` has {} entries (at most {MAX_PART_STYLES})",
                styles.len()
            )));
        }
        let mut seen = BTreeMap::new();
        for (i, m) in materials.iter().enumerate() {
            let what = format!("material `{}` (`materials[{i}]`)", truncate(&m.id));
            m.validate(&what)?;
            if seen.insert(m.id.as_str(), i).is_some() {
                return Err(invalid(format!("{what}: duplicate material id `{}`", m.id)));
            }
            lib.materials.insert(m.id.clone(), m.clone());
        }
        for (i, s) in styles.iter().enumerate() {
            let what = format!("part style `{}` (`part_styles[{i}]`)", truncate(&s.id));
            check_id(&what, &s.id)?;
            s.fields().validate(s.kind, &what)?;
            if let Some(b) = lib.builtin_styles.get(&s.id) {
                if b.kind != s.kind {
                    return Err(invalid(format!(
                        "{what}: overrides built-in style of kind `{}` with kind `{}`",
                        b.kind.name(),
                        s.kind.name()
                    )));
                }
                if s.base.is_some() {
                    return Err(invalid(format!(
                        "{what}: overrides a built-in style field by field and may not give `base`"
                    )));
                }
            }
            if let Some(b) = &s.base {
                check_id(&format!("{what} `base`"), b)?;
            }
            if lib.table_styles.insert(s.id.clone(), s.clone()).is_some() {
                return Err(invalid(format!(
                    "{what}: duplicate part style id `{}`",
                    s.id
                )));
            }
        }
        for (i, s) in styles.iter().enumerate() {
            let what = format!("part style `{}` (`part_styles[{i}]`)", truncate(&s.id));
            let r = lib.resolve_style(&s.id).map_err(|e| prefix(&what, e))?;
            lib.check_style_materials(&r.fields, &what)?;
        }
        Ok(lib)
    }

    /// A library material by id.
    pub fn material(&self, id: &str) -> Option<&MaterialDef> {
        self.materials.get(id)
    }

    /// Every material id, sorted.
    pub fn material_ids(&self) -> impl Iterator<Item = &str> {
        self.materials.keys().map(String::as_str)
    }

    /// A style definition by id: the table's if it defines one, else the built-in.
    pub fn style(&self, id: &str) -> Option<&PartStyleDef> {
        self.table_styles
            .get(id)
            .or_else(|| self.builtin_styles.get(id))
    }

    /// Every style id (built-in and table), sorted, without duplicates.
    pub fn style_ids(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self
            .builtin_styles
            .keys()
            .chain(self.table_styles.keys())
            .map(String::as_str)
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// A ring size by id.
    pub fn ring_size(&self, id: &str) -> Option<&RingSize> {
        self.ring_sizes.get(id)
    }

    /// Every ring-size id, sorted.
    pub fn ring_size_ids(&self) -> impl Iterator<Item = &str> {
        self.ring_sizes.keys().map(String::as_str)
    }

    /// Resolves a library material by name for `what` (a part): an unknown name, or a
    /// `physical` that is not an Appendix C material, is an error naming `what`, the
    /// material and the bad value.
    pub fn resolve_material(
        &self,
        name: &str,
        what: &str,
    ) -> Result<ResolvedMaterial, SchemaError> {
        let def = self
            .materials
            .get(name)
            .ok_or_else(|| invalid(format!("{what}: unknown material `{}`", truncate(name))))?;
        let physical = match &def.physical {
            None => None,
            Some(p) => Some(Material::from_name(p).ok_or_else(|| {
                invalid(format!(
                    "{what}: material `{name}` names unknown physical material `{}` \
                     (expected one of steel, ceramic, plastic, resin, rubber, wood, acrylic)",
                    truncate(p)
                ))
            })?),
        };
        Ok(ResolvedMaterial {
            def: def.clone(),
            physical,
        })
    }

    /// The layers of style `id`, root (kind default) first.
    fn chain(&self, id: &str) -> Result<(PartKind, Vec<ChainLayer>), SchemaError> {
        let mut layers = Vec::new();
        let mut visited: Vec<String> = Vec::new();
        let mut cur = id.to_owned();
        let mut kind: Option<PartKind> = None;
        loop {
            if visited.contains(&cur) {
                return Err(invalid(format!(
                    "part style `{}`: `base` chain has a cycle through `{}`",
                    truncate(id),
                    truncate(&cur)
                )));
            }
            if visited.len() >= MAX_STYLE_DEPTH {
                return Err(invalid(format!(
                    "part style `{}`: `base` chain is deeper than {MAX_STYLE_DEPTH}",
                    truncate(id)
                )));
            }
            let b = self.builtin_styles.get(&cur);
            let t = self.table_styles.get(&cur);
            let Some(def) = t.or(b) else {
                return Err(invalid(match visited.last() {
                    None => format!("unknown part style `{}`", truncate(&cur)),
                    Some(prev) => format!(
                        "part style `{}`: unknown `base` `{}`",
                        truncate(prev),
                        truncate(&cur)
                    ),
                }));
            };
            match kind {
                None => kind = Some(def.kind),
                Some(k) if k != def.kind => {
                    return Err(invalid(format!(
                        "part style `{}`: `base` `{}` is of kind `{}`, not `{}`",
                        truncate(visited.last().map_or(id, String::as_str)),
                        truncate(&cur),
                        def.kind.name(),
                        k.name()
                    )));
                }
                Some(_) => {}
            }
            if let Some(t) = t {
                layers.push(ChainLayer { fields: t.fields() });
            }
            if let Some(b) = b {
                layers.push(ChainLayer { fields: b.fields() });
            }
            // A table style overriding a built-in keeps the built-in's base.
            let base = b.or(t).and_then(|d| d.base.clone());
            visited.push(cur.clone());
            let k = def.kind;
            match base {
                Some(next) => cur = next,
                None if cur != k.name() => cur = k.name().to_owned(),
                None => break,
            }
        }
        layers.reverse();
        // `kind` is set on the first iteration.
        Ok((kind.unwrap_or(PartKind::Decoration), layers))
    }

    /// Resolves style `id` through its `base` chain (kind default first).
    pub fn resolve_style(&self, id: &str) -> Result<ResolvedStyle, SchemaError> {
        let (kind, layers) = self.chain(id)?;
        let mut fields = StyleFields::default();
        for l in &layers {
            fields.overlay(&l.fields);
        }
        Ok(ResolvedStyle {
            id: id.to_owned(),
            kind,
            fields,
        })
    }

    fn check_style_materials(&self, f: &StyleFields, what: &str) -> Result<(), SchemaError> {
        for name in [&f.material, &f.trim_material].into_iter().flatten() {
            self.resolve_material(name, what)?;
        }
        if let Some(r) = &f.ring_size
            && !self.ring_sizes.contains_key(r)
        {
            return Err(invalid(format!(
                "{what}: unknown ring size `{}`",
                truncate(r)
            )));
        }
        Ok(())
    }

    /// Resolves a stand-alone or table-bound part: style chain, instance overrides,
    /// materials and placement. `position` is the part's position (the caller fills it
    /// from a bound element). Errors name the part (`what`).
    pub fn resolve(
        &self,
        part: &PartDef,
        position: [f32; 3],
        what: &str,
    ) -> Result<ResolvedPart, SchemaError> {
        let inst = part.instance_fields();
        inst.validate(part.kind, what)?;
        self.resolve_inner(&Spec {
            name: &part.id,
            what,
            kind: part.kind,
            style: part.style.as_deref(),
            bind: part.bind.as_deref(),
            instance: &inst,
            legacy: &StyleFields::default(),
            legacy_physical: None,
            placement: Placement {
                position,
                rotation_deg: part.rotation_deg,
            },
        })
    }

    fn resolve_inner(&self, s: &Spec<'_>) -> Result<ResolvedPart, SchemaError> {
        let style = s.style.unwrap_or(s.kind.name());
        let (kind, layers) = self.chain(style).map_err(|e| prefix(s.what, e))?;
        if kind != s.kind {
            return Err(invalid(format!(
                "{}: style `{}` is of kind `{}`, not `{}`",
                s.what,
                truncate(style),
                kind.name(),
                s.kind.name()
            )));
        }
        let mut fields = StyleFields::default();
        for l in &layers {
            fields.overlay(&l.fields);
        }
        fields.overlay(s.instance);
        let mut legacy = s.legacy.clone();
        for &f in StyleField::ALL {
            if !s.kind.allows(f) {
                legacy.clear(f);
            }
        }
        fields.overlay(&legacy);
        // A ring size supplies the ring numbers the part does not state.
        if let Some(r) = &fields.ring_size {
            let size = self.ring_sizes.get(r).ok_or_else(|| {
                invalid(format!("{}: unknown ring size `{}`", s.what, truncate(r)))
            })?;
            fields.inner_diameter = fields.inner_diameter.or(Some(size.inner_diameter));
            fields.section = fields.section.or(Some(size.section));
        }
        if let Some(legacy) = s.legacy_physical {
            // The element's own key fixes the physical material. The instance or a named
            // style may change the look with a library material that plays the same; a
            // kind-default material that plays differently gives way to the element's.
            let explicit = s.instance.material.is_some() || s.style.is_some();
            let chosen = match fields.material.take() {
                Some(m) if explicit => m,
                Some(m) if self.resolve_material(&m, s.what)?.physical == Some(legacy) => m,
                _ => legacy.name().to_owned(),
            };
            let m = self.resolve_material(&chosen, s.what)?;
            if m.physical != Some(legacy) {
                return Err(invalid(format!(
                    "{}: material `{}` plays as `{}` but the layout element is `{}` \
                     (looks and plays must agree)",
                    s.what,
                    chosen,
                    m.physical.map_or("none", Material::name),
                    legacy.name()
                )));
            }
            fields.material = Some(chosen);
        }
        let material = match &fields.material {
            Some(n) => Some(self.resolve_material(n, s.what)?),
            None => None,
        };
        let trim_material = match &fields.trim_material {
            Some(n) => Some(self.resolve_material(n, s.what)?),
            None => None,
        };
        if s.kind.is_touchable() {
            match &material {
                None => {
                    return Err(invalid(format!(
                        "{}: kind `{}` is ball-touchable and needs a material",
                        s.what,
                        s.kind.name()
                    )));
                }
                Some(m) if m.physical.is_none() => {
                    return Err(invalid(format!(
                        "{}: material `{}` has no `physical` material but kind `{}` is ball-touchable",
                        s.what,
                        m.def.id,
                        s.kind.name()
                    )));
                }
                Some(_) => {}
            }
        }
        if fields.shape == Some(Shape::Polygon)
            && fields.outline.as_ref().is_none_or(|o| o.len() < 3)
        {
            return Err(invalid(format!(
                "{}: shape `polygon` needs an `outline` of at least 3 points",
                s.what
            )));
        }
        if fields.shape == Some(Shape::Star)
            && fields.star_points.is_none()
            && s.kind.allows(StyleField::StarPoints)
        {
            return Err(invalid(format!(
                "{}: shape `star` needs `star_points`",
                s.what
            )));
        }
        s.placement.validate(s.what)?;
        Ok(ResolvedPart {
            name: s.name.to_owned(),
            kind: s.kind,
            style: style.to_owned(),
            bind: s.bind.map(str::to_owned),
            fields,
            material,
            trim_material,
            placement: s.placement,
        })
    }
}

fn prefix(what: &str, e: SchemaError) -> SchemaError {
    match e {
        SchemaError::Invalid(m) if !m.starts_with(what) => invalid(format!("{what}: {m}")),
        other => other,
    }
}

/// Inputs of one resolution.
struct Spec<'a> {
    name: &'a str,
    what: &'a str,
    kind: PartKind,
    style: Option<&'a str>,
    bind: Option<&'a str>,
    instance: &'a StyleFields,
    legacy: &'a StyleFields,
    legacy_physical: Option<Material>,
    placement: Placement,
}

// ---------------------------------------------------------------------------
// Binding.
// ---------------------------------------------------------------------------

/// The `layout.toml` sections a part may bind to with `"<section>[<n>]"`.
pub const LAYOUT_SECTIONS: [&str; 6] = ["walls", "posts", "lanes", "ramps", "decks", "inserts"];

/// The `table.toml` sections a part may bind to with `"<id>"` or `"<section>:<id>"`.
pub const TABLE_SECTIONS: [&str; 5] = ["mechs", "flippers", "lamps", "splines", "switches"];

/// Splits `"<section>[<n>]"` for a layout section.
pub fn parse_layout_bind(bind: &str) -> Option<(&'static str, usize)> {
    let (section, rest) = bind.split_once('[')?;
    let digits = rest.strip_suffix(']')?;
    if digits.is_empty() || digits.len() > 6 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let section = LAYOUT_SECTIONS.into_iter().find(|s| *s == section)?;
    Some((section, digits.parse().ok()?))
}

/// The kinds a part bound to a layout section may have, default first.
pub fn layout_bind_kinds(section: &str) -> &'static [PartKind] {
    match section {
        "walls" => &[PartKind::Wall],
        "posts" => &[PartKind::Post, PartKind::StarPost, PartKind::RubberRing],
        "lanes" => &[PartKind::LaneGuide],
        "ramps" => &[
            PartKind::Wireform,
            PartKind::LaneGuide,
            PartKind::PlasticRamp,
        ],
        "decks" => &[PartKind::Deck],
        "inserts" => &[PartKind::Insert],
        _ => &[],
    }
}

/// A table element a part can bind to.
struct Element {
    section: &'static str,
    kinds: &'static [PartKind],
    position: Option<[f32; 3]>,
}

fn mech_kinds(k: MechKind) -> &'static [PartKind] {
    match k {
        MechKind::Bumper => &[PartKind::PopBumper],
        MechKind::Sling => &[PartKind::Slingshot],
        MechKind::DropTarget => &[PartKind::DropTarget],
        MechKind::Spinner => &[PartKind::Spinner],
        MechKind::Gate => &[PartKind::Gate],
        MechKind::Vuk
        | MechKind::Plunger
        | MechKind::Kickback
        | MechKind::Diverter
        | MechKind::NewtonBall => &[PartKind::Decoration],
    }
}

fn lamp_kinds(k: LampKind) -> &'static [PartKind] {
    match k {
        LampKind::Insert => &[PartKind::Insert],
        LampKind::Gi => &[PartKind::GiBulb],
        LampKind::Flasher => &[PartKind::FlasherDome],
    }
}

fn spline_kinds(k: SplineKind) -> &'static [PartKind] {
    match k {
        SplineKind::Wireform => &[PartKind::Wireform],
        SplineKind::Guide => &[PartKind::LaneGuide, PartKind::PlasticRamp],
    }
}

fn switch_kinds(k: SwitchKind) -> &'static [PartKind] {
    match k {
        SwitchKind::Target => &[PartKind::StandupTarget, PartKind::DropTarget],
        SwitchKind::Spinner => &[PartKind::Spinner],
        SwitchKind::Ring => &[PartKind::PopBumper],
        SwitchKind::Opto
        | SwitchKind::Rollover
        | SwitchKind::Eddy
        | SwitchKind::Leaf
        | SwitchKind::Trough
        | SwitchKind::Eos => &[PartKind::Decoration],
    }
}

fn find_elements(t: &TableManifest, section: Option<&str>, id: &str) -> Vec<Element> {
    let mut out = Vec::new();
    let want = |s: &str| section.is_none_or(|w| w == s);
    if want("mechs") {
        out.extend(t.mechs.iter().filter(|m| m.id == id).map(|m| Element {
            section: "mechs",
            kinds: mech_kinds(m.kind),
            position: m.position,
        }));
    }
    if want("flippers") {
        out.extend(t.flippers.iter().filter(|f| f.id == id).map(|f| Element {
            section: "flippers",
            kinds: &[PartKind::FlipperBat, PartKind::FlipperRubber],
            position: Some(f.base_position),
        }));
    }
    if want("lamps") {
        out.extend(t.lamps.iter().filter(|l| l.id == id).map(|l| Element {
            section: "lamps",
            kinds: lamp_kinds(l.kind),
            position: l.position,
        }));
    }
    if want("splines") {
        out.extend(t.splines.iter().filter(|s| s.id == id).map(|s| Element {
            section: "splines",
            kinds: spline_kinds(s.kind),
            position: Some([0.0; 3]),
        }));
    }
    if want("switches") {
        out.extend(t.switches.iter().filter(|s| s.id == id).map(|s| Element {
            section: "switches",
            kinds: switch_kinds(s.kind),
            position: s.position,
        }));
    }
    out
}

fn kinds_list(kinds: &[PartKind]) -> String {
    kinds
        .iter()
        .map(|k| k.name())
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// Theme.
// ---------------------------------------------------------------------------

/// `table.toml` `[theme]`: what the art generators draw (REQ-BUILD-003, REQ-BUILD-005).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeDef {
    /// Title on the generated art (default `meta.name`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Palette, 1 to 16 `"#rrggbb"`; replaces `meta.palette` for the art generators.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub palette: Option<Vec<String>>,
    /// Art motif id (default [`DEFAULT_MOTIF`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub motif: Option<String>,
    /// Font name per role (for example `title`, `label`, `score`); names only, no paths.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fonts: BTreeMap<String, String>,
}

impl ThemeDef {
    /// Checks the bounds; errors name the key (`theme.palette[2]`, `theme.fonts.title`, …).
    pub fn validate(&self) -> Result<(), SchemaError> {
        if let Some(t) = &self.title
            && (t.trim().is_empty()
                || t.chars().count() > MAX_TITLE_LEN
                || t.chars().any(char::is_control))
        {
            return Err(invalid(format!(
                "`theme.title` must be 1 to {MAX_TITLE_LEN} printable characters"
            )));
        }
        if let Some(p) = &self.palette {
            if p.is_empty() || p.len() > MAX_PALETTE {
                return Err(invalid(format!(
                    "`theme.palette` has {} colours (1 to {MAX_PALETTE})",
                    p.len()
                )));
            }
            for (i, c) in p.iter().enumerate() {
                check_color(&format!("`theme.palette[{i}]`"), c)?;
            }
        }
        if let Some(m) = &self.motif {
            if m.len() > MAX_MOTIF_LEN {
                return Err(invalid(format!(
                    "`theme.motif` is longer than {MAX_MOTIF_LEN} characters"
                )));
            }
            check_id("`theme.motif`", m)?;
        }
        if self.fonts.len() > MAX_FONTS {
            return Err(invalid(format!(
                "`theme.fonts` has {} entries (at most {MAX_FONTS})",
                self.fonts.len()
            )));
        }
        for (role, name) in &self.fonts {
            check_id("`theme.fonts` role", role)?;
            let ok = !name.is_empty()
                && name.len() <= MAX_ID_LEN
                && !name.starts_with(['.', ' '])
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'));
            if !ok {
                return Err(invalid(format!(
                    "`theme.fonts.{role}` must be a font name (1 to {MAX_ID_LEN} of letters, \
                     digits, space, - _ .; no paths)"
                )));
            }
        }
        Ok(())
    }
}

/// The theme the art generators use ([`TableManifest::effective_theme`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    /// Title.
    pub title: String,
    /// Palette, sRGB.
    pub palette: Vec<[u8; 3]>,
    /// Motif id.
    pub motif: String,
    /// Font name per role, sorted by role.
    pub fonts: BTreeMap<String, String>,
}

// ---------------------------------------------------------------------------
// TableManifest and LayoutManifest integration.
// ---------------------------------------------------------------------------

impl TableManifest {
    /// The theme for the art generators: `[theme]` where given, else `meta.name` as the
    /// title, `meta.palette` as the palette (entries that are not `#rrggbb` are skipped,
    /// as `meta.palette` is not validated), [`DEFAULT_MOTIF`] and no fonts.
    pub fn effective_theme(&self) -> Theme {
        let theme = self.theme.clone().unwrap_or_default();
        let palette = match &theme.palette {
            Some(p) => p.iter().filter_map(|c| parse_color(c)).collect(),
            None => self
                .meta
                .palette
                .iter()
                .filter_map(|c| parse_color(c))
                .collect(),
        };
        Theme {
            title: theme.title.unwrap_or_else(|| self.meta.name.clone()),
            palette,
            motif: theme.motif.unwrap_or_else(|| DEFAULT_MOTIF.to_owned()),
            fonts: theme.fonts,
        }
    }

    /// The built-in library extended by this table's `[[materials]]` and `[[part_styles]]`
    /// ([`PartLibrary::with_table`]).
    pub fn part_library(&self) -> Result<PartLibrary, SchemaError> {
        PartLibrary::with_table(&self.materials, &self.part_styles)
    }

    /// Resolves every `[[parts]]` entry that is not bound to a `layout.toml` element, in
    /// file order. Checks ids (valid, unique), binds to this table's elements (exists,
    /// unambiguous, kind allowed, at most one part per element, `position` only when the
    /// element has none), styles, materials and placement. Parts bound to a layout element
    /// are checked here for everything except the element itself
    /// ([`LayoutManifest::resolve_parts`] resolves them).
    pub fn resolve_parts(&self, lib: &PartLibrary) -> Result<Vec<ResolvedPart>, SchemaError> {
        if self.parts.len() > MAX_PARTS {
            return Err(invalid(format!(
                "`parts` has {} entries (at most {MAX_PARTS})",
                self.parts.len()
            )));
        }
        let mut ids: BTreeMap<&str, usize> = BTreeMap::new();
        let mut binds: BTreeMap<String, usize> = BTreeMap::new();
        let mut out = Vec::new();
        for (i, p) in self.parts.iter().enumerate() {
            p.validate(i)?;
            let what = part_label(&p.id, i);
            if let Some(j) = ids.insert(p.id.as_str(), i) {
                return Err(invalid(format!(
                    "{what}: duplicate part id (also `parts[{j}]`)"
                )));
            }
            let position = match p.bind.as_deref() {
                None => p
                    .position
                    .ok_or_else(|| invalid(format!("{what}: needs `position` (or `bind`)")))?,
                Some(b) => {
                    let key = if let Some((section, n)) = parse_layout_bind(b) {
                        let kinds = layout_bind_kinds(section);
                        if !kinds.contains(&p.kind) {
                            return Err(invalid(format!(
                                "{what}: kind `{}` cannot bind to `{section}` (allowed: {})",
                                p.kind.name(),
                                kinds_list(kinds)
                            )));
                        }
                        if p.position.is_some() {
                            return Err(invalid(format!(
                                "{what}: `position` is not allowed with a `layout.toml` bind \
                                 (the element's own keys place it)"
                            )));
                        }
                        format!("{section}[{n}]")
                    } else {
                        let (section, id) = match b.split_once(':') {
                            Some((s, id)) => match TABLE_SECTIONS.into_iter().find(|t| *t == s) {
                                Some(s) => (Some(s), id),
                                None => {
                                    return Err(invalid(format!(
                                        "{what}: `bind` section `{}` is not one of {}",
                                        truncate(s),
                                        TABLE_SECTIONS.join(", ")
                                    )));
                                }
                            },
                            None => (None, b),
                        };
                        let found = find_elements(self, section, id);
                        let e = match found.as_slice() {
                            [] => {
                                return Err(invalid(format!(
                                    "{what}: `bind` `{}` matches no mech, flipper, lamp, spline, \
                                     switch or layout element",
                                    truncate(b)
                                )));
                            }
                            [e] => e,
                            _ => {
                                return Err(invalid(format!(
                                    "{what}: `bind` `{}` is ambiguous; write `<section>:<id>`",
                                    truncate(b)
                                )));
                            }
                        };
                        if !e.kinds.contains(&p.kind) {
                            return Err(invalid(format!(
                                "{what}: kind `{}` cannot bind to {} `{}` (allowed: {})",
                                p.kind.name(),
                                e.section,
                                truncate(id),
                                kinds_list(e.kinds)
                            )));
                        }
                        let key = format!("{}:{id}", e.section);
                        let pos = match (e.position, p.position) {
                            (Some(_), Some(_)) => {
                                return Err(invalid(format!(
                                    "{what}: `position` is not allowed: {} `{}` has its own",
                                    e.section,
                                    truncate(id)
                                )));
                            }
                            (Some(ep), None) => ep,
                            (None, Some(pp)) => pp,
                            (None, None) => {
                                return Err(invalid(format!(
                                    "{what}: needs `position`: {} `{}` has none",
                                    e.section,
                                    truncate(id)
                                )));
                            }
                        };
                        // A flipper carries a bat and a rubber: one part of each kind.
                        let key = format!(
                            "{key}/{}",
                            if e.section == "flippers" {
                                p.kind.name()
                            } else {
                                ""
                            }
                        );
                        if let Some(j) = binds.insert(key, i) {
                            return Err(invalid(format!(
                                "{what}: `{}` is already bound by `parts[{j}]`",
                                truncate(b)
                            )));
                        }
                        let r = lib.resolve(p, pos, &what)?;
                        out.push(r);
                        continue;
                    };
                    if let Some(j) = binds.insert(key, i) {
                        return Err(invalid(format!(
                            "{what}: `{}` is already bound by `parts[{j}]`",
                            truncate(b)
                        )));
                    }
                    // Checked now with a placeholder position; resolved with the layout.
                    lib.resolve(p, [0.0; 3], &what)?;
                    continue;
                }
            };
            out.push(lib.resolve(p, position, &what)?);
        }
        Ok(out)
    }

    /// Resolves every part: this table's stand-alone and table-bound `[[parts]]`, then every
    /// element of `layout` (with its bound part, if any). Fixed order.
    pub fn all_parts(&self, layout: &LayoutManifest) -> Result<Vec<ResolvedPart>, SchemaError> {
        let lib = self.part_library()?;
        let mut v = self.resolve_parts(&lib)?;
        v.extend(layout.resolve_parts(self, &lib)?);
        Ok(v)
    }

    /// [`Self::all_parts`] followed by [`check_physical_pairs`] against `physics`.
    pub fn check_physical_pairs(
        &self,
        layout: &LayoutManifest,
        physics: &PhysicsDefaults,
    ) -> Result<(), SchemaError> {
        check_physical_pairs(&self.all_parts(layout)?, physics)
    }

    /// The parse-time checks of the new sections (run by `validate`): theme, library and
    /// parts. Skipped entirely when the table has none of them.
    pub(crate) fn validate_parts(&self) -> Result<(), SchemaError> {
        if let Some(t) = &self.theme {
            t.validate()?;
        }
        if self.materials.is_empty() && self.part_styles.is_empty() && self.parts.is_empty() {
            return Ok(());
        }
        let lib = self.part_library()?;
        self.resolve_parts(&lib).map(|_| ())
    }
}

impl LayoutManifest {
    /// Resolves every element of this layout as a part, in the order walls, posts, lanes,
    /// ramps, decks, inserts, applying the `table` `[[parts]]` entry bound to it (if any).
    ///
    /// The element's own keys are instance values and win: a post's `radius` (diameter
    /// `2 × radius`) and `rubber` (physical `rubber`, else `steel`), a wall's or deck's
    /// `material`, a lane's `width`, an insert's `shape` and `size`, positions, points and
    /// a deck's `z`. A bound part or table style may change the look of a wall, post or
    /// deck only with a library material whose `physical` equals that element's material
    /// (an error naming the part otherwise). Posts, walls and lane guides whose height is
    /// unset take `playfield.wall_height`. A bind to an element index that does not exist
    /// is an error naming the part.
    pub fn resolve_parts(
        &self,
        table: &TableManifest,
        lib: &PartLibrary,
    ) -> Result<Vec<ResolvedPart>, SchemaError> {
        let mut bound: BTreeMap<(&'static str, usize), (usize, &PartDef)> = BTreeMap::new();
        for (i, p) in table.parts.iter().enumerate() {
            if let Some((section, n)) = p.bind.as_deref().and_then(parse_layout_bind) {
                let len = match section {
                    "walls" => self.walls.len(),
                    "posts" => self.posts.len(),
                    "lanes" => self.lanes.len(),
                    "ramps" => self.ramps.len(),
                    "decks" => self.decks.len(),
                    _ => self.inserts.len(),
                };
                if n >= len {
                    return Err(invalid(format!(
                        "{}: `bind` `{section}[{n}]` but layout.toml has {len} {section}",
                        part_label(&p.id, i)
                    )));
                }
                bound.insert((section, n), (i, p));
            }
        }
        let wall_height = self.playfield.wall_height;
        let mut out = Vec::new();
        let mut one = |section: &'static str,
                       n: usize,
                       default_kind: PartKind,
                       legacy: StyleFields,
                       legacy_physical: Option<Material>,
                       position: [f32; 3]|
         -> Result<(), SchemaError> {
            let name = format!("{section}[{n}]");
            let b = bound.get(&(section, n));
            let what = match b {
                Some((i, p)) => format!("part `{name}` (bound by {})", part_label(&p.id, *i)),
                None => format!("part `{name}`"),
            };
            let kind = b.map_or(default_kind, |(_, p)| p.kind);
            let inst = b.map(|(_, p)| p.instance_fields()).unwrap_or_default();
            let mut r = lib.resolve_inner(&Spec {
                name: &name,
                what: &what,
                kind,
                style: b.and_then(|(_, p)| p.style.as_deref()),
                bind: None,
                instance: &inst,
                legacy: &legacy,
                legacy_physical,
                placement: Placement {
                    position,
                    rotation_deg: b.map_or([0.0; 3], |(_, p)| p.rotation_deg),
                },
            })?;
            if matches!(kind, PartKind::Post | PartKind::Wall | PartKind::LaneGuide)
                && r.fields.height.is_none()
            {
                r.fields.height = Some(wall_height);
            }
            out.push(r);
            Ok(())
        };
        for (n, w) in self.walls.iter().enumerate() {
            one(
                "walls",
                n,
                PartKind::Wall,
                StyleFields::default(),
                Some(w.material),
                [0.0; 3],
            )?;
        }
        for (n, p) in self.posts.iter().enumerate() {
            let legacy = StyleFields {
                diameter: Some(2.0 * p.radius),
                ..StyleFields::default()
            };
            let phys = if p.rubber {
                Material::Rubber
            } else {
                Material::Steel
            };
            one(
                "posts",
                n,
                PartKind::Post,
                legacy,
                Some(phys),
                [p.position[0], p.position[1], 0.0],
            )?;
        }
        for n in 0..self.lanes.len() {
            one(
                "lanes",
                n,
                PartKind::LaneGuide,
                StyleFields::default(),
                None,
                [0.0; 3],
            )?;
        }
        for (n, r) in self.ramps.iter().enumerate() {
            let kind = match table
                .splines
                .iter()
                .find(|s| s.id == r.spline)
                .map(|s| s.kind)
            {
                Some(SplineKind::Guide) => PartKind::LaneGuide,
                _ => PartKind::Wireform,
            };
            one("ramps", n, kind, StyleFields::default(), None, [0.0; 3])?;
        }
        for (n, d) in self.decks.iter().enumerate() {
            one(
                "decks",
                n,
                PartKind::Deck,
                StyleFields::default(),
                Some(d.material),
                [0.0, 0.0, d.z],
            )?;
        }
        for (n, ins) in self.inserts.iter().enumerate() {
            let legacy = StyleFields {
                shape: Some(ins.shape.into()),
                size: Some(ins.size),
                ..StyleFields::default()
            };
            one(
                "inserts",
                n,
                PartKind::Insert,
                legacy,
                None,
                [ins.position[0], ins.position[1], 0.0],
            )?;
        }
        Ok(out)
    }

    /// Checks this layout's elements and the `table` parts bound to them
    /// ([`Self::resolve_parts`] with `table`'s library).
    pub fn check_parts(&self, table: &TableManifest) -> Result<(), SchemaError> {
        let lib = table.part_library()?;
        self.resolve_parts(table, &lib).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_colours() {
        assert!(is_valid_id("a-b_c.9"));
        assert!(!is_valid_id(""));
        assert!(!is_valid_id("Abc"));
        assert!(!is_valid_id(&"a".repeat(MAX_ID_LEN + 1)));
        assert_eq!(parse_color("#FF7a00"), Some([255, 122, 0]));
        assert_eq!(parse_color("ff7a00"), None);
        assert_eq!(parse_color("#ff7a0"), None);
        assert_eq!(parse_color("#ff7a0g"), None);
        assert_eq!(parse_color("#ééé"), None);
    }

    #[test]
    fn layout_bind_syntax() {
        assert_eq!(parse_layout_bind("posts[3]"), Some(("posts", 3)));
        assert_eq!(parse_layout_bind("inserts[0]"), Some(("inserts", 0)));
        assert_eq!(parse_layout_bind("posts[]"), None);
        assert_eq!(parse_layout_bind("posts[1234567]"), None);
        assert_eq!(parse_layout_bind("posts[-1]"), None);
        assert_eq!(parse_layout_bind("rails[1]"), None);
        assert_eq!(parse_layout_bind("posts[1"), None);
        assert!(layout_bind_kinds("nope").is_empty());
        for s in LAYOUT_SECTIONS {
            assert!(!layout_bind_kinds(s).is_empty());
        }
    }

    #[test]
    fn element_kind_tables_cover_every_kind() {
        for k in [
            MechKind::Bumper,
            MechKind::Sling,
            MechKind::DropTarget,
            MechKind::Spinner,
            MechKind::Vuk,
            MechKind::Plunger,
            MechKind::Kickback,
            MechKind::Diverter,
            MechKind::NewtonBall,
            MechKind::Gate,
        ] {
            assert!(!mech_kinds(k).is_empty());
        }
        for k in [LampKind::Insert, LampKind::Gi, LampKind::Flasher] {
            assert!(!lamp_kinds(k).is_empty());
        }
        for k in [
            SwitchKind::Opto,
            SwitchKind::Rollover,
            SwitchKind::Eddy,
            SwitchKind::Leaf,
            SwitchKind::Target,
            SwitchKind::Trough,
            SwitchKind::Eos,
            SwitchKind::Spinner,
            SwitchKind::Ring,
        ] {
            assert!(!switch_kinds(k).is_empty());
        }
        assert_eq!(kinds_list(&[PartKind::Post, PartKind::Wall]), "post, wall");
    }

    #[test]
    fn prefix_keeps_other_errors() {
        assert_eq!(
            prefix("x", SchemaError::UnsupportedVersion(2)),
            SchemaError::UnsupportedVersion(2)
        );
        assert_eq!(prefix("x", invalid("x: y".into())), invalid("x: y".into()));
        assert_eq!(prefix("x", invalid("y".into())), invalid("x: y".into()));
    }
}
