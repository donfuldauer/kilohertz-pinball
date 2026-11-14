//! The egui widgets of one menu frame, built from a [`MenuState`].
//!
//! CPU only: the result is an `egui::FullOutput` that the render thread
//! tessellates and paints with `pin_render::ui::OverlayPainter`. Clicks,
//! slider drags and text edits become [`MenuInput`]s, which are applied to the
//! state (so the state machine stays the single source of truth) and also
//! returned. The focused row is highlighted, so cabinet navigation is visible.

use super::config::AppConfig;
use super::menu::{MenuInput, MenuState, Panel, SettingChange, SettingRow, TableLoadState};
use super::scan::{TableEntry, TableFeature, TableStatus};
use pin_core::input::{CabinetAxis, CabinetInput};
use pin_render::ui::{egui, show_overlay};
use std::collections::BTreeMap;
use std::path::PathBuf;

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

/// Per-session view data: preview textures uploaded to egui, keyed by bundle
/// path.
#[derive(Default)]
pub struct MenuView {
    previews: BTreeMap<PathBuf, egui::TextureHandle>,
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
        state.apply_all(out.iter().cloned());
        out
    }

    fn prune_previews(&mut self, state: &MenuState) {
        self.previews
            .retain(|p, _| state.tables().iter().any(|t| t.path == *p));
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
                    let tex = self.previews.entry(t.path.clone()).or_insert_with(|| {
                        let img = egui::ColorImage::from_rgba_unmultiplied(
                            [art.width as usize, art.height as usize],
                            &art.rgba,
                        );
                        ui.ctx().load_texture(
                            format!("preview:{}", t.path.display()),
                            img,
                            egui::TextureOptions::LINEAR,
                        )
                    });
                    let max_w = ui.available_width().min(256.0);
                    let scale = (max_w / art.width as f32).min(160.0 / art.height as f32);
                    ui.image((
                        tex.id(),
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

/// The scrolling table list. The focused row is scrolled into view only when
/// `scroll` (the focus just changed).
fn table_list(
    ui: &mut egui::Ui,
    state: &MenuState,
    focused: Option<usize>,
    scroll: bool,
    out: &mut Vec<MenuInput>,
) {
    egui::ScrollArea::vertical()
        .id_salt("pin_menu_tables")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if state.visible_tables().is_empty() {
                ui.label("No tables found.");
            }
            for &i in state.visible_tables() {
                let t = &state.tables()[i];
                let resp = table_row(ui, t, focused == Some(i));
                if scroll && focused == Some(i) {
                    resp.scroll_to_me(None);
                }
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
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!(" {} ", f.label()))
                        .small()
                        .color(egui::Color32::WHITE)
                        .background_color(BADGE_FILL),
                )
                .wrap_mode(egui::TextWrapMode::Extend),
            );
        }
    });
}

fn table_row(ui: &mut egui::Ui, t: &TableEntry, focused: bool) -> egui::Response {
    let frame = egui::Frame::new()
        .fill(if focused {
            FOCUS_FILL
        } else {
            egui::Color32::TRANSPARENT
        })
        .inner_margin(egui::Margin::symmetric(4, 2));
    let inner = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(egui::RichText::new(t.title()).strong());
        match &t.status {
            TableStatus::Ready(info) => {
                ui.label(egui::RichText::new(&info.author).small());
                badges(ui, &info.features);
            }
            TableStatus::Broken(e) => {
                let first = e.lines().next().unwrap_or("");
                ui.label(
                    egui::RichText::new(format!("Broken: {first}"))
                        .small()
                        .color(ERROR_TEXT),
                );
            }
        }
    });
    ui.interact(
        inner.response.rect,
        egui::Id::unique(("pin_menu_table_row", &t.path)),
        egui::Sense::click(),
    )
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
