//! `pin_sim` command line: run, record and replay a table headlessly
//! (Subphase 2.5). See [`pin_sim::cli`].

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match pin_sim::cli::main_with_args(&args) {
        Ok(line) => println!("{line}"),
        Err(e) => {
            eprintln!("pin_sim: {e}");
            std::process::exit(e.exit_code());
        }
    }
}
