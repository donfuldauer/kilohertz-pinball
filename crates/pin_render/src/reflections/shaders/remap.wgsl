// probe_remap: six captured cube faces -> one octahedral probe map.
// Concatenated after pbr_reflection (oct_decode); its own bindings are in
// group 0.

struct FaceBasis {
    // Per face: right, up, forward (xyz; w unused).
    right: array<vec4<f32>, 6>,
    up: array<vec4<f32>, 6>,
    forward: array<vec4<f32>, 6>,
    // x: octahedral map side (texels); yzw unused.
    params: vec4<f32>,
    // The frame's placeholder environment (rgb; w unused): what a probe
    // shows where the capture saw no geometry.
    env_sky: vec4<f32>,
    env_horizon: vec4<f32>,
    env_ground: vec4<f32>,
};

@group(0) @binding(0) var faces: texture_2d_array<f32>;
@group(0) @binding(1) var face_sampler: sampler;
@group(0) @binding(2) var<uniform> basis: FaceBasis;

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(i & 2u) * 2.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

// Same gradient as env_radiance in pbr_forward.
fn env_background(dir: vec3<f32>) -> vec3<f32> {
    let z = clamp(dir.z, -1.0, 1.0);
    let up = smoothstep(0.0, 0.6, z);
    let down = smoothstep(0.0, 0.6, -z);
    return basis.env_horizon.rgb * (1.0 - up - down) + basis.env_sky.rgb * up + basis.env_ground.rgb * down;
}

@fragment
fn fs_remap(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = frag.xy / basis.params.x;
    let dir = oct_decode(uv);
    // The face the direction leaves through: the largest forward component.
    var best = 0u;
    var best_ma = -2.0;
    for (var k = 0u; k < 6u; k = k + 1u) {
        let ma = dot(dir, basis.forward[k].xyz);
        if (ma > best_ma) {
            best_ma = ma;
            best = k;
        }
    }
    let ma = max(best_ma, 1e-6);
    let sc = dot(dir, basis.right[best].xyz) / ma;
    let tc = dot(dir, basis.up[best].xyz) / ma;
    let st = vec2<f32>(sc * 0.5 + 0.5, 0.5 - tc * 0.5);
    // Faces are cleared to transparent black; geometry writes coverage in
    // alpha (premultiplied for blended draws), the rest is environment.
    let c = textureSampleLevel(faces, face_sampler, st, best, 0.0);
    let cover = clamp(c.a, 0.0, 1.0);
    return vec4<f32>(c.rgb + (1.0 - cover) * env_background(dir), 1.0);
}
