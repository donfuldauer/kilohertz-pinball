//! `pin_tablegen <layout.toml> --table <table.toml> --out <bundle_dir>`:
//! generates `playfield.glb` and `table.toml` (see `pin_tablegen::layout`).

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match pin_tablegen::layout::run_cli(&args) {
        Ok(summary) => println!("{summary}"),
        Err(e) => {
            eprintln!("pin_tablegen: {e}");
            std::process::exit(e.exit_code());
        }
    }
}
