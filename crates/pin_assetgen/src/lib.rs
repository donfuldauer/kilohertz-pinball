//! `pin_assetgen`: Code-generated, licence-clean placeholder sounds, fonts and
//! art (Subphase 4.6, D32: no table depends on third-party files).
//!
//! [`generate_kit`] writes the whole kit for one table into a bundle
//! directory and records it in `<out_dir>/assets.toml` ([`MANIFEST_FILE`]):
//! stable id → bundle path, kind, generator version and SHA-256. A table's
//! `table.toml` / `backglass.toml` refer to these paths (and rules to the
//! ids), so real assets later replace placeholders file by file without
//! touching rules or TOML.
//!
//! # Stable ids and files
//!
//! | kind | ids | file |
//! |---|---|---|
//! | [`AssetKind::Sound`] | [`sounds::SFX_IDS`] | `audio/<id>.wav` |
//! | [`AssetKind::Music`] | [`sounds::MUSIC_ID`] (`music_main`) | `audio/music_main.wav` |
//! | [`AssetKind::Motif`] | [`sounds::MOTIF_IDS`] (`callout_*`) | `audio/<id>.wav` |
//! | [`AssetKind::Font`] | `font_dmd_5x7`, `font_dmd_8x16` | `fonts/dmd_5x7.png`, `fonts/dmd_8x16.png` (glyph sheets) |
//! | [`AssetKind::Image`] | `img_backglass_base`, `img_backglass_logo`, `img_backglass_logo_mask` | `backglass/base.png`, `backglass/logo.png`, `backglass/logo_mask.png` (the A.2 layer layout) |
//! | [`AssetKind::Image`] | `img_playfield` | `art/playfield.png` |
//! | [`AssetKind::Image`] | `img_insert_<name>` for [`art::INSERT_LABELS`] | `art/inserts/<name>.png` |
//!
//! Sound effects, motifs and font sheets are the same for every table; the
//! music loop and all images depend on the [`TableMeta`] (title, palette,
//! seed). The kit provides no `[[scenes]]` files: the DMD scene format belongs
//! to Subphase 5.3, which renders text with [`fonts`].
//!
//! # Determinism
//!
//! Generation uses the kit's own PCG ([`rng::Pcg32`]) seeded from
//! [`meta_seed`], integer drawing, `f64` synthesis in a fixed order with
//! `libm` for transcendental functions (no FMA, no time, no thread RNG, no
//! hash-map iteration), 16-bit WAV through `hound` and a fixed-setting PNG
//! encoder ([`art::png`]). The same inputs give byte-identical files on every
//! OS.

pub mod art;
pub mod cli;
pub mod fonts;
pub mod rng;
mod sha256;
pub mod sounds;

use std::path::Path;

use serde::{Deserialize, Serialize};

pub use sha256::{sha256, sha256_hex};

/// Version of the generators; recorded per manifest entry. Bump when any
/// generator's output changes.
pub const GENERATOR_VERSION: u32 = 1;

/// Name of the manifest written into the bundle directory.
pub const MANIFEST_FILE: &str = "assets.toml";

/// Version of the `assets.toml` format.
pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

/// Table metadata the kit is derived from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableMeta {
    /// Table title (any Unicode; drawn upper-cased, non-ASCII as the font's
    /// fallback box).
    pub title: String,
    /// Palette, as in `table.toml` `meta.palette` (empty: a default palette).
    pub palette: Vec<[u8; 3]>,
    /// Extra seed, so tables with the same title and palette can differ.
    pub seed: u64,
}

/// What a kit entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetKind {
    /// A sound effect (bus `sfx`).
    Sound,
    /// The music loop (bus `music`).
    Music,
    /// A tone motif in place of a spoken callout (bus `callout`).
    Motif,
    /// A bitmap font sheet.
    Font,
    /// A PNG image.
    Image,
}

/// One generated file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KitEntry {
    /// Stable id.
    pub id: String,
    /// Bundle-relative path with `/` separators.
    pub path: String,
    /// Kind.
    pub kind: AssetKind,
    /// Lowercase hex SHA-256 of the file ([`sha256_hex`]).
    pub sha256_or_hash: String,
}

/// The kit's manifest (`assets.toml`).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct KitManifest {
    /// Every generated file, in generation order.
    pub entries: Vec<KitEntry>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestFile {
    schema_version: u32,
    generator: String,
    generator_version: u32,
    table: ManifestTable,
    #[serde(default)]
    entries: Vec<ManifestEntry>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestTable {
    title: String,
    palette: Vec<String>,
    /// Decimal string: TOML integers cannot hold every `u64`.
    seed: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestEntry {
    id: String,
    path: String,
    kind: AssetKind,
    generator_version: u32,
    sha256_or_hash: String,
}

impl KitManifest {
    /// The entry with stable id `id`.
    pub fn get(&self, id: &str) -> Option<&KitEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// `[[sounds]]` definitions for every sound entry (sound effects on bus
    /// `sfx`, music on `music`, motifs on `callout`), ready to put into a
    /// `table.toml`.
    pub fn sound_defs(&self) -> Vec<pin_core::schema::table::SoundDef> {
        use pin_core::schema::table::{SoundBus, SoundDef};
        self.entries
            .iter()
            .filter_map(|e| {
                let bus = match e.kind {
                    AssetKind::Sound => SoundBus::Sfx,
                    AssetKind::Music => SoundBus::Music,
                    AssetKind::Motif => SoundBus::Callout,
                    AssetKind::Font | AssetKind::Image => return None,
                };
                Some(SoundDef {
                    id: e.id.clone(),
                    file: e.path.clone(),
                    bus,
                })
            })
            .collect()
    }

    /// The manifest as written to `assets.toml` for `meta`.
    pub fn to_toml(&self, meta: &TableMeta) -> String {
        let file = ManifestFile {
            schema_version: MANIFEST_SCHEMA_VERSION,
            generator: "pin_assetgen".into(),
            generator_version: GENERATOR_VERSION,
            table: ManifestTable {
                title: meta.title.clone(),
                palette: meta
                    .palette
                    .iter()
                    .map(|c| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]))
                    .collect(),
                seed: meta.seed.to_string(),
            },
            entries: self
                .entries
                .iter()
                .map(|e| ManifestEntry {
                    id: e.id.clone(),
                    path: e.path.clone(),
                    kind: e.kind,
                    generator_version: GENERATOR_VERSION,
                    sha256_or_hash: e.sha256_or_hash.clone(),
                })
                .collect(),
        };
        // Plain data with string keys: serialisation cannot fail.
        toml::to_string(&file).unwrap_or_default()
    }

    /// Parses an `assets.toml`. Never panics; unknown keys, a different
    /// schema version, duplicate ids and paths that are absolute, empty or
    /// contain `..` / `\` are errors.
    pub fn from_toml(text: &str) -> Result<KitManifest, String> {
        let file: ManifestFile = toml::from_str(text).map_err(|e| e.to_string())?;
        if file.schema_version != MANIFEST_SCHEMA_VERSION {
            return Err(format!(
                "unsupported assets.toml schema_version {}",
                file.schema_version
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        let mut entries = Vec::with_capacity(file.entries.len());
        for e in file.entries {
            if !seen.insert(e.id.clone()) {
                return Err(format!("duplicate asset id {:?}", e.id));
            }
            if !is_bundle_path(&e.path) {
                return Err(format!("invalid bundle path {:?} for {:?}", e.path, e.id));
            }
            entries.push(KitEntry {
                id: e.id,
                path: e.path,
                kind: e.kind,
                sha256_or_hash: e.sha256_or_hash,
            });
        }
        Ok(KitManifest { entries })
    }

    /// Reads and parses `<dir>/assets.toml`.
    pub fn read(dir: &Path) -> std::io::Result<KitManifest> {
        let text = std::fs::read_to_string(dir.join(MANIFEST_FILE))?;
        KitManifest::from_toml(&text)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}

/// A relative, `/`-separated path without empty, `.` or `..` components.
fn is_bundle_path(p: &str) -> bool {
    !p.is_empty()
        && !p.contains('\\')
        && !p.contains(':')
        && p.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}

/// The 64-bit seed every metadata-dependent generator starts from: FNV-1a over
/// the length-prefixed title bytes, palette bytes and seed, finished with
/// SplitMix64.
pub fn meta_seed(meta: &TableMeta) -> u64 {
    use rng::{FNV_OFFSET, fnv1a, mix64};
    let mut h = fnv1a(FNV_OFFSET, &(meta.title.len() as u64).to_le_bytes());
    h = fnv1a(h, meta.title.as_bytes());
    h = fnv1a(h, &(meta.palette.len() as u64).to_le_bytes());
    for c in &meta.palette {
        h = fnv1a(h, c);
    }
    h = fnv1a(h, &meta.seed.to_le_bytes());
    mix64(h)
}

/// Every stable id the kit provides, in manifest order.
pub fn stable_ids() -> Vec<String> {
    plan().into_iter().map(|p| p.id).collect()
}

/// What to generate for one entry.
enum Source {
    Sfx(&'static str),
    Music,
    Motif(&'static str),
    FontSheet(&'static fonts::BitmapFont),
    BackglassBase,
    BackglassLogo,
    BackglassLogoMask,
    Playfield,
    Insert(&'static str),
}

struct Planned {
    id: String,
    path: String,
    kind: AssetKind,
    source: Source,
}

fn plan() -> Vec<Planned> {
    let mut v = Vec::new();
    let mut push = |id: String, path: String, kind, source| {
        v.push(Planned {
            id,
            path,
            kind,
            source,
        })
    };
    for &id in sounds::SFX_IDS {
        push(id.into(), format!("audio/{id}.wav"), AssetKind::Sound, Source::Sfx(id));
    }
    push(
        sounds::MUSIC_ID.into(),
        format!("audio/{}.wav", sounds::MUSIC_ID),
        AssetKind::Music,
        Source::Music,
    );
    for &id in sounds::MOTIF_IDS {
        push(id.into(), format!("audio/{id}.wav"), AssetKind::Motif, Source::Motif(id));
    }
    for font in [fonts::FONT_5X7, fonts::FONT_8X16] {
        let n = font.name();
        push(
            format!("font_dmd_{n}"),
            format!("fonts/dmd_{n}.png"),
            AssetKind::Font,
            Source::FontSheet(font),
        );
    }
    let img = AssetKind::Image;
    push("img_backglass_base".into(), "backglass/base.png".into(), img, Source::BackglassBase);
    push("img_backglass_logo".into(), "backglass/logo.png".into(), img, Source::BackglassLogo);
    push(
        "img_backglass_logo_mask".into(),
        "backglass/logo_mask.png".into(),
        img,
        Source::BackglassLogoMask,
    );
    push("img_playfield".into(), "art/playfield.png".into(), img, Source::Playfield);
    for &(name, text) in art::INSERT_LABELS {
        push(
            format!("img_insert_{name}"),
            format!("art/inserts/{name}.png"),
            img,
            Source::Insert(text),
        );
    }
    v
}

fn invalid(what: &str) -> std::io::Error {
    std::io::Error::other(format!("pin_assetgen: could not encode {what}"))
}

fn render(meta: &TableMeta, p: &Planned) -> std::io::Result<Vec<u8>> {
    let rate = sounds::KIT_SAMPLE_RATE;
    let wav = |s: Option<Vec<f32>>| s.and_then(|s| sounds::encode_wav(&s, rate));
    let bytes = match p.source {
        Source::Sfx(id) => wav(sounds::render_sfx(id, rate)),
        Source::Music => wav(sounds::render_music(meta, rate)),
        Source::Motif(id) => wav(sounds::render_motif(id, rate)),
        Source::FontSheet(f) => art::font_sheet(f).to_png(),
        Source::BackglassBase => art::translite_base(meta).to_png(),
        Source::BackglassLogo => art::translite_logo(meta).to_png(),
        Source::BackglassLogoMask => art::translite_logo_mask(meta).to_png(),
        Source::Playfield => art::playfield(meta).to_png(),
        Source::Insert(text) => art::insert_label(meta, text).to_png(),
    };
    bytes.ok_or_else(|| invalid(&p.id))
}

/// Generates the kit for `meta` into `out_dir` (created if missing; existing
/// kit files are overwritten, other files are left alone), writes
/// `<out_dir>/assets.toml` and returns the manifest.
pub fn generate_kit(meta: &TableMeta, out_dir: &Path) -> std::io::Result<KitManifest> {
    std::fs::create_dir_all(out_dir)?;
    let mut manifest = KitManifest::default();
    for p in plan() {
        let bytes = render(meta, &p)?;
        let mut target = out_dir.to_path_buf();
        for part in p.path.split('/') {
            target.push(part);
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, &bytes)?;
        manifest.entries.push(KitEntry {
            id: p.id,
            path: p.path,
            kind: p.kind,
            sha256_or_hash: sha256_hex(&bytes),
        });
    }
    std::fs::write(out_dir.join(MANIFEST_FILE), manifest.to_toml(meta))?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> TableMeta {
        TableMeta {
            title: "Apex \"Circuit\"\n\u{1F680}".into(),
            palette: vec![[0x0b, 0x1d, 0x3a], [0xf2, 0xa9, 0x00]],
            seed: u64::MAX,
        }
    }

    #[test]
    fn plan_is_unique_and_valid() {
        let ids = stable_ids();
        let set: std::collections::BTreeSet<_> = ids.iter().collect();
        assert_eq!(set.len(), ids.len());
        for p in plan() {
            assert!(is_bundle_path(&p.path), "{}", p.path);
        }
        for id in sounds::SFX_IDS.iter().chain(sounds::MOTIF_IDS) {
            assert!(ids.iter().any(|i| i == id));
        }
    }

    #[test]
    fn meta_seed_depends_on_every_field() {
        let m = meta();
        let s = meta_seed(&m);
        assert_eq!(s, meta_seed(&m.clone()));
        let mut t = m.clone();
        t.title.push('x');
        assert_ne!(meta_seed(&t), s);
        let mut t = m.clone();
        t.palette[1][2] ^= 1;
        assert_ne!(meta_seed(&t), s);
        let mut t = m.clone();
        t.seed = 0;
        assert_ne!(meta_seed(&t), s);
        // Length prefixes keep "ab"+[] apart from "a"+... style collisions.
        let a = TableMeta {
            title: "ab".into(),
            palette: vec![],
            seed: 0,
        };
        let b = TableMeta {
            title: "a".into(),
            palette: vec![],
            seed: 0,
        };
        assert_ne!(meta_seed(&a), meta_seed(&b));
    }

    #[test]
    fn manifest_round_trip_and_errors() {
        let m = KitManifest {
            entries: vec![KitEntry {
                id: "sfx_x".into(),
                path: "audio/x.wav".into(),
                kind: AssetKind::Sound,
                sha256_or_hash: sha256_hex(b"x"),
            }],
        };
        let text = m.to_toml(&meta());
        assert_eq!(KitManifest::from_toml(&text).unwrap(), m);
        assert!(text.contains("seed = \"18446744073709551615\""));
        assert!(KitManifest::from_toml("").is_err());
        assert!(KitManifest::from_toml(&text.replace("schema_version = 1", "schema_version = 2")).is_err());
        assert!(KitManifest::from_toml(&text.replace("audio/x.wav", "../x.wav")).is_err());
        assert!(KitManifest::from_toml(&text.replace("audio/x.wav", "/x.wav")).is_err());
        assert!(KitManifest::from_toml(&text.replace("audio/x.wav", "a\\\\x.wav")).is_err());
        let dup = format!(
            "{text}\n[[entries]]\nid = \"sfx_x\"\npath = \"b.wav\"\nkind = \"sound\"\ngenerator_version = 1\nsha256_or_hash = \"\"\n"
        );
        assert!(KitManifest::from_toml(&dup).unwrap_err().contains("duplicate"));
        assert!(KitManifest::from_toml(&text.replace("kind = \"sound\"", "kind = \"scene\"")).is_err());
    }

    #[test]
    fn sound_defs_cover_sounds_only() {
        use pin_core::schema::table::SoundBus;
        let m = KitManifest {
            entries: plan()
                .into_iter()
                .map(|p| KitEntry {
                    id: p.id,
                    path: p.path,
                    kind: p.kind,
                    sha256_or_hash: String::new(),
                })
                .collect(),
        };
        let defs = m.sound_defs();
        assert_eq!(defs.len(), sounds::SFX_IDS.len() + 1 + sounds::MOTIF_IDS.len());
        assert_eq!(defs.iter().filter(|d| d.bus == SoundBus::Music).count(), 1);
        assert_eq!(
            defs.iter().filter(|d| d.bus == SoundBus::Callout).count(),
            sounds::MOTIF_IDS.len()
        );
        assert_eq!(m.get("img_playfield").unwrap().path, "art/playfield.png");
        assert!(m.get("nope").is_none());
    }

    #[test]
    fn bundle_paths() {
        assert!(is_bundle_path("a/b.png"));
        for bad in ["", "/a", "a//b", "a/./b", "a/../b", "c:x", "a\\b", "a/"] {
            assert!(!is_bundle_path(bad), "{bad}");
        }
    }
}
