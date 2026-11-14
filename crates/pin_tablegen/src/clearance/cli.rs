//! `pin_tablegen validate <bundle>`: loads a bundle and runs the clearance
//! validator.

use super::{ClearanceReport, validate_bundle};
use crate::layout::CliError;
use pin_assets::{AssetError, Vfs};
use std::path::Path;

/// Usage text of the `validate` subcommand.
pub const VALIDATE_USAGE: &str = "usage: pin_tablegen validate <bundle_dir_or_.pinbundle>";

/// Runs `pin_tablegen validate` with `args` (the arguments after
/// `validate`). On success (the bundle loads and every declared ball fits)
/// returns a summary line followed by one `warning: ...` line per
/// clearance warning.
///
/// # Errors
/// [`CliError::Usage`] (exit code 2) for bad arguments;
/// [`CliError::Failed`] (exit code 1) listing every load error or
/// clearance error, one `file: path: message` per line.
pub fn run_validate_cli(args: &[String]) -> Result<String, CliError> {
    let mut bundle = None;
    for a in args {
        match a.as_str() {
            "-h" | "--help" => return Err(usage("help requested")),
            s if s.starts_with('-') && s.len() > 1 => {
                return Err(usage(&format!("unknown option {s:?}")));
            }
            _ => {
                if bundle.replace(a).is_some() {
                    return Err(usage(&format!("unexpected argument {a:?}")));
                }
            }
        }
    }
    let bundle = bundle.ok_or_else(|| usage("missing <bundle>"))?;
    let path = Path::new(bundle);
    let fail = |e: AssetError| match e {
        AssetError::Invalid(list) => CliError::Failed(
            list.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        other => CliError::Failed(format!("{}: {other}", path.display())),
    };
    let vfs = Vfs::open(path).map_err(fail)?;
    let report = validate_bundle(&vfs).map_err(fail)?;
    summarize(path, report)
}

fn usage(m: &str) -> CliError {
    CliError::Usage(format!("{m}\n{VALIDATE_USAGE}"))
}

fn summarize(path: &Path, report: ClearanceReport) -> Result<String, CliError> {
    if !report.is_ok() {
        return Err(CliError::Failed(
            report
                .errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        ));
    }
    let mut out = format!(
        "{}: clearance ok ({} warning{})",
        path.display(),
        report.warnings.len(),
        if report.warnings.len() == 1 { "" } else { "s" }
    );
    for w in &report.warnings {
        out.push_str(&format!("\nwarning: {w}"));
    }
    Ok(out)
}
