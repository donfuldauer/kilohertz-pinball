// pbr_forward: clustered forward shading (vertex + opaque and blended
// fragment entry points). Concatenated after pbr_brdf.
//
// Frames: world = table frame (z up, metres); view space right-handed,
// camera looking down -Z. Output: linear HDR radiance (Rgba16Float target),
// clamped to MAX_RADIANCE; blended materials output premultiplied colour.

const MAX_LIGHTS: u32 = 128u;
const MAX_RADIANCE: f32 = 60000.0;

struct Frame {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    env_sky: vec4<f32>,
    env_horizon: vec4<f32>,
    env_ground: vec4<f32>,
    // GI tint: rgb, intensity.
    gi: vec4<f32>,
    // Tiles x, tiles y, depth slices, light count.
    cluster_dims: vec4<u32>,
    // near, far, slice scale (slices / ln(far / near)), unused.
    cluster_params: vec4<f32>,
    // width, height, 1 / width, 1 / height (pixels).
    screen: vec4<f32>,
};

struct Light {
    // xyz position (world), w range.
    position_range: vec4<f32>,
    // rgb colour, w intensity.
    color_intensity: vec4<f32>,
};

struct Lights {
    items: array<Light, 128>,
};

struct Draw {
    model: mat4x4<f32>,
    normal_matrix: mat4x4<f32>,
    base_color: vec4<f32>,
    // rgb emissive, w metallic.
    emissive_metallic: vec4<f32>,
    // roughness, specular, clearcoat, clearcoat roughness.
    surface: vec4<f32>,
    // transmission, subsurface, gi_emissive, lamp_emissive.
    extra: vec4<f32>,
    // x: lamp emission level (0..1); yzw unused.
    emission: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<uniform> lights: Lights;
@group(0) @binding(2) var<storage, read> cluster_ranges: array<vec2<u32>>;
@group(0) @binding(3) var<storage, read> cluster_indices: array<u32>;
@group(1) @binding(0) var<uniform> draw: Draw;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) view_depth: f32,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let world = draw.model * vec4<f32>(in.position, 1.0);
    let view = frame.view * world;
    out.clip = frame.proj * view;
    out.world_pos = world.xyz;
    out.world_normal = (draw.normal_matrix * vec4<f32>(in.normal, 0.0)).xyz;
    out.view_depth = -view.z;
    return out;
}

// Placeholder environment (5.2 replaces it with probes): a sky above, a
// horizon band and a ground below, in world z-up.
fn env_radiance(dir: vec3<f32>) -> vec3<f32> {
    let z = clamp(dir.z, -1.0, 1.0);
    let up = smoothstep(0.0, 0.6, z);
    let down = smoothstep(0.0, 0.6, -z);
    return frame.env_horizon.rgb * (1.0 - up - down) + frame.env_sky.rgb * up + frame.env_ground.rgb * down;
}

// Cosine-weighted average of env_radiance around `n` (approximation: the
// lower-hemisphere average facing down, the upper one facing up).
fn env_irradiance(n: vec3<f32>) -> vec3<f32> {
    let t = clamp(n.z * 0.5 + 0.5, 0.0, 1.0);
    let lower = mix(frame.env_ground.rgb, frame.env_horizon.rgb, 0.5);
    let upper = mix(frame.env_horizon.rgb, frame.env_sky.rgb, 0.5);
    return mix(lower, upper, t);
}

// Cluster of a fragment (pixel position, view depth); same formulas as
// ClusterGrid::cluster_of on the CPU.
fn cluster_index(frag_xy: vec2<f32>, depth: f32) -> u32 {
    let dims = frame.cluster_dims;
    let fx = frag_xy.x * frame.screen.z * f32(dims.x);
    let fy = (1.0 - frag_xy.y * frame.screen.w) * f32(dims.y);
    let tx = min(u32(max(fx, 0.0)), dims.x - 1u);
    let ty = min(u32(max(fy, 0.0)), dims.y - 1u);
    let near = frame.cluster_params.x;
    let s = log(max(depth, near) / near) * frame.cluster_params.z;
    let tz = min(u32(max(s, 0.0)), dims.z - 1u);
    return tx + dims.x * (ty + dims.y * tz);
}

struct Shaded {
    color: vec3<f32>,
    // Coverage for blended output (1 for opaque materials).
    alpha: f32,
};

fn shade(in: VsOut, front: bool, frag_xy: vec2<f32>) -> Shaded {
    var n = in.world_normal;
    let n_len = length(n);
    n = select(vec3<f32>(0.0, 0.0, 1.0), n / n_len, n_len > 1e-8);
    if (!front) {
        n = -n;
    }
    let to_eye = frame.camera_pos.xyz - in.world_pos;
    let v = to_eye / max(length(to_eye), 1e-8);
    let n_o_v = clamp(dot(n, v), 1e-4, 1.0);

    let base = max(draw.base_color.rgb, vec3<f32>(0.0));
    let metallic = clamp(draw.emissive_metallic.w, 0.0, 1.0);
    let roughness = clamp(draw.surface.x, MIN_ROUGHNESS, 1.0);
    let alpha = roughness * roughness;
    let specular = max(draw.surface.y, 0.0);
    let clearcoat = clamp(draw.surface.z, 0.0, 1.0);
    let cc_roughness = clamp(draw.surface.w, MIN_ROUGHNESS, 1.0);
    let cc_alpha = cc_roughness * cc_roughness;
    let transmission = clamp(draw.extra.x, 0.0, 1.0);
    let wrap = clamp(draw.extra.y, 0.0, 1.0);

    let f0 = mix(vec3<f32>(min(0.04 * specular, 1.0)), base, metallic);
    let f90 = f90_of(f0);
    let diffuse = base * (1.0 - metallic) * (1.0 - transmission);

    var color = vec3<f32>(0.0);
    let ci = cluster_index(frag_xy, in.view_depth);
    let range = cluster_ranges[ci];
    let count = min(range.y, MAX_LIGHTS);
    for (var k = 0u; k < count; k = k + 1u) {
        let li = cluster_indices[range.x + k];
        if (li >= min(frame.cluster_dims.w, MAX_LIGHTS)) {
            continue;
        }
        let light = lights.items[li];
        let to_l = light.position_range.xyz - in.world_pos;
        let d2 = dot(to_l, to_l);
        let d = sqrt(d2);
        let r = light.position_range.w;
        if (d >= r || d < 1e-6) {
            continue;
        }
        let l = to_l / d;
        let ratio = d / r;
        let w = clamp(1.0 - ratio * ratio * ratio * ratio, 0.0, 1.0);
        let radiance = light.color_intensity.rgb * (light.color_intensity.w * w * w / max(d2, 1e-4));

        let n_o_l_signed = dot(n, l);
        let n_o_l = clamp(n_o_l_signed, 0.0, 1.0);
        let hv = l + v;
        let h = hv / max(length(hv), 1e-6);
        let n_o_h = clamp(dot(n, h), 0.0, 1.0);
        let v_o_h = clamp(dot(v, h), 0.0, 1.0);

        let f = f_schlick(f0, f90, v_o_h);
        let spec = f * (d_ggx(n_o_h, alpha) * v_smith_ggx(n_o_v, n_o_l, alpha) * n_o_l);
        let diff = (vec3<f32>(1.0) - f) * diffuse * (wrapped_n_o_l(n_o_l_signed, wrap) / PI);
        var lobe = diff + spec;
        let fc = f_schlick_scalar(0.04, 1.0, v_o_h) * clearcoat;
        let spec_c = d_ggx(n_o_h, cc_alpha) * v_kelemen(v_o_h) * fc * n_o_l;
        lobe = lobe * (1.0 - fc) + vec3<f32>(spec_c);
        color = color + lobe * radiance;
    }

    // Ambient from the placeholder environment.
    let refl = reflect(-v, n);
    let f_amb = f_schlick_roughness(f0, f90, n_o_v, roughness);
    let spec_env = mix(env_radiance(refl), env_irradiance(refl), roughness);
    var ambient = diffuse * env_irradiance(n) * (vec3<f32>(1.0) - f_amb) + spec_env * f_amb;
    let fc_amb = f_schlick_scalar(0.04, 1.0, n_o_v) * clearcoat;
    let cc_env = mix(env_radiance(refl), env_irradiance(refl), cc_roughness);
    ambient = ambient * (1.0 - fc_amb) + cc_env * fc_amb;
    color = color + ambient;

    // Emission: constant, GI-tint driven (UV), lamp driven (inserts).
    let gi = max(frame.gi.rgb, vec3<f32>(0.0)) * max(frame.gi.w, 0.0);
    color = color + max(draw.emissive_metallic.rgb, vec3<f32>(0.0));
    color = color + gi * max(draw.extra.z, 0.0);
    color = color + base * (max(draw.extra.w, 0.0) * clamp(draw.emission.x, 0.0, 1.0));

    // NaN and overflow guard (the HDR target is 16-bit float).
    color = select(color, vec3<f32>(0.0), color != color);
    color = clamp(color, vec3<f32>(0.0), vec3<f32>(MAX_RADIANCE));

    let f_avg = (f_amb.x + f_amb.y + f_amb.z) / 3.0;
    var out: Shaded;
    out.color = color;
    out.alpha = clamp(1.0 - transmission * (1.0 - f_avg), 0.0, 1.0);
    return out;
}

@fragment
fn fs_opaque(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let s = shade(in, front, in.clip.xy);
    return vec4<f32>(s.color, 1.0);
}

// Premultiplied output: blend with src One, dst OneMinusSrcAlpha.
@fragment
fn fs_blended(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let s = shade(in, front, in.clip.xy);
    return vec4<f32>(s.color, s.alpha);
}
