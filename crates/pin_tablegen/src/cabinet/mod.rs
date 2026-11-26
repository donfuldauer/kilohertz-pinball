//! Cabinet model generator: `cabinet.glb` from a resolved `cabinet.toml`
//! geometry (SPEC §7, REQ-CAB-001, REQ-CAB-003, REQ-CAB-007).
//!
//! Owners: KP-029 (#110, body, legs, trim and controls) and KP-030 (#111,
//! backbox and art blades).
//!
//! [`generate_cabinet`] turns a [`CabinetGeometry`] (from
//! `CabinetManifest::from_toml_str(..)?.resolve()` or
//! `CabinetGeometry::preset(..)`) into the bytes of [`CABINET_GLB_FILE`]: the
//! cabinet body, the four legs with their brackets and levelers, the side
//! rails, lockdown bar, playfield glass, coin door, the four buttons and the
//! plunger; the backbox (its shell, the speaker panel with the
//! score-display window and the round grilles, the translite frame with the
//! translite opening, the backglass sheet behind it, the score-display quad
//! and the hinge brackets); and the two inside art blades on the side walls
//! above the playfield. Every dimension comes from the geometry; nothing
//! about the cabinet is hard-coded (REQ-CAB-003). The same geometry always
//! gives the same bytes; an invalid one is a [`GenError`] naming its
//! `cabinet.toml` key, never a panic.
//!
//! # Frame and units
//!
//! Metres. The cabinet frame of SPEC Appendix A.4: `x` across from the body
//! centreline (player's right positive), `y` along the cabinet from the
//! body's front face (`y = 0`, player side) toward the rear, `z` up, with
//! **`z` the height above the floor plane** (the floor is `z = 0`). A
//! body-fixed point given in `cabinet.toml` as `(y, z above the body
//! bottom)` is placed at `CabinetGeometry::floor_height_mm(y, z) / 1000`.
//! Static nodes have an identity transform and their vertices in this frame.
//!
//! # Nodes
//!
//! The node list is stable: [`NODE_NAMES`], the same 37 names in the same
//! order for every geometry: the 28 body, leg, trim and control nodes, then
//! `cabinet.backbox.shell`, `.speaker_panel`, `.translite_frame`,
//! `.translite`, `.display`, `.grilles`, `.hinge`, `cabinet.blade.left` and
//! `cabinet.blade.right`. Each node has one triangle primitive with
//! `POSITION`, `NORMAL` and `TEXCOORD_0` (UVs in `[0, 1]`: each face is
//! projected on the node's bounding box along its dominant normal axis, so
//! the outer face of a body side covers `0..1` whole), `u32` indices,
//! counter-clockwise front faces and finite floats. The one exception: a
//! backbox with no grilles writes `cabinet.backbox.grilles` with no mesh
//! (the node stays in the list; `pin_assets::load_gltf` loads it and lists
//! no mesh for it).
//!
//! # Backbox
//!
//! From `[backbox]` alone. The front ("face") is the plane `y = front_y`
//! (flat) or leans back 10 degrees about its bottom edge (angled); face
//! coordinates `(u, v)` run across from the backbox centreline and up the
//! face from its bottom edge, as in `cabinet.toml`. The top is flat, slopes
//! down 10 degrees toward the rear (angled) or is a parabolic crown across
//! `x` that sags `0.06 * width` at the sides (arched, 32 segments). The
//! bottom is level at `zb = floor_height_mm(front_y, bottom_z)`.
//!
//! * `shell`: bottom, side, rear and top panels, each `body.wall_thickness`
//!   (`T`) thick, behind the front plates.
//! * `speaker_panel` (`v` from 0 to `speaker_panel_height`) and
//!   `translite_frame` (from there up to the top): front plates `T` thick
//!   whose through holes are exactly the score-display window and the
//!   translite opening.
//! * `translite`: the backglass sheet, centred on the opening, its front on
//!   the frame's rear face; `display`: one quad of the window's size 5 mm
//!   behind the face. Both are UV-mapped to their image: `(0, 0)` at the
//!   upper-left corner seen from the front, `(1, 1)` at the lower-right.
//! * `grilles`: one disc (32 segments) per grille, 2 mm proud of the face.
//! * `hinge`: two 50 mm bracket plates, mirrored about `x = 0`, at the
//!   backbox rear, from the lower of the backbox bottom and the body's rear
//!   wall top up to 80 mm above the backbox bottom.
//!
//! # Art blades
//!
//! `cabinet.blade.left` / `.right`: 3 mm panels on the side walls' inner
//! faces from `y = mount.inset` to the rear wall's inner face, their bottom
//! edge on the playfield plane (`mount.height` at the inset, rising at
//! `mount.pitch_deg`; roll ignored), their top two glass-channel widths
//! below the side wall top (under the glass-channel notch). The inner face
//! is box-projected, so its UVs cover `0..1`.
//!
//! # Moving nodes
//!
//! `cabinet.button.*` and `cabinet.plunger.rod` are moving: the node's
//! `translation` is the control's pivot (its `[controls]` position, converted
//! with `floor_height_mm`, metres) and the mesh is local to it. Their
//! `extras` (strings) are `moving = "true"`, `travel_axis` (the unit axis of
//! the press or pull in the cabinet frame: `"+x"` left flipper, `"-x"` right
//! flipper, `"+y"` start and action, `"-y"` plunger) and `travel_mm` (`"4"`
//! for buttons, `"80"` for the plunger). Static nodes carry no extras.
//!
//! # Materials
//!
//! Twenty-four named materials, in this order: `cabinet.body` (bottom, rear),
//! `cabinet.side_left`, `cabinet.side_right`, `cabinet.front` (the art
//! surfaces, swapped by asset ID, D32), `cabinet.trim_metal` (rails and
//! lockdown bar: `trim.rail_colour` tint, `trim.metal_finish`),
//! `cabinet.glass` (alpha-blended), `cabinet.coin_door`, `cabinet.leveler`,
//! `cabinet.leg.<leg>` for each leg (shaft and bracket: `leg.colour`
//! converted from sRGB to linear, metallic and roughness from `finish`, wood
//! never metallic), `cabinet.button.<button>`, `cabinet.plunger` (housing
//! and rod), `cabinet.backbox` (the shell, a white art surface),
//! `cabinet.backbox.speaker_panel` (near-black), `cabinet.backbox.grille`
//! (dark metal, `trim.metal_finish`), `cabinet.backbox.translite` and
//! `cabinet.backbox.display` (white texture slots), `cabinet.blade_left` and
//! `cabinet.blade_right` (white art surfaces). The translite frame and the
//! hinge use `cabinet.trim_metal`.
//!
//! # Dimension guarantees (within 1 mm)
//!
//! * The `cabinet.body.*` nodes together span `x` in `±body.width / 2` and
//!   `y` in `[0, body.length]`; the front wall's top is at
//!   `floor_height_mm(0, front_height)`, the rear wall's at
//!   `floor_height_mm(length, rear_height)`, and the side wall tops run
//!   linearly between them; the body bottom's underside follows
//!   `body_bottom_height_mm(y)`.
//! * The lockdown bar's top is at `lockdown_bar_top_height_mm()`.
//! * The glass is `glass_panel.width` across.
//! * Each leg shaft runs from `z = leveler.extension` to
//!   `leveler.extension + length` at the `corner_inset` centreline; each
//!   leveler's lowest point is on the floor `z = 0`, so the whole file's
//!   lowest point is `z = 0`.
//! * The backbox shell, speaker panel and translite frame together span
//!   `x` in `±backbox.width / 2`, `y` in `[front_y, front_y + depth]` and
//!   `z` from `zb` to `zb + height = backbox_top_height_mm()` (the apex:
//!   the front top edge, or the crown of an arched top).
//! * The plates' holes are the translite opening and the score-display
//!   window, at their `(u, v)` and sizes; the display quad has the window's
//!   size, so its aspect is `width / height` from the data.
//!
//! # Errors
//!
//! After the body, leg, control and trim checks: the `[backbox]` values
//! (finite, positive, within [`MAX_DIMENSION_MM`]; at most 8 grilles), their
//! fit (the opening, window, grilles and backglass inside the face, the
//! window inside the speaker panel, the speaker panel under the backbox
//! height), room for the panels (each of `backbox.width`, `backbox.height`
//! and `backbox.depth` at least twice `body.wall_thickness`), the profiles (`backbox.front_profile`: an angled front's lean
//! must fit the depth; `backbox.top_profile`: an arched top must clear the
//! opening and the speaker panel, an angled one must leave room at the
//! rear) and the blades (`mount.inset`, `mount.height`, `mount.pitch_deg`).

mod backbox;
mod glb;
mod mesh;
mod parts;

#[cfg(test)]
mod tests;

use crate::GenError;
use mesh::Axis;
use parts::Part;
use pin_core::schema::cabinet::{CabinetGeometry, Finish, Leg, LegMaterial};

/// File name of the generated cabinet model in a bundle.
pub const CABINET_GLB_FILE: &str = "cabinet.glb";

/// Largest magnitude of any cabinet dimension or position the generator
/// accepts, mm (100 m): keeps every derived float finite.
pub const MAX_DIMENSION_MM: f32 = 100_000.0;

/// The stable node list of `cabinet.glb`, in file order.
pub const NODE_NAMES: [&str; 37] = [
    "cabinet.body.bottom",
    "cabinet.body.side_left",
    "cabinet.body.side_right",
    "cabinet.body.front",
    "cabinet.body.rear",
    "cabinet.rail.left",
    "cabinet.rail.right",
    "cabinet.lockdown_bar",
    "cabinet.glass",
    "cabinet.leg.front_left",
    "cabinet.leg.front_right",
    "cabinet.leg.rear_left",
    "cabinet.leg.rear_right",
    "cabinet.leg.front_left.bracket",
    "cabinet.leg.front_right.bracket",
    "cabinet.leg.rear_left.bracket",
    "cabinet.leg.rear_right.bracket",
    "cabinet.leg.front_left.leveler",
    "cabinet.leg.front_right.leveler",
    "cabinet.leg.rear_left.leveler",
    "cabinet.leg.rear_right.leveler",
    "cabinet.coin_door",
    "cabinet.button.left_flipper",
    "cabinet.button.right_flipper",
    "cabinet.button.start",
    "cabinet.button.action",
    "cabinet.plunger.housing",
    "cabinet.plunger.rod",
    "cabinet.backbox.shell",
    "cabinet.backbox.speaker_panel",
    "cabinet.backbox.translite_frame",
    "cabinet.backbox.translite",
    "cabinet.backbox.display",
    "cabinet.backbox.grilles",
    "cabinet.backbox.hinge",
    "cabinet.blade.left",
    "cabinet.blade.right",
];

/// The material names of `cabinet.glb`, in file order.
pub const MATERIAL_NAMES: [&str; 24] = [
    "cabinet.body",
    "cabinet.side_left",
    "cabinet.side_right",
    "cabinet.front",
    "cabinet.trim_metal",
    "cabinet.glass",
    "cabinet.coin_door",
    "cabinet.leveler",
    "cabinet.leg.front_left",
    "cabinet.leg.front_right",
    "cabinet.leg.rear_left",
    "cabinet.leg.rear_right",
    "cabinet.button.left_flipper",
    "cabinet.button.right_flipper",
    "cabinet.button.start",
    "cabinet.button.action",
    "cabinet.plunger",
    "cabinet.backbox",
    "cabinet.backbox.speaker_panel",
    "cabinet.backbox.grille",
    "cabinet.backbox.translite",
    "cabinet.backbox.display",
    "cabinet.blade_left",
    "cabinet.blade_right",
];

// Material indices into MATERIAL_NAMES.
const M_BODY: usize = 0;
const M_SIDE_LEFT: usize = 1;
const M_SIDE_RIGHT: usize = 2;
const M_FRONT: usize = 3;
const M_TRIM: usize = 4;
const M_GLASS: usize = 5;
const M_COIN_DOOR: usize = 6;
const M_LEVELER: usize = 7;
const M_LEG0: usize = 8;
const M_BUTTON0: usize = 12;
const M_PLUNGER: usize = 16;
const M_BACKBOX: usize = 17;
const M_SPEAKER_PANEL: usize = 18;
const M_GRILLE: usize = 19;
const M_TRANSLITE: usize = 20;
const M_DISPLAY: usize = 21;
const M_BLADE_LEFT: usize = 22;
const M_BLADE_RIGHT: usize = 23;
/// Index of `cabinet.backbox.grilles` in [`NODE_NAMES`] (the one node that
/// may have no mesh: a backbox with no grilles).
const N_GRILLES: usize = 33;

/// A generated cabinet model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedCabinet {
    /// The bytes of `cabinet.glb` (binary glTF 2.0).
    pub glb: Vec<u8>,
    /// The node names in file order (always [`NODE_NAMES`]).
    pub nodes: Vec<String>,
}

fn err(element: impl Into<String>, message: impl Into<String>) -> GenError {
    GenError {
        element: element.into(),
        message: message.into(),
    }
}

fn positive(key: &str, v: f32) -> Result<(), GenError> {
    if v.is_finite() && v > 0.0 && v <= MAX_DIMENSION_MM {
        Ok(())
    } else {
        Err(err(
            key,
            format!("must be finite, > 0 and at most {MAX_DIMENSION_MM} mm, got {v}"),
        ))
    }
}

fn non_negative(key: &str, v: f32) -> Result<(), GenError> {
    if (0.0..=MAX_DIMENSION_MM).contains(&v) {
        Ok(())
    } else {
        Err(err(
            key,
            format!("must be finite, >= 0 and at most {MAX_DIMENSION_MM} mm, got {v}"),
        ))
    }
}

fn finite(key: &str, v: &[f32]) -> Result<(), GenError> {
    if v.iter()
        .all(|x| x.is_finite() && x.abs() <= MAX_DIMENSION_MM)
    {
        Ok(())
    } else {
        Err(err(
            key,
            format!("must be finite and within ±{MAX_DIMENSION_MM} mm, got {v:?}"),
        ))
    }
}

/// `"#rrggbb"` sRGB to linear RGBA (alpha 1), `libm::pow` for the curve.
fn srgb_to_linear(key: &str, s: &str) -> Result<[f32; 4], GenError> {
    let b = s.as_bytes();
    let hex = |i: usize| -> Option<u8> {
        let pair = s.get(i..i + 2)?;
        u8::from_str_radix(pair, 16).ok()
    };
    let rgb = (b.len() == 7 && b[0] == b'#' && b[1..].iter().all(u8::is_ascii_hexdigit))
        .then(|| Some([hex(1)?, hex(3)?, hex(5)?]))
        .flatten()
        .ok_or_else(|| err(key, format!("must be \"#rrggbb\", got {s:?}")))?;
    let lin = |c: u8| -> f32 {
        let c = f64::from(c) / 255.0;
        let l = if c <= 0.04045 {
            c / 12.92
        } else {
            libm::pow((c + 0.055) / 1.055, 2.4)
        };
        l as f32
    };
    Ok([lin(rgb[0]), lin(rgb[1]), lin(rgb[2]), 1.0])
}

/// Metallic and roughness factors of a finish.
fn finish_factors(finish: Finish) -> (f32, f32) {
    match finish {
        Finish::Chrome => (1.0, 0.15),
        Finish::BrushedSteel => (1.0, 0.4),
        Finish::PowderCoat => (0.0, 0.6),
        Finish::Wrinkle => (0.0, 0.8),
        Finish::Painted => (0.0, 0.5),
    }
}

fn check_leg(name: &str, leg: &Leg) -> Result<(), GenError> {
    let p = |f: &str| format!("legs.{name}.{f}");
    positive(&p("length"), leg.length)?;
    positive(&p("section.top"), leg.section.top)?;
    positive(&p("section.bottom"), leg.section.bottom)?;
    let lv = &leg.leveler;
    positive(&p("leveler.thread_length"), lv.thread_length)?;
    positive(&p("leveler.thread_diameter"), lv.thread_diameter)?;
    non_negative(&p("leveler.extension"), lv.extension)?;
    positive(&p("leveler.foot_diameter"), lv.foot_diameter)?;
    finite(&p("corner_inset"), &leg.corner_inset)?;
    srgb_to_linear(&p("colour"), &leg.colour)?;
    Ok(())
}

/// Checks every value the generator uses: sizes finite and > 0 (or >= 0
/// where zero is meaningful), positions finite, all within
/// [`MAX_DIMENSION_MM`], colours `"#rrggbb"`. The error names the
/// `cabinet.toml` key.
fn check(g: &CabinetGeometry) -> Result<(), GenError> {
    let b = &g.body;
    positive("body.width", b.width)?;
    positive("body.length", b.length)?;
    positive("body.front_height", b.front_height)?;
    positive("body.rear_height", b.rear_height)?;
    positive("body.wall_thickness", b.wall_thickness)?;
    non_negative("body.leg_overlap", b.leg_overlap)?;
    positive("body.glass_channel.width", b.glass_channel.width)?;
    positive("body.glass_channel.depth", b.glass_channel.depth)?;
    non_negative("body.lockdown_bar.height", b.lockdown_bar.height)?;
    positive("body.lockdown_bar.depth", b.lockdown_bar.depth)?;
    positive("body.side_rail.width", b.side_rail.width)?;
    positive("body.side_rail.height", b.side_rail.height)?;
    for (name, leg) in g.legs.all() {
        check_leg(name, leg)?;
    }
    let gp = &g.glass_panel;
    positive("glass_panel.width", gp.width)?;
    positive("glass_panel.length", gp.length)?;
    positive("glass_panel.thickness", gp.thickness)?;
    let c = &g.controls;
    for (key, v) in [
        ("controls.left_flipper_button", &c.left_flipper_button),
        ("controls.right_flipper_button", &c.right_flipper_button),
        ("controls.start_button", &c.start_button),
        ("controls.action_button", &c.action_button),
        ("controls.plunger", &c.plunger),
        ("controls.coin_door.centre", &c.coin_door.centre),
    ] {
        finite(key, v)?;
    }
    positive("controls.coin_door.width", c.coin_door.width)?;
    positive("controls.coin_door.height", c.coin_door.height)?;
    srgb_to_linear("trim.rail_colour", &g.trim.rail_colour)?;
    backbox::check(g)
}

fn materials(g: &CabinetGeometry) -> Result<Vec<glb::Material>, GenError> {
    let flat = |name: &'static str, base_color: [f32; 4], (metallic, roughness): (f32, f32)| {
        glb::Material {
            name,
            base_color,
            metallic,
            roughness,
            blend: false,
        }
    };
    // Art surfaces: white, so a later art texture shows its own colours.
    let white = [1.0, 1.0, 1.0, 1.0];
    let art = (0.0, 0.5);
    let mut out = vec![
        flat(MATERIAL_NAMES[M_BODY], white, art),
        flat(MATERIAL_NAMES[M_SIDE_LEFT], white, art),
        flat(MATERIAL_NAMES[M_SIDE_RIGHT], white, art),
        flat(MATERIAL_NAMES[M_FRONT], white, art),
        flat(
            MATERIAL_NAMES[M_TRIM],
            srgb_to_linear("trim.rail_colour", &g.trim.rail_colour)?,
            finish_factors(g.trim.metal_finish),
        ),
        glb::Material {
            name: MATERIAL_NAMES[M_GLASS],
            base_color: [1.0, 1.0, 1.0, 0.15],
            metallic: 0.0,
            roughness: 0.05,
            blend: true,
        },
        flat(
            MATERIAL_NAMES[M_COIN_DOOR],
            [0.02, 0.02, 0.02, 1.0],
            finish_factors(g.trim.metal_finish),
        ),
        flat(MATERIAL_NAMES[M_LEVELER], [0.6, 0.6, 0.6, 1.0], (1.0, 0.35)),
    ];
    for (i, (name, leg)) in g.legs.all().into_iter().enumerate() {
        let (metallic, roughness) = finish_factors(leg.finish);
        let metallic = if leg.material == LegMaterial::Wood {
            0.0
        } else {
            metallic
        };
        out.push(flat(
            MATERIAL_NAMES[M_LEG0 + i],
            srgb_to_linear(&format!("legs.{name}.colour"), &leg.colour)?,
            (metallic, roughness),
        ));
    }
    // Buttons: red flipper buttons, a yellow start and a red action button.
    for (i, colour) in [
        [0.6, 0.02, 0.02, 1.0],
        [0.6, 0.02, 0.02, 1.0],
        [0.8, 0.6, 0.02, 1.0],
        [0.6, 0.02, 0.02, 1.0],
    ]
    .into_iter()
    .enumerate()
    {
        out.push(flat(MATERIAL_NAMES[M_BUTTON0 + i], colour, (0.0, 0.3)));
    }
    out.push(flat(
        MATERIAL_NAMES[M_PLUNGER],
        [0.8, 0.8, 0.8, 1.0],
        (1.0, 0.15),
    ));
    // Backbox and blades: art surfaces and texture slots are white.
    out.extend([
        flat(MATERIAL_NAMES[M_BACKBOX], white, art),
        flat(
            MATERIAL_NAMES[M_SPEAKER_PANEL],
            [0.02, 0.02, 0.02, 1.0],
            (0.0, 0.7),
        ),
        flat(
            MATERIAL_NAMES[M_GRILLE],
            [0.05, 0.05, 0.05, 1.0],
            finish_factors(g.trim.metal_finish),
        ),
        flat(MATERIAL_NAMES[M_TRANSLITE], white, art),
        flat(MATERIAL_NAMES[M_DISPLAY], white, art),
        flat(MATERIAL_NAMES[M_BLADE_LEFT], white, art),
        flat(MATERIAL_NAMES[M_BLADE_RIGHT], white, art),
    ]);
    Ok(out)
}

/// Format of a travel value in `extras.travel_mm` (an integer when whole).
fn travel_text(mm: f64) -> String {
    if mm.fract() == 0.0 {
        format!("{mm:.0}")
    } else {
        format!("{mm}")
    }
}

fn node(name: &'static str, part: Part, material: usize) -> glb::Node {
    let extras = match part.travel {
        Some((axis, mm)) => vec![
            ("moving", "true".to_owned()),
            ("travel_axis", axis.label().to_owned()),
            ("travel_mm", travel_text(mm)),
        ],
        None => Vec::new(),
    };
    glb::Node {
        name,
        mesh: Some(part.mesh.to_glb()),
        material,
        translation: part.translation.map(|t| t.map(|c| (c / 1000.0) as f32)),
        extras,
    }
}

/// Every node finite and every mesh non-empty (a guard: the checked inputs
/// keep every derived float finite). A node may have no mesh at all (the
/// grilles of a backbox without grilles).
fn check_nodes(nodes: &[glb::Node]) -> Result<(), GenError> {
    match nodes
        .iter()
        .find(|n| !n.all_finite() || n.mesh.as_ref().is_some_and(|m| m.indices.is_empty()))
    {
        Some(bad) => Err(err(
            "cabinet",
            format!(
                "node {} has non-finite or empty geometry (inconsistent cabinet dimensions)",
                bad.name
            ),
        )),
        None => Ok(()),
    }
}

/// Generates `cabinet.glb` from a resolved cabinet geometry (REQ-CAB-001,
/// REQ-CAB-003, REQ-CAB-007). See the module docs for the frame, the node
/// and material lists and the guarantees.
///
/// Errors: a non-finite, non-positive (or beyond ±[`MAX_DIMENSION_MM`]) size
/// or position, or a malformed colour, is a [`GenError`] whose `element` is
/// the dotted `cabinet.toml` key (`body.width`, `legs.rear_left.length`,
/// `controls.plunger`, `backbox.grilles[1].u`, `mount.inset`, ...); so is a
/// backbox part that does not fit (`backbox.translite_opening.bottom`,
/// `backbox.top_profile`, ...). See the module docs' "Errors". Never
/// panics.
pub fn generate_cabinet(geom: &CabinetGeometry) -> Result<GeneratedCabinet, GenError> {
    let (nodes, mats) = build(geom)?;
    Ok(GeneratedCabinet {
        glb: glb::write("cabinet", &nodes, &mats),
        nodes: NODE_NAMES.iter().map(|s| (*s).to_owned()).collect(),
    })
}

/// Checks `geom` and builds the nodes (in [`NODE_NAMES`] order) and the
/// materials (in [`MATERIAL_NAMES`] order).
fn build(geom: &CabinetGeometry) -> Result<(Vec<glb::Node>, Vec<glb::Material>), GenError> {
    check(geom)?;
    let g = geom;
    let mut parts: Vec<(Part, usize)> = Vec::with_capacity(NODE_NAMES.len());
    let [bottom, side_left, side_right, front, rear] = parts::body(g);
    parts.extend([
        (bottom, M_BODY),
        (side_left, M_SIDE_LEFT),
        (side_right, M_SIDE_RIGHT),
        (front, M_FRONT),
        (rear, M_BODY),
    ]);
    let [rail_left, rail_right] = parts::rails(g);
    parts.extend([
        (rail_left, M_TRIM),
        (rail_right, M_TRIM),
        (parts::lockdown_bar(g), M_TRIM),
        (parts::glass(g), M_GLASS),
    ]);
    let legs = g.legs.all();
    for (i, (name, leg)) in legs.iter().enumerate() {
        parts.push((parts::leg_shaft(g, name, leg), M_LEG0 + i));
    }
    for (i, (name, leg)) in legs.iter().enumerate() {
        parts.push((parts::bracket(g, name, leg), M_LEG0 + i));
    }
    for (name, leg) in legs {
        parts.push((parts::leveler(g, name, leg), M_LEVELER));
    }
    parts.push((parts::coin_door(g), M_COIN_DOOR));
    let c = &g.controls;
    for (i, (pos, travel)) in [
        (c.left_flipper_button, Axis::PosX),
        (c.right_flipper_button, Axis::NegX),
        (c.start_button, Axis::PosY),
        (c.action_button, Axis::PosY),
    ]
    .into_iter()
    .enumerate()
    {
        parts.push((parts::button(g, pos, travel), M_BUTTON0 + i));
    }
    parts.push((parts::plunger_housing(g), M_PLUNGER));
    parts.push((parts::plunger_rod(g), M_PLUNGER));
    let speaker_panel = backbox::speaker_panel(g);
    if speaker_panel.mesh.is_empty() {
        return Err(err(
            "backbox.score_display.width",
            "the score-display window covers the whole speaker panel",
        ));
    }
    let frame = backbox::translite_frame(g);
    if frame.mesh.is_empty() {
        return Err(err(
            "backbox.translite_opening.width",
            "the translite opening covers the whole translite frame",
        ));
    }
    let [blade_left, blade_right] = backbox::blades(g);
    parts.extend([
        (backbox::shell(g), M_BACKBOX),
        (speaker_panel, M_SPEAKER_PANEL),
        (frame, M_TRIM),
        (backbox::translite(g), M_TRANSLITE),
        (backbox::display(g), M_DISPLAY),
        (backbox::grilles(g), M_GRILLE),
        (backbox::hinge(g), M_TRIM),
        (blade_left, M_BLADE_LEFT),
        (blade_right, M_BLADE_RIGHT),
    ]);
    let mut nodes: Vec<glb::Node> = parts
        .into_iter()
        .zip(NODE_NAMES)
        .map(|((part, material), name)| node(name, part, material))
        .collect();
    if g.backbox.grilles.is_empty() {
        nodes[N_GRILLES].mesh = None;
    }
    check_nodes(&nodes)?;
    let mats = materials(g)?;
    Ok((nodes, mats))
}
