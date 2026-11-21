//! Bits of std (platform libm) vs `libm` crate sin/cos for the seam test's rotation half-angles,
//! plus a sweep: how many of 1 000 000 f32 inputs in [-4, 4] give different bits.
fn main() {
    use core::hint::black_box as bb;
    let pitch = 6.5f32.to_radians();
    for (n, a) in [("y(0.3)/2", 0.15f32), ("x(0.12)/2", 0.06), ("pitch6.5/2", pitch * 0.5)] {
        let (s, c) = bb(a).sin_cos();
        println!("{n}: std sin {:08x} cos {:08x} | libm sinf {:08x} cosf {:08x}", s.to_bits(), c.to_bits(), libm::sinf(bb(a)).to_bits(), libm::cosf(bb(a)).to_bits());
    }
    let (mut ds, mut dc) = (0u32, 0u32);
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for i in 0..1_000_000u32 {
        let x = (i as f32 / 1_000_000.0) * 8.0 - 4.0;
        let (s, c) = bb(x).sin_cos();
        ds += u32::from(s.to_bits() != libm::sinf(x).to_bits());
        dc += u32::from(c.to_bits() != libm::cosf(x).to_bits());
        for b in s.to_bits().to_le_bytes().into_iter().chain(c.to_bits().to_le_bytes()) {
            h = (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
        }
    }
    println!("sweep 1e6 in [-4,4]: std vs libm-crate differ: sin {ds} cos {dc}; std sin/cos digest {h:016x}");
}
