//! The display scene format (GPU-free): parsing, placeholder scenes, the
//! scene library and the scene player.
//!
//! A scene is a TOML file (`[[scenes]] file` in `table.toml`) of layers drawn
//! back to front on the score display, DMD or LCD alike. Positions and sizes
//! are fractions of the display, so one scene fits every display size. See
//! DECISIONS "5.3 — Display scene format" for the full key list.
//!
//! ```toml
//! schema_version = 1
//! duration_ms = 3000          # 0 = until another scene replaces it
//! repeat = false              # loop the timeline (then it never ends)
//! background = "#000000"
//!
//! [[layers]]
//! kind = "text"               # text | rect | image | score
//! text = "JACKPOT {score}"
//! x = 0.5                     # anchor, fraction of the width
//! y = 0.5                     # anchor, fraction of the height
//! align = "center"            # left | center | right
//! valign = "middle"           # top | middle | bottom
//! size = 0.5                  # glyph cell height, fraction of the height
//! color = "#ffd000"
//! blink_ms = 120              # on/off half period
//! ```

use pin_core::SceneId;
use pin_core::schema::table::SceneDef;
use pin_core::script::HostCommandQueue;
use serde::Deserialize;

use core::fmt;
use std::collections::BTreeMap;

/// Largest scene file accepted, bytes.
pub const MAX_SCENE_BYTES: usize = 64 * 1024;
/// Most layers in one scene.
pub const MAX_SCENE_LAYERS: usize = 64;
/// Longest text of one layer, characters.
pub const MAX_TEXT_CHARS: usize = 256;
/// Longest time value accepted (one hour), ms.
pub const MAX_TIME_MS: u64 = 3_600_000;
/// Coordinates must lie within `±MAX_COORD` (display fractions).
pub const MAX_COORD: f32 = 4.0;

/// Name of the built-in score-view scene.
pub const SCORE_SCENE: &str = "scene_score";

/// Placeholder scenes `(id, TOML)`: attract, score view, game over, the A.1
/// example's `scene_powerball_mania`, ball saved and jackpot (4.6 hand-off,
/// D32: `display_scenes` commands always have content).
pub const PLACEHOLDER_SCENES: &[(&str, &str)] = &[
    ("scene_attract", include_str!("scenes/scene_attract.toml")),
    (SCORE_SCENE, include_str!("scenes/scene_score.toml")),
    ("scene_game_over", include_str!("scenes/scene_game_over.toml")),
    (
        "scene_powerball_mania",
        include_str!("scenes/scene_powerball_mania.toml"),
    ),
    ("scene_ball_saved", include_str!("scenes/scene_ball_saved.toml")),
    ("scene_jackpot", include_str!("scenes/scene_jackpot.toml")),
];

/// The placeholder scene `id`, parsed.
pub fn placeholder_scene(id: &str) -> Option<Scene> {
    PLACEHOLDER_SCENES
        .iter()
        .find(|(name, _)| *name == id)
        .map(|(_, text)| Scene::from_toml_str(text).expect("placeholder scenes are valid"))
}

/// Why a scene could not be loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SceneError {
    /// The file is larger than [`MAX_SCENE_BYTES`].
    TooLarge(usize),
    /// The file is not UTF-8.
    NotUtf8,
    /// TOML syntax or an unknown / mistyped key.
    Parse(String),
    /// `schema_version` is not 1.
    SchemaVersion(u32),
    /// A value is out of range or missing (layer index, reason).
    Invalid(String),
    /// A scene file could not be read and there is no placeholder of that id.
    Read {
        /// Scene id.
        id: String,
        /// Bundle path.
        file: String,
        /// Reader's message.
        reason: String,
    },
    /// A scene file failed to load.
    InFile {
        /// Scene id.
        id: String,
        /// Bundle path.
        file: String,
        /// What was wrong.
        error: Box<SceneError>,
    },
}

impl fmt::Display for SceneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SceneError::TooLarge(n) => write!(f, "scene file of {n} bytes exceeds {MAX_SCENE_BYTES}"),
            SceneError::NotUtf8 => write!(f, "scene file is not UTF-8"),
            SceneError::Parse(e) => write!(f, "scene TOML: {e}"),
            SceneError::SchemaVersion(v) => write!(f, "scene schema_version {v} (expected 1)"),
            SceneError::Invalid(e) => write!(f, "invalid scene: {e}"),
            SceneError::Read { id, file, reason } => {
                write!(f, "scene {id}: cannot read {file}: {reason}")
            }
            SceneError::InFile { id, file, error } => write!(f, "scene {id} ({file}): {error}"),
        }
    }
}

impl std::error::Error for SceneError {}

/// Horizontal alignment of a layer at its anchor.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    /// Anchor at the left edge.
    Left,
    /// Anchor at the centre.
    Center,
    /// Anchor at the right edge.
    Right,
}

/// Vertical alignment of a layer at its anchor.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VAlign {
    /// Anchor at the top edge.
    Top,
    /// Anchor at the middle.
    Middle,
    /// Anchor at the bottom edge.
    Bottom,
}

impl Align {
    /// Left edge of a box of width `w` anchored at `x`.
    pub fn left_of(self, x: f32, w: f32) -> f32 {
        match self {
            Align::Left => x,
            Align::Center => x - w / 2.0,
            Align::Right => x - w,
        }
    }
}

impl VAlign {
    /// Top edge of a box of height `h` anchored at `y`.
    pub fn top_of(self, y: f32, h: f32) -> f32 {
        match self {
            VAlign::Top => y,
            VAlign::Middle => y - h / 2.0,
            VAlign::Bottom => y - h,
        }
    }
}

/// Font of a text layer on the DMD (the LCD always uses the vector font).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub enum FontChoice {
    /// 8×16 when it fits the size, else 5×7.
    #[default]
    #[serde(rename = "auto")]
    Auto,
    /// The 5×7 font.
    #[serde(rename = "5x7")]
    Dmd5x7,
    /// The 8×16 font.
    #[serde(rename = "8x16")]
    Dmd8x16,
}

/// What a layer draws.
#[derive(Clone, Debug, PartialEq)]
pub enum LayerKind {
    /// One line of text (placeholders: see [`super::expand_text`]).
    Text(String),
    /// A filled rectangle.
    Rect,
    /// An image (bundle path; supplied through [`super::ImageSet`]).
    Image(String),
    /// The score view ([`super::score_view_layout`]) inside the layer box.
    Score,
}

/// One validated scene layer. Coordinates are display fractions.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneLayer {
    /// What the layer draws.
    pub kind: LayerKind,
    /// Anchor x.
    pub x: f32,
    /// Anchor y.
    pub y: f32,
    /// Box width (rect, image, score).
    pub w: f32,
    /// Box height (rect, score; image: 0 keeps the image's aspect).
    pub h: f32,
    /// Horizontal alignment.
    pub align: Align,
    /// Vertical alignment.
    pub valign: VAlign,
    /// Text glyph cell height (fraction of the display height).
    pub size: f32,
    /// DMD font.
    pub font: FontChoice,
    /// Colour (DMD mono modes use its brightest channel).
    pub color: [u8; 3],
    /// Opacity, `0..=1`.
    pub alpha: f32,
    /// First visible ms.
    pub start_ms: u64,
    /// End of visibility, ms (0 = end of the scene).
    pub end_ms: u64,
    /// Blink half period, ms (0 = steady).
    pub blink_ms: u64,
    /// Fade-in after `start_ms`, ms.
    pub fade_in_ms: u64,
    /// Fade-out before the end, ms.
    pub fade_out_ms: u64,
    /// Start of a linear move to `(x, y)`.
    pub from: Option<[f32; 2]>,
    /// Duration of the move from `start_ms`, ms.
    pub move_ms: u64,
}

/// A layer's animated state at one moment.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct LayerState {
    /// Anchor x.
    pub x: f32,
    /// Anchor y.
    pub y: f32,
    /// Opacity in `(0, 1]`.
    pub opacity: f32,
}

impl SceneLayer {
    /// The layer at `elapsed` ms into a scene of `scene_duration_ms` (0 =
    /// open-ended), or `None` while invisible (before `start_ms`, after the
    /// end, in a blink-off phase, or fully transparent).
    ///
    /// Blink: visible while `⌊(t − start) / blink⌋` is even. Fades scale the
    /// opacity linearly. A move interpolates linearly from `from` to `(x, y)`
    /// over `move_ms`.
    pub fn state_at(&self, elapsed: u64, scene_duration_ms: u64) -> Option<LayerState> {
        if elapsed < self.start_ms {
            return None;
        }
        let end = if self.end_ms > 0 {
            self.end_ms
        } else if scene_duration_ms > 0 {
            scene_duration_ms
        } else {
            u64::MAX
        };
        if elapsed >= end {
            return None;
        }
        let t = elapsed - self.start_ms;
        if self.blink_ms > 0 && (t / self.blink_ms) % 2 == 1 {
            return None;
        }
        let mut opacity = self.alpha;
        if self.fade_in_ms > 0 && t < self.fade_in_ms {
            opacity *= t as f32 / self.fade_in_ms as f32;
        }
        if self.fade_out_ms > 0 && end != u64::MAX {
            let left = end - elapsed;
            if left < self.fade_out_ms {
                opacity *= left as f32 / self.fade_out_ms as f32;
            }
        }
        if opacity <= 0.0 {
            return None;
        }
        let (x, y) = match self.from {
            Some([fx, fy]) if self.move_ms > 0 && t < self.move_ms => {
                let s = t as f32 / self.move_ms as f32;
                (fx + (self.x - fx) * s, fy + (self.y - fy) * s)
            }
            _ => (self.x, self.y),
        };
        Some(LayerState { x, y, opacity })
    }
}

/// A parsed, validated display scene.
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    /// Length in ms; 0 = shown until replaced.
    pub duration_ms: u64,
    /// Loop the timeline (the scene then never ends by itself).
    pub repeat: bool,
    /// Background colour.
    pub background: [u8; 3],
    /// Layers, back to front.
    pub layers: Vec<SceneLayer>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawScene {
    schema_version: u32,
    #[serde(default)]
    duration_ms: u64,
    #[serde(default)]
    repeat: bool,
    #[serde(default)]
    background: Option<String>,
    #[serde(default)]
    layers: Vec<RawLayer>,
}

#[derive(Deserialize, Copy, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RawKind {
    Text,
    Rect,
    Image,
    Score,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLayer {
    kind: RawKind,
    text: Option<String>,
    file: Option<String>,
    x: Option<f32>,
    y: Option<f32>,
    w: Option<f32>,
    h: Option<f32>,
    align: Option<Align>,
    valign: Option<VAlign>,
    size: Option<f32>,
    #[serde(default)]
    font: FontChoice,
    color: Option<String>,
    alpha: Option<f32>,
    #[serde(default)]
    start_ms: u64,
    #[serde(default)]
    end_ms: u64,
    #[serde(default)]
    blink_ms: u64,
    #[serde(default)]
    fade_in_ms: u64,
    #[serde(default)]
    fade_out_ms: u64,
    from: Option<[f32; 2]>,
    #[serde(default)]
    move_ms: u64,
}

/// `#rrggbb` (case-insensitive) as bytes.
pub fn parse_color(s: &str) -> Option<[u8; 3]> {
    let hex = s.strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

fn invalid(i: usize, what: impl fmt::Display) -> SceneError {
    SceneError::Invalid(format!("layers[{i}]: {what}"))
}

impl Scene {
    /// Parses and validates a scene from TOML text.
    pub fn from_toml_str(text: &str) -> Result<Scene, SceneError> {
        if text.len() > MAX_SCENE_BYTES {
            return Err(SceneError::TooLarge(text.len()));
        }
        let raw: RawScene = toml::from_str(text).map_err(|e| SceneError::Parse(e.to_string()))?;
        if raw.schema_version != 1 {
            return Err(SceneError::SchemaVersion(raw.schema_version));
        }
        if raw.duration_ms > MAX_TIME_MS {
            return Err(SceneError::Invalid(format!(
                "duration_ms {} > {MAX_TIME_MS}",
                raw.duration_ms
            )));
        }
        if raw.repeat && raw.duration_ms == 0 {
            return Err(SceneError::Invalid(
                "repeat needs a duration_ms > 0".into(),
            ));
        }
        let background = match raw.background.as_deref() {
            None => [0, 0, 0],
            Some(s) => parse_color(s)
                .ok_or_else(|| SceneError::Invalid(format!("background {s:?} is not #rrggbb")))?,
        };
        if raw.layers.len() > MAX_SCENE_LAYERS {
            return Err(SceneError::Invalid(format!(
                "{} layers > {MAX_SCENE_LAYERS}",
                raw.layers.len()
            )));
        }
        let layers = raw
            .layers
            .into_iter()
            .enumerate()
            .map(|(i, l)| validate_layer(i, l))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Scene {
            duration_ms: raw.duration_ms,
            repeat: raw.repeat,
            background,
            layers,
        })
    }

    /// [`from_toml_str`](Self::from_toml_str) on file bytes (size checked
    /// first, then UTF-8). Never panics.
    pub fn from_bytes(bytes: &[u8]) -> Result<Scene, SceneError> {
        if bytes.len() > MAX_SCENE_BYTES {
            return Err(SceneError::TooLarge(bytes.len()));
        }
        let text = core::str::from_utf8(bytes).map_err(|_| SceneError::NotUtf8)?;
        Scene::from_toml_str(text)
    }

    /// Bundle paths of the scene's images (in layer order, repeats kept).
    pub fn image_files(&self) -> impl Iterator<Item = &str> {
        self.layers.iter().filter_map(|l| match &l.kind {
            LayerKind::Image(f) => Some(f.as_str()),
            _ => None,
        })
    }
}

fn validate_layer(i: usize, l: RawLayer) -> Result<SceneLayer, SceneError> {
    if l.text.is_some() && l.kind != RawKind::Text {
        return Err(invalid(i, "`text` is only for kind = \"text\""));
    }
    if l.file.is_some() && l.kind != RawKind::Image {
        return Err(invalid(i, "`file` is only for kind = \"image\""));
    }
    let kind = match l.kind {
        RawKind::Text => {
            let text = l.text.ok_or_else(|| invalid(i, "kind = \"text\" needs `text`"))?;
            if text.chars().count() > MAX_TEXT_CHARS {
                return Err(invalid(i, format!("text longer than {MAX_TEXT_CHARS}")));
            }
            LayerKind::Text(text)
        }
        RawKind::Image => {
            let file = l.file.ok_or_else(|| invalid(i, "kind = \"image\" needs `file`"))?;
            if file.is_empty() || file.len() > 256 {
                return Err(invalid(i, "file must be 1..=256 bytes"));
            }
            LayerKind::Image(file)
        }
        RawKind::Rect => LayerKind::Rect,
        RawKind::Score => LayerKind::Score,
    };
    let is_text = l.kind == RawKind::Text;
    let (dx, dy, da, dv) = if is_text {
        (0.5, 0.5, Align::Center, VAlign::Middle)
    } else {
        (0.0, 0.0, Align::Left, VAlign::Top)
    };
    let x = l.x.unwrap_or(dx);
    let y = l.y.unwrap_or(dy);
    let w = l.w.unwrap_or(1.0);
    let h = l.h.unwrap_or(if l.kind == RawKind::Image { 0.0 } else { 1.0 });
    let size = l.size.unwrap_or(0.5);
    let alpha = l.alpha.unwrap_or(1.0);
    let coord = |name: &str, v: f32| {
        if v.is_finite() && v.abs() <= MAX_COORD {
            Ok(())
        } else {
            Err(invalid(i, format!("{name} = {v} outside ±{MAX_COORD}")))
        }
    };
    coord("x", x)?;
    coord("y", y)?;
    if let Some([fx, fy]) = l.from {
        coord("from[0]", fx)?;
        coord("from[1]", fy)?;
    }
    for (name, v) in [("w", w), ("h", h)] {
        if !(v.is_finite() && (0.0..=MAX_COORD).contains(&v)) {
            return Err(invalid(i, format!("{name} = {v} outside 0..={MAX_COORD}")));
        }
    }
    if !(size.is_finite() && size > 0.0 && size <= MAX_COORD) {
        return Err(invalid(i, format!("size = {size} outside (0, {MAX_COORD}]")));
    }
    if !(alpha.is_finite() && (0.0..=1.0).contains(&alpha)) {
        return Err(invalid(i, format!("alpha = {alpha} outside 0..=1")));
    }
    for (name, v) in [
        ("start_ms", l.start_ms),
        ("end_ms", l.end_ms),
        ("blink_ms", l.blink_ms),
        ("fade_in_ms", l.fade_in_ms),
        ("fade_out_ms", l.fade_out_ms),
        ("move_ms", l.move_ms),
    ] {
        if v > MAX_TIME_MS {
            return Err(invalid(i, format!("{name} = {v} > {MAX_TIME_MS}")));
        }
    }
    let color = match l.color.as_deref() {
        None => [255, 255, 255],
        Some(s) => parse_color(s).ok_or_else(|| invalid(i, format!("color {s:?} is not #rrggbb")))?,
    };
    Ok(SceneLayer {
        kind,
        x,
        y,
        w,
        h,
        align: l.align.unwrap_or(da),
        valign: l.valign.unwrap_or(dv),
        size,
        font: l.font,
        color,
        alpha,
        start_ms: l.start_ms,
        end_ms: l.end_ms,
        blink_ms: l.blink_ms,
        fade_in_ms: l.fade_in_ms,
        fade_out_ms: l.fade_out_ms,
        from: l.from,
        move_ms: l.move_ms,
    })
}

/// The scenes of a table, by [`SceneId`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SceneLibrary {
    scenes: Vec<Option<(String, Scene)>>,
    names: BTreeMap<String, SceneId>,
}

impl SceneLibrary {
    /// An empty library.
    pub fn new() -> SceneLibrary {
        SceneLibrary::default()
    }

    /// The placeholder scenes, `SceneId(i)` = position in
    /// [`PLACEHOLDER_SCENES`].
    pub fn placeholders() -> SceneLibrary {
        let mut lib = SceneLibrary::new();
        for (i, (name, _)) in PLACEHOLDER_SCENES.iter().enumerate() {
            let scene = placeholder_scene(name).expect("listed placeholder");
            lib.insert(SceneId(i as u16), name, scene);
        }
        lib
    }

    /// Adds or replaces scene `id` named `name`.
    pub fn insert(&mut self, id: SceneId, name: &str, scene: Scene) {
        let i = usize::from(id.0);
        if self.scenes.len() <= i {
            self.scenes.resize(i + 1, None);
        }
        if let Some((old, _)) = &self.scenes[i]
            && self.names.get(old) == Some(&id)
        {
            self.names.remove(old);
        }
        self.scenes[i] = Some((name.to_owned(), scene));
        self.names.insert(name.to_owned(), id);
    }

    /// Loads `table.toml`'s `[[scenes]]`: `SceneId(i)` is declaration index
    /// `i` (as `pin_assets` interns scene names). `read` returns a bundle
    /// file's bytes. A file that cannot be read falls back to the
    /// placeholder scene of the same id, if there is one (so the A.1 example
    /// table shows content before real scenes exist); a file that reads but
    /// does not parse is an error.
    pub fn load(
        defs: &[SceneDef],
        mut read: impl FnMut(&str) -> Result<Vec<u8>, String>,
    ) -> Result<SceneLibrary, SceneError> {
        if defs.len() > usize::from(u16::MAX) + 1 {
            return Err(SceneError::Invalid(format!("{} scenes", defs.len())));
        }
        let mut lib = SceneLibrary::new();
        for (i, def) in defs.iter().enumerate() {
            let scene = match read(&def.file) {
                Ok(bytes) => Scene::from_bytes(&bytes).map_err(|e| SceneError::InFile {
                    id: def.id.clone(),
                    file: def.file.clone(),
                    error: Box::new(e),
                })?,
                Err(reason) => placeholder_scene(&def.id).ok_or_else(|| SceneError::Read {
                    id: def.id.clone(),
                    file: def.file.clone(),
                    reason,
                })?,
            };
            lib.insert(SceneId(i as u16), &def.id, scene);
        }
        Ok(lib)
    }

    /// Scene `id`.
    pub fn get(&self, id: SceneId) -> Option<&Scene> {
        self.scenes
            .get(usize::from(id.0))
            .and_then(|s| s.as_ref())
            .map(|(_, s)| s)
    }

    /// Name of scene `id`.
    pub fn name(&self, id: SceneId) -> Option<&str> {
        self.scenes
            .get(usize::from(id.0))
            .and_then(|s| s.as_ref())
            .map(|(n, _)| n.as_str())
    }

    /// Id of the scene named `name`.
    pub fn id_of(&self, name: &str) -> Option<SceneId> {
        self.names.get(name).copied()
    }

    /// Number of scenes.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether the library is empty.
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

/// Which scene is on the display, and since when.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct ScenePlayer {
    current: Option<(SceneId, u64)>,
}

impl ScenePlayer {
    /// A player showing nothing (the score view).
    pub fn new() -> ScenePlayer {
        ScenePlayer::default()
    }

    /// Starts scene `id` at `now_ms` (restarts it if already showing).
    pub fn show(&mut self, id: SceneId, now_ms: u64) {
        self.current = Some((id, now_ms));
    }

    /// Applies the rules' `display_scenes` commands: the last one wins.
    pub fn apply_commands(&mut self, q: &HostCommandQueue, now_ms: u64) {
        if let Some(&id) = q.display_scenes.last() {
            self.show(id, now_ms);
        }
    }

    /// Back to the score view.
    pub fn clear(&mut self) {
        self.current = None;
    }

    /// The last scene shown and its start time.
    pub fn current(&self) -> Option<(SceneId, u64)> {
        self.current
    }

    /// The scene on the display at `now_ms` and the ms elapsed in it (wrapped
    /// for `repeat` scenes); `None` once a timed scene has ended, for unknown
    /// ids, and when nothing was shown.
    pub fn active<'a>(&self, lib: &'a SceneLibrary, now_ms: u64) -> Option<(&'a Scene, u64)> {
        let (id, start) = self.current?;
        let scene = lib.get(id)?;
        let mut elapsed = now_ms.saturating_sub(start);
        if scene.duration_ms > 0 {
            if scene.repeat {
                elapsed %= scene.duration_ms;
            } else if elapsed >= scene.duration_ms {
                return None;
            }
        }
        Some((scene, elapsed))
    }
}
