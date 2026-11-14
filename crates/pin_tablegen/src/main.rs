//! `pin_tablegen <layout.toml> --table <table.toml> --out <bundle_dir>`:
//! generates `playfield.glb` and `table.toml` (see `pin_tablegen::layout`).
//!
//! `pin_tablegen validate <bundle>`: loads a bundle and checks that every
//! declared ball fits its geometry (see `pin_tablegen::clearance`).

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("validate") => pin_tablegen::clearance::run_validate_cli(&args[1..]),
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
