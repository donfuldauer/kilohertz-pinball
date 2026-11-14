//! `[dof]` name resolution and the bridge's JSON map file.

use pin_core::schema::table::{DofElementType, LampKind, TableManifest};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// `format` value of the map file.
pub const MAP_FORMAT: &str = "kilohertz-dof-map";
/// `version` value of the map file.
pub const MAP_VERSION: u32 = 1;

/// Highest switch, coil and lamp telemetry id (`TelemetryState` holds 256 of each).
const MAX_ELEMENT_ID: u16 = 255;
/// Highest GI telemetry id (`GI_COUNT` = 32 strings).
const MAX_GI_ID: u16 = 31;

/// The telemetry element a DOF mapping reads (telemetry kinds 0–3).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum DofSourceKind {
    /// Switch, telemetry kind 0.
    Switch,
    /// Coil, telemetry kind 1.
    Coil,
    /// Lamp, telemetry kind 2 (lamps whose `kind` is not `gi`).
    Lamp,
    /// GI string, telemetry kind 3 (lamps with `kind = "gi"`; id = lamp number).
    Gi,
}

impl DofSourceKind {
    /// The map-file spelling: `switch`, `coil`, `lamp` or `gi`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Switch => "switch",
            Self::Coil => "coil",
            Self::Lamp => "lamp",
            Self::Gi => "gi",
        }
    }

    /// The telemetry kind byte (`TELEMETRY_PROTOCOL.md` §2.3).
    pub fn telemetry_kind(self) -> u8 {
        match self {
            Self::Switch => 0,
            Self::Coil => 1,
            Self::Lamp => 2,
            Self::Gi => 3,
        }
    }
}

/// One resolved `[dof].map` entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DofMapEntry {
    /// Telemetry element kind.
    pub source: DofSourceKind,
    /// Telemetry id (switch/coil/lamp number, or GI string number).
    pub id: u16,
    /// The element's name in `table.toml`.
    pub name: String,
    /// DOF table-element type.
    pub element_type: DofElementType,
    /// DOF table-element number.
    pub number: u16,
}

/// The resolved id → DOF element map handed to the bridge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedDofMap {
    /// ROM name DOF configs are keyed by (`[dof].rom_name`).
    pub rom_name: String,
    /// Table display name (`[meta].name`).
    pub table_name: String,
    /// Entries in `[dof].map` order.
    pub entries: Vec<DofMapEntry>,
}

/// Errors from resolving `[dof]` or launching the bridge.
#[derive(Debug)]
pub enum DofError {
    /// A `[dof].map` entry does not name exactly one of coil, switch, lamp.
    InvalidEntry {
        /// Entry path, e.g. `dof.map[2]`.
        field: String,
        /// Number of sources named.
        sources: usize,
    },
    /// A mapped name is not defined in the table.
    UnknownName {
        /// Entry path, e.g. `dof.map[2].coil`.
        field: String,
        /// The name.
        name: String,
    },
    /// A mapped element's number does not fit the telemetry id range.
    IdOutOfRange {
        /// Entry path.
        field: String,
        /// The element number.
        number: u16,
        /// Highest allowed number.
        max: u16,
    },
    /// Writing the map file failed.
    MapFile(io::Error),
    /// Starting the bridge process failed.
    Spawn(io::Error),
}

impl fmt::Display for DofError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEntry { field, sources } => write!(
                f,
                "{field}: must name exactly one of coil, switch, lamp (found {sources})"
            ),
            Self::UnknownName { field, name } => write!(f, "{field}: unknown name '{name}'"),
            Self::IdOutOfRange { field, number, max } => write!(
                f,
                "{field}: number {number} is above {max}, the highest telemetry id"
            ),
            Self::MapFile(e) => write!(f, "cannot write the DOF map file: {e}"),
            Self::Spawn(e) => write!(f, "cannot start the DOF bridge: {e}"),
        }
    }
}

impl std::error::Error for DofError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::MapFile(e) | Self::Spawn(e) => Some(e),
            _ => None,
        }
    }
}

fn element_char(t: DofElementType) -> char {
    match t {
        DofElementType::S => 'S',
        DofElementType::W => 'W',
        DofElementType::L => 'L',
        DofElementType::G => 'G',
        DofElementType::E => 'E',
    }
}

fn check_range(field: &str, number: u16, max: u16) -> Result<u16, DofError> {
    if number > max {
        Err(DofError::IdOutOfRange {
            field: field.to_owned(),
            number,
            max,
        })
    } else {
        Ok(number)
    }
}

/// Appends `s` as a JSON string literal.
fn push_json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

static MAP_FILE_SEQ: AtomicU64 = AtomicU64::new(0);

/// An unpredictable 64-bit value: SipHash keyed by a fresh OS-seeded
/// [`RandomState`](std::collections::hash_map::RandomState), over the
/// sequence number and the current time (no extra dependency; not
/// cryptographic, but not guessable by another local user in advance).
fn random_suffix(seq: u64) -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(seq);
    if let Ok(d) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        h.write_u128(d.as_nanos());
    }
    h.finish()
}

impl ResolvedDofMap {
    /// Resolves the table's `[dof]` section: `Ok(None)` without one.
    ///
    /// Coil and switch names resolve to their numbers; a lamp resolves to
    /// [`DofSourceKind::Gi`] with its number as the GI string when its kind is
    /// `gi`, otherwise to [`DofSourceKind::Lamp`]. Numbers must fit the
    /// telemetry ids (≤ 255; GI ≤ 31).
    pub fn resolve(table: &TableManifest) -> Result<Option<Self>, DofError> {
        let Some(dof) = &table.dof else {
            return Ok(None);
        };
        let mut entries = Vec::with_capacity(dof.map.len());
        for (i, m) in dof.map.iter().enumerate() {
            let sources = [&m.coil, &m.switch, &m.lamp]
                .iter()
                .filter(|s| s.is_some())
                .count();
            let (source, id, name) = match (&m.coil, &m.switch, &m.lamp) {
                (Some(c), None, None) => {
                    let field = format!("dof.map[{i}].coil");
                    let def = table.coils.iter().find(|d| &d.id == c).ok_or_else(|| {
                        DofError::UnknownName {
                            field: field.clone(),
                            name: c.clone(),
                        }
                    })?;
                    let id = check_range(&field, def.number, MAX_ELEMENT_ID)?;
                    (DofSourceKind::Coil, id, c)
                }
                (None, Some(s), None) => {
                    let field = format!("dof.map[{i}].switch");
                    let def = table.switches.iter().find(|d| &d.id == s).ok_or_else(|| {
                        DofError::UnknownName {
                            field: field.clone(),
                            name: s.clone(),
                        }
                    })?;
                    let id = check_range(&field, def.number, MAX_ELEMENT_ID)?;
                    (DofSourceKind::Switch, id, s)
                }
                (None, None, Some(l)) => {
                    let field = format!("dof.map[{i}].lamp");
                    let def = table.lamps.iter().find(|d| &d.id == l).ok_or_else(|| {
                        DofError::UnknownName {
                            field: field.clone(),
                            name: l.clone(),
                        }
                    })?;
                    if def.kind == LampKind::Gi {
                        (
                            DofSourceKind::Gi,
                            check_range(&field, def.number, MAX_GI_ID)?,
                            l,
                        )
                    } else {
                        (
                            DofSourceKind::Lamp,
                            check_range(&field, def.number, MAX_ELEMENT_ID)?,
                            l,
                        )
                    }
                }
                _ => {
                    return Err(DofError::InvalidEntry {
                        field: format!("dof.map[{i}]"),
                        sources,
                    });
                }
            };
            entries.push(DofMapEntry {
                source,
                id,
                name: name.clone(),
                element_type: m.element_type,
                number: m.number,
            });
        }
        Ok(Some(Self {
            rom_name: dof.rom_name.clone(),
            table_name: table.meta.name.clone(),
            entries,
        }))
    }

    /// The map file text (JSON, one entry per line, deterministic).
    pub fn to_json(&self) -> String {
        let mut out = String::with_capacity(128 + 96 * self.entries.len());
        out.push_str("{\n  \"format\": ");
        push_json_string(&mut out, MAP_FORMAT);
        out.push_str(&format!(
            ",\n  \"version\": {MAP_VERSION},\n  \"rom_name\": "
        ));
        push_json_string(&mut out, &self.rom_name);
        out.push_str(",\n  \"table_name\": ");
        push_json_string(&mut out, &self.table_name);
        out.push_str(",\n  \"entries\": [");
        for (i, e) in self.entries.iter().enumerate() {
            out.push_str(if i == 0 { "\n    " } else { ",\n    " });
            out.push_str("{ \"source\": ");
            push_json_string(&mut out, e.source.as_str());
            out.push_str(&format!(", \"id\": {}, \"name\": ", e.id));
            push_json_string(&mut out, &e.name);
            out.push_str(&format!(
                ", \"type\": \"{}\", \"number\": {} }}",
                element_char(e.element_type),
                e.number
            ));
        }
        if !self.entries.is_empty() {
            out.push_str("\n  ");
        }
        out.push_str("]\n}\n");
        out
    }

    /// Writes [`to_json`](Self::to_json) to a new file
    /// `kilohertz-dof-<pid>-<n>-<random>.json` in `dir` and returns its path.
    ///
    /// The file is created with create-new semantics (`O_EXCL` /
    /// `CREATE_NEW`): an existing file or symlink at that path is never
    /// followed or overwritten, and the 64-bit random suffix (from the OS-seeded
    /// `RandomState` keys) makes the name unpredictable; on a collision a new
    /// name is tried (up to 16 times). The bridge's reaper deletes the file
    /// once the bridge has stopped; if the engine itself exits or crashes
    /// before that, the file (a few hundred bytes of non-secret table data)
    /// stays in `dir` until the OS cleans its temp dir.
    pub fn write_file(&self, dir: &Path) -> Result<PathBuf, DofError> {
        use std::io::Write as _;
        const ATTEMPTS: usize = 16;
        let json = self.to_json();
        let mut last_err = None;
        for _ in 0..ATTEMPTS {
            let n = MAP_FILE_SEQ.fetch_add(1, Ordering::Relaxed);
            let path = dir.join(format!(
                "kilohertz-dof-{}-{n}-{:016x}.json",
                std::process::id(),
                random_suffix(n)
            ));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut f) => {
                    if let Err(e) = f.write_all(json.as_bytes()).and_then(|()| f.sync_all()) {
                        drop(f);
                        let _ = std::fs::remove_file(&path);
                        return Err(DofError::MapFile(e));
                    }
                    return Ok(path);
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last_err = Some(e),
                Err(e) => return Err(DofError::MapFile(e)),
            }
        }
        Err(DofError::MapFile(last_err.unwrap_or_else(|| {
            io::Error::new(io::ErrorKind::AlreadyExists, "no free map file name")
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_escapes_strings() {
        let mut s = String::new();
        push_json_string(&mut s, "a\"b\\c\nd\re\tf\u{1}g é");
        assert_eq!(s, "\"a\\\"b\\\\c\\nd\\re\\tf\\u0001g é\"");
    }

    #[test]
    fn empty_map_json() {
        let m = ResolvedDofMap {
            rom_name: "r".into(),
            table_name: "t".into(),
            entries: Vec::new(),
        };
        assert!(m.to_json().contains("\"entries\": []"));
    }

    #[test]
    fn source_kinds() {
        for (k, s, t) in [
            (DofSourceKind::Switch, "switch", 0),
            (DofSourceKind::Coil, "coil", 1),
            (DofSourceKind::Lamp, "lamp", 2),
            (DofSourceKind::Gi, "gi", 3),
        ] {
            assert_eq!(k.as_str(), s);
            assert_eq!(k.telemetry_kind(), t);
        }
    }
}
