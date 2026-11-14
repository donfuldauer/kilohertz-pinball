#[path = "fixtures/spec_2_8/support.rs"]
mod support;
use support::*;

#[test]
fn probe() {
    let deck = r#"
[[decks]]
layer = 1
z = 65.0
outline = [[260.0, 700.0], [514.0, 700.0], [514.0, 1070.0], [260.0, 1070.0]]
material = "acrylic"
"#;
    let r = validate("probe", &layout_toml(deck), &table_toml(&balls(&[STD, XL]), ""));
    println!("{}", describe(&r));
    let dir = bundle("probe2", &layout_toml(deck), &table_toml(&balls(&[STD, XL]), ""));
    let l = load(&dir);
    println!("{:?}", l.scene.meshes.iter().map(|m| (&m.name, m.extras.collider, m.extras.collision_layer)).collect::<Vec<_>>());
    println!("{:?}", l.table.warnings);
}
