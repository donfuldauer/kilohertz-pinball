//! Work item w7pre-backbox-colliders: the bundle path of the backbox
//! collision geometry is the name `pin_tablegen::backbox` writes (checked
//! against pin_tablegen's own constant in pin_sim's `backbox_colliders.rs`),
//! a bundle-relative `.glb` path that `Vfs` accepts.

use pin_assets::BACKBOX_COLLIDERS_FILE;

#[test]
fn backbox_colliders_file_is_a_bundle_root_glb() {
    assert_eq!(BACKBOX_COLLIDERS_FILE, "backbox_colliders.glb");
    assert!(!BACKBOX_COLLIDERS_FILE.contains(['/', '\\']));
}

#[test]
fn a_bundle_without_the_file_reports_it_absent() {
    let dir = std::env::temp_dir().join(format!(
        "pin_assets_backbox_colliders_file_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("x.txt"), b"x").unwrap();
    let vfs = pin_assets::Vfs::open(&dir).unwrap();
    assert!(!vfs.exists(BACKBOX_COLLIDERS_FILE));
    std::fs::write(dir.join(BACKBOX_COLLIDERS_FILE), b"glTF").unwrap();
    let vfs = pin_assets::Vfs::open(&dir).unwrap();
    assert!(vfs.exists(BACKBOX_COLLIDERS_FILE));
    let _ = std::fs::remove_dir_all(&dir);
}
