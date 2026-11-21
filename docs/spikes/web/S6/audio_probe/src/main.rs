use s6_audio_probe::*;
fn main() {
    let quanta: u32 = 600;
    let mut st = new_state(48_000, 128);
    let mut h = 0xcbf29ce484222325u64;
    let mut peak = 0f32;
    for q in 0..quanta {
        for &(at, id, g) in SCRIPT { if at == q { trigger(&mut st, id, g); } }
        let out = render(&mut st, 128);
        for x in out { peak = peak.max(x.abs()); }
        fnv(&mut h, out);
    }
    println!("native quanta={quanta} frames={} fnv={h:016x} peak={peak}", quanta * 128);
}
