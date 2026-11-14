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
        if table
            .flipper_stages
            .iter()
            .any(|s| *s == FlipperStageHint::Upper)
        {
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
    /// `width * height * 4` bytes.
    pub rgba: Vec<u8>,
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

/// Scans `root` for table bundles. Never panics; an unreadable or missing
/// root gives an empty list. See the module docs for the rules.
pub fn scan_tables(root: &Path) -> Vec<TableEntry> {
    let Ok(dir) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut candidates: Vec<(String, PathBuf)> = Vec::new();
    for entry in dir.flatten() {
        let path = entry.path();
        // Follows symlinks: a linked table folder is listed (its contents are
        // still confined by the VFS rules).
        let is_dir = std::fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false);
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
        .into_iter()
        .map(|(file_name, path)| {
            let status = inspect_bundle(&path);
            TableEntry {
                path,
                file_name,
                status,
            }
        })
        .collect()
}

/// Opens and validates one bundle (folder or archive) and reads its metadata
/// and preview. Never panics.
pub fn inspect_bundle(path: &Path) -> TableStatus {
    let vfs = match Vfs::open(path) {
        Ok(v) => v,
        Err(e) => return TableStatus::Broken(e.to_string()),
    };
    let table = match load_table(&vfs) {
        Ok(t) => t,
        Err(e) => return TableStatus::Broken(e.to_string()),
    };
    let (preview, preview_error) = match read_preview(&vfs) {
        Ok(p) => (p, None),
        Err(e) => (None, Some(e)),
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
        return Err(format!(
            "{file} is {size} bytes (max {MAX_PREVIEW_BYTES})"
        ));
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
        rgba,
    })
}

/// Box-filter downscale of RGBA8 so that both sides are ≤ `max_side`
/// (integer arithmetic, deterministic). Images already small enough are
/// returned unchanged.
pub fn box_downscale(rgba: &[u8], w: u32, h: u32, max_side: u32) -> (Vec<u8>, u32, u32) {
    let max_side = max_side.max(1);
    if w <= max_side && h <= max_side {
        return (rgba.to_vec(), w, h);
    }
    let (tw, th) = if w >= h {
        (max_side, (u64::from(h) * u64::from(max_side) / u64::from(w)).max(1) as u32)
    } else {
        ((u64::from(w) * u64::from(max_side) / u64::from(h)).max(1) as u32, max_side)
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
