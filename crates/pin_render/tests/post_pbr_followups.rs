//! 5.1 follow-ups assigned to 5.2 and the 5.2 PBR hooks, on a real adapter
//! (gpu-tests; skips without an adapter unless `PIN_REQUIRE_GPU=1`).
//!
//! * N2: white-furnace test of the grazing-angle albedo. A plane is lit by
//!   4096 distant lights spread evenly over the upper hemisphere, which
//!   together approximate a uniform sky of radiance 1; a pixel then shows
//!   the directional albedo of the material at its view angle. Measured as
//!   a ratio against a pure Lambert white plane (albedo 1) rendered the
//!   same way, at grazing view angles.
//! * Acrylic: the two-pass transparent draw (back faces, then front faces).
//! * Hooks: MSAA 4×, external targets, reflection bindings.
#![cfg(feature = "gpu-tests")]

use pin_render::gpu::{GpuContext, context_for_tests};
use pin_render::pbr::math::{IDENTITY, scale_translation};
use pin_render::pbr::{
    DEPTH_FORMAT, DrawItem, Environment, ExternalTarget, FrameDesc, HDR_FORMAT, HdrImage,
    MaterialKind, MaterialParams, MeshGeometry, NO_PROBE, PbrCamera, PbrRenderer, PointLight,
    ReflectionBinding,
};
use std::sync::OnceLock;

fn ctx() -> Option<&'static GpuContext> {
    static CTX: OnceLock<Option<GpuContext>> = OnceLock::new();
    CTX.get_or_init(|| context_for_tests("post_pbr_followups"))
        .as_ref()
}

const N: u32 = 128;

fn norm(a: [f32; 3]) -> [f32; 3] {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    [a[0] / l, a[1] / l, a[2] / l]
}

/// World direction through the centre of pixel (x, y).
fn pixel_ray(cam: &PbrCamera, x: u32, y: u32, w: u32, h: u32) -> [f32; 3] {
    let t = (0.5 * cam.fov_y).tan();
    let nx = (2.0 * (x as f32 + 0.5) / w as f32 - 1.0) * t * cam.aspect;
    let ny = (1.0 - 2.0 * (y as f32 + 0.5) / h as f32) * t;
    let v = &cam.view;
    let s = [v[0][0], v[1][0], v[2][0]];
    let u = [v[0][1], v[1][1], v[2][1]];
    let f = [-v[0][2], -v[1][2], -v[2][2]];
    norm([
        s[0] * nx + u[0] * ny + f[0],
        s[1] * nx + u[1] * ny + f[1],
        s[2] * nx + u[2] * ny + f[2],
    ])
}

/// `count` directions evenly over the upper hemisphere (Fibonacci).
fn hemisphere(count: usize) -> Vec<[f32; 3]> {
    let golden = core::f64::consts::PI * (3.0 - 5f64.sqrt());
    (0..count)
        .map(|i| {
            let z = 1.0 - (i as f64 + 0.5) / count as f64;
            let r = (1.0 - z * z).sqrt();
            let phi = golden * i as f64;
            [(r * phi.cos()) as f32, (r * phi.sin()) as f32, z as f32]
        })
        .collect()
}

/// Sum over light batches of the plane image under a uniform sky of
/// radiance 1 (camera close to the plane, looking along it).
fn furnace(ctx: &GpuContext, material: MaterialParams) -> (HdrImage, PbrCamera) {
    let mut r = PbrRenderer::new(ctx, N, N).unwrap();
    let quad = r.upload_mesh(&MeshGeometry::quad(1.0)).unwrap();
    let cam = PbrCamera::look_at(
        [0.0, -2.0, 0.12],
        [0.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        40f32.to_radians(),
        1.0,
        0.02,
        50.0,
    );
    let draw = DrawItem {
        mesh: quad,
        model: scale_translation(8.0, [0.0, 0.0, 0.0]),
        material,
        transparent: false,
        emission: 0.0,
    };
    const COUNT: usize = 4096;
    const DIST: f32 = 400.0;
    let omega = 2.0 * core::f32::consts::PI / COUNT as f32;
    let dirs = hemisphere(COUNT);
    let mut sum = vec![[0.0f64; 4]; (N * N) as usize];
    for batch in dirs.chunks(128) {
        let lights: Vec<PointLight> = batch
            .iter()
            .map(|d| {
                PointLight::new(
                    [d[0] * DIST, d[1] * DIST, d[2] * DIST],
                    [1.0; 3],
                    omega * DIST * DIST,
                    1.0e5,
                )
            })
            .collect();
        let draws = [draw];
        let mut frame = FrameDesc::new(&cam, &lights, &draws);
        frame.environment = Environment::BLACK;
        frame.gi = [0.0; 4];
        let img = r.render_to_image(ctx, &frame).unwrap();
        for (s, p) in sum.iter_mut().zip(&img.pixels) {
            for k in 0..4 {
                s[k] += f64::from(p[k]);
            }
        }
    }
    let img = HdrImage {
        width: N,
        height: N,
        pixels: sum
            .iter()
            .map(|s| [s[0] as f32, s[1] as f32, s[2] as f32, 1.0])
            .collect(),
    };
    (img, cam)
}

/// Mean ratio material / Lambert over plane pixels with n·v in `lo..hi`
/// (luminance; the plane normal is +z, so n·v = −ray.z).
fn band_ratio(m: &HdrImage, lambert: &HdrImage, cam: &PbrCamera, lo: f32, hi: f32) -> (f64, usize) {
    let lum = |p: [f32; 4]| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
    let (mut a, mut b, mut n) = (0.0f64, 0.0f64, 0usize);
    for y in 0..N {
        for x in 0..N {
            let d = pixel_ray(cam, x, y, N, N);
            let nv = -d[2];
            // Plane hit inside the 8 m quad, away from its far edge.
            if nv <= 0.0 {
                continue;
            }
            let t = cam.eye[2] / nv;
            let hit = [cam.eye[0] + t * d[0], cam.eye[1] + t * d[1]];
            if hit[0].abs() > 3.5 || hit[1].abs() > 3.5 || !(lo..hi).contains(&nv) {
                continue;
            }
            a += f64::from(lum(m.at(x, y)));
            b += f64::from(lum(lambert.at(x, y)));
            n += 1;
        }
    }
    (a / b.max(1e-12), n)
}

#[test]
fn grazing_albedo_stays_below_one_n2() {
    let Some(ctx) = ctx() else { return };
    let (lambert, cam) = furnace(ctx, MaterialParams::lambert([1.0; 3]));
    // Sanity: the Lambert plane shows albedo 1 (the sky is uniform).
    let lum = |p: [f32; 4]| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
    let (mut s, mut n) = (0.0f64, 0);
    for y in N / 2 + 4..N {
        for x in 0..N {
            s += f64::from(lum(lambert.at(x, y)));
            n += 1;
        }
    }
    let lambert_mean = s / f64::from(n);
    println!("lambert furnace mean {lambert_mean:.4}");
    assert!(
        (lambert_mean - 1.0).abs() < 0.03,
        "Lambert furnace {lambert_mean}"
    );

    let white_dielectric = MaterialParams {
        base_color: [1.0, 1.0, 1.0, 1.0],
        roughness: 0.1,
        ..MaterialParams::DEFAULT
    };
    let cases = [
        ("white dielectric r0.1", white_dielectric),
        ("ceramic pearl", MaterialKind::CeramicPearl.params()),
        ("playfield wood", MaterialKind::PlayfieldWood.params()),
    ];
    for (name, material) in cases {
        let (img, _) = furnace(ctx, material);
        for (lo, hi) in [(0.02f32, 0.08f32), (0.08, 0.2), (0.2, 0.5)] {
            let (ratio, px) = band_ratio(&img, &lambert, &cam, lo, hi);
            println!("{name}: n.v in {lo}..{hi}: albedo {ratio:.3} over {px} px");
            assert!(px >= 100, "{name}: only {px} px in band {lo}..{hi}");
            // Energy conservation within the light-sampling noise of the
            // narrow specular lobes (before the fix: 1.14 .. 1.72).
            assert!(
                ratio <= 1.06,
                "{name}: grazing albedo {ratio:.3} > 1 at n.v {lo}..{hi}"
            );
        }
    }
}

fn sphere_scene(ctx: &GpuContext) -> (PbrRenderer, pin_render::pbr::MeshId, PbrCamera) {
    let mut r = PbrRenderer::new(ctx, N, N).unwrap();
    let sphere = r
        .upload_mesh(&MeshGeometry::uv_sphere(0.5, 48, 32))
        .unwrap();
    let cam = PbrCamera::look_at(
        [0.0, -3.0, 0.0],
        [0.0; 3],
        [0.0, 0.0, 1.0],
        30f32.to_radians(),
        1.0,
        0.1,
        20.0,
    );
    (r, sphere, cam)
}

/// Acrylic in front of a red card: its back faces are drawn before its
/// front faces, so the result does not depend on the triangle order.
#[test]
fn acrylic_two_pass_is_independent_of_triangle_order() {
    let Some(ctx) = ctx() else { return };
    let (mut r, sphere, cam) = sphere_scene(ctx);
    let mut reversed = MeshGeometry::uv_sphere(0.5, 48, 32);
    let tris: Vec<[u32; 3]> = reversed
        .indices
        .chunks(3)
        .rev()
        .map(|t| [t[0], t[1], t[2]])
        .collect();
    reversed.indices = tris.concat();
    let sphere_rev = r.upload_mesh(&reversed).unwrap();
    let card = r.upload_mesh(&MeshGeometry::quad(1.0)).unwrap();
    let card_model = {
        // Quad facing −y (towards the camera), 1 m behind the sphere.
        let mut m = IDENTITY;
        m[0] = [2.0, 0.0, 0.0, 0.0];
        m[1] = [0.0, 0.0, -2.0, 0.0];
        m[2] = [0.0, 2.0, 0.0, 0.0];
        m[3] = [0.0, 1.0, 0.0, 1.0];
        m
    };
    let red = DrawItem {
        mesh: card,
        model: card_model,
        material: MaterialParams::lambert([0.8, 0.05, 0.05]),
        transparent: false,
        emission: 0.0,
    };
    let lights = [PointLight::new([-1.5, -2.5, 2.0], [1.0; 3], 12.0, 50.0)];
    let render = |r: &mut PbrRenderer, mesh| {
        let draws = [red, DrawItem::new(mesh, IDENTITY, MaterialKind::ClearAcrylic)];
        let frame = FrameDesc::new(&cam, &lights, &draws);
        r.render_to_image(ctx, &frame).unwrap()
    };
    let a = render(&mut r, sphere);
    let b = render(&mut r, sphere_rev);
    let mut max = 0.0f32;
    for (p, q) in a.pixels.iter().zip(&b.pixels) {
        for k in 0..3 {
            max = max.max((p[k] - q[k]).abs() / (1.0 + p[k].abs()));
        }
    }
    println!("acrylic: max relative difference between triangle orders {max}");
    assert!(max < 2e-3, "triangle order changes acrylic by {max}");
    // The red card shows through the centre.
    let c = a.region_mean(60, 60, 68, 68);
    assert!(c[0] > 2.0 * c[1], "centre {c:?} not red");
}

fn lum(p: [f32; 4]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

/// Mean |horizontal gradient| along the silhouette rows of a bright sphere
/// on black: aliasing shows as staircase steps of full contrast.
fn edge_steps(img: &HdrImage) -> (usize, f32) {
    let mut partial = 0usize;
    let mut total = 0.0f32;
    for p in &img.pixels {
        let l = lum(*p).clamp(0.0, 1.0);
        if l > 0.02 && l < 0.98 {
            partial += 1;
        }
        total += l;
    }
    (partial, total)
}

#[test]
fn msaa_hook_softens_edges_and_keeps_the_interior() {
    let Some(ctx) = ctx() else { return };
    let (mut r, sphere, cam) = sphere_scene(ctx);
    let mut white = MaterialParams::lambert([0.0; 3]);
    white.emissive = [1.0, 1.0, 1.0];
    let draws = [DrawItem {
        mesh: sphere,
        model: IDENTITY,
        material: white,
        transparent: false,
        emission: 0.0,
    }];
    let mut frame = FrameDesc::new(&cam, &[], &draws);
    frame.environment = Environment::BLACK;
    let one = r.render_to_image(ctx, &frame).unwrap();
    assert_eq!(r.msaa_samples(), 1);
    assert!(r.set_msaa_samples(3).is_err());
    r.set_msaa_samples(4).unwrap();
    assert_eq!(r.msaa_samples(), 4);
    let four = r.render_to_image(ctx, &frame).unwrap();
    let (p1, t1) = edge_steps(&one);
    let (p4, t4) = edge_steps(&four);
    println!("edge pixels with partial coverage: 1x {p1}, 4x {p4}; coverage {t1} vs {t4}");
    assert!(p1 < 10, "no MSAA should give hard edges ({p1})");
    assert!(p4 > 100, "MSAA 4x gives partial coverage at the edges ({p4})");
    // Same total coverage (area) within 1 %, same interior.
    assert!((t1 - t4).abs() < 0.01 * t1);
    assert_eq!(one.at(64, 64), four.at(64, 64));
    // Resize keeps MSAA working.
    r.resize(64, 48).unwrap();
    let small = r.render_to_image(ctx, &frame).unwrap();
    assert_eq!(small.pixels.len(), 64 * 48);
    r.set_msaa_samples(1).unwrap();
    let back = r.render_to_image(ctx, &frame).unwrap();
    assert_eq!(back.width, 64);
}

#[test]
fn external_targets_and_reflection_bindings() {
    let Some(ctx) = ctx() else { return };
    let (mut r, sphere, cam) = sphere_scene(ctx);
    let draws = [DrawItem::new(sphere, IDENTITY, MaterialKind::ChromeSteel)];
    let lights = [PointLight::new([-1.5, -2.5, 2.0], [1.0; 3], 12.0, 50.0)];
    let frame = FrameDesc::new(&cam, &lights, &draws);
    let plain = r.render_to_image(ctx, &frame).unwrap();
    // A binding whose layers are all NO_PROBE renders exactly like none.
    let device = ctx.device();
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 4,
            height: 4,
            depth_or_array_layers: 2,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: HDR_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: r.reflection_layout(),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });
    let none = ReflectionBinding {
        bind_group: &bind,
        draw_layers: &[NO_PROBE],
    };
    let same = r.render_reflected_to_image(ctx, &frame, Some(&none)).unwrap();
    assert_eq!(same.pixels, plain.pixels);
    // Layer 1 of an all-black atlas: the mirror term goes black, the
    // highlight from the light stays.
    let black = ReflectionBinding {
        bind_group: &bind,
        draw_layers: &[1],
    };
    let dark = r
        .render_reflected_to_image(ctx, &frame, Some(&black))
        .unwrap();
    let centre_plain = lum(plain.at(64, 64));
    let centre_dark = lum(dark.at(64, 64));
    println!("chrome centre: environment {centre_plain}, black probe {centre_dark}");
    assert!(centre_dark < 0.1 * centre_plain.max(1e-3));
    // An out-of-range layer is clamped to the last one (no GPU error).
    let far = ReflectionBinding {
        bind_group: &bind,
        draw_layers: &[1000],
    };
    let clamped = r.render_reflected_to_image(ctx, &frame, Some(&far)).unwrap();
    assert_eq!(clamped.pixels, dark.pixels);

    // External target: same image as the main target at the same size.
    let mk = |format, usage| {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: N,
                    height: N,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
    };
    let color = mk(
        HDR_FORMAT,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
    );
    let depth = mk(DEPTH_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT);
    let cv = color.create_view(&Default::default());
    let dv = depth.create_view(&Default::default());
    let mut enc = device.create_command_encoder(&Default::default());
    r.encode_to_target(
        &mut enc,
        &frame,
        &ExternalTarget {
            color: &cv,
            depth: &dv,
            width: N,
            height: N,
        },
    );
    ctx.queue().submit([enc.finish()]);
    // Read the external target back and compare with the main target.
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(N * N * 8),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &color,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(N * 8),
                rows_per_image: Some(N),
            },
        },
        color.size(),
    );
    ctx.queue().submit([enc.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    readback.map_async(wgpu::MapMode::Read, .., move |r| {
        let _ = tx.send(r);
    });
    pin_render::gpu::wait_for_device(device, std::time::Duration::from_secs(60)).unwrap();
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .unwrap()
        .unwrap();
    let bytes = readback.get_mapped_range(..).unwrap().to_vec();
    let half = |i: usize| {
        let h = u16::from_le_bytes([bytes[i], bytes[i + 1]]);
        // Decode via f32 bits (normal numbers only, enough for this check).
        let e = u32::from((h >> 10) & 0x1f);
        let f = u32::from(h & 0x3ff);
        if e == 0 {
            0.0
        } else {
            f32::from_bits(((e + 112) << 23) | (f << 13))
        }
    };
    for (x, y) in [(64u32, 64u32), (50, 70), (80, 40), (5, 5)] {
        let i = ((y * N + x) * 8) as usize;
        let want = plain.at(x, y);
        for k in 0..3 {
            let got = half(i + 2 * k);
            assert!(
                (got - want[k]).abs() <= 1e-3 * (1.0 + want[k]),
                "external target ({x},{y}) channel {k}: {got} vs {}",
                want[k]
            );
        }
    }
}
