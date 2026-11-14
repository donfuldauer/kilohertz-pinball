#[path = "fixtures/spec_2_8/support.rs"]
mod support;
use pin_tablegen::clearance::validate_clearance;
use support::*;

#[test]
fn small_areas() {
    let big = r#"
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
z = 65.0
outline = [[10.0, 10.0], [4990.0, 10.0], [4990.0, 4990.0], [10.0, 4990.0]]
material = "acrylic"
"#;
    let std_deck = layout_toml("\n[[decks]]\nlayer = 1\nz = 65.0\noutline = [[260.0, 700.0], [514.0, 700.0], [514.0, 1070.0], [260.0, 1070.0]]\nmaterial = \"acrylic\"\n");
    for (tag, layout, x0, y0) in [("big", big.to_owned(), 1234.3f32, 3456.7f32), ("big2", big.to_owned(), 2500.0, 2500.0), ("std", std_deck.clone(), 383.3, 881.7), ("std2", std_deck.clone(), 351.9, 803.3), ("std3", std_deck.clone(), 420.1, 955.5)] {
        for size in [200.0f32, 50.0, 20.0, 8.0, 3.0, 1.0, 0.2] {
            let area = [[x0, y0], [x0 + size, y0], [x0 + size, y0 + size], [x0, y0 + size]];
            let r = validate(tag, &layout, &table_toml(&balls_restricted(&[STD, XL], XL, &[0], Some(&area)), ""));
            println!("{tag} area {size} mm at ({x0}, {y0}): ok={} errors={}", r.is_ok(), r.errors.len());
        }
    }
}
