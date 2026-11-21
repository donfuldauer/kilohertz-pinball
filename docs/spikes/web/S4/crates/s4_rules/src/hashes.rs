//! Per-tick hashes: the repo's [`Simulation::state_hash`] plus a stronger
//! supplementary hash over everything else the sim publishes.

use pin_sim::Simulation;
use pin_sim::hash::Fnv64;

/// FNV-1a 64 over: `state_hash`; the full published snapshot (every ball,
/// flipper incl. zipper offset, dynamic mesh transform, lamp and GI value as
/// exact bits); each ball's magnetisation; the sim counters; the 64
/// persistent values; the last applied command queue; the haptic events.
/// It covers what `state_hash` leaves out (mechs, magnetisation, lamps,
/// persist, command queue).
#[must_use]
pub fn ext_hash(sim: &Simulation) -> u64 {
    let mut h = Fnv64::new();
    h.write_u64(sim.state_hash());
    let s = sim.snapshot();
    h.write_u64(s.tick);
    h.write_u8(s.ball_count);
    for b in s.balls() {
        for v in b
            .position_m
            .iter()
            .chain(&b.velocity_m_s)
            .chain(&b.angular_velocity_rad_s)
        {
            h.write_f32(*v);
        }
        for v in b.orientation {
            h.write_f32(v);
        }
        h.write_u32(u32::from(b.profile.0));
        h.write_u8(b.layer);
    }
    for f in s.flippers() {
        h.write_u32(u32::from(f.id.0));
        h.write_f32(f.angle_rad);
        h.write_f32(f.angular_velocity_rad_s);
        h.write_f32(f.zipper_offset_m);
    }
    for m in s.dynamic_meshes() {
        for v in m.position_m.iter().chain(&m.orientation) {
            h.write_f32(*v);
        }
    }
    for l in s.lamps {
        h.write_f32(l);
    }
    for g in s.gi {
        h.write_f32(g);
    }
    for bh in sim.ball_handles() {
        h.write_f32(sim.magnetisation(bh));
    }
    let c = sim.counters();
    for v in [
        c.edges_dropped,
        c.haptics_dropped,
        c.inputs_dropped,
        c.spawns_failed,
        c.ball_events_dropped,
        c.balls_drained,
        c.balls_escaped,
    ] {
        h.write_u32(v);
    }
    for k in 0..64u16 {
        h.write_i64(sim.persist(k));
    }
    let q = sim.last_commands();
    for &(c, ms) in &q.coil_pulses {
        h.write_u32(u32::from(c.0) << 16 | u32::from(ms));
    }
    h.write_u8(0xA0);
    for &(c, on) in &q.coil_holds {
        h.write_u32(u32::from(c.0) << 1 | u32::from(on));
    }
    h.write_u8(0xA1);
    for &(m, p) in &q.magnets {
        h.write_u32(u32::from(m.0));
        h.write_f32(p);
    }
    h.write_u8(0xA2);
    for &(l, i) in &q.lamps {
        h.write_u32(u32::from(l.0));
        h.write_f32(i);
    }
    h.write_u8(0xA3);
    for &(p, s) in &q.scores {
        h.write_u8(p);
        h.write_i64(s);
    }
    h.write_u8(0xA4);
    for s in &q.display_scenes {
        h.write_u32(u32::from(s.0));
    }
    for &(s, g) in &q.sounds {
        h.write_u32(u32::from(s.0));
        h.write_f32(g);
    }
    for e in sim.haptic_events() {
        h.write_u64(e.tick);
        h.write_u32(u32::from(e.source));
        h.write_f32(e.strength);
    }
    h.finish()
}
