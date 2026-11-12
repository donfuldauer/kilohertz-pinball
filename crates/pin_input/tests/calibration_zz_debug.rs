use pin_input::calibration::*;
#[test]
fn dbg() {
    let mut f = PlungerFilter::new(PlungerConfig::default());
    let mut t = 0u64;
    let mut tr: Vec<f32> = vec![0.5869142];
    for k in 1..=500 { tr.push(0.5869142 + (-0.02664064 - 0.5869142) * k as f32 / 500.0); }
    tr.extend(vec![-0.02664064; 667]);
    for k in 1..=300 { tr.push(-0.02664064 + (0.02664064) * k as f32 / 300.0); }
    tr.extend(vec![0.0; 2000]);
    for k in 1..=300 { tr.push(0.1 * k as f32 / 300.0); }
    tr.extend(vec![0.1; 100]);
    for k in 1..=5 { tr.push(0.1 - 0.1 * k as f32 / 5.0); }
    tr.extend(vec![0.0; 300]);
    let mut prev = (f32::NAN, false);
    for x in tr {
        let o = f.update(x, t);
        if (f.rest(), f.is_pulled()) != prev || o.release.is_some() {
            println!("t={t} raw={x:.4} rest={:.5} pulled={} pos={:.3} rel={:?}", f.rest(), f.is_pulled(), o.position, o.release);
            prev = (f.rest(), f.is_pulled());
        }
        t += 1;
    }
}
