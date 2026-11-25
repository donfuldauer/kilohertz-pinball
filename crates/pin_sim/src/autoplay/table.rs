//! What the bot and the soak monitor know about a table, read from its
//! bundle with the same loaders the simulation uses ([`load_table`] and the
//! optional `layout.toml`).
//!
//! Everything here is setup-time data in SI units (D17): flipper geometry
//! and buttons for the bot, switch kinds and positions for coverage and the
//! stuck-ball exemptions, shots and modes for coverage, and the named
//! features (switches, mechs, magnets, dispensers, flipper pivots, layout
//! walls and posts) a finding's subject is named after.

use glam::{Vec2, Vec3};
use pin_assets::{IdKind, Vfs, load_table};
use pin_core::flipper::{FlipperShape, FlipperStagingMode};
use pin_core::fmath::min_f32;
use pin_core::ids::{FlipperId, SwitchId};
use pin_core::input::CabinetInput;
use pin_core::schema::LayoutManifest;
use pin_core::schema::table::{MechKind, SwitchKind};
use std::path::Path;

use crate::SimError;

/// Prefix of the `[persist]` key a script increments each time a mode
/// starts: a mode `id` is observed started when the value of key
/// `mode:<id>` increases (DECISIONS.md "2.9 — mode-start signal").
pub const MODE_PERSIST_PREFIX: &str = "mode:";

/// Default bat length when a flipper's geometry is unknown, m.
pub const DEFAULT_BAT_LENGTH_M: f32 = 0.076;

/// One flipper as the bot sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct FlipperInfo {
    /// Flipper id (the simulation's).
    pub id: FlipperId,
    /// `[[flippers]]` id.
    pub name: String,
    /// Collision layer index.
    pub layer: u8,
    /// Pivot-to-tip length, m.
    pub length_m: f32,
    /// Largest bat radius (base), m.
    pub radius_m: f32,
    /// Signed sweep, rad (+ = counter-clockwise about the surface normal).
    pub sweep_rad: f32,
    /// The cabinet button that drives it (`None` for analog or automatic
    /// staging, which the bot does not press).
    pub button: Option<CabinetInput>,
}

/// A declared `[[switches]]` entry.
#[derive(Clone, Debug, PartialEq)]
pub struct SwitchInfo {
    /// Name.
    pub name: String,
    /// Switch id (= declared number).
    pub id: SwitchId,
    /// Kind.
    pub kind: SwitchKind,
    /// Zone centre on the playfield, m (when declared).
    pub position_m: Option<[f32; 3]>,
    /// Zone radius, m (when declared).
    pub radius_m: Option<f32>,
    /// Whether the switch number is also a cabinet input (pressed by the
    /// bot, so excluded from coverage).
    pub cabinet: bool,
}

/// A `[[mechs]]` entry the stuck-ball exemptions need.
#[derive(Clone, Debug, PartialEq)]
pub struct MechInfo {
    /// Name.
    pub name: String,
    /// Kind.
    pub kind: MechKind,
    /// Its switch.
    pub switch: Option<SwitchId>,
    /// Position, m.
    pub position_m: Option<[f32; 3]>,
}

/// A `[[shots]]` entry.
#[derive(Clone, Debug, PartialEq)]
pub struct ShotInfo {
    /// Name.
    pub name: String,
    /// Switches in order.
    pub sequence: Vec<SwitchId>,
    /// Window from the first to the last switch, ticks (ms).
    pub window_ticks: u64,
}

/// A `[[modes]]` entry.
#[derive(Clone, Debug, PartialEq)]
pub struct ModeInfo {
    /// Name.
    pub name: String,
    /// Its start scene's name.
    pub start_scene: String,
    /// The `mode:<id>` persist key index, when the table declares one.
    pub persist_key: Option<u16>,
}

/// The shape of a named feature.
#[derive(Clone, Debug, PartialEq)]
pub enum FeatureShape {
    /// A point on the playfield, m.
    Point(Vec2),
    /// A polyline (wall), m.
    Polyline(Vec<Vec2>),
}

/// A named table feature a finding can point at.
#[derive(Clone, Debug, PartialEq)]
pub struct Feature {
    /// Its name (`trap_pocket`, `walls[2]`, `flipper lower_left`, ...).
    pub name: String,
    /// Where it is.
    pub shape: FeatureShape,
}

impl Feature {
    /// Distance from `p` (table plane, m) to the feature.
    #[must_use]
    pub fn distance(&self, p: Vec2) -> f32 {
        match &self.shape {
            FeatureShape::Point(c) => p.distance(*c),
            FeatureShape::Polyline(pts) => {
                let mut best = f32::INFINITY;
                for w in pts.windows(2) {
                    best = min_f32(best, segment_distance(p, w[0], w[1]));
                }
                if pts.len() == 1 {
                    best = p.distance(pts[0]);
                }
                best
            }
        }
    }
}

/// Distance from `p` to the segment `a`–`b`.
#[must_use]
pub fn segment_distance(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_squared();
    let t = if len2 > 0.0 {
        ((p - a).dot(ab) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.distance(a + ab * t)
}

/// Everything the autoplay needs to know about one table.
#[derive(Clone, Debug, PartialEq)]
pub struct TableInfo {
    /// `meta.id`.
    pub table_id: String,
    /// Flippers, by id.
    pub flippers: Vec<FlipperInfo>,
    /// Declared switches, in declaration order.
    pub switches: Vec<SwitchInfo>,
    /// Mechs.
    pub mechs: Vec<MechInfo>,
    /// Shots.
    pub shots: Vec<ShotInfo>,
    /// Modes.
    pub modes: Vec<ModeInfo>,
    /// Named features for subjects.
    pub features: Vec<Feature>,
}

fn sim_err(e: impl core::fmt::Display) -> SimError {
    SimError::Build(format!("autoplay table info: {e}"))
}

impl TableInfo {
    /// Reads the bundle at `bundle` (folder or zip) with the simulation's
    /// loaders.
    ///
    /// # Errors
    /// [`SimError::Asset`] when the bundle does not load or validate,
    /// [`SimError::Build`] for an unreadable `layout.toml`.
    pub fn load(bundle: &Path) -> Result<TableInfo, SimError> {
        let vfs = Vfs::open(bundle)?;
        let table = load_table(&vfs)?;
        let m = &table.manifest;
        let cab_numbers: Vec<u16> = CabinetInput::ALL
            .iter()
            .map(|&c| m.cabinet_switches.number(c))
            .collect();

        let flippers = table
            .flippers
            .iter()
            .zip(&m.flippers)
            .map(|(cfg, def)| {
                let (length_mm, radius_mm) = match cfg.shape {
                    FlipperShape::StraightCapsule {
                        length_mm,
                        base_radius_mm,
                        ..
                    } => (length_mm, base_radius_mm),
                    FlipperShape::CurvedBanana {
                        length_mm,
                        tube_radius_mm,
                        ..
                    } => (length_mm, tube_radius_mm),
                };
                let button = match cfg.staging {
                    FlipperStagingMode::DedicatedButton(c)
                    | FlipperStagingMode::ParallelLinked(c) => Some(c),
                    _ => None,
                };
                FlipperInfo {
                    id: cfg.id,
                    name: def.id.clone(),
                    layer: cfg.collision_layer,
                    length_m: length_mm * 1.0e-3,
                    radius_m: radius_mm * 1.0e-3,
                    sweep_rad: cfg.sweep_angle_deg.to_radians(),
                    button,
                }
            })
            .collect();

        let switches = table
            .switches
            .iter()
            .map(|s| SwitchInfo {
                name: s.name.clone(),
                id: s.id,
                kind: s.kind,
                position_m: s.position_m,
                radius_m: s.radius_m,
                cabinet: cab_numbers.contains(&s.id.0),
            })
            .collect::<Vec<_>>();

        let mechs = table
            .mechs
            .iter()
            .map(|x| MechInfo {
                name: x.name.clone(),
                kind: x.kind,
                switch: x.switch,
                position_m: x.position_m,
            })
            .collect::<Vec<_>>();

        let shots = table
            .shots
            .iter()
            .map(|s| ShotInfo {
                name: s.name.clone(),
                sequence: s.sequence.clone(),
                window_ticks: ms_ticks(s.window_s),
            })
            .collect();

        let modes = m
            .modes
            .iter()
            .map(|d| ModeInfo {
                name: d.id.clone(),
                start_scene: d.start_scene.clone(),
                persist_key: table.ids.get(
                    IdKind::PersistKey,
                    &format!("{MODE_PERSIST_PREFIX}{}", d.id),
                ),
            })
            .collect();

        let mut features = Vec::new();
        let point = |p: [f32; 3]| FeatureShape::Point(Vec2::new(p[0], p[1]));
        for s in &switches {
            if let Some(p) = s.position_m {
                features.push(Feature {
                    name: s.name.clone(),
                    shape: point(p),
                });
            }
        }
        for x in &mechs {
            if let Some(p) = x.position_m {
                features.push(Feature {
                    name: x.name.clone(),
                    shape: point(p),
                });
            }
        }
        for g in &table.magnets {
            features.push(Feature {
                name: g.name.clone(),
                shape: point(g.position_m),
            });
        }
        for d in &table.dispensers {
            features.push(Feature {
                name: d.name.clone(),
                shape: point(d.exit_position_m),
            });
        }
        for (cfg, def) in table.flippers.iter().zip(&m.flippers) {
            let p = cfg.base_position_mm;
            features.push(Feature {
                name: format!("flipper {}", def.id),
                shape: point([p[0] * 1.0e-3, p[1] * 1.0e-3, p[2] * 1.0e-3]),
            });
        }
        if vfs.exists("layout.toml") {
            let text = String::from_utf8(vfs.read("layout.toml")?).map_err(sim_err)?;
            let layout = LayoutManifest::from_toml_str(&text).map_err(sim_err)?;
            for (i, w) in layout.walls.iter().enumerate() {
                let mut pts: Vec<Vec2> = w
                    .points
                    .iter()
                    .map(|p| Vec2::new(p[0], p[1]) * 1.0e-3)
                    .collect();
                if w.closed
                    && let Some(&first) = pts.first()
                {
                    pts.push(first);
                }
                features.push(Feature {
                    name: format!("walls[{i}]"),
                    shape: FeatureShape::Polyline(pts),
                });
            }
            for (i, p) in layout.posts.iter().enumerate() {
                features.push(Feature {
                    name: format!("posts[{i}]"),
                    shape: FeatureShape::Point(Vec2::new(p.position[0], p.position[1]) * 1.0e-3),
                });
            }
        }

        Ok(TableInfo {
            table_id: m.meta.id.clone(),
            flippers,
            switches,
            mechs,
            shots,
            modes,
            features,
        })
    }

    /// The declared switches that count for coverage (not cabinet inputs),
    /// in declaration order.
    pub fn coverage_switches(&self) -> impl Iterator<Item = &SwitchInfo> {
        self.switches.iter().filter(|s| !s.cabinet)
    }

    /// The name of the switch with id `id`.
    #[must_use]
    pub fn switch_name(&self, id: SwitchId) -> Option<&str> {
        self.switches
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.name.as_str())
    }

    /// The feature nearest to `p` (table frame, m; the plane distance) and
    /// its distance; `None` when the table has no positioned feature.
    #[must_use]
    pub fn nearest_feature(&self, p: Vec3) -> Option<(&str, f32)> {
        let q = Vec2::new(p.x, p.y);
        let mut best: Option<(&str, f32)> = None;
        for f in &self.features {
            let d = f.distance(q);
            if best.is_none_or(|(_, b)| d < b) {
                best = Some((f.name.as_str(), d));
            }
        }
        best
    }

    /// A subject for a position: `<nearest feature> @ (x, y, z) mm`.
    #[must_use]
    pub fn subject_at(&self, p: Vec3) -> String {
        let name = self.nearest_feature(p).map_or("playfield", |(n, _)| n);
        format!("{name} @ {}", fmt_mm(p))
    }
}

/// `(x, y, z) mm`, whole millimetres (non-finite components print as such).
#[must_use]
pub fn fmt_mm(p: Vec3) -> String {
    let c = |v: f32| {
        if v.is_finite() {
            format!("{:.0}", v * 1.0e3)
        } else {
            format!("{v}")
        }
    };
    format!("({}, {}, {}) mm", c(p.x), c(p.y), c(p.z))
}

/// Seconds to whole ticks (ms), rounded; non-finite or negative is 0.
#[must_use]
pub fn ms_ticks(s: f32) -> u64 {
    if s.is_finite() && s > 0.0 {
        (f64::from(s) * 1000.0).round() as u64
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_distance_cases() {
        let a = Vec2::new(0.0, 0.0);
        let b = Vec2::new(1.0, 0.0);
        assert!((segment_distance(Vec2::new(0.5, 2.0), a, b) - 2.0).abs() < 1e-6);
        assert!((segment_distance(Vec2::new(-3.0, 4.0), a, b) - 5.0).abs() < 1e-6);
        assert!((segment_distance(Vec2::new(2.0, 0.0), a, a) - 2.0).abs() < 1e-6);
    }

    #[test]
    fn feature_distance_polyline_and_point() {
        let f = Feature {
            name: "w".into(),
            shape: FeatureShape::Polyline(vec![Vec2::ZERO, Vec2::X, Vec2::new(1.0, 1.0)]),
        };
        assert!((f.distance(Vec2::new(2.0, 0.5)) - 1.0).abs() < 1e-6);
        let one = Feature {
            name: "p".into(),
            shape: FeatureShape::Polyline(vec![Vec2::ZERO]),
        };
        assert!((one.distance(Vec2::new(0.0, 3.0)) - 3.0).abs() < 1e-6);
    }

    #[test]
    fn ms_ticks_and_fmt() {
        assert_eq!(ms_ticks(1.5), 1500);
        assert_eq!(ms_ticks(f32::NAN), 0);
        assert_eq!(ms_ticks(-1.0), 0);
        assert_eq!(fmt_mm(Vec3::new(0.1, 0.2, f32::NAN)), "(100, 200, NaN) mm");
    }
}
