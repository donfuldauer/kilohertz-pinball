//! S4 native CLI (spike tooling).
//!
//! ```text
//! s4 play <engine|design> <bundle.zip> <scn> <out.hashes> [every]
//! s4 bench <design> <bundle.zip> <scn>
//! s4 alloc <design> <bundle.zip> <scn>
//! s4 adv <design> <prod.wasm>
//! s4 case <design> <case> <prod.wasm>
//! s4 worst <design> [reps] [case]
//! s4 loadtime <design> <rules.wasm> [reps]
//! s4 instrument <in.wasm> <out.wasm> [depth]
//! s4 zip <file> [web]
//! ```

use s4_rules::run::{self, Design};
use std::sync::OnceLock;
use std::time::Instant;

fn now_ms() -> f64 {
    static T0: OnceLock<Instant> = OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_secs_f64() * 1e3
}

fn read(p: &str) -> Vec<u8> {
    std::fs::read(p).unwrap_or_else(|e| panic!("{p}: {e}"))
}

fn design(s: &str) -> Design {
    Design::parse(s).unwrap_or_else(|| panic!("unknown design {s}"))
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let arg = |i: usize| a.get(i).map(String::as_str).unwrap_or("");
    match arg(1) {
        "play" => {
            let d = if arg(2) == "engine" {
                None
            } else {
                Some(design(arg(2)))
            };
            let every = a.get(6).and_then(|s| s.parse().ok()).unwrap_or(1);
            match run::play(&read(arg(3)), &read(arg(4)), d, every) {
                Ok(o) => {
                    std::fs::write(arg(5), o.lines).expect("write hashes");
                    println!("{}", o.summary);
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            }
        }
        "bench" => match run::bench(&read(arg(3)), &read(arg(4)), design(arg(2)), now_ms) {
            Ok(j) => println!("{j}"),
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        },
        "alloc" => match run::alloc_check(&read(arg(3)), &read(arg(4)), design(arg(2))) {
            Ok(j) => println!("{j}"),
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        },
        "adv" => print!(
            "{}",
            s4_rules::adversarial::run_all(design(arg(2)), &read(arg(3)))
        ),
        "case" => {
            let prod = read(arg(4));
            for c in s4_rules::adversarial::corpus(&prod) {
                if c.name == arg(3) {
                    println!("{}", s4_rules::adversarial::run_case(design(arg(2)), &c));
                    println!("{}", s4_rules::adversarial::boundary(design(arg(2)), &c));
                }
            }
        }
        "worst" => {
            let reps = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(5);
            print!(
                "{}",
                s4_rules::worst::run(design(arg(2)), now_ms, reps, a.get(4).map(String::as_str))
            );
        }
        "loadtime" => {
            let reps = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(20);
            println!(
                "{}",
                s4_rules::worst::load_time(
                    design(arg(2)),
                    &read(arg(3)),
                    &Default::default(),
                    now_ms,
                    reps
                )
            );
        }
        "instrument" => {
            let src = read(arg(2));
            let cfg = pin_script::wasm::WasmConfig::engine();
            let (bin, _) = s4_rules::abi::prepare(&src, &cfg).expect("prepare");
            let mut ic =
                s4_rules::instrument::Config::engine(cfg.fuel_per_callback, cfg.memory_limit_bytes);
            if arg(4) == "depth" {
                ic.depth_limit = Some(run::DEPTH);
            }
            if arg(5) == "jit" || arg(4) == "jit" {
                ic.host_err_import = true;
            }
            let out = s4_rules::instrument::instrument(&bin, &ic).expect("instrument");
            std::fs::write(arg(3), &out.bytes).expect("write");
            println!(
                "{} -> {} bytes, checks={} flushes={} names={:?}",
                src.len(),
                out.bytes.len(),
                out.checks,
                out.flushes,
                out.names
            );
        }
        "maxdepth" => match run::max_depth(&read(arg(2)), &read(arg(3))) {
            Ok(s) => print!("{s}"),
            Err(e) => eprintln!("error: {e}"),
        },
        "stack" => {
            let reps = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(1);
            print!(
                "{}",
                s4_rules::adversarial::stack_probe(design(arg(2)), reps)
            );
        }
        "zip" => println!(
            "{}",
            s4_rules::zipcheck::check(&read(arg(2)), arg(3) == "web")
        ),
        _ => {
            eprintln!("see the module docs for usage");
            std::process::exit(2);
        }
    }
}
