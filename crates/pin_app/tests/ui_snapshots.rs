//! Review images of the menu (not an assertion test): with
//! `PIN_UI_SNAPSHOT_DIR=<dir>` set and a GPU adapter (lavapipe), paints every
//! panel at 1024×768 @1× and in portrait 1080×1920 @1.5× over a bright
//! "playfield" and writes `<panel>_<size>.png` into `<dir>`. Without the
//! variable it does nothing.

use pin_app::ui::{
    MenuInput, MenuState, MenuView, Panel, PreviewArt, TableEntry, TableFeature, TableInfo,
    TableStatus,
};
use pin_render::ui::{OverlayFrame, egui, overlay_style, paint_offscreen, raw_input};
use std::path::PathBuf;

fn entries() -> Vec<TableEntry> {
    let preview = PreviewArt {
        file: "preview.png".into(),
        width: 64,
        height: 32,
        rgba: (0..64 * 32)
            .flat_map(|i| [(i % 64 * 4) as u8, (i / 64 * 8) as u8, 160, 255])
            .collect(),
    };
    let mut v = vec![
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
                    TableFeature::UpperFlippers,
                    TableFeature::Ramps,
                    TableFeature::Magnets,
                    TableFeature::Multiball,
                    TableFeature::Spinner,
                    TableFeature::DmdDisplay,
                    TableFeature::Dof,
                ],
                preview: Some(preview),
                preview_error: None,
                warning_count: 1,
            }),
        },
        TableEntry {
            path: PathBuf::from("/t/broken"),
            file_name: "broken".into(),
            status: TableStatus::Broken(
                "table.toml: flippers[0].coil: unknown coil \"left\"".into(),
            ),
        },
    ];
    for k in 0..8 {
        v.push(TableEntry {
            path: PathBuf::from(format!("/t/extra{k}")),
            file_name: format!("extra{k}"),
            status: TableStatus::Ready(TableInfo {
                id: format!("extra{k}"),
                name: format!("Extra Table {k}"),
                author: "Someone".into(),
                version: "0.1".into(),
                players_max: 2,
                features: vec![TableFeature::WasmRules, TableFeature::LcdDisplay],
                preview: None,
                preview_error: None,
                warning_count: 0,
            }),
        });
    }
    v
}

#[test]
fn write_review_images() {
    let Ok(dir) = std::env::var("PIN_UI_SNAPSHOT_DIR") else {
        return;
    };
    let Some(gpu) = pin_render::gpu::context_for_tests("ui_snapshots") else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    for (size, ppp, tag) in [
        ([1024, 768], 1.0, "1024x768"),
        ([1080, 1920], 1.5, "portrait"),
    ] {
        for panel in Panel::ALL {
            let mut m = MenuState::default();
            m.set_tables(entries());
            m.apply(MenuInput::Toggle);
            m.apply(MenuInput::ShowPanel(panel));
            m.apply(MenuInput::Down);
            m.monitor_mut()
                .set_button(0, pin_core::input::CabinetInput::LeftFlipper, true);
            let ctx = egui::Context::default();
            overlay_style(&ctx);
            let mut view = MenuView::new();
            let mut delta = egui::TexturesDelta::default();
            let mut last = None;
            for f in 0..3 {
                let raw = raw_input(size, ppp, Vec::new(), Some(f as f64 / 60.0));
                let (mut out, _) = view.run_frame(&ctx, raw, &mut m);
                delta.append(std::mem::take(&mut out.textures_delta));
                if let Some(prev) = last.replace(out) {
                    let prev: egui::FullOutput = prev;
                    prev.drop_without_applying_deltas();
                }
            }
            let mut out = last.unwrap();
            out.textures_delta = delta;
            let frame = OverlayFrame::new(&ctx, out);
            let px =
                paint_offscreen(&gpu, size[0], size[1], [0.92, 0.86, 0.55, 1.0], frame).unwrap();
            let img = image::RgbaImage::from_raw(size[0], size[1], px).unwrap();
            let path = format!("{dir}/{}_{tag}.png", panel.label().to_lowercase());
            img.save(&path).unwrap();
            println!("wrote {path}");
        }
    }
}
