//! Subphase 3.2 round-3 regression tests: plunger launches under sensor
//! noise (review B3), plus N-f / N-g. Every test is seeded and deterministic.
//!
//! Noise models, added to every sample: uniform ±a, and n-bit ADC readings
//! (`round(x·(2ⁿ−1))` plus a random −1/0/+1 LSB dither).

use pin_input::calibration::{PlungerConfig, PlungerFilter};

/// xorshift64: deterministic, no dependency.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// Uniform in −1..1.
    fn sym(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 23) as f32 - 1.0
    }
}

#[derive(Copy, Clone, Debug)]
enum Noise {
    Uniform(f32),
    Bits(u32),
}

impl Noise {
    const ALL: [Noise; 6] = [
        Noise::Uniform(0.001),
        Noise::Uniform(0.002),
        Noise::Uniform(0.004),
        Noise::Uniform(0.008),
        Noise::Bits(8),
        Noise::Bits(10),
    ];

    fn apply(self, x: f32, r: &mut Rng) -> f32 {
        match self {
            Noise::Uniform(a) => x + a * r.sym(),
            Noise::Bits(b) => {
                let m = ((1u32 << b) - 1) as f32;
                let dither = (r.next() % 3) as f32 - 1.0;
                ((x * m).round() + dither) / m
            }
        }
    }

    /// Peak deviation from the true value (2 LSB for an ADC).
    fn amplitude(self) -> f32 {
        match self {
            Noise::Uniform(a) => a,
            Noise::Bits(b) => 2.0 / ((1u32 << b) - 1) as f32,
        }
    }
}

/// A noisy signal source feeding one filter.
struct Rig {
    f: PlungerFilter,
    t: u64,
    noise: Option<Noise>,
    rng: Rng,
    releases: Vec<f32>,
}

impl Rig {
    fn new(cfg: PlungerConfig, noise: Option<Noise>, seed: u64) -> Self {
        Rig {
            f: PlungerFilter::new(cfg),
            t: 0,
            noise,
            rng: Rng::new(seed),
            releases: Vec::new(),
        }
    }
    fn feed(&mut self, x: f32) {
        let x = match self.noise {
            Some(n) => n.apply(x, &mut self.rng),
            None => x,
        };
        if let Some(v) = self.f.update(x, self.t).release {
            self.releases.push(v);
        }
        self.t += 1;
    }
    fn hold(&mut self, x: f32, ms: u64) {
        for _ in 0..ms {
            self.feed(x);
        }
    }
    fn ramp(&mut self, a: f32, b: f32, ms: u64) {
        for k in 1..=ms {
            self.feed(a + (b - a) * k as f32 / ms as f32);
        }
    }
    fn take(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.releases)
    }
}

/// Runs the B3 grid for one noise model: 2 s at rest, a linear pull to
/// `depth` over `pull_ms`, 200 ms hold, a 10–20 ms linear snap, 300 ms rest.
/// Every launch must report exactly one release. Its speed must be within
/// 5 % of the true speed where the noise resolves it (depth ≥ 200 × noise
/// amplitude), and within max(5 %, 12 × amplitude / depth) otherwise — the
/// noise of a 1 kHz reading over a 10–20 ms stroke makes better impossible
/// for shallow pulls. Returns (launches, losses, worst error).
fn grid(noise: Noise, tracking: f32) -> (u32, u32, f32) {
    let cfg = PlungerConfig {
        rest_tracking_per_s: tracking,
        ..PlungerConfig::default()
    };
    let a = noise.amplitude();
    let (mut launches, mut lost, mut worst) = (0u32, 0u32, 0.0f32);
    for pull_ms in [300u64, 400, 1000, 3000] {
        for depth in [0.1f32, 0.4, 0.8] {
            for seed in 0..200u64 {
                let mut r = Rig::new(
                    cfg,
                    Some(noise),
                    seed * 1009 + pull_ms + (depth * 10.0) as u64,
                );
                let snap_ms = 10 + r.rng.next() % 11;
                r.hold(0.0, 2000);
                r.ramp(0.0, depth, pull_ms);
                r.hold(depth, 200);
                r.ramp(depth, 0.0, snap_ms);
                r.hold(0.0, 300);
                let rel = r.take();
                launches += 1;
                let what = format!(
                    "{noise:?} tracking {tracking} pull {pull_ms} depth {depth} seed {seed}"
                );
                if rel.is_empty() {
                    lost += 1;
                    continue;
                }
                assert_eq!(rel.len(), 1, "{what}: {rel:?}");
                let want = depth / snap_ms as f32 * 1000.0;
                let err = (rel[0] - want).abs() / want;
                worst = worst.max(err);
                let allowed = if depth >= 200.0 * a {
                    0.05
                } else {
                    (12.0 * a / depth).max(0.05)
                };
                assert!(
                    err <= allowed,
                    "{what}: {} vs {want} ({:.1} %)",
                    rel[0],
                    err * 100.0
                );
            }
        }
    }
    (launches, lost, worst)
}

fn check_grid(noise: Noise) {
    for tracking in [1.0f32, 0.0] {
        let (n, lost, worst) = grid(noise, tracking);
        assert_eq!(
            lost, 0,
            "{noise:?} tracking {tracking}: {lost}/{n} launches lost"
        );
        println!(
            "{noise:?} tracking {tracking}: {n} launches, 0 lost, worst {:.1} %",
            worst * 100.0
        );
    }
}

#[test]
fn noisy_launches_uniform_0_001() {
    check_grid(Noise::ALL[0]);
}

#[test]
fn noisy_launches_uniform_0_002() {
    check_grid(Noise::ALL[1]);
}

#[test]
fn noisy_launches_uniform_0_004() {
    check_grid(Noise::ALL[2]);
}

#[test]
fn noisy_launches_uniform_0_008() {
    check_grid(Noise::ALL[3]);
}

#[test]
fn noisy_launches_8_bit() {
    check_grid(Noise::ALL[4]);
}

#[test]
fn noisy_launches_10_bit() {
    check_grid(Noise::ALL[5]);
}

/// The reviewer's minimal case: an edge crossing, one sample back inside,
/// then a clean launch.
#[test]
fn edge_dip_before_the_pull_keeps_the_launch() {
    let mut r = Rig::new(PlungerConfig::default(), None, 0);
    r.hold(0.0, 2000);
    r.feed(0.021);
    r.feed(0.019);
    r.ramp(0.019, 0.8, 300);
    r.hold(0.8, 200);
    r.ramp(0.8, 0.0, 20);
    r.hold(0.0, 300);
    let rel = r.take();
    assert_eq!(rel.len(), 1);
    assert!((rel[0] - 40.0).abs() < 0.4, "{}", rel[0]);
}

/// A hesitant start: wiggles across the deadzone edge before the pull.
#[test]
fn hesitant_start_keeps_the_launch() {
    for noise in [None, Some(Noise::Uniform(0.004)), Some(Noise::Bits(8))] {
        for seed in 0..50 {
            let mut r = Rig::new(PlungerConfig::default(), noise, seed);
            r.hold(0.0, 2000);
            for _ in 0..2 {
                r.ramp(0.0, 0.03, 15);
                r.ramp(0.03, 0.01, 15);
                r.ramp(0.01, 0.03, 15);
                r.ramp(0.03, 0.0, 15);
            }
            r.ramp(0.0, 0.6, 400);
            r.hold(0.6, 100);
            r.ramp(0.6, 0.0, 15);
            r.hold(0.0, 300);
            assert_eq!(r.take().len(), 1, "{noise:?} seed {seed}");
        }
    }
}

/// B1 a–d still hold with noise at rest: nothing launches.
#[test]
fn noisy_rest_and_b1_cases_never_launch() {
    for noise in Noise::ALL {
        for seed in 0..20 {
            for tracking in [1.0f32, 0.0] {
                let cfg = PlungerConfig {
                    rest_tracking_per_s: tracking,
                    ..PlungerConfig::default()
                };
                let what = format!("{noise:?} seed {seed} tracking {tracking}");
                // Plain rest.
                let mut r = Rig::new(cfg, Some(noise), seed);
                r.hold(0.0, 3000);
                // (a) one sample at 0.025, and one below the qualifying depth.
                r.feed(0.025);
                r.hold(0.0, 200);
                r.feed(0.049);
                r.hold(0.0, 200);
                // Two-sample spikes.
                r.feed(0.3);
                r.feed(0.3);
                r.hold(0.0, 300);
                assert!(r.take().is_empty(), "{what}: rest/spikes");
                // (b) resting at the deadzone edge with jitter.
                let mut r = Rig::new(cfg, Some(noise), seed + 100);
                for i in 0..3000 {
                    r.feed(if i % 2 == 0 { 0.015 } else { 0.021 });
                }
                assert!(r.take().is_empty(), "{what}: edge jitter");
                // (c) 8-bit code toggle.
                let mut r = Rig::new(cfg, Some(noise), seed + 200);
                for i in 0..2000 {
                    r.feed(if (i / 3) % 2 == 0 {
                        5.0 / 255.0
                    } else {
                        6.0 / 255.0
                    });
                }
                assert!(r.take().is_empty(), "{what}: code toggle");
                // (d) low rest, then small jitter.
                let mut r = Rig::new(cfg, Some(noise), seed + 300);
                r.hold(-0.015, 3000);
                for i in 0..2000 {
                    r.feed(if i % 2 == 0 { 0.008 } else { 0.004 });
                }
                assert!(r.take().is_empty(), "{what}: low rest jitter");
            }
        }
    }
}

/// Smooth (cosine) snaps under noise keep their peak: no systematic loss
/// from the straight-stroke estimator.
#[test]
fn noisy_cosine_snaps_report_the_peak() {
    for noise in [None, Some(Noise::Uniform(0.002)), Some(Noise::Bits(10))] {
        for snap_ms in [15u64, 25, 40] {
            for seed in 0..40 {
                let mut r = Rig::new(PlungerConfig::default(), noise, seed);
                r.hold(0.0, 2000);
                r.ramp(0.0, 0.8, 500);
                r.hold(0.8, 100);
                for k in 1..=snap_ms {
                    let u = k as f32 / snap_ms as f32;
                    r.feed(0.4 * (1.0 + (std::f32::consts::PI * u).cos()));
                }
                r.hold(0.0, 300);
                let rel = r.take();
                assert_eq!(rel.len(), 1, "{noise:?} {snap_ms} {seed}");
                let want = 0.4 * std::f32::consts::PI / snap_ms as f32 * 1000.0;
                let err = (rel[0] - want).abs() / want;
                assert!(
                    err <= 0.05,
                    "{noise:?} snap {snap_ms} seed {seed}: {} vs {want}",
                    rel[0]
                );
            }
        }
    }
}

/// N-f: an offset device (rest 0.1) held at 0.0, let go and launched at once;
/// the old rest comes back and later launches all report.
#[test]
fn offset_device_held_at_zero_recovers() {
    for quick in [true, false] {
        let mut r = Rig::new(PlungerConfig::default(), None, 0);
        r.hold(0.1, 2000);
        assert!((r.f.rest() - 0.1).abs() < 1e-3);
        r.hold(0.0, 1500);
        r.ramp(0.0, 0.1, 10);
        if quick {
            // A launch within 0.3 s of letting go (may or may not report).
            r.hold(0.1, 100);
            r.ramp(0.1, 0.9, 100);
            r.ramp(0.9, 0.1, 20);
        }
        r.hold(0.1, 2000);
        r.take();
        assert!(!r.f.is_pulled(), "rest {}", r.f.rest());
        for _ in 0..3 {
            r.ramp(0.1, 0.9, 300);
            r.hold(0.9, 100);
            r.ramp(0.9, 0.1, 20);
            r.hold(0.1, 500);
            let rel = r.take();
            assert_eq!(rel.len(), 1, "quick {quick}");
            assert!((rel[0] - 40.0).abs() < 2.0, "{}", rel[0]);
        }
    }
}

/// N-g: a device resting below nominal at power-on launches at 5 s.
#[test]
fn power_on_rest_below_nominal() {
    for rest in [-0.05f32, -0.2, -0.25] {
        for noise in [None, Some(Noise::Uniform(0.004))] {
            let mut r = Rig::new(PlungerConfig::default(), noise, 1);
            r.hold(rest, 5000);
            assert!((r.f.rest() - rest).abs() < 0.01, "{rest}: {}", r.f.rest());
            r.ramp(rest, rest + 0.8, 300);
            r.hold(rest + 0.8, 100);
            r.ramp(rest + 0.8, rest, 20);
            r.hold(rest, 300);
            let rel = r.take();
            assert_eq!(rel.len(), 1, "{rest} {noise:?}");
            assert!((rel[0] - 40.0).abs() < 2.0, "{}", rel[0]);
        }
    }
}
