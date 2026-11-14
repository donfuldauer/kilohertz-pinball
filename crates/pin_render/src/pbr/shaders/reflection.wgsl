// pbr_reflection: the reflection source the forward shader samples (5.2 hook).
//
// Group 2 holds an array of octahedral probe maps (world z-up, +z at the
// centre of the map, -z at the corners). A draw picks its layer through
// `draw.emission.y` (0 = no probe: the placeholder environment; k + 1 =
// layer k). 5.4 swaps this include for its ray-query source.

@group(2) @binding(0) var probe_atlas: texture_2d_array<f32>;
@group(2) @binding(1) var probe_sampler: sampler;

fn sign_not_zero(v: f32) -> f32 {
    return select(-1.0, 1.0, v >= 0.0);
}

// Unit direction -> octahedral map coordinates in 0..1 (same as
// reflections::oct_encode on the CPU).
fn oct_encode(d: vec3<f32>) -> vec2<f32> {
    let s = abs(d.x) + abs(d.y) + abs(d.z);
    var p = d.xy / max(s, 1e-20);
    if (d.z < 0.0) {
        p = vec2<f32>((1.0 - abs(p.y)) * sign_not_zero(p.x), (1.0 - abs(p.x)) * sign_not_zero(p.y));
    }
    return p * 0.5 + vec2<f32>(0.5);
}

// Octahedral map coordinates in 0..1 -> unit direction.
fn oct_decode(uv: vec2<f32>) -> vec3<f32> {
    let p = uv * 2.0 - vec2<f32>(1.0);
    var d = vec3<f32>(p.x, p.y, 1.0 - abs(p.x) - abs(p.y));
    if (d.z < 0.0) {
        d = vec3<f32>((1.0 - abs(p.y)) * sign_not_zero(p.x), (1.0 - abs(p.x)) * sign_not_zero(p.y), d.z);
    }
    return normalize(d);
}

// Radiance of probe `layer` towards world direction `dir`.
fn probe_radiance(dir: vec3<f32>, layer: u32) -> vec3<f32> {
    let uv = oct_encode(dir);
    return textureSampleLevel(probe_atlas, probe_sampler, uv, layer, 0.0).rgb;
}
