// pbr_brdf: the BRDF terms of the pinball shading model (no bindings).
//
// Metallic-roughness Cook-Torrance: GGX normal distribution, height-
// correlated Smith visibility, Schlick Fresnel; Lambert diffuse; a clear-coat
// lobe (height-correlated Smith visibility since 5.2; `v_kelemen` is kept for
// reference). `alpha` is perceptual roughness squared.

const PI: f32 = 3.14159265358979;
const MIN_ROUGHNESS: f32 = 0.03;

fn pow5(x: f32) -> f32 {
    let x2 = x * x;
    return x2 * x2 * x;
}

// GGX / Trowbridge-Reitz normal distribution.
fn d_ggx(n_o_h: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let f = (n_o_h * a2 - n_o_h) * n_o_h + 1.0;
    return a2 / (PI * f * f);
}

// Height-correlated Smith-GGX visibility (G / (4 n.l n.v)).
fn v_smith_ggx(n_o_v: f32, n_o_l: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let gv = n_o_l * sqrt(n_o_v * n_o_v * (1.0 - a2) + a2);
    let gl = n_o_v * sqrt(n_o_l * n_o_l * (1.0 - a2) + a2);
    return 0.5 / max(gv + gl, 1e-6);
}

// Kelemen visibility for the clear coat.
fn v_kelemen(l_o_h: f32) -> f32 {
    return 0.25 / max(l_o_h * l_o_h, 1e-4);
}

fn f_schlick(f0: vec3<f32>, f90: f32, v_o_h: f32) -> vec3<f32> {
    return f0 + (vec3<f32>(f90) - f0) * pow5(1.0 - v_o_h);
}

fn f_schlick_scalar(f0: f32, f90: f32, v_o_h: f32) -> f32 {
    return f0 + (f90 - f0) * pow5(1.0 - v_o_h);
}

// Schlick Fresnel for image-based (ambient) lighting, damped by roughness.
fn f_schlick_roughness(f0: vec3<f32>, f90: f32, n_o_v: f32, roughness: f32) -> vec3<f32> {
    let top = max(vec3<f32>((1.0 - roughness) * f90), f0);
    return f0 + (top - f0) * pow5(1.0 - n_o_v);
}

// F90 from F0: 0 for a zero-reflectance (pure Lambert) surface, 1 for every
// real material (F0 >= 0.02).
fn f90_of(f0: vec3<f32>) -> f32 {
    return clamp(dot(f0, vec3<f32>(50.0 * 0.33)), 0.0, 1.0);
}

// Wrapped diffuse cosine (0 = Lambert), energy-normalised: the integral of
// max(cos + w, 0) / (1 + w)^2 over the sphere is pi, as for Lambert's
// max(cos, 0), so under uniform illumination a wrapped surface reflects
// exactly what a Lambert one does; the light only spreads past the
// terminator (5.1 review N1).
fn wrapped_n_o_l(n_o_l_signed: f32, wrap: f32) -> f32 {
    let w1 = 1.0 + wrap;
    return max(n_o_l_signed + wrap, 0.0) / (w1 * w1);
}
