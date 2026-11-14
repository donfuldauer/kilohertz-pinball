//! Headless egui layout of the menu (CPU only): each panel lays out inside
//! the overlay panel, shows its controls and badges, and pointer clicks turn
//! into menu inputs.

use pin_app::ui::{
    MenuInput, MenuState, MenuView, Panel, PreviewArt, SettingChange, TableEntry, TableFeature,
    TableInfo, TableStatus,
};
use pin_render::ui::{OverlayFrame, egui, menu_panel_rect, overlay_style, raw_input};
use std::path::PathBuf;

const SIZE: [u32; 2] = [1024, 768];

fn entries() -> Vec<TableEntry> {
    vec![
        TableEntry {
            path: PathBuf::from("/t/apex"),
            file_name: "apex".into(),
            status: TableStatus::Ready(TableInfo {
                id: "apex".into(),
                name: "Apex Circuit".into(),
                author: "Reference".into(),
                version: "1.0.0".into(),
                players_max: 4,
                features: vec![TableFeature::LuaRules, TableFeature::Magnets, TableFeature::Dof],
                preview: Some(PreviewArt {
                    file: "preview.png".into(),
                    width: 2,
                    height: 1,
                    rgba: vec![255, 0, 0, 255, 0, 255, 0, 255],
                }),
                preview_error: Some("preview.jpg ignored".into()),
                warning_count: 2,
            }),
        },
        TableEntry {
            path: PathBuf::from("/t/broken"),
            file_name: "broken".into(),
            status: TableStatus::Broken("table.toml: line 1\nsecond line".into()),
        },
    ]
}

struct Harness {
    ctx: egui::Context,
    view: MenuView,
    time: f64,
}

impl Harness {
    fn new() -> Self {
        let ctx = egui::Context::default();
        overlay_style(&ctx);
        Harness {
            ctx,
            view: MenuView::new(),
            time: 0.0,
        }
    }

    fn frame(&mut self, m: &mut MenuState, events: Vec<egui::Event>) -> (egui::FullOutput, Vec<MenuInput>) {
        self.time += 1.0 / 60.0;
        let raw = raw_input(SIZE, 1.0, events, Some(self.time));
        self.view.run_frame(&self.ctx, raw, m)
    }

    /// Clicks at `pos` (press and release over two frames).
    fn click(&mut self, m: &mut MenuState, pos: egui::Pos2) -> Vec<MenuInput> {
        let press = egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        };
        let release = egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        };
        let (o1, mut a) = self.frame(m, vec![egui::Event::PointerMoved(pos), press]);
        o1.drop_without_applying_deltas();
        let (o2, b) = self.frame(m, vec![release]);
        o2.drop_without_applying_deltas();
        a.extend(b);
        a
    }
}

/// Every text in the output with its screen rect.
fn texts(out: &egui::FullOutput) -> Vec<(String, egui::Rect)> {
    fn walk(s: &egui::Shape, v: &mut Vec<(String, egui::Rect)>) {
        match s {
            egui::Shape::Text(t) => v.push((
                t.galley.text().to_owned(),
                t.galley.rect.translate(t.pos.to_vec2()),
            )),
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

fn find(out: &egui::FullOutput, needle: &str) -> Option<egui::Rect> {
    texts(out)
        .into_iter()
        .find(|(t, _)| t == needle)
        .map(|(_, r)| r)
}

fn has_text(out: &egui::FullOutput, needle: &str) -> bool {
    texts(out).iter().any(|(t, _)| t.contains(needle))
}

fn open_menu() -> MenuState {
    let mut m = MenuState::default();
    m.set_tables(entries());
    m.apply(MenuInput::Toggle);
    m.drain_commands();
    m
}

#[test]
fn closed_menu_draws_nothing() {
    let mut h = Harness::new();
    let mut m = MenuState::default();
    let (out, inputs) = h.frame(&mut m, Vec::new());
    assert!(inputs.is_empty());
    let frame = OverlayFrame::new(&h.ctx, out);
    assert!(frame.is_empty());
    frame.discard();
}

#[test]
fn every_panel_lays_out_inside_the_overlay() {
    let mut h = Harness::new();
    let mut m = open_menu();
    let screen = egui::Rect::from_min_size(
        egui::Pos2::ZERO,
        egui::vec2(SIZE[0] as f32, SIZE[1] as f32),
    );
    let panel = menu_panel_rect(screen).expand(1.0);
    m.apply(MenuInput::FocusTable(0));
    for p in Panel::ALL {
        m.apply(MenuInput::ShowPanel(p));
        m.apply(MenuInput::Down);
        let (out, inputs) = h.frame(&mut m, Vec::new());
        assert!(inputs.is_empty(), "{p:?}: {inputs:?}");
        for (label, _) in Panel::ALL.iter().map(|q| (q.label(), ())) {
            assert!(find(&out, label).is_some(), "{p:?}: tab {label} missing");
        }
        match p {
            Panel::Tables => {
                for t in ["Apex Circuit", "Lua", "Magnets", "DOF", "Play", "by Reference", "broken"] {
                    assert!(has_text(&out, t), "{t} missing");
                }
                assert!(has_text(&out, "Broken: table.toml: line 1"));
            }
            Panel::Display => {
                for t in ["Monitors", "Camera", "Score display", "Advanced · audio"] {
                    assert!(has_text(&out, t), "{t} missing");
                }
            }
            Panel::Input => {
                for t in ["Input profile", "Live input", "Plunger raw", "Debug wireframe"] {
                    assert!(has_text(&out, t), "{t} missing");
                }
            }
        }
        let frame = OverlayFrame::new(&h.ctx, out);
        for prim in &frame.primitives {
            if let egui::epaint::Primitive::Mesh(mesh) = &prim.primitive {
                for v in &mesh.vertices {
                    assert!(panel.contains(v.pos), "{p:?}: vertex {:?} outside", v.pos);
                }
            }
        }
        frame.discard();
    }
    // The broken table's details show its full error.
    m.apply(MenuInput::ShowPanel(Panel::Tables));
    m.apply(MenuInput::FocusTable(1));
    let (out, _) = h.frame(&mut m, Vec::new());
    assert!(has_text(&out, "Cannot be loaded"));
    assert!(has_text(&out, "second line"));
    out.drop_without_applying_deltas();
}

#[test]
fn clicks_become_menu_inputs() {
    let mut h = Harness::new();
    let mut m = open_menu();
    // Tab click.
    let (out, _) = h.frame(&mut m, Vec::new());
    let tab = find(&out, "Display").expect("tab");
    out.drop_without_applying_deltas();
    let inputs = h.click(&mut m, tab.center());
    assert!(inputs.contains(&MenuInput::ShowPanel(Panel::Display)), "{inputs:?}");
    assert_eq!(m.panel(), Panel::Display);
    // Choice row ">" button steps the value.
    let (out, _) = h.frame(&mut m, Vec::new());
    let next = texts(&out)
        .into_iter()
        .filter(|(t, _)| t == ">")
        .map(|(_, r)| r)
        .min_by(|a, b| a.top().total_cmp(&b.top()))
        .expect("> button");
    out.drop_without_applying_deltas();
    let inputs = h.click(&mut m, next.center());
    assert!(inputs.contains(&MenuInput::Right), "{inputs:?}");
    assert_eq!(m.config().display_mode, pin_app::ui::DisplayMode::Multi);
    // Table list click focuses, Play loads.
    m.apply(MenuInput::ShowPanel(Panel::Tables));
    let (out, _) = h.frame(&mut m, Vec::new());
    let row = find(&out, "Apex Circuit").expect("row");
    out.drop_without_applying_deltas();
    let inputs = h.click(&mut m, row.center());
    assert!(inputs.contains(&MenuInput::FocusTable(0)), "{inputs:?}");
    let (out, _) = h.frame(&mut m, Vec::new());
    let play = find(&out, "Play").expect("play");
    out.drop_without_applying_deltas();
    let inputs = h.click(&mut m, play.center());
    assert!(inputs.contains(&MenuInput::LoadTable(0)), "{inputs:?}");
    assert!(m.load_state().is_busy());
    let (out, _) = h.frame(&mut m, Vec::new());
    assert!(has_text(&out, "Loading"));
    out.drop_without_applying_deltas();
    // A wireframe checkbox click toggles the setting.
    m.apply(MenuInput::ShowPanel(Panel::Input));
    let (out, _) = h.frame(&mut m, Vec::new());
    let off = find(&out, "Off").expect("checkbox");
    out.drop_without_applying_deltas();
    let inputs = h.click(&mut m, off.center());
    assert!(
        inputs.contains(&MenuInput::Set(SettingChange::Wireframe(true))),
        "{inputs:?}"
    );
    assert!(m.config().wireframe);
}

#[test]
fn notices_and_failures_are_shown() {
    let mut h = Harness::new();
    let mut m = open_menu();
    m.apply(MenuInput::LoadTable(0));
    m.drain_commands();
    m.report_load_result(Err("disk on fire".into()));
    m.apply(MenuInput::FocusTable(0));
    let (out, _) = h.frame(&mut m, Vec::new());
    assert!(has_text(&out, "disk on fire"));
    out.drop_without_applying_deltas();
    m.apply(MenuInput::SetFilter("zzz".into()));
    let (out, _) = h.frame(&mut m, Vec::new());
    assert!(has_text(&out, "No tables found."));
    assert!(has_text(&out, "Select a table."));
    out.drop_without_applying_deltas();
    // Tables removed: the cached preview is dropped without trouble.
    m.set_tables(Vec::new());
    let (out, _) = h.frame(&mut m, Vec::new());
    out.drop_without_applying_deltas();
    assert!(format!("{:?}", h.view).contains("MenuView"));
}
