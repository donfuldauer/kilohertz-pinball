use pin_input::calibration::*;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 { self.0 ^= self.0 << 13; self.0 ^= self.0 >> 7; self.0 ^= self.0 << 17; self.0 }
    fn unit(&mut self) -> f32 { (self.next() >> 40) as f32 / (1u64 << 24) as f32 } // 0..1
    fn sym(&mut self) -> f32 { self.unit() * 2.0 - 1.0 }
}
#[derive(Clone, Copy, Debug)]
enum Noise { U(f32), Bits(u32) }
fn noisy(x: f32, n: Noise, r: &mut Rng) -> f32 {
    match n {
        Noise::U(a) => x + a * r.sym(),
        Noise::Bits(b) => { let m = ((1u32 << b) - 1) as f32; let d = (r.next() % 3) as f32 - 1.0; ((x * m).round() + d) / m }
    }
}
#[test]
fn exp() {
    let noises = [Noise::U(0.0), Noise::U(0.001), Noise::U(0.002), Noise::U(0.004), Noise::U(0.008), Noise::Bits(8), Noise::Bits(10)];
    for tracking in [1.0f32, 0.0] {
    for n in noises {
        for pull_ms in [300u64, 1000, 3000] {
            for depth in [0.1f32, 0.4, 0.8] {
                let mut lost = 0; let mut multi = 0; let mut worst = 0.0f32; let mut over5 = 0;
                for seed in 0..200u64 {
                    let mut r = Rng(seed * 7919 + 12345 + pull_ms + (depth * 1000.0) as u64);
                    let snap = 10 + r.next() % 11;
                    let mut f = PlungerFilter::new(PlungerConfig { rest_tracking_per_s: tracking, ..PlungerConfig::default() });
                    let mut t = 0u64; let mut rel = Vec::new();
                    let mut feed = |x: f32, f: &mut PlungerFilter, r: &mut Rng, rel: &mut Vec<f32>, t: &mut u64| { if let Some(v) = f.update(noisy(x, n, r), *t).release { rel.push(v) } *t += 1; };
                    for _ in 0..2000 { feed(0.0, &mut f, &mut r, &mut rel, &mut t); }
                    for k in 1..=pull_ms { feed(depth * k as f32 / pull_ms as f32, &mut f, &mut r, &mut rel, &mut t); }
                    for _ in 0..200 { feed(depth, &mut f, &mut r, &mut rel, &mut t); }
                    for k in 1..=snap { feed(depth - depth * k as f32 / snap as f32, &mut f, &mut r, &mut rel, &mut t); }
                    for _ in 0..300 { feed(0.0, &mut f, &mut r, &mut rel, &mut t); }
                    let want = depth / snap as f32 * 1000.0;
                    match rel.len() { 0 => lost += 1, 1 => { let e = (rel[0] - want).abs() / want; worst = worst.max(e); if e > 0.05 { over5 += 1 } }, _ => multi += 1 }
                }
                println!("trk {tracking} {n:?} pull {pull_ms} depth {depth}: lost {lost} multi {multi} >5% {over5} worst {:.1}%", worst * 100.0);
            }
        }
    }
    }
}
