//! Table directory scan for the browser.
//!
//! [`scan_tables`] is a plain function over a root folder. In the app it runs
//! on the I/O worker (the only thread that touches the file system at
//! runtime, PLAN §2.2); the result is handed to
//! [`MenuState::set_tables`](super::MenuState::set_tables).
//!
//! Rules (DECISIONS.md "4.4 — Menu UI decisions"):
//! * Only the direct children of the root are candidates (no recursion): every
//!   folder and every `.zip` / `.pinbundle` file. Names starting with `.` and
//!   other files are skipped.
//! * Each candidate is opened with [`Vfs::open`] and validated with
//!   [`load_table`]. A broken bundle is still listed, with its error text;
//!   nothing in the scan panics on bad input.
//! * Entries are sorted by [`TableEntry::sort_key`]: the file name, ASCII
//!   lower-cased, then the exact file name bytes (deterministic on every OS,
//!   independent of `read_dir` order).
//! * Preview art is `preview.png` (else `preview.jpg` / `preview.jpeg`) at the
//!   bundle root, decoded to RGBA8 and box-downscaled to at most
//!   [`PREVIEW_MAX_SIDE`] pixels per side. A broken or oversized preview is
//!   dropped (the table still loads) and noted in [`TableInfo::preview_error`].
//! * Preview byte budget (D25: scanned folders and images are untrusted): the
//!   decoded thumbnails of one scan hold at most [`PREVIEW_BUDGET_BYTES`] of
//!   RGBA (256 thumbnails of 256²; without it, 4,096 candidates could pin
//!   about 1 GiB). Previews are decoded in list order while the running total
//!   fits; once one does not fit, it and every later preview are dropped and
//!   no further preview is decoded, so the scan never holds more than the
//!   budget plus one thumbnail. Such a table is still `Ready` and loadable;
//!   the browser shows "(no preview)" and [`TableInfo::preview_error`] stays
//!   `None` ([`enforce_preview_budget`] applies the same rule to any list).

use pin_assets::validate::FlipperStageHint;
use pin_assets::{AssetError, LoadedTable, Vfs, load_table};
use pin_core::schema::backglass::ScoreDisplayMode;
use pin_core::schema::table::{FlipperShapeDef, MechKind, ScriptBackendKind};
use std::path::{Path, PathBuf};

/// Largest preview file read, bytes.
pub const MAX_PREVIEW_BYTES: u64 = 8 << 20;
/// Largest preview image side accepted before downscaling, pixels.
pub const MAX_PREVIEW_DECODE_SIDE: u32 = 4096;
/// Longest side of a decoded preview thumbnail, pixels.
pub const PREVIEW_MAX_SIDE: u32 = 256;
/// Most candidates one scan looks at (the rest are ignored, sorted first).
pub const MAX_SCAN_ENTRIES: usize = 4096;

/// Most decoded preview bytes (RGBA8, summed over every entry) one
/// [`scan_tables`] result holds: 64 MiB = 256 thumbnails of 256 × 256.
pub const PREVIEW_BUDGET_BYTES: u64 = 64 << 20;

/// Preview file names tried, in order.
pub const PREVIEW_FILES: [&str; 3] = ["preview.png", "preview.jpg", "preview.jpeg"];

/// A badge shown in the browser list, derived from the validated manifests.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TableFeature {
    /// Rules in Lua.
    LuaRules,
    /// Rules in WebAssembly.
    WasmRules,
    /// Upper flippers (a flipper staged from an upper button or above layer 0).
    UpperFlippers,
    /// Curved "banana" flippers.
    BananaFlippers,
    /// Ramps / wireforms (at least one spline).
    Ramps,
    /// Playfield magnets.
    Magnets,
    /// More than one ball in the dispensers at start (multiball capable).
    Multiball,
    /// Spinner.
    Spinner,
    /// Vertical up-kicker.
    Vuk,
    /// Kickback.
    Kickback,
    /// Diverter.
    Diverter,
    /// Captive Newton ball.
    NewtonBall,
    /// Backbox mini-playfield.
    Backbox,
    /// The backglass score display is an LCD.
    LcdDisplay,
    /// The backglass score display is a DMD.
    DmdDisplay,
    /// Direct Output Framework mappings.
    Dof,
}

impl TableFeature {
    /// Short badge text.
    pub const fn label(self) -> &'static str {
        match self {
            TableFeature::LuaRules => "Lua",
            TableFeature::WasmRules => "Wasm",
            TableFeature::UpperFlippers => "Upper flippers",
            TableFeature::BananaFlippers => "Banana flippers",
            TableFeature::Ramps => "Ramps",
            TableFeature::Magnets => "Magnets",
            TableFeature::Multiball => "Multiball",
            TableFeature::Spinner => "Spinner",
            TableFeature::Vuk => "VUK",
            TableFeature::Kickback => "Kickback",
            TableFeature::Diverter => "Diverter",
            TableFeature::NewtonBall => "Newton ball",
            TableFeature::Backbox => "Backbox",
            TableFeature::LcdDisplay => "LCD",
            TableFeature::DmdDisplay => "DMD",
            TableFeature::Dof => "DOF",
        }
    }

    /// The badges of a validated table, sorted (enum order), no duplicates.
    pub fn of(table: &LoadedTable) -> Vec<TableFeature> {
        let m = &table.manifest;
        let mut f = Vec::new();
        match m.meta.script_backend() {
            Some(ScriptBackendKind::Lua) => f.push(TableFeature::LuaRules),
            Some(ScriptBackendKind::Wasm) => f.push(TableFeature::WasmRules),
            None => {}
        }
        if table.flipper_stages.contains(&FlipperStageHint::Upper) {
            f.push(TableFeature::UpperFlippers);
        }
        if m.flippers
            .iter()
            .any(|fl| matches!(fl.shape, FlipperShapeDef::CurvedBanana { .. }))
        {
            f.push(TableFeature::BananaFlippers);
        }
        if !m.splines.is_empty() {
            f.push(TableFeature::Ramps);
        }
        if !m.magnets.is_empty() {
            f.push(TableFeature::Magnets);
        }
        if m.dispensers.iter().map(|d| d.contents.len()).sum::<usize>() > 1 {
            f.push(TableFeature::Multiball);
        }
        for (kind, feature) in [
            (MechKind::Spinner, TableFeature::Spinner),
            (MechKind::Vuk, TableFeature::Vuk),
            (MechKind::Kickback, TableFeature::Kickback),
            (MechKind::Diverter, TableFeature::Diverter),
            (MechKind::NewtonBall, TableFeature::NewtonBall),
        ] {
            if m.mechs.iter().any(|x| x.kind == kind) {
                f.push(feature);
            }
        }
        if table.backbox.is_some() || table.backglass.backbox.is_some() {
            f.push(TableFeature::Backbox);
        }
        f.push(match table.backglass.score_display.mode {
            ScoreDisplayMode::Lcd => TableFeature::LcdDisplay,
            ScoreDisplayMode::Dmd => TableFeature::DmdDisplay,
        });
        if m.dof.is_some() {
            f.push(TableFeature::Dof);
        }
        f.sort_unstable();
        f.dedup();
        f
    }
}

/// A decoded preview thumbnail (RGBA8, unpremultiplied, row-major).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewArt {
    /// Bundle path it was read from (e.g. `preview.png`).
    pub file: String,
    /// Width, pixels (1..=[`PREVIEW_MAX_SIDE`]).
    pub width: u32,
    /// Height, pixels (1..=[`PREVIEW_MAX_SIDE`]).
    pub height: u32,
    /// `width * height * 4` bytes, shared and immutable so that cloning a
    /// [`MenuState`](crate::ui::MenuState) copies only the reference count.
    pub rgba: std::sync::Arc<[u8]>,
}

/// Metadata of a bundle that validated.
#[derive(Clone, Debug, PartialEq)]
pub struct TableInfo {
    /// `[meta] id`.
    pub id: String,
    /// `[meta] name` (the title shown in the list).
    pub name: String,
    /// `[meta] author`.
    pub author: String,
    /// `[meta] version`.
    pub version: String,
    /// `[meta] players_max`.
    pub players_max: u8,
    /// Badges, sorted.
    pub features: Vec<TableFeature>,
    /// Preview thumbnail, if the bundle has a usable one.
    pub preview: Option<PreviewArt>,
    /// Why a preview file that exists could not be used.
    pub preview_error: Option<String>,
    /// Number of non-fatal validation warnings.
    pub warning_count: usize,
}

/// Whether a candidate validated.
#[derive(Clone, Debug, PartialEq)]
pub enum TableStatus {
    /// Validated; loadable.
    Ready(TableInfo),
    /// Could not be opened or did not validate. The text is the error
    /// (`AssetError`'s display: every validation error, one per line).
    Broken(String),
}

/// One scanned candidate.
#[derive(Clone, Debug, PartialEq)]
pub struct TableEntry {
    /// Full path of the folder or archive.
    pub path: PathBuf,
    /// Its file name (lossy UTF-8), shown when there is no title.
    pub file_name: String,
    /// Validation outcome.
    pub status: TableStatus,
}

impl TableEntry {
    /// Validated metadata, if the bundle is loadable.
    pub fn info(&self) -> Option<&TableInfo> {
        match &self.status {
            TableStatus::Ready(i) => Some(i),
            TableStatus::Broken(_) => None,
        }
    }

    /// The error text of a broken bundle.
    pub fn error(&self) -> Option<&str> {
        match &self.status {
            TableStatus::Ready(_) => None,
            TableStatus::Broken(e) => Some(e),
        }
    }

    /// Whether the bundle validated.
    pub fn is_ready(&self) -> bool {
        matches!(self.status, TableStatus::Ready(_))
    }

    /// Title shown in the list: `[meta] name`, or the file name.
    pub fn title(&self) -> &str {
        match &self.status {
            TableStatus::Ready(i) if !i.name.trim().is_empty() => &i.name,
            _ => &self.file_name,
        }
    }

    /// The scan order key: `(file name lower-cased (ASCII), file name)`.
    pub fn sort_key(&self) -> (String, &str) {
        (self.file_name.to_ascii_lowercase(), &self.file_name)
    }
}

fn is_candidate(path: &Path, is_dir: bool) -> bool {
    let Some(name) = path.file_name() else {
        return false;
    };
    if name.to_string_lossy().starts_with('.') {
        return false;
    }
    is_dir
        || path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("zip") || e.eq_ignore_ascii_case("pinbundle"))
}

/// Name of the tables folder in the packaged layout (SPEC §4 Subphase 7.2):
/// `<exe dir>/tables` on Windows and Linux, and `<Contents>/Resources/tables`
/// inside a macOS `.app` (the executable sits in `Contents/MacOS`).
pub const TABLES_DIR_NAME: &str = "tables";

/// Folder name of a macOS bundle's executable directory.
const MACOS_DIR_NAME: &str = "MacOS";
/// Folder name of a macOS bundle's `Contents` directory.
const CONTENTS_DIR_NAME: &str = "Contents";
/// Folder name of a macOS bundle's resources directory.
const RESOURCES_DIR_NAME: &str = "Resources";

/// The folder the table browser scans (REQ-VIEW-006, Subphase 4.4, SPEC §4
/// Subphase 7.2): `configured` (`config.toml`'s `tables_dir`) when set, else
/// [`TABLES_DIR_NAME`] next to the executable `exe` (its parent folder); an
/// `exe` without a parent folder (a bare file name) gives `./tables`. Inside
/// a macOS `.app` (the executable's folder is named `MacOS` and its parent
/// `Contents`, case-sensitive) the folder is `<Contents>/Resources/tables`.
/// Pure path logic: the filesystem is never consulted.
#[must_use]
pub fn resolve_tables_dir(configured: Option<&Path>, exe: &Path) -> PathBuf {
    if let Some(dir) = configured {
        return dir.to_path_buf();
    }
    let macos = exe.parent();
    let contents = macos.and_then(Path::parent);
    if let (Some(macos), Some(contents)) = (macos, contents) {
        let is_bundle = macos.file_name().is_some_and(|n| n == MACOS_DIR_NAME)
            && contents.file_name().is_some_and(|n| n == CONTENTS_DIR_NAME);
        if is_bundle {
            return contents.join(RESOURCES_DIR_NAME).join(TABLES_DIR_NAME);
        }
    }
    match exe.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(TABLES_DIR_NAME),
        _ => Path::new(".").join(TABLES_DIR_NAME),
    }
}

/// The table browser's folder for a run with the command line `args`:
/// `None` when the run names its table (any argument equal to `--table` or
/// starting with `--table=`). The app accepts only the separate form
/// `--table <path>` as naming a table; `--table=<path>` is rejected with a
/// command-line error (`reject_table_equals` in `main.rs`) before any table is
/// loaded, so `browser_dir` returning `None` for it never starts a run. Else [`resolve_tables_dir`]`(configured, exe)`.
#[must_use]
pub fn browser_dir(args: &[String], configured: Option<&Path>, exe: &Path) -> Option<PathBuf> {
    if args
        .iter()
        .any(|a| a == "--table" || a.starts_with("--table="))
    {
        return None;
    }
    Some(resolve_tables_dir(configured, exe))
}

/// Decoded preview bytes held by `entries`: the sum of `rgba.len()` over
/// every `Ready` entry's preview.
pub fn preview_bytes(entries: &[TableEntry]) -> u64 {
    entries
        .iter()
        .filter_map(TableEntry::info)
        .filter_map(|i| i.preview.as_ref())
        .map(|p| p.rgba.len() as u64)
        .sum()
}

/// Puts `entries` under a decoded-preview byte `budget`: in list order,
/// previews are kept while the running total stays within `budget`; the
/// first preview that would exceed it and every later one are set to `None`
/// (`preview_error` and every other field are left unchanged). Idempotent.
pub fn enforce_preview_budget(entries: &mut [TableEntry], budget: u64) {
    let mut used = 0u64;
    let mut full = false;
    for e in entries {
        let TableStatus::Ready(info) = &mut e.status else {
            continue;
        };
        let Some(p) = &info.preview else { continue };
        let n = p.rgba.len() as u64;
        if !full && used.saturating_add(n) <= budget {
            used += n;
        } else {
            full = true;
            info.preview = None;
        }
    }
}

/// Scans `root` for table bundles. Never panics; an unreadable or missing
/// root gives an empty list. The decoded previews of the result stay within
/// [`PREVIEW_BUDGET_BYTES`] ([`preview_bytes`]). See the module docs for the
/// rules.
pub fn scan_tables(root: &Path) -> Vec<TableEntry> {
    scan_tables_with_budget(root, PREVIEW_BUDGET_BYTES)
}

/// [`scan_tables`] with a preview byte `budget` (tests use a small one).
fn scan_tables_with_budget(root: &Path, budget: u64) -> Vec<TableEntry> {
    let candidates = scan_candidates(root);
    inspect_within_budget(candidates, budget, inspect)
}

/// Inspects `candidates` in order with `inspect(path, decode_preview)`,
/// decoding previews only while the running total of kept preview bytes is
/// within `budget`: the first preview that does not fit is dropped and no
/// later preview is decoded (at most `budget` + one thumbnail is held at any
/// time). The result satisfies `preview_bytes(..) <= budget`.
fn inspect_within_budget(
    candidates: Vec<(String, PathBuf)>,
    budget: u64,
    mut inspect: impl FnMut(&Path, bool) -> TableStatus,
) -> Vec<TableEntry> {
    let mut used = 0u64;
    let mut full = false;
    let mut out: Vec<TableEntry> = candidates
        .into_iter()
        .map(|(file_name, path)| {
            let mut status = inspect(&path, !full);
            if let TableStatus::Ready(info) = &mut status
                && let Some(p) = &info.preview
            {
                let n = p.rgba.len() as u64;
                if !full && used.saturating_add(n) <= budget {
                    used += n;
                } else {
                    full = true;
                    info.preview = None;
                }
            }
            TableEntry {
                path,
                file_name,
                status,
            }
        })
        .collect();
    // Already true by construction; kept so the bound never depends on it.
    enforce_preview_budget(&mut out, budget);
    out
}

/// The sorted, truncated candidate list of `root` (`(file name, path)`).
fn scan_candidates(root: &Path) -> Vec<(String, PathBuf)> {
    let Ok(dir) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut candidates: Vec<(String, PathBuf)> = Vec::new();
    for entry in dir.flatten() {
        let path = entry.path();
        // Follows symlinks: a linked table folder is listed (its contents are
        // still confined by the VFS rules).
        let is_dir = std::fs::metadata(&path)
            .map(|m| m.is_dir())
            .unwrap_or(false);
        if is_candidate(&path, is_dir) {
            let name = entry.file_name().to_string_lossy().into_owned();
            candidates.push((name, path));
        }
    }
    candidates.sort_by(|a, b| {
        (a.0.to_ascii_lowercase(), &a.0, &a.1).cmp(&(b.0.to_ascii_lowercase(), &b.0, &b.1))
    });
    candidates.truncate(MAX_SCAN_ENTRIES);
    candidates
}

/// Opens and validates one bundle (folder or archive) and reads its metadata
/// and preview. Never panics.
pub fn inspect_bundle(path: &Path) -> TableStatus {
    inspect(path, true)
}

/// [`inspect_bundle`]; the preview is read and decoded only when
/// `decode_preview` (otherwise `preview` and `preview_error` are `None`).
fn inspect(path: &Path, decode_preview: bool) -> TableStatus {
    let vfs = match Vfs::open(path) {
        Ok(v) => v,
        Err(e) => return TableStatus::Broken(e.to_string()),
    };
    let table = match load_table(&vfs) {
        Ok(t) => t,
        Err(e) => return TableStatus::Broken(e.to_string()),
    };
    let (preview, preview_error) = if !decode_preview {
        (None, None)
    } else {
        match read_preview(&vfs) {
            Ok(p) => (p, None),
            Err(e) => (None, Some(e)),
        }
    };
    let meta = &table.manifest.meta;
    TableStatus::Ready(TableInfo {
        id: meta.id.clone(),
        name: meta.name.clone(),
        author: meta.author.clone(),
        version: meta.version.clone(),
        players_max: meta.players_max,
        features: TableFeature::of(&table),
        preview,
        preview_error,
        warning_count: table.warnings.len(),
    })
}

/// Reads and decodes the bundle's preview, if any. `Ok(None)` = no preview file.
fn read_preview(vfs: &Vfs) -> Result<Option<PreviewArt>, String> {
    let Some(file) = PREVIEW_FILES.iter().find(|f| vfs.exists(f)) else {
        return Ok(None);
    };
    let size = vfs.file_size(file).map_err(|e| e.to_string())?;
    if size > MAX_PREVIEW_BYTES {
        return Err(format!("{file} is {size} bytes (max {MAX_PREVIEW_BYTES})"));
    }
    let bytes = vfs.read(file).map_err(|e: AssetError| e.to_string())?;
    let art = decode_preview(&bytes).map_err(|e| format!("{file}: {e}"))?;
    Ok(Some(PreviewArt {
        file: (*file).to_owned(),
        ..art
    }))
}

/// Decodes PNG/JPEG bytes into a thumbnail of at most [`PREVIEW_MAX_SIDE`]
/// per side (box filter, aspect kept). Images larger than
/// [`MAX_PREVIEW_DECODE_SIDE`] per side are refused before decoding. Never
/// panics. The returned `file` is empty.
pub fn decode_preview(bytes: &[u8]) -> Result<PreviewArt, String> {
    use image::ImageReader;
    let mut reader = ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_PREVIEW_DECODE_SIDE);
    limits.max_image_height = Some(MAX_PREVIEW_DECODE_SIDE);
    limits.max_alloc = Some(u64::from(MAX_PREVIEW_DECODE_SIDE).pow(2) * 4 + (1 << 20));
    reader.limits(limits);
    let img = reader.decode().map_err(|e| e.to_string())?.into_rgba8();
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return Err("empty image".to_owned());
    }
    let (rgba, tw, th) = box_downscale(img.as_raw(), w, h, PREVIEW_MAX_SIDE);
    Ok(PreviewArt {
        file: String::new(),
        width: tw,
        height: th,
        rgba: rgba.into(),
    })
}

/// Box-filter downscale of RGBA8 so that both sides are ≤ `max_side`
/// (integer arithmetic, deterministic). Images already small enough are
/// returned unchanged. `rgba` must hold `w * h * 4` bytes (the decoder's
/// output); anything else yields an empty 0×0 image instead of indexing out
/// of bounds.
fn box_downscale(rgba: &[u8], w: u32, h: u32, max_side: u32) -> (Vec<u8>, u32, u32) {
    if w == 0 || h == 0 || rgba.len() as u64 != u64::from(w) * u64::from(h) * 4 {
        return (Vec::new(), 0, 0);
    }
    let max_side = max_side.max(1);
    if w <= max_side && h <= max_side {
        return (rgba.to_vec(), w, h);
    }
    let (tw, th) = if w >= h {
        (
            max_side,
            (u64::from(h) * u64::from(max_side) / u64::from(w)).max(1) as u32,
        )
    } else {
        (
            (u64::from(w) * u64::from(max_side) / u64::from(h)).max(1) as u32,
            max_side,
        )
    };
    let mut out = vec![0u8; (tw * th * 4) as usize];
    for ty in 0..th {
        let y0 = (u64::from(ty) * u64::from(h) / u64::from(th)) as u32;
        let y1 = ((u64::from(ty + 1) * u64::from(h)).div_ceil(u64::from(th)) as u32).max(y0 + 1);
        for tx in 0..tw {
            let x0 = (u64::from(tx) * u64::from(w) / u64::from(tw)) as u32;
            let x1 =
                ((u64::from(tx + 1) * u64::from(w)).div_ceil(u64::from(tw)) as u32).max(x0 + 1);
            let mut acc = [0u64; 4];
            for y in y0..y1.min(h) {
                for x in x0..x1.min(w) {
                    let i = ((y * w + x) * 4) as usize;
                    for c in 0..4 {
                        acc[c] += u64::from(rgba[i + c]);
                    }
                }
            }
            let n = u64::from(y1.min(h) - y0) * u64::from(x1.min(w) - x0);
            let o = ((ty * tw + tx) * 4) as usize;
            for c in 0..4 {
                out[o + c] = ((acc[c] + n / 2) / n) as u8;
            }
        }
    }
    (out, tw, th)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_downscale_rejects_inconsistent_input() {
        assert_eq!(box_downscale(&[0; 7], 2, 1, 1), (Vec::new(), 0, 0));
        assert_eq!(box_downscale(&[], 0, 5, 4), (Vec::new(), 0, 0));
        assert_eq!(box_downscale(&[1; 8], 2, 1, 1), (vec![1, 1, 1, 1], 1, 1));
        assert_eq!(box_downscale(&[1; 8], 2, 1, 8), (vec![1; 8], 2, 1));
    }

    /// REQ-VIEW-006: the tables folder is the configured one, else
    /// `tables` next to the executable, else `./tables`.
    #[test]
    fn req_view_006_resolve_tables_dir() {
        let exe = Path::new("/opt/kp/pin_app");
        assert_eq!(
            resolve_tables_dir(None, exe),
            PathBuf::from("/opt/kp/tables")
        );
        assert_eq!(
            resolve_tables_dir(Some(Path::new("/data/t")), exe),
            PathBuf::from("/data/t")
        );
        assert_eq!(
            resolve_tables_dir(None, Path::new("pin_app")),
            PathBuf::from("./tables")
        );
        assert_eq!(
            resolve_tables_dir(None, Path::new("/")),
            PathBuf::from("./tables")
        );
    }

    /// REQ-VIEW-006: `--table` (also `--table=x` and a malformed bare
    /// `--table`) disables the browser.
    #[test]
    fn req_view_006_browser_dir_respects_the_table_flag() {
        let a = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let exe = Path::new("/opt/kp/pin_app");
        for args in [
            a(&["--table", "x"]),
            a(&["--table=x"]),
            a(&["--run-ms", "5", "--table"]),
        ] {
            assert_eq!(browser_dir(&args, None, exe), None, "{args:?}");
        }
        assert_eq!(
            browser_dir(&a(&["--config", "c.toml", "--tables"]), None, exe),
            Some(PathBuf::from("/opt/kp/tables"))
        );
        assert_eq!(
            browser_dir(&[], Some(Path::new("mine")), exe),
            Some(PathBuf::from("mine"))
        );
    }

    /// REQ-VIEW-006, SPEC §4 Subphase 7.2: inside a macOS `.app` the tables
    /// folder is `Contents/Resources/tables`.
    #[test]
    fn req_view_006_resolve_tables_dir_macos_bundle() {
        assert_eq!(
            resolve_tables_dir(None, Path::new("/x/K.app/Contents/MacOS/kilohertz-pinball")),
            PathBuf::from("/x/K.app/Contents/Resources/tables")
        );
    }

    /// REQ-VIEW-006, SPEC §4 Subphase 7.2: `MacOS` / `Contents` names alone
    /// (wrong nesting, wrong case) do not trigger the bundle rule.
    #[test]
    fn req_view_006_resolve_tables_dir_non_bundle_macos_names() {
        for (exe, want) in [
            ("/x/MacOS/kilohertz-pinball", "/x/MacOS/tables"),
            ("/x/Other/MacOS/kilohertz-pinball", "/x/Other/MacOS/tables"),
            ("/x/Contents/kilohertz-pinball", "/x/Contents/tables"),
            (
                "/x/contents/macos/kilohertz-pinball",
                "/x/contents/macos/tables",
            ),
            (
                "/x/Contents/Other/kilohertz-pinball",
                "/x/Contents/Other/tables",
            ),
        ] {
            assert_eq!(
                resolve_tables_dir(None, Path::new(exe)),
                PathBuf::from(want)
            );
        }
    }

    /// REQ-VIEW-006, SPEC §4 Subphase 7.2: Windows and Linux layouts keep
    /// `<exe dir>/tables`.
    #[test]
    fn req_view_006_resolve_tables_dir_windows_and_linux_unchanged() {
        assert_eq!(
            resolve_tables_dir(None, Path::new("C:/Games/K/kilohertz-pinball.exe")),
            PathBuf::from("C:/Games/K/tables")
        );
        assert_eq!(
            resolve_tables_dir(None, Path::new("/opt/k/kilohertz-pinball")),
            PathBuf::from("/opt/k/tables")
        );
    }

    /// REQ-VIEW-006, SPEC §4 Subphase 7.2: a configured folder wins, also
    /// for a bundle executable path; `browser_dir` follows the same rules.
    #[test]
    fn req_view_006_configured_and_browser_dir_with_macos_bundle() {
        let exe = Path::new("/x/K.app/Contents/MacOS/kilohertz-pinball");
        assert_eq!(
            resolve_tables_dir(Some(Path::new("/data/t")), exe),
            PathBuf::from("/data/t")
        );
        assert_eq!(
            browser_dir(&[], None, exe),
            Some(PathBuf::from("/x/K.app/Contents/Resources/tables"))
        );
        assert_eq!(browser_dir(&["--table".to_owned()], None, exe), None);
        assert_eq!(
            browser_dir(&[], Some(Path::new("mine")), exe),
            Some(PathBuf::from("mine"))
        );
    }

    /// A synthetic ready entry with a `side`² RGBA preview (zeroed, so the
    /// pages of large test lists are never touched).
    fn ready(i: usize, side: u32) -> TableEntry {
        TableEntry {
            path: PathBuf::from(format!("/t/{i:05}")),
            file_name: format!("{i:05}"),
            status: TableStatus::Ready(TableInfo {
                id: format!("t{i}"),
                name: format!("Table {i}"),
                author: "a".into(),
                version: "1".into(),
                players_max: 1,
                features: Vec::new(),
                preview: Some(PreviewArt {
                    file: "preview.png".into(),
                    width: side,
                    height: side,
                    rgba: vec![0; (side * side * 4) as usize].into(),
                }),
                preview_error: Some("kept".into()),
                warning_count: 0,
            }),
        }
    }

    /// SPEC §4 Subphase 4.4 / D25 (#131): 4,096 entries with 256² previews
    /// (1 GiB of RGBA) end within `PREVIEW_BUDGET_BYTES`; the first 256 keep
    /// their previews, later ones lose only the preview; idempotent.
    #[test]
    fn spec_4_4_preview_budget_on_4096_thumbnails() {
        let mut v: Vec<TableEntry> = (0..MAX_SCAN_ENTRIES).map(|i| ready(i, 256)).collect();
        v.push(TableEntry {
            path: "/t/broken".into(),
            file_name: "broken".into(),
            status: TableStatus::Broken("x".into()),
        });
        assert_eq!(preview_bytes(&v), 4096 * 256 * 256 * 4);
        let before = v.clone_without_previews();
        enforce_preview_budget(&mut v, PREVIEW_BUDGET_BYTES);
        assert_eq!(preview_bytes(&v), PREVIEW_BUDGET_BYTES);
        let kept = v
            .iter()
            .filter(|e| e.info().is_some_and(|i| i.preview.is_some()))
            .count();
        assert_eq!(kept, 256);
        assert!(v[..256].iter().all(|e| e.info().unwrap().preview.is_some()));
        assert!(
            v[256..4096]
                .iter()
                .all(|e| e.info().unwrap().preview.is_none())
        );
        // Nothing but the preview changed (still Ready, error text kept).
        assert_eq!(v.clone_without_previews(), before);
        let snapshot = v.clone();
        enforce_preview_budget(&mut v, PREVIEW_BUDGET_BYTES);
        assert_eq!(v, snapshot, "idempotent");
        // Once a preview does not fit, later (smaller) ones are dropped too.
        let mut w = vec![ready(0, 2), ready(1, 4), ready(2, 1)];
        enforce_preview_budget(&mut w, 20);
        let has: Vec<bool> = w
            .iter()
            .map(|e| e.info().unwrap().preview.is_some())
            .collect();
        assert_eq!(has, [true, false, false]);
        enforce_preview_budget(&mut w, 0);
        assert_eq!(preview_bytes(&w), 0);
    }

    trait WithoutPreviews {
        fn clone_without_previews(&self) -> Vec<TableEntry>;
    }

    impl WithoutPreviews for Vec<TableEntry> {
        fn clone_without_previews(&self) -> Vec<TableEntry> {
            // Field by field: cloning the previews would touch every page.
            self.iter()
                .map(|e| TableEntry {
                    path: e.path.clone(),
                    file_name: e.file_name.clone(),
                    status: match &e.status {
                        TableStatus::Ready(i) => TableStatus::Ready(TableInfo {
                            id: i.id.clone(),
                            name: i.name.clone(),
                            author: i.author.clone(),
                            version: i.version.clone(),
                            players_max: i.players_max,
                            features: i.features.clone(),
                            preview: None,
                            preview_error: i.preview_error.clone(),
                            warning_count: i.warning_count,
                        }),
                        TableStatus::Broken(b) => TableStatus::Broken(b.clone()),
                    },
                })
                .collect()
        }
    }

    /// SPEC §4 Subphase 4.4 / D25 (#131): a scan of 4,096 candidates whose
    /// previews are 256² decodes only until the budget is reached (never
    /// holding more than the budget plus one thumbnail) and ends within it.
    #[test]
    fn spec_4_4_scan_stops_decoding_at_the_budget() {
        let candidates: Vec<(String, PathBuf)> = (0..MAX_SCAN_ENTRIES)
            .map(|i| (format!("{i:05}"), PathBuf::from(format!("/t/{i:05}"))))
            .collect();
        let mut decodes = 0usize;
        let mut asked = 0usize;
        let out = inspect_within_budget(candidates, PREVIEW_BUDGET_BYTES, |path, decode| {
            asked += 1;
            let i: usize = path.file_name().unwrap().to_str().unwrap().parse().unwrap();
            let mut e = ready(i, 256);
            if decode {
                decodes += 1;
            } else if let TableStatus::Ready(info) = &mut e.status {
                info.preview = None;
                info.preview_error = None;
            }
            e.status
        });
        assert_eq!(asked, MAX_SCAN_ENTRIES, "every table is still inspected");
        assert_eq!(decodes, 257, "256 kept + the one that did not fit");
        assert_eq!(preview_bytes(&out), PREVIEW_BUDGET_BYTES);
        assert!(out.iter().all(TableEntry::is_ready));
        assert!(
            out[256..]
                .iter()
                .all(|e| e.info().unwrap().preview.is_none())
        );
    }

    /// Writes a copy of the reference bundle (plus stand-in meshes) to
    /// `dir`, with `preview` as its `preview.png`.
    fn write_bundle(dir: &Path, preview: &[u8]) {
        fn copy(from: &Path, to: &Path) {
            std::fs::create_dir_all(to).unwrap();
            for e in std::fs::read_dir(from).unwrap() {
                let e = e.unwrap();
                let p = e.path();
                if p.is_dir() {
                    copy(&p, &to.join(e.file_name()));
                } else {
                    std::fs::copy(&p, to.join(e.file_name())).unwrap();
                }
            }
        }
        let apex = Path::new(env!("CARGO_MANIFEST_DIR")).join("../pin_assets/tests/fixtures/apex");
        copy(&apex, dir);
        for mesh in ["playfield.glb", "backbox.glb"] {
            std::fs::write(dir.join(mesh), triangle_glb()).unwrap();
        }
        std::fs::write(dir.join("preview.png"), preview).unwrap();
    }

    /// A one-triangle GLB.
    fn triangle_glb() -> Vec<u8> {
        let mut bin: Vec<u8> = [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        bin.extend([0u16, 1, 2].iter().flat_map(|i| i.to_le_bytes()));
        bin.extend([0, 0]);
        let json = r#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],
"nodes":[{"name":"tri","mesh":0}],
"meshes":[{"name":"tri","primitives":[{"attributes":{"POSITION":0},"indices":1}]}],
"accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0.0,0.0,0.0],"max":[1.0,1.0,0.0]},
{"bufferView":1,"componentType":5123,"count":3,"type":"SCALAR"}],
"bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":36},{"buffer":0,"byteOffset":36,"byteLength":6}],
"buffers":[{"byteLength":44}]}"#;
        let mut j = json.as_bytes().to_vec();
        while !j.len().is_multiple_of(4) {
            j.push(b' ');
        }
        let total = 12 + 8 + j.len() + 8 + bin.len();
        let mut out = Vec::with_capacity(total);
        out.extend(b"glTF");
        out.extend(2u32.to_le_bytes());
        out.extend((total as u32).to_le_bytes());
        out.extend((j.len() as u32).to_le_bytes());
        out.extend(0x4E4F_534Au32.to_le_bytes());
        out.extend(&j);
        out.extend((bin.len() as u32).to_le_bytes());
        out.extend(0x004E_4942u32.to_le_bytes());
        out.extend(&bin);
        out
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([x as u8, y as u8, 7, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    /// SPEC §4 Subphase 4.4 / D25 (#131): a real scan with a tiny budget
    /// keeps the first preview, drops the one that does not fit and does not
    /// even decode later ones (a broken later preview reports no error);
    /// every table stays ready.
    #[test]
    fn spec_4_4_real_scan_respects_a_tiny_budget() {
        let root = std::env::temp_dir().join(format!("kp_scan131_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write_bundle(&root.join("a"), &png(4, 4));
        write_bundle(&root.join("b"), &png(4, 4));
        write_bundle(&root.join("c"), b"\x89PNG broken");
        let full = scan_tables(&root);
        assert_eq!(full.len(), 3);
        assert!(full.iter().all(TableEntry::is_ready), "{full:?}");
        assert_eq!(preview_bytes(&full), 2 * 64);
        assert!(full[2].info().unwrap().preview_error.is_some());
        let small = scan_tables_with_budget(&root, 100);
        let _ = std::fs::remove_dir_all(&root);
        let info = |i: usize| small[i].info().unwrap();
        assert!(small.iter().all(TableEntry::is_ready));
        assert_eq!(preview_bytes(&small), 64);
        assert!(info(0).preview.is_some());
        assert!(info(1).preview.is_none() && info(1).preview_error.is_none());
        assert!(info(2).preview.is_none());
        assert!(
            info(2).preview_error.is_none(),
            "a preview after the budget is not decoded"
        );
    }
}
