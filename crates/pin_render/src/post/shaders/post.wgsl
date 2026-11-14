// post: the HDR post chain (5.2). Entry points: vs_fullscreen, fs_blur
// (exposure + ball motion blur), fs_prefilter (bloom threshold), fs_down
// (13-tap downsample), fs_up (tent upsample + level blend), fs_composite
// (bloom composite, tone mapping, sRGB). Group 0: the frame uniform;
// group 1: the pass inputs (tex_a, tex_b, a linear clamp sampler).

const MAX_BLUR_BALLS: u32 = 16u;

struct BlurBall {
    // Centre (px), radius (px), unused.
    center_radius: vec4<f32>,
    // Displacement over the shutter (px), unused.
    disp: vec4<f32>,
};

struct Post {
    // exposure, ball count, samples, tone map (0 ACES, 1 linear).
    params: vec4<f32>,
    // bloom threshold, knee, firefly clamp, intensity.
    bloom: vec4<f32>,
    // bloom radius, encode sRGB (0/1), output HDR (0/1), bloom on (0/1).
    flags: vec4<f32>,
    balls: array<BlurBall, 16>,
};

@group(0) @binding(0) var<uniform> post: Post;
@group(1) @binding(0) var tex_a: texture_2d<f32>;
@group(1) @binding(1) var tex_b: texture_2d<f32>;
@group(1) @binding(2) var lin: sampler;

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(i & 2u) * 2.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// NaN / negative -> 0, +inf -> f16 max.
fn clean(c: vec3<f32>) -> vec3<f32> {
    let z = select(c, vec3<f32>(0.0), c != c);
    return clamp(z, vec3<f32>(0.0), vec3<f32>(65504.0));
}

fn load_a(p: vec2<i32>) -> vec3<f32> {
    let dims = vec2<i32>(textureDimensions(tex_a));
    return textureLoad(tex_a, clamp(p, vec2<i32>(0), dims - vec2<i32>(1)), 0).rgb;
}

// ---- 1. exposure + motion blur ------------------------------------------

// Distance from `p` to the segment a..b.
fn segment_distance(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let ab = b - a;
    let t = clamp(dot(p - a, ab) / max(dot(ab, ab), 1e-12), 0.0, 1.0);
    return length(p - (a + ab * t));
}

// Background behind a ball disc at `p`: the image where the line through
// `p` along `dir` leaves the disc, averaged over both exits.
fn background(p: vec2<f32>, c: vec2<f32>, r: f32, dir: vec2<f32>) -> vec3<f32> {
    let pc = p - c;
    let d2 = dot(pc, pc);
    if (d2 >= r * r) {
        return load_a(vec2<i32>(floor(p)));
    }
    let b = dot(pc, dir);
    let s = sqrt(max(b * b - (d2 - r * r), 0.0));
    let e1 = p + dir * (-b + s + 1.5);
    let e2 = p + dir * (-b - s - 1.5);
    return 0.5 * (load_a(vec2<i32>(floor(e1))) + load_a(vec2<i32>(floor(e2))));
}

@fragment
fn fs_blur(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let p = frag.xy;
    let exposure = post.params.x;
    var color = load_a(vec2<i32>(floor(p)));
    let count = min(u32(post.params.y), MAX_BLUR_BALLS);
    let samples = max(u32(post.params.z), 1u);
    for (var k = 0u; k < count; k = k + 1u) {
        let ball = post.balls[k];
        let c = ball.center_radius.xy;
        let r = ball.center_radius.z;
        let d = ball.disp.xy;
        let len = length(d);
        if (len < 1e-3 || r <= 0.0) {
            continue;
        }
        // Pixels the disc sweeps over the shutter: a capsule.
        if (segment_distance(p, c - 0.5 * d, c + 0.5 * d) > r + 1.0) {
            continue;
        }
        let dir = d / len;
        let bg = background(p, c, r, dir);
        var acc = vec3<f32>(0.0);
        for (var i = 0u; i < samples; i = i + 1u) {
            // At shutter time t the ball is at c + d t; p then shows the
            // static ball image at p - d t when that lies on the disc.
            let t = (f32(i) + 0.5) / f32(samples) - 0.5;
            let q = p - d * t;
            let on_ball = dot(q - c, q - c) < r * r;
            acc = acc + select(bg, load_a(vec2<i32>(floor(q))), on_ball);
        }
        color = acc / f32(samples);
        break;
    }
    return vec4<f32>(clean(color * exposure), 1.0);
}

// ---- 2. bloom ---------------------------------------------------------------

// Same as post::bloom_prefilter on the CPU.
fn bloom_bright(rgb: vec3<f32>) -> vec3<f32> {
    let c = clean(rgb);
    let l = luminance(c);
    if (l <= 0.0) {
        return vec3<f32>(0.0);
    }
    let t = max(post.bloom.x, 0.0);
    let knee = min(max(post.bloom.y, 0.0), t);
    var soft = clamp(l - t + knee, 0.0, 2.0 * knee);
    soft = soft * soft / (4.0 * knee + 1e-5);
    let contrib = clamp(max(soft, l - t) / l, 0.0, 1.0);
    var scale = contrib;
    let fc = post.bloom.z;
    if (fc > 0.0 && l * contrib > fc) {
        scale = fc / l;
    }
    return c * scale;
}

@fragment
fn fs_prefilter(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(bloom_bright(load_a(vec2<i32>(floor(frag.xy)))), 1.0);
}

fn sample_a(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(tex_a, lin, uv, 0.0).rgb;
}

fn sample_b(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(tex_b, lin, uv, 0.0).rgb;
}

// 13-tap downsample (Jimenez 2014) of tex_a; weights sum to 1.
@fragment
fn fs_down(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let src = vec2<f32>(textureDimensions(tex_a));
    let dst = max(floor(src * 0.5), vec2<f32>(1.0));
    let uv = frag.xy / dst;
    let t = 1.0 / src;
    let a = sample_a(uv + t * vec2<f32>(-2.0, -2.0));
    let b = sample_a(uv + t * vec2<f32>(0.0, -2.0));
    let c = sample_a(uv + t * vec2<f32>(2.0, -2.0));
    let d = sample_a(uv + t * vec2<f32>(-2.0, 0.0));
    let e = sample_a(uv);
    let f = sample_a(uv + t * vec2<f32>(2.0, 0.0));
    let g = sample_a(uv + t * vec2<f32>(-2.0, 2.0));
    let h = sample_a(uv + t * vec2<f32>(0.0, 2.0));
    let i = sample_a(uv + t * vec2<f32>(2.0, 2.0));
    let j = sample_a(uv + t * vec2<f32>(-1.0, -1.0));
    let k = sample_a(uv + t * vec2<f32>(1.0, -1.0));
    let l = sample_a(uv + t * vec2<f32>(-1.0, 1.0));
    let m = sample_a(uv + t * vec2<f32>(1.0, 1.0));
    let o = e * 0.125 + (a + c + g + i) * 0.03125 + (b + d + f + h) * 0.0625 + (j + k + l + m) * 0.125;
    return vec4<f32>(o, 1.0);
}

// 3x3 tent filter of tex_b at `uv`, one source texel apart; weights sum to 1.
fn tent_b(uv: vec2<f32>) -> vec3<f32> {
    let t = 1.0 / vec2<f32>(textureDimensions(tex_b));
    var o = sample_b(uv) * 4.0;
    o = o + (sample_b(uv + vec2<f32>(t.x, 0.0)) + sample_b(uv - vec2<f32>(t.x, 0.0))
        + sample_b(uv + vec2<f32>(0.0, t.y)) + sample_b(uv - vec2<f32>(0.0, t.y))) * 2.0;
    o = o + sample_b(uv + t) + sample_b(uv - t) + sample_b(uv + vec2<f32>(t.x, -t.y))
        + sample_b(uv + vec2<f32>(-t.x, t.y));
    return o / 16.0;
}

// up_i = mix(down_i, tent(up_(i+1)), radius); tex_a = down_i (same size as
// the target), tex_b = the coarser level.
@fragment
fn fs_up(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let dst = vec2<f32>(textureDimensions(tex_a));
    let uv = frag.xy / dst;
    let here = load_a(vec2<i32>(floor(frag.xy)));
    let coarse = tent_b(uv);
    return vec4<f32>(mix(here, coarse, clamp(post.flags.x, 0.0, 1.0)), 1.0);
}

// ---- 3. composite, tone mapping, encoding --------------------------------

fn aces_film(x: vec3<f32>) -> vec3<f32> {
    let v = max(x, vec3<f32>(0.0));
    return clamp((v * (2.51 * v + 0.03)) / (v * (2.43 * v + 0.59) + 0.14), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn srgb_encode(x: vec3<f32>) -> vec3<f32> {
    let c = clamp(x, vec3<f32>(0.0), vec3<f32>(1.0));
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

// tex_a = the exposed, blurred scene; tex_b = the finest bloom level.
@fragment
fn fs_composite(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let dst = vec2<f32>(textureDimensions(tex_a));
    let scene = load_a(vec2<i32>(floor(frag.xy)));
    var c = scene;
    if (post.flags.w > 0.5) {
        let k = clamp(post.bloom.w, 0.0, 1.0);
        let bloom = tent_b(frag.xy / dst);
        c = clean(scene - k * bloom_bright(scene) + k * bloom);
    }
    if (post.flags.z > 0.5) {
        return vec4<f32>(c, 1.0);
    }
    var mapped: vec3<f32>;
    if (post.params.w > 0.5) {
        mapped = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    } else {
        mapped = aces_film(c);
    }
    if (post.flags.y > 0.5) {
        mapped = srgb_encode(mapped);
    }
    return vec4<f32>(mapped, 1.0);
}
