//! The `pin_assetgen` command line:
//!
//! ```text
//! pin_assetgen --title <t> [--palette <hex,...>] [--seed <n>] --out <dir>
//! ```
//!
//! `--palette` takes comma-separated `#rrggbb` (or `rrggbb`) colours; it
//! defaults to empty (the kit's default palette). `--seed` is a decimal or
//! `0x` hexadecimal `u64`, default 0. Every option also accepts `--opt=value`.

use std::path::PathBuf;

use crate::{KitManifest, TableMeta, generate_kit};

/// Usage text.
pub const USAGE: &str = "usage: pin_assetgen --title <t> [--palette <hex,...>] [--seed <n>] --out <dir>";

/// Parsed command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Args {
    /// Table metadata.
    pub meta: TableMeta,
    /// Bundle directory to write into.
    pub out: PathBuf,
}

/// Parses `#rrggbb` / `rrggbb`.
pub fn parse_color(s: &str) -> Result<[u8; 3], String> {
    let hex = s.trim();
    let hex = hex.strip_prefix('#').unwrap_or(hex);
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("invalid colour {s:?} (expected #rrggbb)"));
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|e| e.to_string());
    Ok([byte(0)?, byte(2)?, byte(4)?])
}

/// Parses a comma-separated palette; an empty string is an empty palette.
pub fn parse_palette(s: &str) -> Result<Vec<[u8; 3]>, String> {
    if s.trim().is_empty() {
        return Ok(Vec::new());
    }
    s.split(',').map(parse_color).collect()
}

/// Parses a decimal or `0x` hexadecimal `u64`.
pub fn parse_seed(s: &str) -> Result<u64, String> {
    let t = s.trim();
    let r = match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        Some(h) => u64::from_str_radix(h, 16),
        None => t.parse::<u64>(),
    };
    r.map_err(|e| format!("invalid seed {s:?}: {e}"))
}

/// Parses the arguments (without the program name). `Err` carries a message
/// for the user; `--help` returns `Err(USAGE)`.
pub fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Result<Args, String> {
    let mut title = None;
    let mut palette = Vec::new();
    let mut seed = 0;
    let mut out = None;
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        if arg == "--help" || arg == "-h" {
            return Err(USAGE.into());
        }
        let (key, inline) = match arg.split_once('=') {
            Some((k, v)) if k.starts_with("--") => (k.to_owned(), Some(v.to_owned())),
            _ => (arg.clone(), None),
        };
        let mut value = || {
            inline
                .clone()
                .or_else(|| it.next())
                .ok_or_else(|| format!("{key} needs a value\n{USAGE}"))
        };
        match key.as_str() {
            "--title" => title = Some(value()?),
            "--palette" => palette = parse_palette(&value()?)?,
            "--seed" => seed = parse_seed(&value()?)?,
            "--out" => out = Some(PathBuf::from(value()?)),
            _ => return Err(format!("unknown argument {arg:?}\n{USAGE}")),
        }
    }
    let title = title.ok_or_else(|| format!("--title is required\n{USAGE}"))?;
    let out = out.ok_or_else(|| format!("--out is required\n{USAGE}"))?;
    Ok(Args {
        meta: TableMeta {
            title,
            palette,
            seed,
        },
        out,
    })
}

/// Parses the arguments and generates the kit.
pub fn run<I: IntoIterator<Item = String>>(args: I) -> Result<KitManifest, String> {
    let a = parse_args(args)?;
    generate_kit(&a.meta, &a.out).map_err(|e| format!("{}: {e}", a.out.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_full_command_line() {
        let a = parse_args(v(&[
            "--title",
            "Apex Circuit",
            "--palette",
            "#0b1d3a,f2a900, #E63946",
            "--seed=0x10",
            "--out",
            "/tmp/x",
        ]))
        .unwrap();
        assert_eq!(a.meta.title, "Apex Circuit");
        assert_eq!(a.meta.palette, vec![[0x0b, 0x1d, 0x3a], [0xf2, 0xa9, 0], [0xe6, 0x39, 0x46]]);
        assert_eq!(a.meta.seed, 16);
        assert_eq!(a.out, PathBuf::from("/tmp/x"));
    }

    #[test]
    fn errors() {
        assert!(parse_args(v(&["--out", "x"])).unwrap_err().contains("--title"));
        assert!(parse_args(v(&["--title", "t"])).unwrap_err().contains("--out"));
        assert!(parse_args(v(&["--title"])).unwrap_err().contains("needs a value"));
        assert!(parse_args(v(&["--bogus"])).unwrap_err().contains("unknown"));
        assert_eq!(parse_args(v(&["--help"])).unwrap_err(), USAGE);
        assert!(parse_args(v(&["--title", "t", "--seed", "-1", "--out", "x"])).is_err());
        assert!(parse_palette("#12345").is_err());
        assert!(parse_palette("#12345g").is_err());
        assert!(parse_palette("#1234567").is_err());
        assert!(parse_palette("é12345").is_err());
        assert!(parse_palette(" ").unwrap().is_empty());
        assert_eq!(parse_seed("18446744073709551615").unwrap(), u64::MAX);
        assert!(parse_seed("18446744073709551616").is_err());
        assert!(run(v(&["--title"])).is_err());
    }
}
