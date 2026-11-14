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
    let c = textureSampleLevel(faces, face_sampler, st, best, 0.0).rgb;
    return vec4<f32>(c, 1.0);
}
