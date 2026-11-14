#[path = "fixtures/spec_2_8/support.rs"]
mod support;
use pin_tablegen::clearance::validate_clearance;
use support::*;

#[test]
fn big() {
    let layout = r#"
schema_version = 1
[playfield]
width = 5000.0
length = 5000.0
wall_height = 28.0
[[walls]]
points = [[0.0, 0.0], [0.0, 5000.0], [5000.0, 5000.0], [5000.0, 0.0]]
closed = false
material = "wood"
[[decks]]
layer = 1
z = 10.0
outline = [[10.0, 10.0], [4990.0, 10.0], [4990.0, 4990.0], [10.0, 4990.0]]
material = "acrylic"
"#;
    let dir = bundle("big", layout, &table_toml(&balls(&[MINI, STD, XL]), ""));
    let l = load(&dir);
    let t = std::time::Instant::now();
    let r = validate_clearance(&l.table, &l.scene);
    println!("5 m x 5 m deck: {:?}\n{}", t.elapsed(), describe(&r));
    for s in [10.0f32, 100.0, 1000.0] {
        let dir = bundle("big2", &layout_toml("\n[[decks]]\nlayer = 1\nz = 65.0\noutline = [[260.0, 700.0], [514.0, 700.0], [514.0, 1070.0], [260.0, 1070.0]]\nmaterial = \"acrylic\"\n"), &table_toml(&balls(&[MINI, STD, XL]), ""));
        let mut l = load(&dir);
        for m in &mut l.scene.meshes {
            m.transform = [[s, 0.0, 0.0, 0.0], [0.0, s, 0.0, 0.0], [0.0, 0.0, s, 0.0], [0.0, 0.0, 0.0, 1.0]];
        }
        let t = std::time::Instant::now();
        let r = validate_clearance(&l.table, &l.scene);
        println!("scene x{s}: {:?}\n{}", t.elapsed(), describe(&r));
    }
}
