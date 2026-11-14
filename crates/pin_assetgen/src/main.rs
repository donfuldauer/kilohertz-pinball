//! `pin_assetgen --title <t> --palette <hex,...> --seed <n> --out <dir>`:
//! writes the placeholder asset kit into a bundle directory.

fn main() {
    match pin_assetgen::cli::run(std::env::args().skip(1)) {
        Ok(manifest) => {
            println!(
                "pin_assetgen: wrote {} assets and {}",
                manifest.entries.len(),
                pin_assetgen::MANIFEST_FILE
            );
        }
        Err(msg) if msg == pin_assetgen::cli::USAGE => {
            // --help / -h: usage on stdout, success.
            println!("{msg}");
        }
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    }
}
