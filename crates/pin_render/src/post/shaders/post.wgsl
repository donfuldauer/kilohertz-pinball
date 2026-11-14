// post: the HDR post chain (5.2). Entry points: vs_fullscreen, fs_blur
// (exposure + ball motion blur), fs_prefilter (bloom threshold), fs_down
// (6x6 binomial downsample), fs_up (binomial upsample + level blend),
// fs_composite (bloom composite, tone mapping, sRGB). Group 0: the frame
// uniform; group 1: the pass inputs (tex_a, tex_b, and a linear clamp
// sampler the bloom no longer uses: every pass reads with textureLoad).

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
    // image width, height (px), unused, unused.
    image: vec4<f32>,
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
            // The pixel q falls in decides, by its centre (the same test
            // the rasteriser used), so the disc smears without losing its
            // rim pixels.
            let q = p - d * t;
            let qp = floor(q);
            let qc = qp + vec2<f32>(0.5) - c;
            let on_ball = dot(qc, qc) < r * r;
            acc = acc + select(bg, load_a(vec2<i32>(qp)), on_ball);
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

// Bloom resampling works on texel indices with explicit weights. Level
// sizes are ceil(n / 2). 1-D kernel: binomial [1 5 10 10 5 1] / 32 at
// offsets k = -2..3 around the texel pair (2i, 2i + 1); its even and odd
// taps each sum to 1/2, so with a stride of 2 every source texel feeds the
// same total, whatever the parity of the size or the position (energy
// conserving), and every output texel's weights sum to 1 (a uniform image
// stays uniform). Outside the image the levels are mirrored about their
// edges (half-texel symmetric), which keeps both properties at the
// borders as long as the fine and the coarse edge coincide (even sizes);
// at an odd last row or column they are a texel apart (see DECISIONS
// "5.2 — fix round 1").

fn b6(k: i32) -> f32 {
    switch k {
        case -2, 3: { return 1.0 / 32.0; }
        case -1, 2: { return 5.0 / 32.0; }
        case 0, 1: { return 10.0 / 32.0; }
        default: { return 0.0; }
    }
}

// Where the image edge falls on the grid of a bloom level `n` texels wide
// along an axis whose full image size is `full` (level sizes are
// ceil(n / 2) per step): full / 2^level, in that level's texels.
fn level_edge(n: i32, full: f32) -> f32 {
    var size = i32(full);
    var edge = full;
    for (var l = 0; l < 16 && size > n; l = l + 1) {
        size = (size + 1) / 2;
        edge = edge * 0.5;
    }
    return edge;
}

// Mirror of texel index `m` into a level of `n` texels whose image edge is
// at `edge`: half-texel symmetric at 0; at the high edge about the nearest
// texel boundary or texel centre to `edge` (an odd image size puts the edge
// of the next level through the middle of its last texel; mirroring there
// keeps the half-outside texel from counting twice).
fn mirror(m0: i32, n: i32, edge: f32) -> i32 {
    var m = m0;
    if (m < 0) {
        m = -1 - m;
    }
    let p2 = i32(round(2.0 * edge));
    if ((p2 & 1) == 0) {
        let p = p2 / 2;
        if (m >= p) {
            m = 2 * p - 1 - m;
        }
    } else {
        let c = (p2 - 1) / 2;
        if (m > c) {
            m = 2 * c - m;
        }
    }
    return clamp(m, 0, n - 1);
}

// Downsample of tex_a into a ceil(size / 2) target.
@fragment
fn fs_down(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let src = vec2<i32>(textureDimensions(tex_a));
    let edge = vec2<f32>(level_edge(src.x, post.image.x), level_edge(src.y, post.image.y));
    let i = vec2<i32>(floor(frag.xy));
    var acc = vec3<f32>(0.0);
    for (var ky = -2; ky <= 3; ky = ky + 1) {
        let sy = mirror(2 * i.y + ky, src.y, edge.y);
        let wy = b6(ky);
        for (var kx = -2; kx <= 3; kx = kx + 1) {
            let sx = mirror(2 * i.x + kx, src.x, edge.x);
            acc = acc + (b6(kx) * wy) * textureLoad(tex_a, vec2<i32>(sx, sy), 0).rgb;
        }
    }
    return vec4<f32>(acc, 1.0);
}

// Upsample of tex_b (the coarser level) at fine texel `p`: per axis, the
// three coarse texels whose kernel reaches `p`, weight 2·b6 (a coarse
// texel covers two fine ones).
fn upsample_b(p: vec2<i32>) -> vec3<f32> {
    let coarse = vec2<i32>(textureDimensions(tex_b));
    let fine = vec2<i32>(textureDimensions(tex_a));
    let edge = 0.5 * vec2<f32>(level_edge(fine.x, post.image.x), level_edge(fine.y, post.image.y));
    var acc = vec3<f32>(0.0);
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        let iy = (p.y >> 1u) + dy;
        let wy = 2.0 * b6(p.y - 2 * iy);
        let ry = mirror(iy, coarse.y, edge.y);
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let ix = (p.x >> 1u) + dx;
            let wx = 2.0 * b6(p.x - 2 * ix);
            let rx = mirror(ix, coarse.x, edge.x);
            acc = acc + (wx * wy) * textureLoad(tex_b, vec2<i32>(rx, ry), 0).rgb;
        }
    }
    return acc;
}

// up_i = mix(down_i, upsample(up_(i+1)), radius); tex_a = down_i (same size as
// the target), tex_b = the coarser level.
@fragment
fn fs_up(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(floor(frag.xy));
    let here = load_a(p);
    let coarse = upsample_b(p);
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
    let p = vec2<i32>(floor(frag.xy));
    let scene = load_a(p);
    var c = scene;
    if (post.flags.w > 0.5) {
        let k = clamp(post.bloom.w, 0.0, 1.0);
        let bloom = upsample_b(p);
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
