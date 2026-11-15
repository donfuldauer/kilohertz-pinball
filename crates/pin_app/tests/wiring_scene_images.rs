//! GPU-free half of the LCD scene-image checks (PR #21 review): the bundle's
//! scene images are decoded into `TableVisuals`, and a missing one is a
//! warning. These run in the default `cargo test -p pin_app`; the rendered
//! pixels are checked in `wiring_render.rs` (feature `gpu-tests`).

mod support;

use pin_app::orchestrator::{TableVisuals, load_checked, validate_table};
use pin_core::snapshot::FrameStateSnapshot;
use pin_sim::SimConfig;
use support::on_big_stack;

/// The colour of the synthetic scene image (display-encoded RGB).
const SOLID: [u8; 3] = [40, 160, 220];
/// The LCD canvas of the synthetic table.
const LCD: (u32, u32) = (160, 90);

/// The grey-box bundle with an LCD score display whose attract scene is one
/// full-canvas `kind = "image"` layer over `display/solid.png` (written
/// only when `with_image`).
fn lcd_image_bundle(with_image: bool) -> std::path::PathBuf {
    let d = support::scratch(if with_image {
        "lcd_image_nogpu"
    } else {
        "lcd_no_image_nogpu"
    });
    support::build_greybox_bundle(&d);
    let bg = std::fs::read_to_string(d.join("backglass.toml")).unwrap();
    assert!(bg.contains("mode = \"dmd\""), "{bg}");
    let bg = bg.replace("mode = \"dmd\"", "mode = \"lcd\"").replace(
        "lcd = { width = 1920, height = 1080 }",
        &format!("lcd = {{ width = {}, height = {} }}", LCD.0, LCD.1),
    );
    std::fs::write(d.join("backglass.toml"), bg).unwrap();
    std::fs::write(
        d.join("display/scene_attract.toml"),
        "schema_version = 1\nbackground = \"#000000\"\n\n[[layers]]\nkind = \"image\"\n\
         file = \"display/solid.png\"\nx = 0.0\ny = 0.0\nw = 1.0\nh = 1.0\n",
    )
    .unwrap();
    if with_image {
        let img =
            image::RgbaImage::from_pixel(8, 8, image::Rgba([SOLID[0], SOLID[1], SOLID[2], 255]));
        img.save(d.join("display/solid.png")).unwrap();
    }
    d
}

fn lcd_table(with_image: bool) -> (TableVisuals, FrameStateSnapshot, pin_core::SceneId) {
    let dir = lcd_image_bundle(with_image);
    on_big_stack(move || {
        let checked = validate_table(&dir).unwrap();
        let sim = load_checked(&checked, SimConfig::default()).unwrap();
        let attract = checked.table.ids.scene("scene_attract").unwrap();
        (TableVisuals::of(&checked), *sim.snapshot(), attract)
    })
}

#[test]
fn lcd_scene_images_are_decoded_from_the_bundle() {
    let (v, _snap, _attract) = lcd_table(true);
    assert!(v.warnings.is_empty(), "{:?}", v.warnings);
    let d = v.displays.as_ref().expect("displays");
    assert!(matches!(
        d.score,
        pin_render::displays::ScoreDisplayConfig::Lcd(_)
    ));
    assert_eq!(d.scene_images.len(), 1, "{:?}", d.scene_images.len());
    assert_eq!(d.scene_images[0].0, "display/solid.png");
    assert_eq!(
        d.scene_images[0].1.pixel(3, 3),
        [SOLID[0], SOLID[1], SOLID[2], 255]
    );
}

#[test]
fn a_missing_scene_image_is_a_warning() {
    let (v, _snap, _attract) = lcd_table(false);
    assert!(
        v.warnings
            .iter()
            .any(|w| w.starts_with("scene image display/solid.png: ")),
        "{:?}",
        v.warnings
    );
    assert!(v.displays.as_ref().unwrap().scene_images.is_empty());
}
