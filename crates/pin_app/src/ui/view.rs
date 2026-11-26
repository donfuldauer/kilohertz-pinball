//! The egui widgets of one menu frame, built from a [`MenuState`].
//!
//! CPU only: the result is an `egui::FullOutput` that the render thread
//! tessellates and paints with `pin_render::ui::OverlayPainter`. Clicks,
//! slider drags and text edits become [`MenuInput`]s, which are applied to the
//! state (so the state machine stays the single source of truth) and also
//! returned. The focused row is highlighted, so cabinet navigation is visible.
//!
//! Large lists stay cheap (SPEC §4 Subphase 4.4, D25):
//! * the table list is virtualised: rows have a fixed height (`table_row_height`:
//!   a title line plus an author / badges line, clipped to the row) and only
//!   the rows inside the scroll viewport are laid out, so 4,096 tables cost
//!   the same per frame as a screenful;
//! * uploaded preview textures are a least-recently-used cache under
//!   [`PREVIEW_TEXTURE_BUDGET_BYTES`] (the table focused this frame is always
//!   kept), and textures of tables no longer listed are dropped each frame in
//!   time linear in the cache and the table count.

use super::config::AppConfig;
use super::menu::{MenuInput, MenuState, Panel, SettingChange, SettingRow, TableLoadState};
use super::scan::{TableEntry, TableFeature, TableStatus};
use pin_core::input::{CabinetAxis, CabinetInput};
use pin_render::ui::{egui, show_overlay};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// Most bytes (`width × height × 4`, summed) of preview textures the menu
/// keeps uploaded: 64 MiB = 256 thumbnails of 256². Older textures are
/// evicted least recently used first.
pub const PREVIEW_TEXTURE_BUDGET_BYTES: u64 = 64 << 20;

/// Vertical inner margin of a table row, points (each side).
const ROW_MARGIN_Y: f32 = 2.0;
/// Horizontal inner margin of a table row, points (each side).
const ROW_MARGIN_X: f32 = 4.0;

/// Highlight of the focused row.
const FOCUS_FILL: egui::Color32 = egui::Color32::from_rgb(40, 70, 120);
/// Badge fill.
const BADGE_FILL: egui::Color32 = egui::Color32::from_rgb(60, 90, 60);
/// Error text.
const ERROR_TEXT: egui::Color32 = egui::Color32::from_rgb(255, 120, 110);
/// Footer key hints: light grey, ≈ 10:1 contrast on the near-black backdrop
/// (4.4 review N15 asks for ≥ 4.5:1).
pub const HINT_TEXT: egui::Color32 = egui::Color32::from_rgb(200, 200, 205);
/// Footer hint size relative to the body text height.
const HINT_SIZE_FRACTION: f32 = 0.8;
/// Accent for titles.
const TITLE_TEXT: egui::Color32 = egui::Color32::from_rgb(250, 230, 160);

/// Width share of the settings list in the input panel.
const INPUT_SETTINGS_FRACTION: f32 = 0.62;
/// Narrowest body (points) that shows two columns side by side (settings +
/// live readings, table list + details); narrower panels stack them.
const SIDE_BY_SIDE_MIN_WIDTH: f32 = 700.0;
/// The same for the table browser (list + details need less width).
const TABLES_SIDE_BY_SIDE_MIN_WIDTH: f32 = 560.0;
/// Width (side by side) or height (stacked) share of the table list.
const TABLE_LIST_FRACTION: f32 = 0.5;

/// Short names of the cabinet inputs for the live display.
const INPUT_NAMES: [&str; CabinetInput::COUNT] = [
    "L flip", "R flip", "L upper", "R upper", "Action", "Start", "Launch", "Tilt",
];

/// One uploaded preview texture.
struct CachedPreview {
    tex: egui::TextureHandle,
    /// `width × height × 4`.
    bytes: u64,
    /// [`MenuView::frame`] of the last frame that showed it.
    last_used: u64,
}

/// Per-session view data: preview textures uploaded to egui (an LRU cache
/// keyed by bundle path, see the module docs) and the scroll-into-view state.
#[derive(Default)]
pub struct MenuView {
    previews: BTreeMap<PathBuf, CachedPreview>,
    /// Sum of `bytes` over `previews`.
    preview_bytes: u64,
    /// Frames shown so far (the LRU clock).
    frame: u64,
    /// `(panel, focused row)` of the previous frame: the focused row is
    /// scrolled into view only on the frame the focus changed, so mouse-wheel
    /// scrolling is not undone every frame.
    last_focus: Option<(Panel, usize)>,
}

impl std::fmt::Debug for MenuView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MenuView")
            .field("previews", &self.previews.len())
            .finish()
    }
}

impl MenuView {
    /// An empty view.
    pub fn new() -> Self {
        Self::default()
    }

    /// Bytes (`width × height × 4`, summed) of the preview textures currently
    /// uploaded; at most [`PREVIEW_TEXTURE_BUDGET_BYTES`] after every
    /// [`run_frame`](Self::run_frame).
    pub fn preview_texture_bytes(&self) -> u64 {
        self.preview_bytes
    }

    /// Number of preview textures currently uploaded.
    pub fn preview_texture_count(&self) -> usize {
        self.previews.len()
    }

    /// Runs one egui pass of the menu: lays out the widgets from `state`,
    /// applies the inputs they produced to `state`, and returns egui's output
    /// plus those inputs. A closed menu produces an empty frame.
    pub fn run_frame(
        &mut self,
        ctx: &egui::Context,
        raw: egui::RawInput,
        state: &mut MenuState,
    ) -> (egui::FullOutput, Vec<MenuInput>) {
        let mut inputs = Vec::new();
        let out = ctx.run_ui(raw, |ui| {
            inputs = self.show(ui, state);
        });
        (out, inputs)
    }

    /// Builds the menu into the root `ui` of a pass and applies the produced
    /// inputs to `state`. Returns the inputs.
    pub fn show(&mut self, ui: &mut egui::Ui, state: &mut MenuState) -> Vec<MenuInput> {
        if !state.is_open() {
            self.last_focus = None;
            return Vec::new();
        }
        self.frame += 1;
        self.prune_previews(state);
        let focus_now = Some((state.panel(), state.focus()));
        let scroll = focus_now != self.last_focus;
        self.last_focus = focus_now;
        let mut out = Vec::new();
        show_overlay(ui, |ui| {
            ui.style_mut().interaction.selectable_labels = false;
            self.header(ui, state, &mut out);
            ui.separator();
            // Notice line + key hints (which may wrap to two lines).
            let footer_h = ui.text_style_height(&egui::TextStyle::Body) * 3.0 + 8.0;
            let body_h = (ui.available_height() - footer_h).max(0.0);
            ui.allocate_ui(egui::vec2(ui.available_width(), body_h), |ui| {
                match state.panel() {
                    Panel::Tables => self.tables(ui, state, scroll, &mut out),
                    Panel::Display => settings(ui, state, &SettingRow::DISPLAY, scroll, &mut out),
                    Panel::Input => {
                        // Settings get ~60 % of the width (label + slider +
                        // value box), the live readings the rest.
                        // A narrow (portrait) panel stacks them instead.
                        let (w, h) = (ui.available_width(), ui.available_height());
                        if w >= SIDE_BY_SIDE_MIN_WIDTH {
                            let left = (w * INPUT_SETTINGS_FRACTION).floor();
                            ui.horizontal_top(|ui| {
                                ui.allocate_ui_with_layout(
                                    egui::vec2(left, h),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        ui.set_max_width(left);
                                        ui.set_max_height(h);
                                        ui.set_max_height(h);
                                        settings(ui, state, &SettingRow::INPUT, scroll, &mut out);
                                    },
                                );
                                ui.separator();
                                column(ui, h, |ui| live(ui, state));
                            });
                        } else {
                            let top = (h * INPUT_SETTINGS_FRACTION).floor();
                            ui.allocate_ui(egui::vec2(w, top), |ui| {
                                ui.set_max_height(top);
                                settings(ui, state, &SettingRow::INPUT, scroll, &mut out);
                            });
                            ui.separator();
                            live(ui, state);
                        }
                    }
                }
            });
            ui.separator();
            footer(ui, state);
        });
        // The table whose details were shown this frame (before the inputs
        // move the focus) keeps its texture.
        let keep = state
            .focused_table()
            .map(|i| state.tables()[i].path.clone());
        self.evict_previews(PREVIEW_TEXTURE_BUDGET_BYTES, keep.as_deref());
        state.apply_all(out.iter().cloned());
        out
    }

    /// Drops the textures of tables that are no longer listed: one pass
    /// over the tables (a set of their paths) and one over the cache.
    fn prune_previews(&mut self, state: &MenuState) {
        if self.previews.is_empty() {
            return;
        }
        let listed: HashSet<&Path> = state.tables().iter().map(|t| t.path.as_path()).collect();
        let mut freed = 0;
        self.previews.retain(|p, c| {
            let keep = listed.contains(p.as_path());
            if !keep {
                freed += c.bytes;
            }
            keep
        });
        self.preview_bytes -= freed;
    }

    /// Evicts least recently used textures (ties: path order) until the
    /// cache holds at most `budget` bytes, never the one of `keep`.
    fn evict_previews(&mut self, budget: u64, keep: Option<&Path>) {
        while self.preview_bytes > budget {
            let oldest = self
                .previews
                .iter()
                .filter(|(p, _)| Some(p.as_path()) != keep)
                .min_by_key(|(_, c)| c.last_used)
                .map(|(p, _)| p.clone());
            let Some(path) = oldest else { break };
            if let Some(c) = self.previews.remove(&path) {
                self.preview_bytes -= c.bytes;
            }
        }
    }

    /// The texture of `art` (the preview of the table at `path`), uploaded
    /// on first use and marked as used this frame.
    fn preview_texture(
        &mut self,
        ctx: &egui::Context,
        path: &Path,
        art: &super::scan::PreviewArt,
    ) -> egui::TextureId {
        let frame = self.frame;
        if let Some(c) = self.previews.get_mut(path) {
            c.last_used = frame;
            return c.tex.id();
        }
        let img = egui::ColorImage::from_rgba_unmultiplied(
            [art.width as usize, art.height as usize],
            &art.rgba,
        );
        let tex = ctx.load_texture(
            format!("preview:{}", path.display()),
            img,
            egui::TextureOptions::LINEAR,
        );
        let id = tex.id();
        let bytes = u64::from(art.width) * u64::from(art.height) * 4;
        self.preview_bytes += bytes;
        self.previews.insert(
            path.to_path_buf(),
            CachedPreview {
                tex,
                bytes,
                last_used: frame,
            },
        );
        id
    }

    fn header(&mut self, ui: &mut egui::Ui, state: &MenuState, out: &mut Vec<MenuInput>) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Kilohertz Pinball")
                    .strong()
                    .color(TITLE_TEXT),
            );
            ui.add_space(16.0);
            let tab_focus = state.focus() == 0;
            for p in Panel::ALL {
                let selected = state.panel() == p;
                let mut text = egui::RichText::new(p.label());
                if selected && tab_focus {
                    text = text.background_color(FOCUS_FILL).strong();
                }
                if ui.selectable_label(selected, text).clicked() && !selected {
                    out.push(MenuInput::ShowPanel(p));
                }
            }
        });
    }

    fn tables(
        &mut self,
        ui: &mut egui::Ui,
        state: &MenuState,
        scroll: bool,
        out: &mut Vec<MenuInput>,
    ) {
        ui.horizontal_wrapped(|ui| {
            ui.label("Filter:");
            let mut filter = state.filter().to_owned();
            if ui
                .add(egui::TextEdit::singleline(&mut filter).desired_width(160.0))
                .changed()
            {
                out.push(MenuInput::SetFilter(filter));
            }
            if ui.button("Rescan").clicked() {
                out.push(MenuInput::Rescan);
            }
            ui.label(format!(
                "{} of {} tables",
                state.visible_tables().len(),
                state.tables().len()
            ));
        });
        let focused = state.focused_table();
        let (w, h) = (ui.available_width(), ui.available_height());
        // List and details side by side; a narrow (portrait) panel stacks
        // them so badge rows keep a usable width.
        if w >= TABLES_SIDE_BY_SIDE_MIN_WIDTH {
            let left = (w * TABLE_LIST_FRACTION).floor();
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(left, h),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_max_width(left);
                        ui.set_max_height(h);
                        table_list(ui, state, focused, scroll, out);
                    },
                );
                ui.separator();
                column(ui, h, |ui| self.details_or_hint(ui, state, focused, out));
            });
        } else {
            let top = (h * TABLE_LIST_FRACTION).floor();
            ui.allocate_ui(egui::vec2(w, top), |ui| {
                ui.set_max_height(top);
                table_list(ui, state, focused, scroll, out);
            });
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt("pin_menu_details")
                .auto_shrink([false, false])
                .show(ui, |ui| self.details_or_hint(ui, state, focused, out));
        }
    }

    fn details_or_hint(
        &mut self,
        ui: &mut egui::Ui,
        state: &MenuState,
        focused: Option<usize>,
        out: &mut Vec<MenuInput>,
    ) {
        if let Some(i) = focused {
            let t = &state.tables()[i];
            self.table_details(ui, t, state.load_state(), i, out);
        } else {
            ui.label("Select a table.");
        }
    }

    fn table_details(
        &mut self,
        ui: &mut egui::Ui,
        t: &TableEntry,
        load: &TableLoadState,
        index: usize,
        out: &mut Vec<MenuInput>,
    ) {
        ui.label(egui::RichText::new(t.title()).strong().color(TITLE_TEXT));
        match &t.status {
            TableStatus::Ready(info) => {
                if let Some(art) = &info.preview {
                    let tex = self.preview_texture(ui.ctx(), &t.path, art);
                    let max_w = ui.available_width().min(256.0);
                    let scale = (max_w / art.width as f32).min(160.0 / art.height as f32);
                    ui.image((
                        tex,
                        egui::vec2(art.width as f32 * scale, art.height as f32 * scale),
                    ));
                } else {
                    ui.label(egui::RichText::new("(no preview)").weak());
                }
                if let Some(e) = &info.preview_error {
                    ui.label(egui::RichText::new(format!("Preview: {e}")).color(ERROR_TEXT));
                }
                ui.label(format!("by {}", info.author));
                ui.label(format!(
                    "version {} · {} player{}",
                    info.version,
                    info.players_max,
                    if info.players_max == 1 { "" } else { "s" }
                ));
                if info.warning_count > 0 {
                    ui.label(format!("{} validation warning(s)", info.warning_count));
                }
                badges(ui, &info.features);
                let busy = load.is_busy();
                let label = if busy { "Loading…" } else { "Play" };
                if ui
                    .add_enabled(
                        !busy,
                        egui::Button::new(egui::RichText::new(label).strong()),
                    )
                    .clicked()
                {
                    out.push(MenuInput::LoadTable(index));
                }
            }
            TableStatus::Broken(e) => {
                ui.label(egui::RichText::new("Cannot be loaded:").color(ERROR_TEXT));
                egui::ScrollArea::vertical()
                    .id_salt("pin_menu_error")
                    .max_height(200.0)
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(e).monospace().color(ERROR_TEXT));
                    });
            }
        }
        match load {
            TableLoadState::Failed { path, error } if *path == t.path => {
                ui.label(egui::RichText::new(format!("Load failed: {error}")).color(ERROR_TEXT));
            }
            TableLoadState::Loading(path) | TableLoadState::LoadRequested(path)
                if *path == t.path =>
            {
                ui.label("Loading…");
            }
            _ => {}
        }
    }
}

/// A top-down column filling the rest of a horizontal row, `h` high, whose
/// width is fixed so wrapping rows (badges) wrap inside it.
fn column<R>(ui: &mut egui::Ui, h: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let w = ui.available_width();
    ui.allocate_ui_with_layout(
        egui::vec2(w, h),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_max_width(w);
            ui.set_max_height(h);
            add(ui)
        },
    )
    .inner
}

/// Height of one table row in `ui`'s style, points (without the spacing
/// between rows): the margins, the title line, the item spacing and the
/// author / badges line (at least an interactive row high). Every row has
/// this height whatever its content, so the list can be virtualised.
fn table_row_height(ui: &egui::Ui) -> f32 {
    let body = ui.text_style_height(&egui::TextStyle::Body);
    let small = ui.text_style_height(&egui::TextStyle::Small);
    let second = small.max(ui.spacing().interact_size.y);
    (2.0 * ROW_MARGIN_Y + body + ui.spacing().item_spacing.y + second).ceil()
}

/// The scrolling table list, virtualised: only the rows in the viewport are
/// laid out ([`egui::ScrollArea::show_rows`]). The focused row is scrolled
/// into view only when `scroll` (the focus just changed), wherever it is in
/// the list.
fn table_list(
    ui: &mut egui::Ui,
    state: &MenuState,
    focused: Option<usize>,
    scroll: bool,
    out: &mut Vec<MenuInput>,
) {
    let visible = state.visible_tables();
    let area = egui::ScrollArea::vertical()
        .id_salt("pin_menu_tables")
        .auto_shrink([false, false]);
    if visible.is_empty() {
        area.show(ui, |ui| {
            ui.label("No tables found.");
        });
        return;
    }
    let row_h = table_row_height(ui);
    let pitch = row_h + ui.spacing().item_spacing.y;
    let focused_row = focused.and_then(|f| visible.iter().position(|&i| i == f));
    area.show_rows(ui, row_h, visible.len(), |ui, rows| {
        if scroll && let Some(r) = focused_row {
            // The row may be outside `rows` (not laid out): scroll to where
            // it sits in the full-height content.
            let top = ui.max_rect().top() + (r as f32 - rows.start as f32) * pitch;
            let rect = egui::Rect::from_min_size(
                egui::pos2(ui.max_rect().left(), top),
                egui::vec2(ui.max_rect().width(), row_h),
            );
            ui.scroll_to_rect(rect, None);
        }
        for &i in &visible[rows] {
            let t = &state.tables()[i];
            let resp = table_row(ui, t, focused == Some(i), row_h);
            if resp.double_clicked() {
                out.push(MenuInput::LoadTable(i));
            } else if resp.clicked() {
                out.push(MenuInput::FocusTable(i));
            }
        }
    });
}

fn badges(ui: &mut egui::Ui, features: &[TableFeature]) {
    ui.horizontal_wrapped(|ui| {
        for f in features {
            // A plain label (known size, so the row wraps between badges)
            // with a coloured background; it never wraps inside itself.
            ui.add(egui::Label::new(badge_text(f)).wrap_mode(egui::TextWrapMode::Extend));
        }
    });
}

/// One fixed-height (`row_h`) list row: the title (truncated), then the
/// author and as many badges as fit (or a broken table's first error line,
/// truncated), all clipped to the row.
fn table_row(ui: &mut egui::Ui, t: &TableEntry, focused: bool, row_h: f32) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), row_h),
        egui::Sense::hover(),
    );
    if ui.is_rect_visible(rect) {
        if focused {
            ui.painter().rect_filled(rect, 0.0, FOCUS_FILL);
        }
        let inner = rect.shrink2(egui::vec2(ROW_MARGIN_X, ROW_MARGIN_Y));
        let mut row = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inner)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        row.set_clip_rect(rect.intersect(ui.clip_rect()));
        row.add(egui::Label::new(egui::RichText::new(t.title()).strong()).truncate());
        row.horizontal(|ui| match &t.status {
            TableStatus::Ready(info) => {
                ui.add(egui::Label::new(egui::RichText::new(&info.author).small()).truncate());
                for f in &info.features {
                    let galley = badge_text(f).into_galley(
                        ui,
                        Some(egui::TextWrapMode::Extend),
                        f32::INFINITY,
                        egui::TextStyle::Small,
                    );
                    // Only whole badges that fit; the rest are in the details.
                    if galley.size().x + ui.spacing().item_spacing.x > ui.available_width() {
                        break;
                    }
                    ui.add(egui::Label::new(galley));
                }
            }
            TableStatus::Broken(e) => {
                let first = e.lines().next().unwrap_or("");
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!("Broken: {first}"))
                            .small()
                            .color(ERROR_TEXT),
                    )
                    .truncate(),
                );
            }
        });
    }
    ui.interact(
        rect,
        egui::Id::unique(("pin_menu_table_row", &t.path)),
        egui::Sense::click(),
    )
}

/// A badge label's text: the label padded, white on [`BADGE_FILL`].
fn badge_text(f: &TableFeature) -> egui::WidgetText {
    egui::RichText::new(format!(" {} ", f.label()))
        .small()
        .color(egui::Color32::WHITE)
        .background_color(BADGE_FILL)
        .into()
}

fn settings(
    ui: &mut egui::Ui,
    state: &MenuState,
    rows: &[SettingRow],
    scroll: bool,
    out: &mut Vec<MenuInput>,
) {
    let cfg: &AppConfig = state.config();
    let focus = state.focus();
    // Sliders take a share of the width so label + slider + value box fit.
    ui.spacing_mut().slider_width = (ui.available_width() * 0.3).clamp(60.0, 220.0);
    egui::ScrollArea::vertical()
        .id_salt(("pin_menu_settings", state.panel().label()))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new(("pin_menu_grid", state.panel().label()))
                .num_columns(2)
                .spacing([16.0, 6.0])
                .show(ui, |ui| {
                    for (k, row) in rows.iter().enumerate() {
                        let r = k + 1;
                        if *row == SettingRow::DecodedAudioBudget {
                            ui.label(egui::RichText::new("Advanced · audio").strong().weak());
                            ui.end_row();
                        }
                        let mut label = egui::RichText::new(row.label());
                        if focus == r {
                            label = label.background_color(FOCUS_FILL).strong();
                        }
                        let resp = ui.label(label);
                        if scroll && focus == r {
                            resp.scroll_to_me(None);
                        }
                        setting_control(ui, *row, r, cfg, out);
                        ui.end_row();
                    }
                });
        });
}

fn setting_control(
    ui: &mut egui::Ui,
    row: SettingRow,
    r: usize,
    cfg: &AppConfig,
    out: &mut Vec<MenuInput>,
) {
    if let (Some(range), Some(mut v)) = (row.range(), row.number(cfg)) {
        let before = v;
        // No `step_by`: egui anchors its grid at the range minimum and would
        // snap (and so "change") values such as the 100 ms release window
        // on every frame. Cabinet / arrow steps use the row's grid instead.
        let slider = egui::Slider::new(&mut v, range.min..=range.max);
        let slider = if range.step >= 1.0 {
            slider.integer()
        } else {
            slider
        };
        let resp = ui.add(slider);
        // The slider reports `changed` when it merely snaps the stored value
        // to its step grid (float rounding); only a real move is an edit, so
        // drawing a frame never steals the focus.
        if resp.changed()
            && (v - before).abs() > range.step * 1e-3
            && let Some(ch) = SettingChange::number(row, v)
        {
            out.push(MenuInput::FocusRow(r));
            out.push(MenuInput::Set(ch));
        }
    } else if row == SettingRow::Wireframe {
        let mut on = cfg.wireframe;
        if ui.checkbox(&mut on, row.value_text(cfg)).changed() {
            out.push(MenuInput::FocusRow(r));
            out.push(MenuInput::Set(SettingChange::Wireframe(on)));
        }
    } else {
        ui.horizontal(|ui| {
            if ui.small_button("<").clicked() {
                out.push(MenuInput::FocusRow(r));
                out.push(MenuInput::Left);
            }
            ui.label(row.value_text(cfg));
            if ui.small_button(">").clicked() {
                out.push(MenuInput::FocusRow(r));
                out.push(MenuInput::Right);
            }
        });
    }
}

fn live(ui: &mut egui::Ui, state: &MenuState) {
    let m = state.monitor();
    ui.label(egui::RichText::new("Live input").strong());
    ui.horizontal_wrapped(|ui| {
        for (i, input) in CabinetInput::ALL.iter().enumerate() {
            let on = m.is_pressed(*input);
            let text = egui::RichText::new(INPUT_NAMES[i]).small();
            let text = if on {
                text.background_color(egui::Color32::from_rgb(40, 160, 60))
                    .color(egui::Color32::WHITE)
            } else {
                text.weak()
            };
            ui.label(text);
        }
    });
    let p = m.plunger();
    ui.label(format!("Plunger raw {:+.3}  rest {:+.3}", p.raw, p.rest));
    ui.add(
        egui::ProgressBar::new(p.position.clamp(0.0, 1.0))
            .text(format!("position {:.2}", p.position)),
    );
    if let Some(v) = p.last_release {
        ui.label(format!("Last release {v:.2}/s"));
    }
    let [nx, ny] = m.nudge();
    ui.label(format!("Nudge x {nx:+.3}  y {ny:+.3}"));
    ui.add(egui::ProgressBar::new((nx.abs()).clamp(0.0, 1.0)).text(format!("x {nx:+.2}")));
    ui.add(egui::ProgressBar::new((ny.abs()).clamp(0.0, 1.0)).text(format!("y {ny:+.2}")));
    ui.label(format!(
        "Flipper axes L {:.2}  R {:.2}",
        m.axis(CabinetAxis::LeftFlipper),
        m.axis(CabinetAxis::RightFlipper)
    ));
}

fn footer(ui: &mut egui::Ui, state: &MenuState) {
    match state.notice() {
        Some(n) => ui.label(egui::RichText::new(n).color(ERROR_TEXT)),
        None => ui.label(egui::RichText::new(" ").small()),
    };
    let size = ui.text_style_height(&egui::TextStyle::Body) * HINT_SIZE_FRACTION;
    ui.label(
        egui::RichText::new(
            "Esc / Guide: close · Flippers: up/down · Upper flippers: left/right · Start: select · Action: back",
        )
        .size(size)
        .color(HINT_TEXT),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::scan::{PreviewArt, TableInfo};
    use pin_render::ui::{overlay_style, raw_input};

    /// `n` ready tables; each has a `side`² preview when `side > 0`.
    fn tables(n: usize, side: u32) -> Vec<TableEntry> {
        (0..n)
            .map(|k| TableEntry {
                path: PathBuf::from(format!("/t/t{k:04}")),
                file_name: format!("t{k:04}"),
                status: TableStatus::Ready(TableInfo {
                    id: format!("t{k}"),
                    name: format!("Table {k:04}"),
                    author: "Author".into(),
                    version: "1".into(),
                    players_max: 1,
                    features: vec![TableFeature::LuaRules, TableFeature::Multiball],
                    preview: (side > 0).then(|| PreviewArt {
                        file: "preview.png".into(),
                        width: side,
                        height: side,
                        rgba: vec![0; (side * side * 4) as usize],
                    }),
                    preview_error: None,
                    warning_count: 0,
                }),
            })
            .collect()
    }

    struct Frames {
        ctx: egui::Context,
        view: MenuView,
        time: f64,
    }

    impl Frames {
        fn new() -> Frames {
            let ctx = egui::Context::default();
            overlay_style(&ctx);
            Frames {
                ctx,
                view: MenuView::new(),
                time: 0.0,
            }
        }

        fn run(&mut self, m: &mut MenuState) -> egui::FullOutput {
            self.time += 1.0 / 60.0;
            let raw = raw_input([1280, 800], 1.0, Vec::new(), Some(self.time));
            let (out, _) = self.view.run_frame(&self.ctx, raw, m);
            out
        }
    }

    fn count_shapes(out: &egui::FullOutput) -> usize {
        fn n(s: &egui::Shape) -> usize {
            match s {
                egui::Shape::Vec(v) => v.iter().map(n).sum(),
                _ => 1,
            }
        }
        out.shapes.iter().map(|c| n(&c.shape)).sum()
    }

    fn texts(out: &egui::FullOutput) -> Vec<String> {
        fn walk(s: &egui::Shape, v: &mut Vec<String>) {
            match s {
                egui::Shape::Text(t) => v.push(t.galley.text().to_owned()),
                egui::Shape::Vec(xs) => xs.iter().for_each(|x| walk(x, v)),
                _ => {}
            }
        }
        let mut v = Vec::new();
        for c in &out.shapes {
            walk(&c.shape, &mut v);
        }
        v
    }

    fn open(tables: Vec<TableEntry>) -> MenuState {
        let mut m = MenuState::default();
        m.set_tables(tables);
        m.apply(MenuInput::Toggle);
        m.drain_commands();
        m
    }

    /// SPEC §4 Subphase 4.4 (#131): with 4,096 tables one frame lays out
    /// only the rows in view: a bounded number of shapes, about what a
    /// screenful of 40 tables costs.
    #[test]
    fn spec_4_4_table_list_is_virtualised() {
        let mut few = open(tables(40, 0));
        let mut many = open(tables(4096, 0));
        let mut f = Frames::new();
        let mut g = Frames::new();
        let (mut a, mut b) = (0, 0);
        for _ in 0..3 {
            let o = f.run(&mut few);
            a = count_shapes(&o);
            o.drop_without_applying_deltas();
            let o = g.run(&mut many);
            b = count_shapes(&o);
            let t = texts(&o);
            assert!(t.iter().any(|s| s == "Table 0000"));
            assert!(!t.iter().any(|s| s == "Table 4095"));
            o.drop_without_applying_deltas();
        }
        assert!(b < 400, "{b} shapes for 4,096 tables");
        assert!(b <= a + 10, "4,096 tables: {b} shapes, 40 tables: {a}");
    }

    /// SPEC §4 Subphase 4.4 (#131): a focus change scrolls the focused row
    /// into view even when it was never laid out (far down a long list).
    #[test]
    fn spec_4_4_focus_scrolls_into_view() {
        let mut m = open(tables(4096, 0));
        let mut f = Frames::new();
        for target in [4000usize, 17, 4095, 0] {
            m.apply(MenuInput::FocusTable(target));
            let name = format!("Table {target:04}");
            let mut rows_with_name = 0;
            for _ in 0..40 {
                let o = f.run(&mut m);
                // The details column repeats the title; the list row is the
                // second occurrence.
                rows_with_name = texts(&o).iter().filter(|s| **s == name).count();
                o.drop_without_applying_deltas();
            }
            assert_eq!(rows_with_name, 2, "{name} not scrolled into view");
            assert_eq!(m.focused_table(), Some(target));
        }
    }

    /// SPEC §4 Subphase 4.4 / D25 (#131): focusing more tables than the
    /// texture budget holds keeps the uploaded bytes within
    /// `PREVIEW_TEXTURE_BUDGET_BYTES` after every frame, always keeps the
    /// focused one, and evicts least recently used first.
    #[test]
    fn spec_4_4_preview_textures_stay_within_budget() {
        let per = 256u64 * 256 * 4;
        let n = (PREVIEW_TEXTURE_BUDGET_BYTES / per) as usize + 20;
        let mut m = open(tables(n, 256));
        let mut f = Frames::new();
        for i in 0..n {
            m.apply(MenuInput::FocusTable(i));
            let o = f.run(&mut m);
            o.drop_without_applying_deltas();
            let v = &f.view;
            assert!(v.preview_texture_bytes() <= PREVIEW_TEXTURE_BUDGET_BYTES);
            assert_eq!(
                v.preview_texture_bytes(),
                v.preview_texture_count() as u64 * per
            );
            assert!(
                v.previews.contains_key(&m.tables()[i].path),
                "focused {i} evicted"
            );
        }
        assert_eq!(f.view.preview_texture_count(), 256);
        // The oldest were evicted: the first 20 are gone, the last 256 stay.
        assert!(!f.view.previews.contains_key(&m.tables()[0].path));
        assert!(f.view.previews.contains_key(&m.tables()[n - 256].path));
        // Re-focusing an old one keeps it as the newest; the least recently
        // used (n - 256) goes.
        m.apply(MenuInput::FocusTable(3));
        let o = f.run(&mut m);
        o.drop_without_applying_deltas();
        assert!(f.view.previews.contains_key(&m.tables()[3].path));
        assert!(!f.view.previews.contains_key(&m.tables()[n - 256].path));
        assert!(f.view.preview_texture_bytes() <= PREVIEW_TEXTURE_BUDGET_BYTES);
    }

    /// SPEC §4 Subphase 4.4 (#131): textures of tables that are no longer
    /// listed are dropped on the next frame, and the byte count follows.
    #[test]
    fn spec_4_4_removed_tables_drop_their_textures() {
        let all = tables(10, 8);
        let mut m = open(all.clone());
        let mut f = Frames::new();
        for i in 0..10 {
            m.apply(MenuInput::FocusTable(i));
            f.run(&mut m).drop_without_applying_deltas();
        }
        assert_eq!(f.view.preview_texture_count(), 10);
        assert_eq!(f.view.preview_texture_bytes(), 10 * 8 * 8 * 4);
        m.set_tables(all[..4].to_vec());
        f.run(&mut m).drop_without_applying_deltas();
        assert_eq!(f.view.preview_texture_count(), 4);
        assert_eq!(f.view.preview_texture_bytes(), 4 * 8 * 8 * 4);
        for t in &all[..4] {
            assert!(f.view.previews.contains_key(&t.path));
        }
        m.set_tables(Vec::new());
        f.run(&mut m).drop_without_applying_deltas();
        assert_eq!(
            (
                f.view.preview_texture_count(),
                f.view.preview_texture_bytes()
            ),
            (0, 0)
        );
    }
}
