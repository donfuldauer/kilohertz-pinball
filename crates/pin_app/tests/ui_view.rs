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
                features: vec![
                    TableFeature::LuaRules,
                    TableFeature::Magnets,
                    TableFeature::Dof,
                ],
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
    size: [u32; 2],
    ppp: f32,
}

impl Harness {
    fn new() -> Self {
        let ctx = egui::Context::default();
        overlay_style(&ctx);
        Harness {
            ctx,
            view: MenuView::new(),
            time: 0.0,
            size: SIZE,
            ppp: 1.0,
        }
    }

    fn frame(
        &mut self,
        m: &mut MenuState,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, Vec<MenuInput>) {
        self.time += 1.0 / 60.0;
        let raw = raw_input(self.size, self.ppp, events, Some(self.time));
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
    for (size, ppp) in [
        (SIZE, 1.0),
        ([800, 600], 1.0),
        ([2560, 1440], 2.0),
        ([1080, 1920], 1.5),
    ] {
        layout_inside(size, ppp);
    }
}

fn layout_inside(size: [u32; 2], ppp: f32) {
    let mut h = Harness::new();
    h.size = size;
    h.ppp = ppp;
    let mut m = open_menu();
    let screen = egui::Rect::from_min_size(
        egui::Pos2::ZERO,
        egui::vec2(size[0] as f32 / ppp, size[1] as f32 / ppp),
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
                for t in [
                    "Apex Circuit",
                    "Lua",
                    "Magnets",
                    "DOF",
                    "Play",
                    "by Reference",
                    "broken",
                ] {
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
                for t in [
                    "Input profile",
                    "Flipper staging",
                    "Live input",
                    "Plunger raw",
                ] {
                    assert!(has_text(&out, t), "{t} missing");
                }
            }
        }
        // No widget overflows the panel (the only shape with the full-screen
        // clip rect is the backdrop, which is the panel itself).
        for c in &out.shapes {
            let r = c.shape.visual_bounding_rect();
            if r.is_positive() {
                assert!(
                    panel.contains_rect(r),
                    "{p:?}: shape {r:?} outside {panel:?}"
                );
            }
        }
        let frame = OverlayFrame::new(&h.ctx, out);
        for prim in &frame.primitives {
            // What is painted is the mesh scissored by its clip rect: it must
            // stay inside the overlay panel.
            if let egui::epaint::Primitive::Mesh(mesh) = &prim.primitive {
                let bounds = mesh.calc_bounds().intersect(prim.clip_rect);
                if bounds.is_positive() {
                    assert!(
                        panel.contains_rect(bounds),
                        "{p:?}: {bounds:?} outside {panel:?}"
                    );
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
    assert!(
        inputs.contains(&MenuInput::ShowPanel(Panel::Display)),
        "{inputs:?}"
    );
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

fn many_tables(n: usize) -> Vec<TableEntry> {
    (0..n)
        .map(|k| TableEntry {
            path: PathBuf::from(format!("/t/t{k:02}")),
            file_name: format!("t{k:02}"),
            status: TableStatus::Ready(TableInfo {
                id: format!("t{k:02}"),
                name: format!("Table {k:02}"),
                author: "A".into(),
                version: "1".into(),
                players_max: 1,
                features: vec![TableFeature::LuaRules, TableFeature::Multiball],
                preview: None,
                preview_error: None,
                warning_count: 0,
            }),
        })
        .collect()
}

/// 4.4 review B2: mouse-wheel scrolling must not snap back to the focused row.
#[test]
fn wheel_scrolling_is_not_undone_by_the_focus() {
    let mut h = Harness::new();
    h.size = [1280, 720];
    let mut m = MenuState::default();
    m.set_tables(many_tables(60));
    m.apply(MenuInput::Toggle);
    m.apply(MenuInput::Down);
    assert_eq!(m.focused_table(), Some(0));
    let mut y0 = None;
    for _ in 0..3 {
        let (out, _) = h.frame(&mut m, Vec::new());
        y0 = find(&out, "Table 00").map(|r| r.top());
        out.drop_without_applying_deltas();
    }
    let y0 = y0.expect("row 0 visible");
    let (out, _) = h.frame(&mut m, Vec::new());
    let over = find(&out, "Table 03").expect("row 3").center();
    out.drop_without_applying_deltas();
    let wheel = egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: egui::vec2(0.0, -5.0),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Unknown,
        modifiers: egui::Modifiers::NONE,
    };
    // Hover first (egui hit-tests against the previous frame), then wheel.
    let (out, _) = h.frame(&mut m, vec![egui::Event::PointerMoved(over)]);
    out.drop_without_applying_deltas();
    let (out, _) = h.frame(&mut m, std::iter::repeat_n(wheel, 5).collect());
    out.drop_without_applying_deltas();
    let mut y = None;
    for _ in 0..40 {
        let (out, _) = h.frame(&mut m, Vec::new());
        y = find(&out, "Table 00").map(|r| r.top());
        out.drop_without_applying_deltas();
    }
    assert!(
        y.is_none_or(|y| y < y0 - 50.0),
        "row 0 snapped back: {y:?} vs {y0}"
    );
    assert_eq!(m.focused_table(), Some(0));
    // A focus change scrolls the newly focused row into view.
    m.apply(MenuInput::FocusTable(59));
    let mut seen = false;
    for _ in 0..40 {
        let (out, _) = h.frame(&mut m, Vec::new());
        seen = find(&out, "Table 59").is_some();
        out.drop_without_applying_deltas();
    }
    assert!(seen, "focused row 59 not scrolled into view");
}

/// 4.4 review N8: in a portrait panel the list and the details are stacked
/// and no badge wraps inside itself.
#[test]
fn portrait_tables_panel_does_not_overlap() {
    let mut h = Harness::new();
    h.size = [1080, 1920];
    h.ppp = 1.5;
    let mut m = open_menu();
    m.apply(MenuInput::Down);
    let (out, _) = h.frame(&mut m, Vec::new());
    let t = texts(&out);
    // Badges render as single-line labels.
    for name in ["Lua", "Magnets", "DOF"] {
        let rects: Vec<_> = t.iter().filter(|(s, _)| s == name).map(|(_, r)| *r).collect();
        assert!(!rects.is_empty(), "{name} missing");
        for r in rects {
            assert!(r.height() < 20.0, "{name} wrapped: {r:?}");
        }
    }
    // Stacked: the details ("by Reference") sit below every list row text.
    let details = t.iter().find(|(s, _)| s == "by Reference").expect("details").1;
    let list_bottom = t
        .iter()
        .filter(|(s, _)| s == "Reference" || s.starts_with("Broken:"))
        .map(|(_, r)| r.bottom())
        .fold(0.0f32, f32::max);
    assert!(details.top() > list_bottom, "{details:?} vs list bottom {list_bottom}");
    out.drop_without_applying_deltas();
}

#[test]
fn tmp_debug_wheel() {
    let mut h = Harness::new();
    h.size = [1280, 720];
    let mut m = MenuState::default();
    m.set_tables(many_tables(60));
    m.apply(MenuInput::Toggle);
    for f in 0..50 {
        let mut ev = Vec::new();
        if f == 3 { ev.push(egui::Event::PointerMoved(egui::pos2(300.0, 300.0))); }
        if f == 5 { for _ in 0..5 { ev.push(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta: egui::vec2(0.0, -5.0), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Unknown, modifiers: egui::Modifiers::NONE }); } }
        let (out, _) = h.frame(&mut m, ev);
        let (d, pp, raw) = h.ctx.input(|i| (i.smooth_scroll_delta(), i.pointer.interact_pos(), i.time));
        if f < 12 || f % 10 == 0 { println!("f{f} {:?} d={d:?} p={pp:?} raw={raw:?} layer={:?}", find(&out, "Table 00"), pp.and_then(|p| h.ctx.layer_id_at(p))); }
        out.drop_without_applying_deltas();
    }
    let (out, _) = h.frame(&mut m, Vec::new());
    let rows: Vec<_> = texts(&out).into_iter().filter(|(t, _)| t.starts_with("Table ")).collect();
    println!("rows {} first {:?} last {:?}", rows.len(), rows.first(), rows.last());
    for c in out.shapes.iter().take(3) { println!("clip {:?}", c.clip_rect); }
    let clips: std::collections::BTreeSet<String> = out.shapes.iter().map(|c| format!("{:?}", c.clip_rect)).collect();
    println!("{clips:?}");
    out.drop_without_applying_deltas();
}

#[test]
fn tmp_debug_plain_scroll() {
    for overlay in [false, true] {
        let ctx = egui::Context::default();
        let mut ys = Vec::new();
        for f in 0..40 {
            let mut ev = Vec::new();
            if f == 2 { ev.push(egui::Event::PointerMoved(egui::pos2(300.0, 300.0))); }
            if f == 4 { ev.push(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta: egui::vec2(0.0, -5.0), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Unknown, modifiers: egui::Modifiers::NONE }); }
            let raw = raw_input([1280, 720], 1.0, ev, Some(f as f64 / 60.0));
            let out = ctx.run_ui(raw, |ui| {
                let body = |ui: &mut egui::Ui| {
                    egui::ScrollArea::vertical().id_salt("x").auto_shrink([false, false]).show(ui, |ui| {
                        for k in 0..100 { ui.label(format!("Row {k:03}")); }
                    });
                };
                if overlay { pin_render::ui::show_overlay(ui, body); } else { body(ui); }
            });
            ys.push(find(&out, "Row 000").map(|r| r.top()));
            out.drop_without_applying_deltas();
        }
        println!("overlay={overlay} first={:?} last={:?}", ys[0], ys[39]);
    }
}
