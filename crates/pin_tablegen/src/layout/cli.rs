//! The `pin_tablegen` command line:
//! `pin_tablegen <layout.toml> --table <table.toml> --out <bundle_dir>
//! [--force] [--backbox <backbox_layout.toml>]`.
//!
//! With `--backbox` the bundle also gets the backbox (`backbox.glb`,
//! `backbox_colliders.glb`, the aimed VUK and one merged `table.toml`; see
//! [`crate::backbox::cli`]).

use super::{GenError, generate, parse_layout_bytes, write_bundle};
use core::fmt;
use pin_core::schema::TableManifest;
use std::path::{Path, PathBuf};

/// Usage text.
pub const USAGE: &str = "usage: pin_tablegen <layout.toml> --table <table.toml> --out <bundle_dir> \
     [--force] [--backbox <backbox_layout.toml>]";

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
    force: bool,
    backbox: Option<PathBuf>,
}

fn parse_args(args: &[String]) -> Result<Args, CliError> {
    let mut layout = None;
    let mut table = None;
    let mut out = None;
    let mut backbox = None;
    let mut force = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let (key, inline) = match a.split_once('=') {
            Some((k, v)) if k.starts_with("--") => (k, Some(v.to_owned())),
            _ => (a.as_str(), None),
        };
        let slot = match key {
            "--table" => &mut table,
            "--out" => &mut out,
            "--backbox" => &mut backbox,
            "--force" if inline.is_none() => {
                force = true;
                continue;
            }
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
        force,
        backbox,
    })
}

/// One bounded read of at most [`MAX_INPUT_BYTES`]; errors name `path`.
fn read(path: &Path) -> Result<Vec<u8>, CliError> {
    crate::backbox::cli::read_capped(path, &path.display().to_string(), MAX_INPUT_BYTES)
        .map_err(CliError::Failed)
}

/// Runs the command with `args` (without the program name). On success,
/// returns a one-line summary followed by one `warning: ...` line per
/// [`GeneratedTable::warnings`](super::GeneratedTable::warnings) entry
/// (warnings never fail the command).
pub fn run_cli(args: &[String]) -> Result<String, CliError> {
    let args = parse_args(args)?;
    let layout = parse_layout_bytes(&read(&args.layout)?)
        .map_err(|e| CliError::Failed(format!("{}: {}", args.layout.display(), e.message)))?;
    let table_bytes = read(&args.table)?;
    let table_text = String::from_utf8(table_bytes).map_err(|e| {
        CliError::Failed(format!(
            "{}: not valid UTF-8 text: {e}",
            args.table.display()
        ))
    })?;
    // Writing the bundle must never overwrite a hand-written base (4.3 fix
    // round 1, N4). Re-running on the tool's own output in place is fine:
    // its content is carried over and its generated entries replaced.
    if let (Ok(base), Ok(out)) = (
        std::fs::canonicalize(&args.table),
        std::fs::canonicalize(&args.out),
    ) && base == out.join(super::TABLE_FILE)
        && !table_text.starts_with(super::GENERATED_HEADER)
        && !args.force
    {
        return Err(CliError::Failed(format!(
            "{}: the base table.toml is the bundle's own table.toml and was not written by \
             pin_tablegen; the tool would overwrite it (dropping its comments). Keep the \
             hand-written base outside the bundle folder, or pass --force",
            args.table.display()
        )));
    }
    let base = TableManifest::from_toml_str(&table_text)
        .map_err(|e| CliError::Failed(format!("{}: {e}", args.table.display())))?;
    let generated = generate(&layout, &base)
        .map_err(|e| CliError::Failed(format!("{}: {e}", args.layout.display())))?;
    let mut out = match &args.backbox {
        None => {
            write_bundle(&generated, &args.out)
                .map_err(|e| CliError::Failed(format!("{}: {e}", args.out.display())))?;
            format!(
                "wrote {} ({} bytes) and {} to {}",
                super::PLAYFIELD_FILE,
                generated.glb.len(),
                super::TABLE_FILE,
                args.out.display()
            )
        }
        Some(path) => write_with_backbox(&generated, path, &args.out)?,
    };
    for w in &generated.warnings {
        out.push_str("\nwarning: ");
        out.push_str(&w.to_string());
    }
    Ok(out)
}

/// The `--backbox` path: parses `path` (bounded read), generates the
/// backbox on the merged manifest, aims its VUK and writes the playfield,
/// the backbox and one merged `table.toml` into `out`. Nothing is written
/// when any step fails.
fn write_with_backbox(
    generated: &super::GeneratedTable,
    path: &Path,
    out: &Path,
) -> Result<String, CliError> {
    use crate::backbox::{self, cli as bcli};
    let fail = |m: String| CliError::Failed(format!("{}: {m}", path.display()));
    let layout =
        backbox::parse_backbox_layout_bytes(&read(path)?).map_err(|e| fail(e.to_string()))?;
    let bb = backbox::generate_backbox(&layout.backbox, &generated.table)
        .map_err(|e| fail(e.to_string()))?;
    let mut table = bb.table.clone();
    bcli::aim_backbox_vuk(&mut table, &bb.frame).map_err(fail)?;
    let text = bcli::backbox_table_toml(&table).map_err(fail)?;
    bcli::write_bundle_with_backbox(generated, &bb, &text, out)
        .map_err(|e| CliError::Failed(format!("{}: {e}", out.display())))?;
    Ok(format!(
        "wrote {} ({} bytes), {} ({} bytes), {} ({} bytes) and {} to {}",
        super::PLAYFIELD_FILE,
        generated.glb.len(),
        backbox::BACKBOX_SCENE_FILE,
        bb.scene_glb.len(),
        backbox::BACKBOX_COLLIDERS_FILE,
        bb.colliders_glb.len(),
        super::TABLE_FILE,
        out.display()
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
        // --force overwrites it; the result is the tool's output, which a
        // later in-place re-run accepts without --force.
        let mut forced = args.clone();
        forced.push("--force".into());
        run_cli(&forced).unwrap();
        let once = std::fs::read_to_string(&base).unwrap();
        assert!(once.starts_with(crate::layout::GENERATED_HEADER));
        run_cli(&args).unwrap();
        assert_eq!(std::fs::read_to_string(&base).unwrap(), once);
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
    fn input_reads_are_bounded() {
        let dir = std::env::temp_dir().join(format!("pin_tablegen_read_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exact = dir.join("exact");
        std::fs::write(&exact, vec![b'#'; MAX_INPUT_BYTES as usize]).unwrap();
        assert_eq!(read(&exact).unwrap().len() as u64, MAX_INPUT_BYTES);
        let over = dir.join("over");
        std::fs::write(&over, vec![b'#'; MAX_INPUT_BYTES as usize + 1]).unwrap();
        let e = read(&over).unwrap_err();
        assert_eq!(e.exit_code(), 1);
        assert!(e.to_string().contains("max"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
        // A device whose metadata reports no size is still capped.
        #[cfg(unix)]
        {
            let e = read(Path::new("/dev/zero")).unwrap_err();
            assert!(e.to_string().contains("larger than"), "{e}");
        }
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
