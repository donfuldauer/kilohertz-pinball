//! Builds the Feature Lab table bundle (Subphase 6.4) from the sources in
//! `tables/feature_lab`:
//!
//! ```sh
//! cargo run -q -p pin_app --example build_feature_lab -- <out_dir>
//! ```
//!
//! `tables/feature_lab/build.sh <out_dir>` runs this and then
//! `pin_tablegen validate <out_dir>`. The `pin_tablegen` command line does
//! not read `backbox_layout.toml`, so the whole generation lives here and
//! uses the library API (DECISIONS: docs/decisions/2026-11-18-6.4.md
//! "6.4 — build path"):
//!
//! 1. `pin_tablegen::generate` turns `layout.toml` and the base
//!    `table.toml` into `playfield.glb` and a merged manifest;
//! 2. `pin_tablegen::backbox::generate_backbox` adds the backbox from
//!    `backbox_layout.toml` (`backbox.glb`, `backbox_colliders.glb`, the
//!    mini flipper, the VUK `tablegen:backbox.vuk` and the layer
//!    transitions) to that manifest;
//! 3. the generated VUK is aimed up through the generated entry transition
//!    ([`aim_backbox_vuk`]), and the merged `table.toml` is written;
//! 4. the rules, backglass, layout, glass and display scenes are copied;
//! 5. `pin_assetgen::generate_kit` writes the 4.6 placeholder kit (sounds,
//!    fonts, backglass art; title "Feature Lab", the table's palette,
//!    seed 0).
//!
//! The tests build the same bundle by including this file
//! (`crates/pin_app/tests/feature_lab_support`), so there is one
//! implementation of the build.

use pin_assetgen::{TableMeta, generate_kit};
use pin_core::schema::table::TableManifest;
use pin_tablegen::backbox::{
    BACKBOX_LAYOUT_FILE, BACKBOX_PREFIX, BackboxFrame, generate_backbox, parse_backbox_layout,
    write_backbox,
};
use pin_tablegen::layout::{GENERATED_HEADER, GENERATED_PREFIX, TABLE_FILE, parse_layout};
use pin_tablegen::{generate, write_bundle};
use std::path::{Path, PathBuf};

/// The placeholder kit's title.
pub const TITLE: &str = "Feature Lab";

/// Speed (m/s) across the backbox (+x) the VUK gives the ball.
pub const VUK_LATERAL_M_S: f32 = 0.35;
/// Speed (m/s) up the backbox plane the VUK gives the ball.
pub const VUK_RISE_M_S: f32 = 2.5;
/// The exit point's offset (mm, plane `u`) from the middle of the entry
/// hole: back against the lateral drift, so the rising ball clears the
/// hole's edges.
pub const VUK_EXIT_SHIFT_MM: f32 = -15.0;
/// Height of the VUK's exit point above the playfield: a ball's centre at
/// rest on it, m.
pub const VUK_EXIT_Z_M: f32 = 0.0135;

/// `tables/feature_lab` (the sources).
#[must_use]
pub fn feature_lab_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tables/feature_lab")
}

/// Source files copied unchanged into the bundle besides the display
/// scenes (`pin_sim` reads `wall_height` from `layout.toml` and the glass
/// underside from `glass.toml`).
pub const COPIED: [&str; 4] = ["rules.lua", "backglass.toml", "layout.toml", "glass.toml"];

fn read(src: &Path, name: &str) -> Result<String, String> {
    std::fs::read_to_string(src.join(name)).map_err(|e| format!("{name}: {e}"))
}

/// `#rrggbb` to bytes.
///
/// # Errors
/// When `c` is not `#rrggbb` (the `#` optional).
pub fn parse_hex(c: &str) -> Result<[u8; 3], String> {
    let h = c.trim_start_matches('#');
    if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("palette colour {c:?} is not #rrggbb"));
    }
    let b = |i: usize| {
        u8::from_str_radix(&h[i..i + 2], 16).map_err(|e| format!("palette colour {c:?}: {e}"))
    };
    Ok([b(0)?, b(2)?, b(4)?])
}

/// The display scene files of `src/display`, sorted by name.
fn scene_files(src: &Path) -> Result<Vec<String>, String> {
    let dir = src.join("display");
    let mut files = Vec::new();
    for e in std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let name = e.map_err(|e| e.to_string())?.file_name();
        let name = name.to_string_lossy();
        if name.ends_with(".toml") {
            files.push(format!("display/{name}"));
        }
    }
    files.sort();
    Ok(files)
}

/// Aims the generated backbox VUK (`tablegen:backbox.vuk`) of `table` up
/// through the generated entry transition (`tablegen:backbox.entry`, 0 to
/// the backbox layer, under the entry hole).
///
/// The generator puts the VUK's exit inside the backbox on the backbox
/// layer, a point the 2.8 clearance validator cannot see a surface under
/// (it reads the playfield glTF only, and the backbox layer may hold no
/// playfield element), and kicks the ball straight up the table, back into
/// the entry hole. Here the exit is on the playfield (layer 0) under the
/// middle of the entry hole, mid-depth in the box, at [`VUK_EXIT_Z_M`];
/// the eject velocity is [`VUK_RISE_M_S`] up the backbox plane plus
/// [`VUK_LATERAL_M_S`] across it, so the ball rises through the entry zone
/// (which lifts it onto the backbox layer) and the hole without touching
/// the glass or the back panel. DECISIONS: docs/decisions/2026-11-18-6.4.md
/// "6.4 — backbox VUK aimed through the entry transition".
///
/// # Errors
/// When `table` has no generated VUK.
pub fn aim_backbox_vuk(
    table: &mut TableManifest,
    frame: &BackboxFrame,
    entry_at_mm: f32,
    depth_mm: f32,
) -> Result<(), String> {
    let id = format!("{BACKBOX_PREFIX}vuk");
    let vuk = table
        .mechs
        .iter_mut()
        .find(|m| m.id == id)
        .ok_or_else(|| format!("no generated mech {id}"))?;
    let pos = vuk
        .position
        .ok_or_else(|| format!("{id} has no position"))?;
    let (exit, eject, angle_deg, kick) = vuk_aim(frame, entry_at_mm, depth_mm);
    vuk.layer = Some(0);
    vuk.exit_layer = Some(0);
    vuk.exit_offset = Some([exit[0] - pos[0], exit[1] - pos[1], exit[2] - pos[2]]);
    vuk.eject_speed_m_s = Some(eject);
    vuk.angle_deg = Some(angle_deg);
    vuk.kick_speed_m_s = Some(kick);
    Ok(())
}

/// The VUK aim for an entry hole at `entry_at_mm` (plane `u`) of a box of
/// `depth_mm`: the exit point (table frame, mm) and the VUK's
/// `(eject_speed_m_s, angle_deg, kick_speed_m_s)`, whose velocity
/// `z * eject + (cos a, sin a, 0) * kick` is [`VUK_RISE_M_S`] along the
/// backbox's up axis plus [`VUK_LATERAL_M_S`] along `x`.
#[must_use]
pub fn vuk_aim(frame: &BackboxFrame, entry_at_mm: f32, depth_mm: f32) -> ([f32; 3], f32, f32, f32) {
    // Backbox axes in the table frame: x across, y = -normal (into the
    // box), up along the plane.
    let (o, n, up) = (frame.origin_m, frame.normal(), frame.up());
    let (u, y) = (
        (entry_at_mm + VUK_EXIT_SHIFT_MM) * 1.0e-3,
        0.5 * depth_mm * 1.0e-3,
    );
    let p0 = [o.x + u - y * n.x, o.y - y * n.y, o.z - y * n.z];
    let v = (VUK_EXIT_Z_M - p0[2]) / up.z;
    let exit = [
        (p0[0] + v * up.x) * 1.0e3,
        (p0[1] + v * up.y) * 1.0e3,
        (p0[2] + v * up.z) * 1.0e3,
    ];
    let vel = [
        VUK_LATERAL_M_S + VUK_RISE_M_S * up.x,
        VUK_RISE_M_S * up.y,
        VUK_RISE_M_S * up.z,
    ];
    let angle = vel[1].atan2(vel[0]).to_degrees();
    let kick = vel[0].hypot(vel[1]);
    (exit, vel[2], angle, kick)
}

/// Builds the bundle from the sources in `src` into `out_dir` (created if
/// needed). Returns the generator's non-fatal warnings.
///
/// # Errors
/// A message naming the step and the file or element that failed.
pub fn build_from(src: &Path, out_dir: &Path) -> Result<Vec<String>, String> {
    std::fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    let layout =
        parse_layout(&read(src, "layout.toml")?).map_err(|e| format!("layout.toml: {e}"))?;
    let base = TableManifest::from_toml_str(&read(src, TABLE_FILE)?)
        .map_err(|e| format!("table.toml: {e}"))?;
    let playfield = generate(&layout, &base).map_err(|e| format!("pin_tablegen: {e}"))?;
    let mut warnings: Vec<String> = playfield.warnings.iter().map(|w| w.to_string()).collect();
    write_bundle(&playfield, out_dir).map_err(|e| format!("write bundle: {e}"))?;

    let bb = parse_backbox_layout(&read(src, BACKBOX_LAYOUT_FILE)?)
        .map_err(|e| format!("{BACKBOX_LAYOUT_FILE}: {e}"))?;
    let backbox =
        generate_backbox(&bb.backbox, &playfield.table).map_err(|e| format!("backbox: {e}"))?;
    write_backbox(&backbox, out_dir).map_err(|e| format!("write backbox: {e}"))?;

    let mut table = backbox.table.clone();
    aim_backbox_vuk(
        &mut table,
        &backbox.frame,
        bb.backbox.entry.at,
        bb.backbox.depth,
    )?;
    let body = table
        .to_toml_string()
        .map_err(|e| format!("table.toml: {e}"))?;
    let text = format!(
        "{GENERATED_HEADER}# Backbox from {BACKBOX_LAYOUT_FILE} (build_feature_lab example).\n\
         # Entries whose id starts with \"{GENERATED_PREFIX}\" are replaced on every run.\n\n{body}"
    );
    std::fs::write(out_dir.join(TABLE_FILE), text).map_err(|e| format!("write table.toml: {e}"))?;

    let mut copied: Vec<String> = COPIED.iter().map(|s| (*s).to_owned()).collect();
    copied.extend(scene_files(src)?);
    for f in &copied {
        let to = out_dir.join(f);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::copy(src.join(f), &to).map_err(|e| format!("copy {f}: {e}"))?;
    }

    let meta = TableMeta {
        title: TITLE.to_owned(),
        palette: base
            .meta
            .palette
            .iter()
            .map(|c| parse_hex(c))
            .collect::<Result<_, _>>()?,
        seed: 0,
    };
    generate_kit(&meta, out_dir).map_err(|e| format!("pin_assetgen: {e}"))?;
    warnings.sort();
    Ok(warnings)
}

/// Builds the bundle from `tables/feature_lab` into `out_dir`.
///
/// # Errors
/// As [`build_from`].
pub fn build_feature_lab_bundle(out_dir: &Path) -> Result<Vec<String>, String> {
    build_from(&feature_lab_src(), out_dir)
}

#[allow(dead_code)] // the tests include this file as a module
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [out] = args.as_slice() else {
        eprintln!("usage: build_feature_lab <out_dir>");
        std::process::exit(2);
    };
    match build_feature_lab_bundle(Path::new(out)) {
        Ok(warnings) => {
            for w in warnings {
                eprintln!("warning: {w}");
            }
            println!("feature_lab bundle: {out}");
        }
        Err(e) => {
            eprintln!("build_feature_lab: {e}");
            std::process::exit(1);
        }
    }
}
