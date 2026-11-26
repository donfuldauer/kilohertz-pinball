//! #163 gate round 9: `Vfs::open_with_limits` refuses non-regular archive
//! paths (SPEC §3.5, R11, REQ-VIEW-006).
#![cfg(unix)]

#[path = "fixtures/support.rs"]
mod support;

use pin_assets::{AssetError, Vfs};
use support::*;

/// SPEC §3.5 / R11: a FIFO named `x.zip` is refused without being opened
/// (opening a FIFO for read would block forever).
#[test]
fn fifo_named_zip_is_refused_without_blocking() {
    let dir = temp_dir("r9_fifo");
    let fifo = dir.join("x.zip");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("mkfifo available");
    assert!(status.success());
    let err = Vfs::open(&fifo).expect_err("must be refused");
    assert_eq!(
        err,
        AssetError::Io(format!("{}: not a regular file", fifo.display()))
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// R11: a socket named `x.pinbundle` is refused too.
#[test]
fn socket_named_pinbundle_is_refused() {
    let dir = temp_dir("r9_sock");
    let sock = dir.join("x.pinbundle");
    let _listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    let err = Vfs::open(&sock).expect_err("must be refused");
    assert_eq!(
        err,
        AssetError::Io(format!("{}: not a regular file", sock.display()))
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// REQ-VIEW-006: a symlink to a regular archive still opens.
#[test]
fn symlink_to_regular_archive_opens() {
    let dir = temp_dir("r9_link");
    let real = dir.join("real.zip");
    std::fs::write(&real, stored_zip(&[("table.toml", b"t")])).unwrap();
    let link = dir.join("link.zip");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(Vfs::open(&link).unwrap().list(), vec!["table.toml"]);
    let _ = std::fs::remove_dir_all(dir);
}

/// The non-archive error text is unchanged and precedes the new check.
#[test]
fn non_archive_error_text_unchanged() {
    let dir = temp_dir("r9_txt");
    let txt = dir.join("a.txt");
    std::fs::write(&txt, b"x").unwrap();
    assert_eq!(
        Vfs::open(&txt).err(),
        Some(AssetError::Io(format!(
            "{}: not a folder, .zip or .pinbundle",
            txt.display()
        )))
    );
    let _ = std::fs::remove_dir_all(dir);
}
