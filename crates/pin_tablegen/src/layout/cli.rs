//! The `pin_tablegen` command line:
//! `pin_tablegen <layout.toml> --table <table.toml> --out <bundle_dir>`.

use super::{GenError, generate, parse_layout_bytes, write_bundle};
use core::fmt;
use pin_core::schema::TableManifest;
use std::path::{Path, PathBuf};

/// Usage text.
pub const USAGE: &str = "usage: pin_tablegen <layout.toml> --table <table.toml> --out <bundle_dir>";

/// Largest input file read, bytes (the loader's manifest limit).
pub const MAX_INPUT_BYTES: u64 = 4 << 20;

/// Why the command failed.
#[derive(Debug)]
pub enum CliError {
    /// Bad arguments (exit code 2).
    Usage(String),
    /// An input could not be read or parsed, the layout cannot be
    /// generated, or the bundle could not be written (exit code 1).
    Failed(String),
}

impl CliError {
    /// Process exit code.
    pub fn exit_code(&self) -> i32 {
        match self {
            CliError::Usage(_) => 2,
            CliError::Failed(_) => 1,
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CliError::Usage(m) => write!(f, "{m}\n{USAGE}"),
            CliError::Failed(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for CliError {}

impl From<GenError> for CliError {
    fn from(e: GenError) -> Self {
        CliError::Failed(format!("layout.toml: {e}"))
    }
}

struct Args {
    layout: PathBuf,
    table: PathBuf,
    out: PathBuf,
}

fn parse_args(args: &[String]) -> Result<Args, CliError> {
    let mut layout = None;
    let mut table = None;
    let mut out = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let (key, inline) = match a.split_once('=') {
            Some((k, v)) if k.starts_with("--") => (k, Some(v.to_owned())),
            _ => (a.as_str(), None),
        };
        let slot = match key {
            "--table" => &mut table,
            "--out" => &mut out,
            "-h" | "--help" => return Err(CliError::Usage("help requested".into())),
            k if k.starts_with('-') && k.len() > 1 => {
                return Err(CliError::Usage(format!("unknown option {k:?}")));
            }
            _ => {
                if layout.replace(PathBuf::from(a)).is_some() {
                    return Err(CliError::Usage(format!("unexpected argument {a:?}")));
                }
                continue;
            }
        };
        let value = match inline {
            Some(v) => v,
            None => it
                .next()
                .cloned()
                .ok_or_else(|| CliError::Usage(format!("{key} needs a value")))?,
        };
        if slot.replace(PathBuf::from(value)).is_some() {
            return Err(CliError::Usage(format!("{key} given twice")));
        }
    }
    Ok(Args {
        layout: layout.ok_or_else(|| CliError::Usage("missing <layout.toml>".into()))?,
        table: table.ok_or_else(|| CliError::Usage("missing --table <table.toml>".into()))?,
        out: out.ok_or_else(|| CliError::Usage("missing --out <bundle_dir>".into()))?,
    })
}

fn read(path: &Path) -> Result<Vec<u8>, CliError> {
    let fail = |e: std::io::Error| CliError::Failed(format!("{}: {e}", path.display()));
    let size = std::fs::metadata(path).map_err(fail)?.len();
    if size > MAX_INPUT_BYTES {
        return Err(CliError::Failed(format!(
            "{}: file is {size} bytes (max {MAX_INPUT_BYTES})",
            path.display()
        )));
    }
    std::fs::read(path).map_err(fail)
}

/// Runs the command with `args` (without the program name). On success,
/// returns a one-line summary.
pub fn run_cli(args: &[String]) -> Result<String, CliError> {
    let args = parse_args(args)?;
    // Writing the bundle must never overwrite the hand-written base.
    if let (Ok(base), Ok(out)) = (
        std::fs::canonicalize(&args.table),
        std::fs::canonicalize(&args.out),
    ) && base == out.join(super::TABLE_FILE)
    {
        return Err(CliError::Failed(format!(
            "{}: the base table.toml is the bundle's own table.toml, which the tool \
             overwrites; keep the hand-written base outside the bundle folder",
            args.table.display()
        )));
    }
    let layout = parse_layout_bytes(&read(&args.layout)?)
        .map_err(|e| CliError::Failed(format!("{}: {}", args.layout.display(), e.message)))?;
    let table_bytes = read(&args.table)?;
    let table_text = String::from_utf8(table_bytes).map_err(|e| {
        CliError::Failed(format!(
            "{}: not valid UTF-8 text: {e}",
            args.table.display()
        ))
    })?;
    let base = TableManifest::from_toml_str(&table_text)
        .map_err(|e| CliError::Failed(format!("{}: {e}", args.table.display())))?;
    let generated = generate(&layout, &base)
        .map_err(|e| CliError::Failed(format!("{}: {e}", args.layout.display())))?;
    write_bundle(&generated, &args.out)
        .map_err(|e| CliError::Failed(format!("{}: {e}", args.out.display())))?;
    Ok(format!(
        "wrote {} ({} bytes) and {} to {}",
        super::PLAYFIELD_FILE,
        generated.glb.len(),
        super::TABLE_FILE,
        args.out.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn argument_parsing() {
        let a = parse_args(&s(&["l.toml", "--table", "t.toml", "--out=o"])).unwrap();
        assert_eq!(a.layout, PathBuf::from("l.toml"));
        assert_eq!(a.table, PathBuf::from("t.toml"));
        assert_eq!(a.out, PathBuf::from("o"));
        for bad in [
            &["--table", "t", "--out", "o"][..],
            &["l", "--out", "o"],
            &["l", "--table", "t"],
            &["l", "--table"],
            &["l", "m", "--table", "t", "--out", "o"],
            &["l", "--table", "t", "--table", "u", "--out", "o"],
            &["l", "--bogus", "t"],
            &["--help"],
        ] {
            let e = parse_args(&s(bad)).err().unwrap();
            assert_eq!(e.exit_code(), 2, "{bad:?}");
            assert!(e.to_string().contains("usage:"));
        }
    }

    /// 4.3 fix round 1, N4: the base is never overwritten.
    #[test]
    fn base_inside_the_output_is_refused() {
        let dir = std::env::temp_dir().join(format!("pin_tablegen_n4_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let base = dir.join("table.toml");
        std::fs::write(&base, crate::layout::tests::BASE).unwrap();
        let layout = dir.join("layout.toml");
        std::fs::write(
            &layout,
            "schema_version = 1\n[playfield]\nwidth = 100.0\nlength = 100.0\nwall_height = 10.0\n",
        )
        .unwrap();
        let d = dir.to_str().unwrap();
        let args = s(&[
            layout.to_str().unwrap(),
            "--table",
            base.to_str().unwrap(),
            "--out",
            &format!("{d}/./"),
        ]);
        let e = run_cli(&args).unwrap_err();
        assert_eq!(e.exit_code(), 1);
        assert!(e.to_string().contains("outside the bundle folder"), "{e}");
        assert_eq!(
            std::fs::read_to_string(&base).unwrap(),
            crate::layout::tests::BASE
        );
        // Elsewhere it works.
        let out = dir.join("out");
        let args = s(&[
            layout.to_str().unwrap(),
            "--table",
            base.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ]);
        run_cli(&args).unwrap();
        assert!(out.join("playfield.glb").is_file());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_files_fail() {
        let e = run_cli(&s(&[
            "/nonexistent/layout.toml",
            "--table",
            "/nonexistent/t.toml",
            "--out",
            "/nonexistent/out",
        ]))
        .unwrap_err();
        assert_eq!(e.exit_code(), 1);
        assert!(e.to_string().contains("/nonexistent/layout.toml"));
        let g: CliError = GenError::new("walls[0]", "x").into();
        assert_eq!(g.to_string(), "layout.toml: walls[0]: x");
    }
}
