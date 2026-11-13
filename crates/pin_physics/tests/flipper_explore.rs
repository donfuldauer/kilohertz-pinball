#[path = "flipper_util.rs"]
mod util;
use util::*;

/// Ball dropped (−y, 3.5 m/s at contact) onto the bat at `fraction` of its
/// length as seen with the bat at `aim_deg`.
fn drop_rig(fraction: f32, aim_deg: f32, lead_ticks: u32) -> (Rig, pin_physics::ball::BallHandle) {
    let mut rig = Rig::new(&[left_config(0, [0.0, 0.0, 0.0])]);
    let dir = bat_dir(aim_deg);
    let n = v(-dir.y, dir.x, 0.0);
    let radius = 0.011 + (0.0065 - 0.011) * fraction;
    let contact = dir * (BAT_LEN * fraction) + n * (radius + BALL_R);
    let speed = 3.5;
    let start = contact + v(0.0, speed * lead_ticks as f32 * DT, 0.0);
    let h = rig.add_ball(start + v(0.0, 0.0, BALL_R), v(0.0, -speed, 0.0));
    (rig, h)
}

#[test]
#[ignore]
fn explore_stroke() {
    let mut rig = Rig::new(&[left_config(0, [0.0, 0.0, 0.0])]);
    rig.flippers.set_energised(LEFT, true);
    for t in 0..60 {
        rig.step();
        println!(
            "{t:3} angle {:8.3} w {:8.2} eos {} torque {:.3}",
            rig.flippers.angle_deg(LEFT),
            rig.flippers.angular_velocity(LEFT),
            rig.flippers.eos_closed(LEFT),
            rig.flippers.coil_torque_nm(LEFT)
        );
    }
}

fn rebound(flip_at: Option<i64>, release_at: Option<i64>, held: bool, fraction: f32, verbose: bool) -> (f32, f32, i64) {
    let lead = 60u32;
    let (mut rig, h) = drop_rig(fraction, 20.0, lead);
    // Settle the bat first without moving the ball: pre-run flippers alone.
    if held {
        rig.flippers.set_energised(LEFT, true);
        for _ in 0..200 {
            rig.flippers.advance(DT);
        }
    }
    let mut first_hit = -1i64;
    let mut best = f32::NEG_INFINITY;
    let mut end_speed = 0.0;
    for t in 0..(lead as i64 + 150) {
        if let Some(f) = flip_at && t == lead as i64 + f {
            rig.flippers.set_energised(LEFT, true);
        }
        if let Some(f) = release_at && t == lead as i64 + f {
            rig.flippers.set_energised(LEFT, false);
        }
        rig.step();
        if first_hit < 0 && !rig.hits.is_empty() {
            first_hit = t;
        }
        let b = rig.ball(h);
        if verbose {
            println!("{t} ball y {:.4} vy {:.3} vx {:.3} | bat {:.2} w {:.1} hits {:?}", b.position.y, b.velocity.y, b.velocity.x, rig.flippers.angle_deg(LEFT), rig.flippers.angular_velocity(LEFT), rig.hits);
        }
        if first_hit >= 0 {
            if t <= first_hit + 60 {
                best = best.max(b.velocity.y);
                end_speed = b.velocity.length();
            }
        }
    }
    (best, end_speed, first_hit - lead as i64)
}

#[test]
#[ignore]
fn explore_sweep() {
    for fraction in [0.5f32, 0.7] {
        let (held, hs, ht) = rebound(None, None, true, fraction, false);
        println!("fraction {fraction}: held: max vy {held:.3} speed60 {hs:.3} hit at {ht}");
        let (dead, ds, dt_) = rebound(None, None, false, fraction, false);
        println!("rest (never flipped): max vy {dead:.3} speed60 {ds:.3} hit at {dt_}");
        for k in -30..=20 {
            let (r, s, t) = rebound(Some(k), None, false, fraction, false);
            println!("flip {k:4} ms: max vy {r:7.3} speed60 {s:7.3} first hit {t}");
        }
        for k in [-10i64, -5, -3, -2, -1] {
            let lead = 60;
            let _ = lead;
            // drop catch: held, release k ms before impact
            let (mut best, mut s) = (0.0, 0.0);
            {
                let (mut rig, h) = drop_rig(fraction, 20.0, 60);
                rig.flippers.set_energised(LEFT, true);
                for _ in 0..200 { rig.flippers.advance(DT); }
                let mut first = -1i64;
                let mut b_ = f32::NEG_INFINITY;
                for t in 0..210i64 {
                    if t == 60 + k { rig.flippers.set_energised(LEFT, false); }
                    rig.step();
                    if first < 0 && !rig.hits.is_empty() { first = t; }
                    if first >= 0 && t <= first + 60 { b_ = b_.max(rig.ball(h).velocity.y); s = rig.ball(h).velocity.length(); }
                }
                best = f32::max(best, b_);
            }
            println!("release {k} ms: max vy {best:.3} speed60 {s:.3}");
        }
    }
}

#[test]
#[ignore]
fn explore_trace() {
    let k: i64 = std::env::var("K").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    let held = std::env::var("HELD").is_ok();
    let r = rebound(if held { None } else { Some(k) }, None, held, 0.5, true);
    println!("{r:?}");
}
