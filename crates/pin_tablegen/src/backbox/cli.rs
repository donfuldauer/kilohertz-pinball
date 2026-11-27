//! The backbox half of the `pin_tablegen` command line (SPEC §4 Subphases
//! 4.3, 5.5 and 6.4; DECISIONS "#136 KP-055").
//!
//! * `pin_tablegen <layout.toml> --table <table.toml> --out <dir>
//!   --backbox <backbox_layout.toml>` adds the backbox to the playfield
//!   bundle: [`generate_backbox`] on the merged manifest, the VUK eject
//!   ([`aim_backbox_vuk`]) and one merged `table.toml`
//!   ([`backbox_table_toml`]), written together with `playfield.glb`,
//!   `backbox.glb` and `backbox_colliders.glb` by
//!   [`write_bundle_with_backbox`].
//! * `pin_tablegen build <src_dir> --out <out_dir>` ([`run_build_cli`],
//!   [`build_from`]) builds a whole table bundle from a source folder
//!   holding `layout.toml`, `table.toml` and `backbox_layout.toml`: the
//!   above, the copied sources ([`COPIED`] and `display/*.toml`) and the
//!   `pin_assetgen` placeholder kit. `tables/feature_lab/build.sh` runs it.
//!
//! The float math here ([`vuk_eject`]) uses `libm` only; the crate's
//! `clippy.toml` bans std transcendentals and std `min`/`max` (D9).

use super::{
    BACKBOX_COLLIDERS_FILE, BACKBOX_LAYOUT_FILE, BACKBOX_PREFIX, BACKBOX_SCENE_FILE, BackboxFrame,
    GeneratedBackbox, generate_backbox, parse_backbox_layout,
};
use crate::layout::{
    CliError, GENERATED_HEADER, GENERATED_PREFIX, GeneratedTable, LOCK_FILE, PLAYFIELD_FILE,
    TABLE_FILE, generate, parse_layout,
};
use pin_assetgen::{TableMeta, generate_kit};
use pin_core::schema::table::TableManifest;
use std::path::{Path, PathBuf};

/// Usage text of `pin_tablegen build`.
pub const BUILD_USAGE: &str = "usage: pin_tablegen build <src_dir> --out <out_dir>";

/// Largest source file read, bytes (the same limit as the layout command
/// line's inputs, the loader's manifest limit).
pub const MAX_SOURCE_BYTES: u64 = 4 << 20;

/// Speed (m/s) across the backbox (+x, towards the bat) the VUK gives the
/// ball.
pub const VUK_LATERAL_M_S: f32 = 0.35;
/// Speed (m/s) up the backbox plane the VUK gives the ball: from the
/// generated exit just above the entry hole it rises about 225 mm, clear
/// over the entry channel's right wall, and comes down on the bat.
pub const VUK_RISE_M_S: f32 = 2.1;

/// Source files [`build_from`] copies unchanged into the bundle besides the
/// display scenes (`pin_sim` reads `wall_height` from `layout.toml` and the
/// glass underside from `glass.toml`).
pub const COPIED: [&str; 4] = ["rules.lua", "backglass.toml", "layout.toml", "glass.toml"];

/// Reads at most `limit` bytes from `path`; errors start with `label`.
///
/// One bounded read from one handle: the metadata size can be stale or
/// meaningless (a growing file, a pipe, a device), so the limit is enforced
/// on the bytes actually read.
pub(crate) fn read_capped(path: &Path, label: &str, limit: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let fail = |e: std::io::Error| format!("{label}: {e}");
    let file = std::fs::File::open(path).map_err(fail)?;
    let size = file.metadata().map_err(fail)?.len();
    if size > limit {
        return Err(format!("{label}: file is {size} bytes (max {limit})"));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes).map_err(fail)?;
    if bytes.len() as u64 > limit {
        return Err(format!("{label}: file is larger than {limit} bytes"));
    }
    Ok(bytes)
}

/// Reads `src/name` as UTF-8 text, bounded; errors start with `name`.
fn read_source(src: &Path, name: &str) -> Result<String, String> {
    let bytes = read_capped(&src.join(name), name, MAX_SOURCE_BYTES)?;
    String::from_utf8(bytes).map_err(|e| format!("{name}: not valid UTF-8 text: {e}"))
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

/// The VUK's `(eject_speed_m_s, angle_deg, kick_speed_m_s)` for a backbox
/// of `frame`: the velocity `z * eject + (cos a, sin a, 0) * kick` is
/// [`VUK_RISE_M_S`] along the backbox's up axis plus [`VUK_LATERAL_M_S`]
/// along `x`, so it lies in the backbox plane.
#[must_use]
pub fn vuk_eject(frame: &BackboxFrame) -> (f32, f32, f32) {
    let up = frame.up();
    let vel = [
        VUK_LATERAL_M_S + VUK_RISE_M_S * up.x,
        VUK_RISE_M_S * up.y,
        VUK_RISE_M_S * up.z,
    ];
    let angle = libm::atan2f(vel[1], vel[0]).to_degrees();
    let kick = libm::hypot(f64::from(vel[0]), f64::from(vel[1])) as f32;
    (vel[2], angle, kick)
}

/// Sets the eject of the generated backbox VUK (`tablegen:backbox.vuk`) of
/// `table`; its generated exit (`exit_layer` = the backbox layer,
/// `exit_offset` just above the middle of the entry hole, against the back
/// panel) and the generated transitions are kept.
///
/// The generator leaves the eject unset, and the physics defaults (1.2 m/s
/// along table `z`, 0.3 m/s along `x`) would not lift the ball over the
/// entry channel's wall: it would fall back through the entry hole. The
/// eject velocity is [`VUK_RISE_M_S`] up the backbox plane plus
/// [`VUK_LATERAL_M_S`] across it ([`vuk_eject`]). DECISIONS "6.4 — backbox
/// VUK aimed through the entry transition" (note 2026-11-19) and "#136
/// KP-055 — VUK aim is CLI logic".
///
/// # Errors
/// When `table` has no generated VUK.
pub fn aim_backbox_vuk(table: &mut TableManifest, frame: &BackboxFrame) -> Result<(), String> {
    let id = format!("{BACKBOX_PREFIX}vuk");
    let vuk = table
        .mechs
        .iter_mut()
        .find(|m| m.id == id)
        .ok_or_else(|| format!("no generated mech {id}"))?;
    let (eject, angle_deg, kick) = vuk_eject(frame);
    vuk.eject_speed_m_s = Some(eject);
    vuk.angle_deg = Some(angle_deg);
    vuk.kick_speed_m_s = Some(kick);
    Ok(())
}

/// The merged `table.toml` text of a playfield-and-backbox bundle: the
/// generated header, the backbox comment lines and `table`.
///
/// The comment text is kept exactly as the retired Feature Lab build wrote
/// it, so the bundle stays byte-identical (DECISIONS "#136 KP-055 —
/// table.toml header comment kept byte-identical").
///
/// # Errors
/// When `table` cannot be serialised (`table.toml: ...`).
pub fn backbox_table_toml(table: &TableManifest) -> Result<String, String> {
    let body = table
        .to_toml_string()
        .map_err(|e| format!("{TABLE_FILE}: {e}"))?;
    Ok(format!(
        "{GENERATED_HEADER}# Backbox from {BACKBOX_LAYOUT_FILE} (build_feature_lab example).\n\
         # Entries whose id starts with \"{GENERATED_PREFIX}\" are replaced on every run.\n\n{body}"
    ))
}

/// Writes `playfield.glb`, `backbox.glb`, `backbox_colliders.glb` and
/// `table.toml` (`table_toml`, written once) into `dir` (created if
/// needed).
///
/// Like [`crate::write_bundle`]: every file is staged under a temporary
/// name before any output is replaced, an existing output is moved aside
/// and restored if a later replacement fails, and the whole write holds an
/// exclusive lock on [`LOCK_FILE`] inside `dir`.
///
/// # Errors
/// Any I/O error; the previous outputs are then left in place.
pub fn write_bundle_with_backbox(
    playfield: &GeneratedTable,
    backbox: &GeneratedBackbox,
    table_toml: &str,
    dir: &Path,
) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let lock = std::fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(LOCK_FILE))?;
    lock.lock()?;
    let files = [
        (PLAYFIELD_FILE, playfield.glb.as_slice()),
        (BACKBOX_SCENE_FILE, backbox.scene_glb.as_slice()),
        (BACKBOX_COLLIDERS_FILE, backbox.colliders_glb.as_slice()),
        (TABLE_FILE, table_toml.as_bytes()),
    ];
    let result = write_staged(&files, dir);
    drop(lock); // releases the lock
    result
}

fn write_staged(files: &[(&str, &[u8])], dir: &Path) -> std::io::Result<()> {
    let tmp = |name: &str| dir.join(format!(".{name}.tablegen-tmp"));
    let bak = |name: &str| dir.join(format!(".{name}.tablegen-bak"));
    let cleanup = || {
        for (name, _) in files {
            let _ = std::fs::remove_file(tmp(name));
        }
    };
    for (name, bytes) in files {
        if let Err(e) = std::fs::write(tmp(name), bytes) {
            cleanup();
            return Err(e);
        }
    }
    let mut committed: Vec<(&str, bool)> = Vec::new();
    let mut result = Ok(());
    for (name, _) in files {
        let dest = dir.join(name);
        let had_old = std::fs::symlink_metadata(&dest).is_ok_and(|m| !m.is_dir());
        if had_old && let Err(e) = std::fs::rename(&dest, bak(name)) {
            result = Err(e);
            break;
        }
        if let Err(e) = std::fs::rename(tmp(name), &dest) {
            if had_old {
                let _ = std::fs::rename(bak(name), &dest);
            }
            result = Err(e);
            break;
        }
        committed.push((name, had_old));
    }
    for &(name, had_old) in committed.iter().rev() {
        if result.is_ok() {
            if had_old {
                let _ = std::fs::remove_file(bak(name));
            }
        } else if had_old {
            let _ = std::fs::rename(bak(name), dir.join(name));
        } else {
            let _ = std::fs::remove_file(dir.join(name));
        }
    }
    cleanup();
    result
}

/// The display scene files of `src/display` (`display/<name>.toml`),
/// sorted. Only plain names of regular files read from the directory are
/// taken (no symlinks, no subdirectories, nothing outside `src`).
fn scene_files(src: &Path) -> Result<Vec<String>, String> {
    let dir = src.join("display");
    let mut files = Vec::new();
    for e in std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let e = e.map_err(|e| e.to_string())?;
        let is_file = e.file_type().map_err(|e| e.to_string())?.is_file();
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        if is_file && is_plain_name(name) && name.ends_with(".toml") {
            files.push(format!("display/{name}"));
        }
    }
    files.sort();
    Ok(files)
}

/// Whether `name` is one path component (no separator, not `.`/`..`).
fn is_plain_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\'])
}

/// Builds the bundle from the sources in `src` (`layout.toml`,
/// `table.toml`, `backbox_layout.toml`, [`COPIED`], `display/*.toml`) into
/// `out_dir` (created if needed). Returns the generator's non-fatal
/// warnings, sorted.
///
/// The placeholder kit's title is the base table's `meta.name`, its
/// palette the table's `meta.palette`, its seed 0.
///
/// # Errors
/// A message naming the step and the file or element that failed; also
/// when `out_dir` is `src` itself (compared canonically, so a symlink alias
/// or an unnormalised path is caught), checked before anything is written.
pub fn build_from(src: &Path, out_dir: &Path) -> Result<Vec<String>, String> {
    let src_canon = src
        .canonicalize()
        .map_err(|e| format!("{}: {e}", src.display()))?;
    // Creating a missing output directory cannot touch `src`; an existing
    // one (`src` included) is left as it is.
    std::fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    let out_canon = out_dir
        .canonicalize()
        .map_err(|e| format!("{}: {e}", out_dir.display()))?;
    if out_canon == src_canon {
        return Err(format!(
            "output directory {} is the source directory {}: the build would overwrite \
             its own inputs (table.toml); choose another output directory",
            out_dir.display(),
            src.display()
        ));
    }
    let layout =
        parse_layout(&read_source(src, "layout.toml")?).map_err(|e| format!("layout.toml: {e}"))?;
    let base = TableManifest::from_toml_str(&read_source(src, TABLE_FILE)?)
        .map_err(|e| format!("table.toml: {e}"))?;
    let playfield = generate(&layout, &base).map_err(|e| format!("pin_tablegen: {e}"))?;
    let mut warnings: Vec<String> = playfield.warnings.iter().map(|w| w.to_string()).collect();

    let bb = parse_backbox_layout(&read_source(src, BACKBOX_LAYOUT_FILE)?)
        .map_err(|e| format!("{BACKBOX_LAYOUT_FILE}: {e}"))?;
    let backbox =
        generate_backbox(&bb.backbox, &playfield.table).map_err(|e| format!("backbox: {e}"))?;
    let mut table = backbox.table.clone();
    aim_backbox_vuk(&mut table, &backbox.frame)?;
    let text = backbox_table_toml(&table)?;
    let palette = base
        .meta
        .palette
        .iter()
        .map(|c| parse_hex(c))
        .collect::<Result<Vec<_>, _>>()?;
    let mut copied: Vec<String> = COPIED.iter().map(|s| (*s).to_owned()).collect();
    copied.extend(scene_files(src)?);

    write_bundle_with_backbox(&playfield, &backbox, &text, out_dir)
        .map_err(|e| format!("write bundle: {e}"))?;
    for f in &copied {
        let from = src.join(f);
        // Only regular files are copied (a symlink could point out of src).
        let regular = std::fs::symlink_metadata(&from)
            .map_err(|e| format!("copy {f}: {e}"))?
            .is_file();
        if !regular {
            return Err(format!("copy {f}: not a regular file"));
        }
        let to = out_dir.join(f);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::copy(&from, &to).map_err(|e| format!("copy {f}: {e}"))?;
    }

    let meta = TableMeta {
        title: base.meta.name.clone(),
        palette,
        seed: 0,
    };
    generate_kit(&meta, out_dir).map_err(|e| format!("pin_assetgen: {e}"))?;
    warnings.sort();
    Ok(warnings)
}

fn usage(m: &str) -> CliError {
    CliError::Usage(format!("{m}\n{BUILD_USAGE}"))
}

/// Runs `pin_tablegen build` with `args` (after `build`): `<src_dir> --out
/// <out_dir>` (or `--out=<out_dir>`). On success, returns a one-line
/// summary followed by one `warning: ...` line per generator warning.
///
/// # Errors
/// [`CliError::Usage`] for bad arguments (exit code 2), [`CliError::Failed`]
/// with [`build_from`]'s message otherwise (exit code 1).
pub fn run_build_cli(args: &[String]) -> Result<String, CliError> {
    let mut src: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let (key, inline) = match a.split_once('=') {
            Some((k, v)) if k.starts_with("--") => (k, Some(v.to_owned())),
            _ => (a.as_str(), None),
        };
        match key {
            "--out" => {
                let value = match inline {
                    Some(v) => v,
                    None => it
                        .next()
                        .cloned()
                        .ok_or_else(|| usage("--out needs a value"))?,
                };
                if out.replace(PathBuf::from(value)).is_some() {
                    return Err(usage("--out given twice"));
                }
            }
            "-h" | "--help" => return Err(usage("help requested")),
            k if k.starts_with('-') && k.len() > 1 => {
                return Err(usage(&format!("unknown option {k:?}")));
            }
            _ => {
                if src.replace(PathBuf::from(a)).is_some() {
                    return Err(usage(&format!("unexpected argument {a:?}")));
                }
            }
        }
    }
    let src = src.ok_or_else(|| usage("missing <src_dir>"))?;
    let out = out.ok_or_else(|| usage("missing --out <out_dir>"))?;
    let warnings = build_from(&src, &out).map_err(CliError::Failed)?;
    let mut summary = format!("built bundle {} from {}", out.display(), src.display());
    for w in &warnings {
        summary.push_str("\nwarning: ");
        summary.push_str(w);
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn build_argument_errors_are_usage() {
        for bad in [
            &[][..],
            &["src"],
            &["--out", "o"],
            &["src", "--out"],
            &["src", "--out", "o", "--out", "p"],
            &["src", "other", "--out", "o"],
            &["src", "--bogus", "--out", "o"],
            &["-h"],
            &["--help"],
        ] {
            let e = run_build_cli(&s(bad)).unwrap_err();
            assert_eq!(e.exit_code(), 2, "{bad:?}");
            assert!(e.to_string().contains(BUILD_USAGE), "{bad:?}: {e}");
        }
    }

    #[test]
    fn plain_names_only() {
        assert!(is_plain_name("scene.toml"));
        for bad in ["", ".", "..", "a/b.toml", "a\\b.toml"] {
            assert!(!is_plain_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn capped_reads() {
        let dir = std::env::temp_dir().join(format!("pin_tablegen_capped_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let over = dir.join("over");
        std::fs::write(&over, vec![b'#'; MAX_SOURCE_BYTES as usize + 1]).unwrap();
        let e = read_capped(&over, "over", MAX_SOURCE_BYTES).unwrap_err();
        assert!(e.starts_with("over: ") && e.contains("max"), "{e}");
        std::fs::write(dir.join("bad.toml"), [0xff, 0xfe]).unwrap();
        let e = read_source(&dir, "bad.toml").unwrap_err();
        assert!(e.contains("not valid UTF-8"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
        #[cfg(unix)]
        {
            let e = read_capped(Path::new("/dev/zero"), "zero", MAX_SOURCE_BYTES).unwrap_err();
            assert!(e.contains("larger than"), "{e}");
        }
    }
}
