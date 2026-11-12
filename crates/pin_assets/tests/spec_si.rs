//! Subphase 4.1 spec tests (Independent Tester): unit conversion to SI.
//! Brief: GOAL "convert units to SI", `LoadedTable` "SI versions of every
//! positional declaration: metres, kg, radians; field names *_m / *_rad / *_kg",
//! D17 (TOML uses mm/g/deg, converted at load).

#[path = "spec_fixtures/mod.rs"]
mod fx;

use fx::{BACKGLASS_TOML, TABLE_TOML};
use pin_assets::load_table;
use pin_core::units::{mm_to_m, mm3_to_m};

fn close3(got: [f32; 3], want: [f32; 3], what: &str) {
    for i in 0..3 {
        assert!(
            (got[i] - want[i]).abs() <= 1e-6 * want[i].abs().max(1.0),
            "{what}[{i}] = {} want {} (SI metres)",
            got[i],
            want[i]
        );
    }
}

fn close(got: f32, want: f32, what: &str) {
    assert!(
        (got - want).abs() <= 1e-6 * want.abs().max(1.0),
        "{what} = {got} want {want}"
    );
}

#[test]
fn positional_declarations_are_converted_to_si() {
    let t = load_table(&fx::vfs_from(&fx::bundle_with(TABLE_TOML, BACKGLASS_TOML)))
        .unwrap_or_else(|e| panic!("{e}"));

    // switches: position mm -> m, radius mm -> m; absent stays absent.
    let sw = &t.switches[0];
    close3(sw.position_m.expect("declared position"), mm3_to_m([410.0, 720.0, 0.0]), "switch position");
    close(sw.radius_m.expect("declared radius"), mm_to_m(12.0), "switch radius");
    assert!(t.switches[1].position_m.is_none());
    assert!(t.switches[1].radius_m.is_none());

    // magnets: position, d_min, range in metres.
    let m = &t.magnets[0];
    close3(m.position_m, [0.4, 0.7, 0.0], "magnet position");
    close(m.d_min_m, 0.008, "magnet d_min");
    close(m.range_m, 0.06, "magnet range");

    // splines: through-points in metres, gauge in metres, roll in radians.
    let s = &t.splines[0];
    assert_eq!(s.points_m.len(), 3);
    close3(s.points_m[0], [0.06, 0.9, 0.04], "spline point 0");
    close3(s.points_m[2], [0.15, 0.26, 0.02], "spline point 2");

    // layer transitions: zone corners in metres.
    let z = &t.transitions[0];
    close3(z.min_m, [0.24, 0.64, 0.0], "zone min");
    close3(z.max_m, [0.29, 0.68, 0.03], "zone max");

    // dispensers: exit position in metres; exit velocity is already m/s.
    let d = &t.dispensers[0];
    close3(d.exit_position_m, [0.3, 0.88, 0.01], "dispenser exit");
}
