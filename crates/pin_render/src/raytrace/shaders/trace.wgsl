// raytrace_trace: ray-query reflections into the octahedral reflection atlas
// (5.4). Concatenated after `enable wgpu_ray_query;` and pbr_reflection
// (oct_decode), so the PBR pass samples the result exactly like a probe;
// pbr_brdf comes first (the hit shading uses the forward pass's BRDF).
//
// One compute dispatch per frame: x/y cover the octahedral map, z selects a
// job (one atlas layer). Each texel direction R is traced against the
// scene's top-level acceleration structure from an origin that makes the
// reflection exact for the surface the layer serves:
//   MODE_BALL_VIEW   the sphere point whose mirror reflection of the eye
//                    ray is R (three fixed-point iterations);
//   MODE_BALL_CENTRE radially from the sphere (a probe-equivalent map);
//   MODE_PLANE       the plane point where the eye ray reflects into R
//                    (the eye mirrored in the plane), half resolution.
// A ray that misses returns the environment, so a map never goes black.
// Hits are shaded like pbr_forward's `shade` (frame point lights, here
// unshadowed and unclustered; ambient from the environment) with the
// environment as the mirror term at the hit: one bounce, no further rays.

const TRACE_MAX_LIGHTS: u32 = 128u;
const TRACE_MAX_JOBS: u32 = 17u;
const TRACE_MAX_RADIANCE: f32 = 60000.0;
const MODE_BALL_VIEW: u32 = 0u;
const MODE_BALL_CENTRE: u32 = 1u;
const MODE_PLANE: u32 = 2u;
// Instance masks: opaque geometry 1, blended (glass) 2; rays see mask 1.
const MASK_OPAQUE: u32 = 1u;

struct TraceJob {
    // Ball: centre xyz, radius w. Plane: point xyz.
    a: vec4<f32>,
    // Plane: unit normal xyz.
    b: vec4<f32>,
    // x: mode, y: atlas layer, z: block size (1 full, 2 half resolution).
    mode: vec4<u32>,
};

struct TraceLight {
    // xyz position (world), w range.
    position_range: vec4<f32>,
    // rgb colour, w intensity.
    color_intensity: vec4<f32>,
};

struct TraceParams {
    eye: vec4<f32>,
    env_sky: vec4<f32>,
    env_horizon: vec4<f32>,
    env_ground: vec4<f32>,
    // GI tint: rgb, intensity.
    gi: vec4<f32>,
    // x: light count, y: map side (texels), z: job count, w unused.
    counts: vec4<u32>,
    // x: ray t_max (m), y: origin offset (fraction of the ball radius),
    // z: plane origin offset (m), w unused.
    tune: vec4<f32>,
    jobs: array<TraceJob, 17>,
    lights: array<TraceLight, 128>,
};

struct TraceInstance {
    base_color: vec4<f32>,
    // rgb emissive, w metallic.
    emissive_metallic: vec4<f32>,
    // roughness, specular, gi_emissive, lamp emission (lamp_emissive * level).
    surface: vec4<f32>,
    // clearcoat, clearcoat roughness, transmission, subsurface.
    coat: vec4<f32>,
    // first index, first vertex, triangles, unused.
    geometry: vec4<u32>,
};

@group(0) @binding(0) var scene: acceleration_structure;
@group(0) @binding(1) var atlas_out: texture_storage_2d_array<rgba16float, write>;
@group(0) @binding(2) var<uniform> params: TraceParams;
@group(0) @binding(3) var<storage, read> instances: array<TraceInstance>;
// Interleaved per vertex: position (w 1), normal (w 0).
@group(0) @binding(4) var<storage, read> vertices: array<vec4<f32>>;
@group(0) @binding(5) var<storage, read> tri_indices: array<u32>;

// Same gradient as env_radiance in pbr_forward.
fn trace_env(dir: vec3<f32>) -> vec3<f32> {
    let z = clamp(dir.z, -1.0, 1.0);
    let up = smoothstep(0.0, 0.6, z);
    let down = smoothstep(0.0, 0.6, -z);
    return params.env_horizon.rgb * (1.0 - up - down) + params.env_sky.rgb * up + params.env_ground.rgb * down;
}

// Same approximation as env_irradiance in pbr_forward.
fn trace_env_irradiance(n: vec3<f32>) -> vec3<f32> {
    let t = clamp(n.z * 0.5 + 0.5, 0.0, 1.0);
    let lower = mix(params.env_ground.rgb, params.env_horizon.rgb, 0.5);
    let upper = mix(params.env_horizon.rgb, params.env_sky.rgb, 0.5);
    return mix(lower, upper, t);
}

fn safe_normalize(v: vec3<f32>, fallback: vec3<f32>) -> vec3<f32> {
    let l = length(v);
    return select(fallback, v / l, l > 1e-12);
}

// World normal at a committed triangle hit (interpolated vertex normals,
// the face normal where those are missing), facing against `dir`.
fn hit_normal(hit: RayIntersection, dir: vec3<f32>) -> vec3<f32> {
    let inst = instances[hit.instance_custom_data];
    let base = inst.geometry.x + 3u * hit.primitive_index;
    let fv = inst.geometry.y;
    let i0 = 2u * (fv + tri_indices[base]);
    let i1 = 2u * (fv + tri_indices[base + 1u]);
    let i2 = 2u * (fv + tri_indices[base + 2u]);
    let p0 = vertices[i0].xyz;
    let p1 = vertices[i1].xyz;
    let p2 = vertices[i2].xyz;
    let face = cross(p1 - p0, p2 - p0);
    let u = hit.barycentrics.x;
    let v = hit.barycentrics.y;
    let smooth_n = vertices[i0 + 1u].xyz * (1.0 - u - v) + vertices[i1 + 1u].xyz * u + vertices[i2 + 1u].xyz * v;
    let n_obj = select(face, smooth_n, length(smooth_n) > 1e-6);
    // Object -> world for normals: the inverse transpose of the linear part.
    let w2o = hit.world_to_object;
    let lin = mat3x3<f32>(w2o[0], w2o[1], w2o[2]);
    var n = safe_normalize(n_obj * lin, -dir);
    if (dot(n, dir) > 0.0) {
        n = -n;
    }
    return n;
}

// One-bounce radiance leaving a hit towards the ray origin: pbr_forward's
// shading model with the environment as the mirror term.
fn shade_hit(origin: vec3<f32>, dir: vec3<f32>, hit: RayIntersection) -> vec3<f32> {
    let inst = instances[hit.instance_custom_data];
    let n = hit_normal(hit, dir);
    let pos = origin + dir * hit.t;
    let v = -dir;
    let n_o_v = clamp(dot(n, v), 1e-4, 1.0);

    let base = max(inst.base_color.rgb, vec3<f32>(0.0));
    let metallic = clamp(inst.emissive_metallic.w, 0.0, 1.0);
    let roughness = clamp(inst.surface.x, MIN_ROUGHNESS, 1.0);
    let alpha = roughness * roughness;
    let specular = max(inst.surface.y, 0.0);
    let clearcoat = clamp(inst.coat.x, 0.0, 1.0);
    let cc_roughness = clamp(inst.coat.y, MIN_ROUGHNESS, 1.0);
    let cc_alpha = cc_roughness * cc_roughness;
    let transmission = clamp(inst.coat.z, 0.0, 1.0);
    let wrap = clamp(inst.coat.w, 0.0, 1.0);

    let f0 = mix(vec3<f32>(min(0.04 * specular, 1.0)), base, metallic);
    let f90 = f90_of(f0);
    let diffuse = base * (1.0 - metallic) * (1.0 - transmission);
    let f_view = f_schlick(f0, f90, n_o_v);
    let fc_view = f_schlick_scalar(0.04, 1.0, n_o_v) * clearcoat;

    var color = vec3<f32>(0.0);
    let count = min(params.counts.x, TRACE_MAX_LIGHTS);
    for (var k = 0u; k < count; k = k + 1u) {
        let light = params.lights[k];
        let to_l = light.position_range.xyz - pos;
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
        let diff = (vec3<f32>(1.0) - f_view) * diffuse * (wrapped_n_o_l(n_o_l_signed, wrap) / PI);
        var lobe = diff + spec;
        let fc = f_schlick_scalar(0.04, 1.0, v_o_h) * clearcoat;
        let spec_c = d_ggx(n_o_h, cc_alpha) * v_smith_ggx(n_o_v, n_o_l, cc_alpha) * fc * n_o_l;
        lobe = lobe * (1.0 - fc_view) + vec3<f32>(spec_c);
        color = color + lobe * radiance;
    }

    let refl = reflect(dir, n);
    let f_amb = f_schlick_roughness(f0, f90, n_o_v, roughness);
    let spec_env = mix(trace_env(refl), trace_env_irradiance(refl), roughness);
    var ambient = diffuse * trace_env_irradiance(n) * (vec3<f32>(1.0) - f_amb) + spec_env * f_amb;
    let fc_amb = f_schlick_scalar(0.04, 1.0, n_o_v) * clearcoat;
    let cc_env = mix(trace_env(refl), trace_env_irradiance(refl), cc_roughness);
    ambient = ambient * (1.0 - fc_amb) + cc_env * fc_amb;
    color = color + ambient;

    let gi = max(params.gi.rgb, vec3<f32>(0.0)) * max(params.gi.w, 0.0);
    color = color + max(inst.emissive_metallic.rgb, vec3<f32>(0.0));
    color = color + gi * max(inst.surface.z, 0.0);
    color = color + base * max(inst.surface.w, 0.0);
    color = select(color, vec3<f32>(0.0), color != color);
    return clamp(color, vec3<f32>(0.0), vec3<f32>(TRACE_MAX_RADIANCE));
}

// Radiance arriving at `origin` along `dir` (the environment on a miss).
fn trace_radiance(origin: vec3<f32>, dir: vec3<f32>, t_min: f32) -> vec3<f32> {
    var rq: ray_query;
    rayQueryInitialize(&rq, scene, RayDesc(RAY_FLAG_FORCE_OPAQUE, MASK_OPAQUE, t_min, params.tune.x, origin, dir));
    loop {
        if (!rayQueryProceed(&rq)) {
            break;
        }
    }
    let hit = rayQueryGetCommittedIntersection(&rq);
    if (hit.kind == RAY_QUERY_INTERSECTION_NONE) {
        return trace_env(dir);
    }
    return shade_hit(origin, dir, hit);
}

// The point of the sphere (centre c, radius r) whose mirror reflection of
// the eye ray leaves along `dir`: the normal bisects `dir` and the
// direction to the eye (fixed point, converges fast as r << eye distance).
fn ball_view_normal(c: vec3<f32>, r: f32, dir: vec3<f32>) -> vec3<f32> {
    let eye = params.eye.xyz;
    var n = safe_normalize(dir + safe_normalize(eye - c, dir), dir);
    for (var i = 0u; i < 3u; i = i + 1u) {
        let p = c + n * r;
        let h = dir + safe_normalize(eye - p, dir);
        n = safe_normalize(h, n);
    }
    // Outward rays only (the reflection never enters the ball).
    return select(dir, n, dot(n, dir) >= 0.0);
}

fn job_radiance(job: TraceJob, dir: vec3<f32>) -> vec3<f32> {
    let mode = job.mode.x;
    let c = job.a.xyz;
    let r = max(job.a.w, 0.0);
    let lift = 1.0 + max(params.tune.y, 0.0);
    if (mode == MODE_BALL_VIEW) {
        let n = ball_view_normal(c, r, dir);
        return trace_radiance(c + n * (r * lift), dir, 0.0);
    }
    if (mode == MODE_BALL_CENTRE) {
        return trace_radiance(c + dir * (r * lift), dir, 0.0);
    }
    // MODE_PLANE: the eye mirrored in the plane looks through the plane
    // point along `dir`; trace from that point on.
    let np = job.b.xyz;
    let eye = params.eye.xyz;
    let h = dot(eye - c, np);
    let d_n = dot(dir, np);
    if (h <= 0.0 || d_n <= 1e-4) {
        return trace_env(dir);
    }
    let mirrored = eye - 2.0 * h * np;
    // dot(mirrored - c, np) = -h, so the plane is h / d_n along the ray.
    let on_plane = mirrored + dir * (h / d_n);
    return trace_radiance(on_plane + np * max(params.tune.z, 0.0), dir, 0.0);
}

@compute @workgroup_size(8, 8, 1)
fn cs_trace(@builtin(global_invocation_id) gid: vec3<u32>) {
    let j = gid.z;
    if (j >= min(params.counts.z, TRACE_MAX_JOBS)) {
        return;
    }
    let job = params.jobs[j];
    let size = max(params.counts.y, 1u);
    let block = clamp(job.mode.z, 1u, 2u);
    let cells = (size + block - 1u) / block;
    if (gid.x >= cells || gid.y >= cells) {
        return;
    }
    let texel0 = gid.xy * block;
    let centre = (vec2<f32>(texel0) + vec2<f32>(0.5 * f32(block))) / f32(size);
    let dir = oct_decode(centre);
    let c = job_radiance(job, dir);
    for (var dy = 0u; dy < block; dy = dy + 1u) {
        for (var dx = 0u; dx < block; dx = dx + 1u) {
            let t = texel0 + vec2<u32>(dx, dy);
            if (t.x < size && t.y < size) {
                textureStore(atlas_out, vec2<i32>(t), i32(job.mode.y), vec4<f32>(c, 1.0));
            }
        }
    }
}
