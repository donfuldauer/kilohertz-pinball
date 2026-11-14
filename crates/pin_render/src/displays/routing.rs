//! Routing of the displays to PiP regions or dedicated windows, the score
//! display model and change detection.

use super::backglass::{BackglassRenderer, LAMP_REDRAW_EPSILON};
use super::dmd::{DmdFrame, DmdRenderer, DmdSpec};
use super::lcd::{LcdDrawList, LcdRenderer, LcdSpec};
use super::scene::{SCORE_SCENE, Scene, SceneLibrary, ScenePlayer, placeholder_scene};
use super::score::ScoreState;
use super::{DisplayError, ImageSet};
use crate::gpu::GpuContext;
use crate::topology::{Rect, SurfaceRole, TopologyPlan};
use pin_core::LampId;
use pin_core::schema::backglass::{DmdColor, ScoreDisplay, ScoreDisplayMode};
use pin_core::script::HostCommandQueue;

/// Where one display is drawn.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct DisplayRoute {
    /// [`SurfaceRole::Backglass`] or [`SurfaceRole::Score`].
    pub role: SurfaceRole,
    /// Index into [`TopologyPlan::windows`].
    pub window: usize,
    /// Viewport in window pixels.
    pub viewport: Rect,
    /// `true` when the role fills its own window (cabinet mode), `false` for
    /// a PiP on another role's window.
    pub dedicated: bool,
}

/// The backglass and score routes of `plan` (in that order; a role the plan
/// does not show has no route).
pub fn display_routes(plan: &TopologyPlan) -> Vec<DisplayRoute> {
    [SurfaceRole::Backglass, SurfaceRole::Score]
        .into_iter()
        .filter_map(|role| {
            let (window, viewport) = plan.find(role)?;
            Some(DisplayRoute {
                role,
                window,
                viewport,
                dedicated: plan.windows[window].primary_role() == Some(role),
            })
        })
        .collect()
}

/// The largest rectangle of aspect `aspect_w : aspect_h` centred in
/// `viewport` (sizes rounded down, offsets rounded down, positions
/// saturating at `i32::MAX`). A zero aspect returns `viewport`.
pub fn fit_aspect(viewport: Rect, aspect_w: u32, aspect_h: u32) -> Rect {
    if aspect_w == 0 || aspect_h == 0 {
        return viewport;
    }
    let (vw, vh) = (u64::from(viewport.w), u64::from(viewport.h));
    let (aw, ah) = (u64::from(aspect_w), u64::from(aspect_h));
    let (w, h) = if vw * ah <= vh * aw {
        (vw, vw * ah / aw)
    } else {
        (vh * aw / ah, vh)
    };
    // Offsets saturate (a rectangle reaching past `i32::MAX` is not
    // representable anyway); never panics.
    let off = |d: u64| i32::try_from(d / 2).unwrap_or(i32::MAX);
    Rect::new(
        viewport.x.saturating_add(off(vw - w)),
        viewport.y.saturating_add(off(vh - h)),
        w as u32,
        h as u32,
    )
}

/// Which score display a table has.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ScoreDisplayConfig {
    /// Dot matrix.
    Dmd(DmdSpec),
    /// LCD canvas.
    Lcd(LcdSpec),
}

impl ScoreDisplayConfig {
    /// From `backglass.toml` `[score_display]`; a missing `dmd` / `lcd`
    /// table for the chosen mode gives 128 × 32 orange / 1920 × 1080.
    pub fn from_manifest(sd: &ScoreDisplay) -> Result<ScoreDisplayConfig, DisplayError> {
        Ok(match sd.mode {
            ScoreDisplayMode::Dmd => ScoreDisplayConfig::Dmd(match sd.dmd {
                Some(d) => DmdSpec::new(d.width, d.height, d.color)?,
                None => DmdSpec::new(128, 32, DmdColor::Orange)?,
            }),
            ScoreDisplayMode::Lcd => ScoreDisplayConfig::Lcd(match sd.lcd {
                Some(l) => LcdSpec::new(l.width, l.height)?,
                None => LcdSpec::default(),
            }),
        })
    }

    /// Display aspect `(w, h)`.
    pub fn aspect(&self) -> (u32, u32) {
        match self {
            ScoreDisplayConfig::Dmd(s) => (s.width, s.height),
            ScoreDisplayConfig::Lcd(s) => (s.width, s.height),
        }
    }
}

/// What the score display shows at one moment.
#[derive(Clone, Debug, PartialEq)]
pub enum DisplayContent {
    /// A DMD frame.
    Dmd(DmdFrame),
    /// An LCD draw list.
    Lcd(LcdDrawList),
}

/// The GPU-free state of the score display: scores, the scene player, the
/// scene library and scene images.
#[derive(Clone, Debug)]
pub struct ScoreDisplayModel {
    config: ScoreDisplayConfig,
    scores: ScoreState,
    player: ScenePlayer,
    library: SceneLibrary,
    images: ImageSet,
    score_scene: Scene,
}

impl ScoreDisplayModel {
    /// A model showing the score view. The score view is the library's
    /// `scene_score` if it has one, else the placeholder.
    pub fn new(config: ScoreDisplayConfig, library: SceneLibrary) -> ScoreDisplayModel {
        let score_scene = library
            .id_of(SCORE_SCENE)
            .and_then(|id| library.get(id).cloned())
            .unwrap_or_else(|| placeholder_scene(SCORE_SCENE).expect("built-in score scene"));
        ScoreDisplayModel {
            config,
            scores: ScoreState::default(),
            player: ScenePlayer::new(),
            library,
            images: ImageSet::new(),
            score_scene,
        }
    }

    /// The display config.
    pub fn config(&self) -> ScoreDisplayConfig {
        self.config
    }

    /// Scores.
    pub fn scores(&self) -> &ScoreState {
        &self.scores
    }

    /// Scores (ball, credits and players are set by the game layer).
    pub fn scores_mut(&mut self) -> &mut ScoreState {
        &mut self.scores
    }

    /// The scene player.
    pub fn scenes(&self) -> &ScenePlayer {
        &self.player
    }

    /// The scene player.
    pub fn scenes_mut(&mut self) -> &mut ScenePlayer {
        &mut self.player
    }

    /// The scene library.
    pub fn library(&self) -> &SceneLibrary {
        &self.library
    }

    /// Images used by scenes.
    pub fn images_mut(&mut self) -> &mut ImageSet {
        &mut self.images
    }

    /// Applies one tick's rules commands (`scores`, `display_scenes`).
    pub fn apply_commands(&mut self, q: &HostCommandQueue, now_ms: u64) {
        self.scores.apply_commands(q);
        self.player.apply_commands(q, now_ms);
    }

    /// Ms elapsed in the score view at `now_ms`. Its timeline starts when a
    /// timed scene ends (or when a scene with an unknown id was shown), else
    /// at clock 0; `repeat` wraps it, and a timed score scene without
    /// `repeat` holds its last millisecond (the score view never ends).
    fn score_view_elapsed(&self, now_ms: u64) -> u64 {
        let since = match self.player.current() {
            Some((id, start)) => match self.library.get(id) {
                Some(s) if s.duration_ms > 0 && !s.repeat => start.saturating_add(s.duration_ms),
                _ => start,
            },
            None => 0,
        };
        let elapsed = now_ms.saturating_sub(since);
        let d = self.score_scene.duration_ms;
        if d == 0 {
            elapsed
        } else if self.score_scene.repeat {
            elapsed % d
        } else {
            elapsed.min(d - 1)
        }
    }

    /// The content at `now_ms`: the active scene, else the score view (whose
    /// own timeline keeps advancing).
    pub fn compose(&self, now_ms: u64) -> DisplayContent {
        let (scene, elapsed) = self
            .player
            .active(&self.library, now_ms)
            .unwrap_or_else(|| (&self.score_scene, self.score_view_elapsed(now_ms)));
        match &self.config {
            ScoreDisplayConfig::Dmd(spec) => DisplayContent::Dmd(DmdFrame::compose(
                spec,
                scene,
                elapsed,
                &self.scores,
                &self.images,
            )),
            ScoreDisplayConfig::Lcd(spec) => DisplayContent::Lcd(LcdDrawList::compose(
                spec,
                scene,
                elapsed,
                &self.scores,
                &self.images,
            )),
        }
    }
}

/// The score display renderer of either mode.
pub enum ScoreRenderer {
    /// DMD.
    Dmd(DmdRenderer),
    /// LCD.
    Lcd(LcdRenderer),
}

impl ScoreRenderer {
    /// A renderer for `config` and targets of `format`.
    pub fn new(
        ctx: &GpuContext,
        format: wgpu::TextureFormat,
        config: ScoreDisplayConfig,
    ) -> ScoreRenderer {
        match config {
            ScoreDisplayConfig::Dmd(s) => ScoreRenderer::Dmd(DmdRenderer::new(ctx, format, s)),
            ScoreDisplayConfig::Lcd(s) => ScoreRenderer::Lcd(LcdRenderer::new(ctx, format, s)),
        }
    }

    /// Takes `content`; returns whether it changed (redraw needed). Content
    /// of the other mode is ignored (`false`).
    pub fn update(&mut self, queue: &wgpu::Queue, content: &DisplayContent) -> bool {
        match (self, content) {
            (ScoreRenderer::Dmd(r), DisplayContent::Dmd(f)) => r.set_frame(queue, f),
            (ScoreRenderer::Lcd(r), DisplayContent::Lcd(l)) => r.set_draw_list(queue, l),
            _ => false,
        }
    }

    /// Forces the next [`update`](Self::update) to report a change (resize,
    /// expose).
    pub fn mark_dirty(&mut self) {
        match self {
            ScoreRenderer::Dmd(r) => r.mark_dirty(),
            ScoreRenderer::Lcd(r) => r.mark_dirty(),
        }
    }

    /// Records the display into `viewport`.
    pub fn encode(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        target_size: (u32, u32),
        viewport: Rect,
    ) {
        match self {
            ScoreRenderer::Dmd(r) => r.encode(encoder, view, target_size, viewport),
            ScoreRenderer::Lcd(r) => r.encode(encoder, view, target_size, viewport),
        }
    }
}

/// The backglass and score renderers of one target format.
#[derive(Default)]
pub struct DisplaySet {
    /// Backglass, if the table has one.
    pub backglass: Option<BackglassRenderer>,
    /// Score display.
    pub score: Option<ScoreRenderer>,
}

impl DisplaySet {
    /// Records every display routed to `window` into its viewport of `view`
    /// (that window's target, `target_size` pixels). `lamps` drive the
    /// backglass. Returns how many displays were drawn.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_window(
        &mut self,
        window: usize,
        routes: &[DisplayRoute],
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        target_size: (u32, u32),
        lamps: &[f32],
    ) -> usize {
        let mut n = 0;
        for r in routes.iter().filter(|r| r.window == window) {
            match r.role {
                SurfaceRole::Backglass => {
                    if let Some(b) = &mut self.backglass {
                        b.encode(encoder, view, target_size, r.viewport, lamps);
                        n += 1;
                    }
                }
                SurfaceRole::Score => {
                    if let Some(s) = &mut self.score {
                        s.encode(encoder, view, target_size, r.viewport);
                        n += 1;
                    }
                }
                SurfaceRole::Playfield => {}
            }
        }
        n
    }
}

/// Which displays need a redraw.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayDirty {
    /// Backglass lamps changed.
    pub backglass: bool,
    /// Score content changed.
    pub score: bool,
}

/// GPU-free change detection for callers that compose content without a
/// renderer at hand (e.g. the render thread deciding which surfaces to
/// mark for redraw).
#[derive(Clone, Debug)]
pub struct ChangeTracker {
    lamp_ids: Vec<LampId>,
    lamps: Option<Vec<f32>>,
    content: Option<DisplayContent>,
}

impl ChangeTracker {
    /// Watches the backglass lamps `lamp_ids`.
    pub fn new(lamp_ids: Vec<LampId>) -> ChangeTracker {
        ChangeTracker {
            lamp_ids,
            lamps: None,
            content: None,
        }
    }

    /// Compares with the previous observation (the first one is always
    /// dirty). Lamps use the [`BackglassRenderer::needs_redraw`] rule.
    pub fn observe(&mut self, lamps: &[f32], content: &DisplayContent) -> DisplayDirty {
        let now: Vec<f32> = self
            .lamp_ids
            .iter()
            .map(|id| {
                let v = lamps.get(usize::from(id.0)).copied().unwrap_or(0.0);
                if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) }
            })
            .collect();
        let backglass = match &self.lamps {
            None => true,
            Some(old) => old.iter().zip(&now).any(|(&o, &n)| {
                (n - o).abs() >= LAMP_REDRAW_EPSILON || (n != o && (n == 0.0 || n == 1.0))
            }),
        };
        if backglass {
            self.lamps = Some(now);
        }
        let score = self.content.as_ref() != Some(content);
        if score {
            self.content = Some(content.clone());
        }
        DisplayDirty { backglass, score }
    }
}
