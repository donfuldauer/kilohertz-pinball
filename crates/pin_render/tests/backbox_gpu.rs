//! Subphase 5.5 GPU tests (`gpu-tests`, lavapipe in CI): the backbox scene
//! rendered with its own camera and composited between translite layers.
//! D30: analytic pixel assertions first (region means ordered by depth,
//! reel glyph cells against the 5 × 7 font, a one-digit turn changing only
//! that reel, the mini-playfield, bat and ball where the camera projects
//! them), then one PNG regression snapshot in `tests/fixtures/backbox/`
//! (tolerance as the displays: mean |Δ| ≤ 3/255 and ≤ 2 % of channels off by
//! more than 16/255; `PIN_UPDATE_SNAPSHOTS=1` rewrites it). Every image is
//! also written to `$CARGO_TARGET_TMPDIR/5_5_images/` for the visual
//! checklist.
#![cfg(feature = "gpu-tests")]

use pin_assetgen::TableMeta;
use pin_assetgen::art::{self, ColorType, Image};
use pin_core::LampId;
use pin_render::backbox::*;
use pin_render::displays::*;
use pin_render::gpu::{GpuContext, context_for_tests};
use pin_render::pbr::MeshGeometry;
use pin_render::pbr::math::{IDENTITY, transform_point};
use std::sync::OnceLock;

/// One device per test process, shared by every test (lavapipe crashes
/// with a device per test thread; see displays_gpu.rs).
fn ctx(name: &str) -> Option<&'static GpuContext> {
    static CTX: OnceLock<Option<GpuContext>> = OnceLock::new();
    let c = CTX.get_or_init(|| context_for_tests(name)).as_ref();
    if c.is_none() {
        println!("[{name}] SKIP: no GPU adapter");
    }
    c
}

fn out_dir() -> std::path::PathBuf {
    let d = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("5_5_images");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn save(name: &str, img: &RgbaImage) {
    image::save_buffer(
        out_dir().join(format!("{name}.png")),
        &img.pixels,
        img.width,
        img.height,
        image::ColorType::Rgba8,
    )
    .unwrap();
}

fn snapshot(name: &str, img: &RgbaImage) {
    save(name, img);
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/backbox")
        .join(format!("{name}.png"));
    if std::env::var("PIN_UPDATE_SNAPSHOTS").is_ok_and(|v| v.trim() == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        image::save_buffer(
            &path,
            &img.pixels,
            img.width,
            img.height,
            image::ColorType::Rgba8,
        )
        .unwrap();
        return;
    }
    let want = image::open(&path)
        .unwrap_or_else(|e| {
            panic!(
                "[{name}] missing snapshot {} ({e}); run with PIN_UPDATE_SNAPSHOTS=1 after the analytic checks pass",
                path.display()
            )
        })
        .to_rgba8();
    assert_eq!((want.width(), want.height()), (img.width, img.height));
    let (mut sum, mut big) = (0u64, 0usize);
    for (a, b) in want.as_raw().iter().zip(&img.pixels) {
        let d = a.abs_diff(*b);
        sum += u64::from(d);
        big += usize::from(d > 16);
    }
    let n = img.pixels.len();
    let mean = sum as f64 / n as f64;
    assert!(
        mean <= 3.0 && big as f64 <= 0.02 * n as f64,
        "[{name}] snapshot differs: mean {mean:.2}, {big} channels > 16"
    );
}

/// Mean RGBA (0..=255) of the `2r+1` square around `(x, y)`.
fn mean(img: &RgbaImage, x: f32, y: f32, r: i32) -> [f32; 4] {
    let (cx, cy) = (x.floor() as i32, y.floor() as i32);
    let mut acc = [0.0f32; 4];
    let mut n = 0.0f32;
    for dy in -r..=r {
        for dx in -r..=r {
            let (px, py) = (cx + dx, cy + dy);
            if px < 0 || py < 0 || px >= img.width as i32 || py >= img.height as i32 {
                continue;
            }
            let p = img.pixel(px as u32, py as u32);
            for k in 0..4 {
                acc[k] += f32::from(p[k]);
            }
            n += 1.0;
        }
    }
    acc.map(|v| v / n.max(1.0))
}

fn luma(p: [f32; 4]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

fn near(p: [f32; 4], want: [u8; 3], tol: f32) -> bool {
    (0..3).all(|k| (p[k] - f32::from(want[k])).abs() <= tol)
}

fn solid(w: u32, h: u32, c: [u8; 4]) -> RgbaImage {
    RgbaImage::from_rgba(w, h, c.repeat((w * h) as usize)).unwrap()
}

/// A `w × h` image, transparent except `c` in `x0..x1 × y0..y1`.
fn rect_image(w: u32, h: u32, (x0, y0, x1, y1): (u32, u32, u32, u32), c: [u8; 4]) -> RgbaImage {
    let mut img = RgbaImage::new(w, h);
    for y in y0..y1 {
        for x in x0..x1 {
            let i = ((y * w + x) * 4) as usize;
            img.pixels[i..i + 4].copy_from_slice(&c);
        }
    }
    img
}

fn kit_image(img: Image) -> RgbaImage {
    match img.color {
        ColorType::Gray => RgbaImage::from_gray(img.width, img.height, img.data),
        ColorType::Rgb => RgbaImage::from_rgb(img.width, img.height, img.data),
        ColorType::Rgba => RgbaImage::from_rgba(img.width, img.height, img.data),
    }
    .unwrap()
}

/// A flat quad in the plane `y = 0` facing `−y`, `x0..x1 × z0..z1`.
fn wall(name: &str, (x0, x1, z0, z1): (f32, f32, f32, f32), color: [f32; 4]) -> BackboxMesh {
    BackboxMesh {
        name: name.into(),
        geometry: MeshGeometry {
            positions: vec![[x0, 0.0, z0], [x1, 0.0, z0], [x1, 0.0, z1], [x0, 0.0, z1]],
            normals: vec![[0.0, -1.0, 0.0]; 4],
            indices: vec![0, 1, 2, 0, 2, 3],
        },
        color,
        transform: IDENTITY,
    }
}

const BLUE: [u8; 4] = [20, 40, 230, 255];
const RED: [u8; 4] = [230, 30, 20, 255];
const GREEN: [f32; 4] = [0.1, 0.85, 0.2, 1.0];

/// Base (blue, depth 0) and logo (red square, depth 1) translite; a green
/// backbox wall covering the left of the view (camera 1 m in front, 90°).
fn depth_rig(ctx: &GpuContext) -> (BackglassDesc, BackboxRenderer) {
    let (w, h) = (160, 120);
    let desc = BackglassDesc {
        layers: vec![
            BackglassLayer {
                image: solid(w, h, BLUE),
                lamp: None,
            },
            BackglassLayer {
                image: rect_image(w, h, (60, 40, 110, 80), RED),
                lamp: None,
            },
        ],
    };
    let scene = BackboxScene {
        meshes: vec![wall("wall", (-2.0, 0.2, -2.0, 2.0), GREEN)],
    };
    let view = BackboxView {
        eye: [0.0, -1.0, 0.0],
        target: [0.0, 0.0, 0.0],
        fov_y: 90f32.to_radians(),
    };
    let mut bb = BackboxRenderer::new(ctx, wgpu::TextureFormat::Rgba8Unorm, &scene, view).unwrap();
    bb.set_lighting(BackboxLighting::UNLIT);
    (desc, bb)
}

#[test]
fn translite_layers_are_ordered_around_the_backbox_by_depth() {
    let Some(ctx) = ctx("translite_layers_are_ordered_around_the_backbox_by_depth") else {
        return;
    };
    let (desc, mut bb) = depth_rig(ctx);
    let (w, h) = (160, 120);
    // Where the green wall ends on screen.
    let edge = bb.view().project(w, h, [0.2, 0.0, 0.0]).unwrap()[0];
    assert!((edge - 92.0).abs() < 1.0, "{edge}");
    let green = [26u8, 217, 51];
    // Probe points: backbox only, nothing, logo over/under backbox, logo only.
    let (bb_only, none, overlap, logo_only) =
        ((20.0, 20.0), (140.0, 20.0), (75.0, 60.0), (102.0, 60.0));
    for (behind, name) in [(1usize, "plane_0_5"), (2, "plane_1_5"), (0, "plane_neg")] {
        let img =
            render_backglass_with_backbox_to_image(ctx, &desc, behind, &mut bb, &[], w, h).unwrap();
        save(&format!("backbox_depth_{name}"), &img);
        let at = |p: (f32, f32)| mean(&img, p.0, p.1, 2);
        match behind {
            1 => {
                // Base behind, logo over: base < backbox < logo.
                assert!(near(at(bb_only), green, 3.0), "{:?}", at(bb_only));
                assert!(near(at(none), [20, 40, 230], 1.0));
                assert!(near(at(overlap), [230, 30, 20], 1.0), "logo over backbox");
                assert!(near(at(logo_only), [230, 30, 20], 1.0));
            }
            2 => {
                assert!(near(at(overlap), green, 3.0), "backbox over the logo");
                assert!(near(at(logo_only), [230, 30, 20], 1.0));
                assert!(near(at(none), [20, 40, 230], 1.0));
            }
            _ => {
                // Behind everything: the opaque base hides it.
                assert!(near(at(bb_only), [20, 40, 230], 1.0));
                assert!(near(at(overlap), [230, 30, 20], 1.0));
            }
        }
    }
    assert_eq!(layers_behind(&[0, 1], DEFAULT_PLANE_DEPTH), 1);
}

#[test]
fn an_empty_backbox_leaves_the_backglass_byte_identical() {
    let Some(ctx) = ctx("an_empty_backbox_leaves_the_backglass_byte_identical") else {
        return;
    };
    let meta = TableMeta {
        title: "Apex Circuit".into(),
        palette: vec![],
        seed: 7,
    };
    let desc = BackglassDesc {
        layers: vec![
            BackglassLayer {
                image: kit_image(art::translite_base(&meta)),
                lamp: None,
            },
            BackglassLayer {
                image: kit_image(art::translite_logo(&meta)),
                lamp: Some(LayerLamp {
                    lamp: LampId(0),
                    mask: kit_image(art::translite_logo_mask(&meta)),
                    blend: LayerBlend::Over,
                }),
            },
        ],
    };
    let view = BackboxView {
        eye: [0.0, -1.0, 0.0],
        target: [0.0; 3],
        fov_y: 1.0,
    };
    let mut empty = BackboxRenderer::new(
        ctx,
        wgpu::TextureFormat::Rgba8Unorm,
        &BackboxScene::default(),
        view,
    )
    .unwrap();
    for (w, h) in [(512, 384), (300, 300)] {
        let plain = render_backglass_to_image(ctx, &desc, &[0.6], w, h).unwrap();
        for behind in 0..=2 {
            let split = render_backglass_with_backbox_to_image(
                ctx,
                &desc,
                behind,
                &mut empty,
                &[0.6],
                w,
                h,
            )
            .unwrap();
            assert!(split == plain, "{w}x{h}, split at {behind}: bytes differ");
        }
    }
}

/// Three reels (hundreds, tens, units) side by side, seen square-on.
fn reel_rig(ctx: &GpuContext) -> (BackboxRenderer, ToyRig, [ReelSpec; 3]) {
    let specs = [-0.04f32, 0.0, 0.04].map(|x| ReelSpec {
        center: [x, 0.0, 0.0],
        face_height: 0.03,
        width: 0.03,
    });
    let mut scene = BackboxScene::default();
    let mut rig = ToyRig::new();
    for (i, s) in specs.iter().enumerate() {
        let name = format!("reels[{i}]");
        scene.extend(reel_scene(&name, s));
        rig.add_reel(ScoreReel {
            meshes: vec![name.clone(), format!("{name}.digits")],
            player: 0,
            place: 2 - i as u32,
            axis: [1.0, 0.0, 0.0],
            pivot: s.center,
            settle_deg_per_s: 720.0,
        });
    }
    let view = BackboxView {
        eye: [0.0, -0.5, 0.0],
        target: [0.0; 3],
        fov_y: 12f32.to_radians(),
    };
    let mut bb = BackboxRenderer::new(ctx, wgpu::TextureFormat::Rgba8Unorm, &scene, view).unwrap();
    bb.set_lighting(BackboxLighting::UNLIT);
    (bb, rig, specs)
}

#[test]
fn backbox_reels_show_the_score_digits() {
    let Some(ctx) = ctx("backbox_reels_show_the_score_digits") else {
        return;
    };
    let (mut bb, mut rig, specs) = reel_rig(ctx);
    let (w, h) = (320u32, 240u32);
    let font = pin_assetgen::fonts::FONT_5X7;
    for score in [307u64, 958, 41] {
        rig.set_scores(&[score]);
        rig.snap_reels();
        bb.apply_toys(&rig);
        let img = bb.render_scene_to_image(ctx, w, h).unwrap();
        save(&format!("backbox_reels_{score}"), &img);
        for (i, s) in specs.iter().enumerate() {
            let d = score_digit(score, 2 - i as u32);
            let m = rig.reel_transform(i);
            let glyph = font.glyph_or_fallback(char::from(b'0' + d as u8));
            let mut matched = 0;
            for row in 0..7 {
                for col in 0..5 {
                    let c = transform_point(&m, s.cell_center(d, col, row));
                    let p = bb.view().project(w, h, c).unwrap();
                    let l = luma(mean(&img, p[0], p[1], 1));
                    let lit = glyph.pixel(col, row);
                    let ok = if lit { l < 60.0 } else { l > 180.0 };
                    matched += usize::from(ok);
                    assert!(
                        ok,
                        "score {score}, reel {i} digit {d}, cell ({col},{row}): luma {l}"
                    );
                }
            }
            assert_eq!(matched, 35);
        }
    }
}

#[test]
fn backbox_turning_one_reel_by_a_digit_changes_only_its_region() {
    let Some(ctx) = ctx("backbox_turning_one_reel_by_a_digit_changes_only_its_region") else {
        return;
    };
    let (mut bb, mut rig, specs) = reel_rig(ctx);
    let (w, h) = (320u32, 240u32);
    rig.set_scores(&[307]);
    rig.snap_reels();
    bb.apply_toys(&rig);
    let before = bb.render_scene_to_image(ctx, w, h).unwrap();
    // 307 → 317: only the tens reel turns, one digit (36°), with the settle
    // animation in simulated time.
    rig.set_scores(&[317]);
    let mut ticks = 0;
    while !rig.settled() {
        rig.advance(0.001);
        ticks += 1;
    }
    // 36° at 720°/s: 50 ms (one more tick for f32 rounding of the steps).
    assert!((50..=51).contains(&ticks), "{ticks} ticks");
    bb.apply_toys(&rig);
    let after = bb.render_scene_to_image(ctx, w, h).unwrap();
    save("backbox_reels_317", &after);
    // Screen columns of each reel (drum width plus a margin).
    let cols: Vec<(u32, u32)> = specs
        .iter()
        .map(|s| {
            let l = bb
                .view()
                .project(w, h, [s.center[0] - 0.017, -0.03, 0.0])
                .unwrap()[0];
            let r = bb
                .view()
                .project(w, h, [s.center[0] + 0.017, -0.03, 0.0])
                .unwrap()[0];
            (l.floor().max(0.0) as u32, (r.ceil() as u32).min(w))
        })
        .collect();
    let mut changed = [0usize; 3];
    let mut outside = 0usize;
    for y in 0..h {
        for x in 0..w {
            if before.pixel(x, y) != after.pixel(x, y) {
                match cols.iter().position(|&(a, b)| x >= a && x < b) {
                    Some(i) => changed[i] += 1,
                    None => outside += 1,
                }
            }
        }
    }
    assert_eq!(outside, 0);
    assert_eq!(changed[0], 0, "hundreds reel unchanged");
    assert_eq!(changed[2], 0, "units reel unchanged");
    assert!(changed[1] > 200, "tens reel turned ({} pixels)", changed[1]);
}

/// The generated mini-playfield (pin_tablegen) with two flippers, loaded
/// through the runtime glTF loader.
fn mini_playfield() -> BackboxScene {
    use pin_core::schema::table::{CoilDef, CoilKind, SwitchDef, SwitchKind, TableManifest};
    let mut base =
        TableManifest::from_toml_str(include_str!("../../pin_core/fixtures/table.toml")).unwrap();
    for (i, id) in ["bbl", "bbr", "bbv"].into_iter().enumerate() {
        base.coils.push(CoilDef {
            id: id.into(),
            number: 70 + i as u16,
            kind: CoilKind::Flipper,
        });
        base.switches.push(SwitchDef {
            id: format!("{id}_sw"),
            number: 210 + i as u16,
            kind: SwitchKind::Leaf,
            layer: 0,
            position: None,
            radius: None,
        });
    }
    let layout = pin_tablegen::backbox::parse_backbox_layout(
        r#"
schema_version = 1
[backbox]
origin = [0.0, 1150.0, 250.0]
width = 400.0
height = 300.0
depth = 35.0
[[backbox.flippers]]
side = "left"
position = [-75.0, 45.0]
coil = "bbl"
eos_switch = "bbl_sw"
staging = { type = "DedicatedButton", input = "left_upper" }
[[backbox.flippers]]
side = "right"
position = [75.0, 45.0]
coil = "bbr"
eos_switch = "bbr_sw"
staging = { type = "DedicatedButton", input = "right_upper" }
[backbox.entry]
vuk = [120.0, 1000.0]
at = -150.0
coil = "bbv"
switch = "bbv_sw"
[backbox.drain]
at = 160.0
"#,
    )
    .unwrap();
    let g = pin_tablegen::backbox::generate_backbox(&layout.backbox, &base).unwrap();
    static FIXTURE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = FIXTURE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("pin_render_backbox_{}_{seq}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("backbox.glb"), &g.scene_glb).unwrap();
    let vfs = pin_assets::Vfs::open(&dir).unwrap();
    let scene = pin_assets::load_gltf(&vfs, "backbox.glb").unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    BackboxScene::from_gltf(&scene)
}

/// The backbox camera of the tests: in front of the glass, looking at the
/// middle of the box.
fn mini_view() -> BackboxView {
    BackboxView::from_camera(&pin_core::schema::backglass::BackboxCamera {
        position: [0.0, -700.0, 200.0],
        look_at: [0.0, 0.0, 200.0],
        fov_deg: 35.0,
    })
}

const BALL_AT: [f32; 3] = [0.03, 0.035 - 0.0135, 0.16];

#[test]
fn backbox_mini_playfield_bat_and_ball_are_visible_with_their_own_perspective() {
    let Some(ctx) =
        ctx("backbox_mini_playfield_bat_and_ball_are_visible_with_their_own_perspective")
    else {
        return;
    };
    let scene = mini_playfield();
    let view = mini_view();
    let mut bb = BackboxRenderer::new(ctx, wgpu::TextureFormat::Rgba8Unorm, &scene, view).unwrap();
    let (w, h) = (320u32, 240u32);
    let empty = bb.render_scene_to_image(ctx, w, h).unwrap();
    bb.set_balls(&[BALL_AT]);
    let img = bb.render_scene_to_image(ctx, w, h).unwrap();
    save("backbox_mini_playfield", &img);
    let px = |p: [f32; 3]| view.project(w, h, p).unwrap();
    // Back panel: wood (r > g > b), opaque.
    let back = mean(
        &img,
        px([-0.15, 0.035, 0.22])[0],
        px([-0.15, 0.035, 0.22])[1],
        2,
    );
    assert!(
        back[3] == 255.0 && back[0] > back[1] && back[1] > back[2],
        "{back:?}"
    );
    // Above the roof: nothing drawn (transparent).
    let sky = px([0.0, 0.0, 0.33]);
    assert_eq!(mean(&img, sky[0], sky[1], 2)[3], 0.0);
    // Perspective: the back panel's top edge (farther) is lower on screen
    // than the roof's front edge, and both above the shelf.
    let back_top = px([0.0, 0.035, 0.3])[1];
    let front_top = px([0.0, 0.0, 0.3])[1];
    assert!(front_top < back_top, "{front_top} < {back_top}");
    // Left bat at rest: halfway along it (pivot (−75, 45) mm, −30°, 50 mm)
    // is the light bat colour, brighter than the panel.
    let a = (-30f32).to_radians();
    let mid = [
        -0.075 + 0.025 * a.cos(),
        0.035 - 0.022,
        0.045 + 0.025 * a.sin(),
    ];
    let bat = mean(&img, px(mid)[0], px(mid)[1], 1);
    assert!(
        luma(bat) > luma(back) + 40.0,
        "bat {bat:?} vs panel {back:?}"
    );
    // The ball: drawn where projected, absent from the empty frame.
    let b = px(BALL_AT);
    let with = mean(&img, b[0], b[1], 1);
    let without = mean(&empty, b[0], b[1], 1);
    assert!(near(with, [204, 204, 214], 60.0), "ball colour {with:?}");
    assert!((luma(with) - luma(without)).abs() > 10.0);
}

#[test]
fn backbox_golden_reels_and_mini_playfield_composited_with_the_translite() {
    let Some(ctx) = ctx("backbox_golden_reels_and_mini_playfield_composited_with_the_translite")
    else {
        return;
    };
    let meta = TableMeta {
        title: "Apex Circuit".into(),
        palette: vec![],
        seed: 7,
    };
    let base = kit_image(art::translite_base(&meta));
    let (w, h) = (base.width, base.height);
    // A frame layer (depth 2): an opaque band around the window.
    let band = 14;
    let mut frame = solid(w, h, [24, 18, 40, 255]);
    for y in band..h - band {
        for x in band..w - band {
            let i = ((y * w + x) * 4) as usize;
            frame.pixels[i..i + 4].copy_from_slice(&[0, 0, 0, 0]);
        }
    }
    let desc = BackglassDesc {
        layers: vec![
            BackglassLayer {
                image: base,
                lamp: None,
            },
            BackglassLayer {
                image: kit_image(art::translite_logo(&meta)),
                lamp: Some(LayerLamp {
                    lamp: LampId(0),
                    mask: kit_image(art::translite_logo_mask(&meta)),
                    blend: LayerBlend::Over,
                }),
            },
            BackglassLayer {
                image: frame,
                lamp: None,
            },
        ],
    };
    let mut scene = mini_playfield();
    let reel_specs = [-0.04f32, 0.0, 0.04].map(|x| ReelSpec {
        center: [x, 0.02, 0.37],
        face_height: 0.03,
        width: 0.03,
    });
    let mut rig = ToyRig::new();
    for (i, s) in reel_specs.iter().enumerate() {
        let name = format!("reels[{i}]");
        scene.extend(reel_scene(&name, s));
        rig.add_reel(ScoreReel {
            meshes: vec![name.clone(), format!("{name}.digits")],
            player: 0,
            place: 2 - i as u32,
            axis: [1.0, 0.0, 0.0],
            pivot: s.center,
            settle_deg_per_s: 360.0,
        });
    }
    rig.set_scores(&[385]);
    rig.snap_reels();
    let view = BackboxView::from_camera(&pin_core::schema::backglass::BackboxCamera {
        position: [0.0, -800.0, 210.0],
        look_at: [0.0, 0.0, 210.0],
        fov_deg: 38.0,
    });
    let mut bb = BackboxRenderer::new(ctx, wgpu::TextureFormat::Rgba8Unorm, &scene, view).unwrap();
    bb.apply_toys(&rig);
    // Left flipper raised to the end of its stroke (+50° about the normal
    // towards the glass, −y), from simulation state.
    let raised = rotation_about([0.0, -1.0, 0.0], [-0.075, 0.035, 0.045], 50f32.to_radians());
    assert_eq!(bb.set_dynamic_by_name("backbox.flippers[0]", raised), 1);
    bb.set_balls(&[BALL_AT]);
    // The base (depth 0) behind; the logo (1) and the frame (2) over.
    let behind = layers_behind(&[0, 1, 2], DEFAULT_PLANE_DEPTH);
    assert_eq!(behind, 1);
    let img =
        render_backglass_with_backbox_to_image(ctx, &desc, behind, &mut bb, &[1.0], w, h).unwrap();
    // Analytic checks first (D30).
    // The frame band is exactly the frame colour: drawn over the backbox.
    for (x, y) in [(3, 3), (w / 2, 5), (w - 4, h / 2), (w / 2, h - 4)] {
        let p = img.pixel(x, y);
        assert_eq!(&p[..3], &[24, 18, 40], "frame at ({x},{y})");
    }
    // Inside the window the backbox shows: the reels' drums are bright, a
    // reel's digit cell is dark.
    let px = |p: [f32; 3]| view.project(w, h, p).unwrap();
    let front = 0.02 - reel_specs[2].apothem();
    let drum = px([0.04 - 0.012, front, 0.37 + 0.012]);
    assert!(luma(mean(&img, drum[0], drum[1], 1)) > 170.0, "drum");
    let m = rig.reel_transform(2);
    let digit5_cell = transform_point(&m, reel_specs[2].cell_center(5, 0, 0));
    let c = px(digit5_cell);
    assert!(
        luma(mean(&img, c[0], c[1], 0)) < 90.0,
        "digit 5 top-left cell"
    );
    // Without a backbox the same pixel shows the translite only.
    let plain = render_backglass_to_image(ctx, &desc, &[1.0], w, h).unwrap();
    let ball = px(BALL_AT);
    assert_ne!(
        mean(&plain, ball[0], ball[1], 1),
        mean(&img, ball[0], ball[1], 1),
        "the backbox is visible through the translite"
    );
    save("backbox_translite_only", &plain);
    snapshot("backbox_composite", &img);
}
