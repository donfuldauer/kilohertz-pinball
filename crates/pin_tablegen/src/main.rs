//! `pin_tablegen <layout.toml> --table <table.toml> --out <bundle_dir>
//! [--force] [--backbox <backbox_layout.toml>]`: generates `playfield.glb`
//! and `table.toml` (see `pin_tablegen::layout`), and with `--backbox` also
//! `backbox.glb` and `backbox_colliders.glb` (see `pin_tablegen::backbox`).
//!
//! `pin_tablegen build <src_dir> --out <out_dir>`: builds a whole table
//! bundle from a source folder (see `pin_tablegen::backbox::cli`).
//!
//! `pin_tablegen validate <bundle>`: loads a bundle and checks that every
//! declared ball fits its geometry (see `pin_tablegen::clearance`).

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("validate") => {
            pin_tablegen::clearance::validate_cli_output(&args[1..]).map(|(summary, warnings)| {
                for w in warnings {
                    eprintln!("warning: {w}");
                }
                summary
            })
        }
        Some("build") => pin_tablegen::backbox::run_build_cli(&args[1..]),
        _ => pin_tablegen::layout::run_cli(&args),
    };
    match result {
        Ok(summary) => println!("{summary}"),
        Err(e) => {
            eprintln!("pin_tablegen: {e}");
            std::process::exit(e.exit_code());
        }
    }
}
