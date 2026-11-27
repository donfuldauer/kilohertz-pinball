//! #136 (KP-055): the `pin_tablegen` command line builds the backbox
//! (`--backbox`, SPEC §4 Subphases 4.3 and 5.5) and the whole Feature Lab
//! bundle (`pin_tablegen build`, SPEC §4 Subphase 6.4), byte-identical to
//! the retired pin_app example's output at the parent commit (e87b2c7).
//!
//! `tests/fixtures/feature_lab_bundle.digests` holds, for every file of the
//! bundle that example (plus `build.sh`'s `rules.wasm` copy) wrote, its
//! relative path, length and FNV-1a-64 hash. It was produced from the
//! unmodified example, never from this crate's code.

use pin_core::schema::table::TableManifest;
use pin_tablegen::backbox::{
    BackboxFrame, VUK_LATERAL_M_S, VUK_RISE_M_S, build_from, parse_hex, vuk_eject,
};
use proptest::prelude::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

fn feature_lab_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tables/feature_lab")
}

fn scratch(tag: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let p = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "backbox_cli_{tag}_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn run<I, S>(args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    Command::new(env!("CARGO_BIN_EXE_pin_tablegen"))
        .args(args)
        .output()
        .expect("the pin_tablegen binary runs")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// Exit code `code`, no panic, no signal, a message on stderr.
fn assert_exit(o: &Output, code: i32, what: &str) -> String {
    let err = stderr(o);
    assert_eq!(o.status.code(), Some(code), "{what}: {:?}\n{err}", o.status);
    assert!(!err.contains("panicked"), "{what}: panicked\n{err}");
    assert!(!err.trim().is_empty(), "{what}: no error message");
    err
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Every file under `root`: relative path (`/`-separated) -> bytes.
fn files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                let rel = p.strip_prefix(root).unwrap();
                let rel: Vec<String> = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect();
                out.insert(rel.join("/"), std::fs::read(&p).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// The golden digests: relative path -> (length, FNV-1a-64).
fn golden() -> BTreeMap<String, (usize, u64)> {
    let text = include_str!("fixtures/feature_lab_bundle.digests");
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split(' ').collect();
            assert_eq!(f.len(), 3, "digest line {l:?}");
            (
                f[0].to_owned(),
                (
                    f[1].parse().unwrap(),
                    u64::from_str_radix(f[2], 16).unwrap(),
                ),
            )
        })
        .collect()
}

/// `pin_tablegen build <feature_lab> --out <dir>`, then build.sh's
/// `rules.wasm` copy step.
fn build_lab_with_cli(dir: &Path) {
    let o = run([
        "build".as_ref(),
        feature_lab_src().as_os_str(),
        "--out".as_ref(),
        dir.as_os_str(),
    ]);
    assert!(o.status.success(), "build failed: {}", stderr(&o));
    let wasm = feature_lab_src().join("rules.wasm");
    if wasm.is_file() {
        std::fs::copy(&wasm, dir.join("rules.wasm")).unwrap();
    }
}

/// SPEC §4 Subphase 6.4, #136 acceptance 1: the CLI's Feature Lab bundle is
/// byte-identical, file by file, to the example's at the parent commit, and
/// the build is deterministic.
#[test]
fn feature_lab_bundle_is_byte_identical_to_the_parent_commit() {
    let golden = golden();
    let a = scratch("fl_a");
    build_lab_with_cli(&a);
    let got = files(&a);
    for name in golden.keys() {
        assert!(got.contains_key(name), "missing file {name}");
    }
    for (name, bytes) in &got {
        let Some(&(len, hash)) = golden.get(name) else {
            panic!("extra file {name}");
        };
        assert_eq!(bytes.len(), len, "{name}: length");
        assert_eq!(fnv1a64(bytes), hash, "{name}: FNV-1a-64 differs");
    }
    let b = scratch("fl_b");
    build_lab_with_cli(&b);
    assert_eq!(files(&b), got, "two builds differ");
}

/// The library entry gives the same bundle as the command (tests build
/// through it) and its warnings are sorted.
#[test]
fn build_from_equals_the_command() {
    let a = scratch("fl_lib");
    let warnings = build_from(&feature_lab_src(), &a).unwrap();
    let mut sorted = warnings.clone();
    sorted.sort();
    assert_eq!(warnings, sorted);
    let b = scratch("fl_cmd");
    build_lab_with_cli(&b);
    let mut lib = files(&a);
    lib.remove("rules.wasm");
    let mut cmd = files(&b);
    cmd.remove("rules.wasm");
    assert_eq!(lib, cmd);
}

fn layout_args(out: &Path) -> Vec<std::ffi::OsString> {
    let src = feature_lab_src();
    vec![
        src.join("layout.toml").into(),
        "--table".into(),
        src.join("table.toml").into(),
        "--out".into(),
        out.into(),
    ]
}

fn with_backbox(out: &Path, backbox: &Path) -> Output {
    let mut args = layout_args(out);
    args.push("--backbox".into());
    args.push(backbox.into());
    run(args)
}

/// SPEC §4 Subphase 5.5 (and 4.3): `--backbox` writes `backbox.glb` and
/// `backbox_colliders.glb`, the merged table.toml carries the aimed VUK, and
/// the playfield output is the plain run's.
#[test]
fn backbox_option_writes_backbox_files() {
    let plain = scratch("plain");
    let o = run(layout_args(&plain));
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(!plain.join("backbox.glb").exists());
    assert!(!plain.join("backbox_colliders.glb").exists());

    let bb = scratch("bb");
    let o = with_backbox(&bb, &feature_lab_src().join("backbox_layout.toml"));
    assert!(o.status.success(), "{}", stderr(&o));
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.contains("backbox.glb") && out.contains("backbox_colliders.glb"));
    for f in ["backbox.glb", "backbox_colliders.glb"] {
        let bytes = std::fs::read(bb.join(f)).unwrap();
        assert!(bytes.starts_with(b"glTF"), "{f} is a glb");
    }
    assert_eq!(
        std::fs::read(bb.join("playfield.glb")).unwrap(),
        std::fs::read(plain.join("playfield.glb")).unwrap(),
        "playfield.glb is unchanged by --backbox"
    );
    let text = std::fs::read_to_string(bb.join("table.toml")).unwrap();
    assert!(text.starts_with(pin_tablegen::layout::GENERATED_HEADER));
    let t = TableManifest::from_toml_str(&text).unwrap();
    let vuk = t
        .mechs
        .iter()
        .find(|m| m.id == "tablegen:backbox.vuk")
        .expect("the generated VUK");
    assert!(vuk.eject_speed_m_s.is_some());
    assert!(vuk.angle_deg.is_some());
    assert!(vuk.kick_speed_m_s.is_some());
    // The written files are the Feature Lab bundle's.
    let golden = golden();
    for f in [
        "table.toml",
        "playfield.glb",
        "backbox.glb",
        "backbox_colliders.glb",
    ] {
        let bytes = std::fs::read(bb.join(f)).unwrap();
        assert_eq!((bytes.len(), fnv1a64(&bytes)), golden[f], "{f}");
    }
    // No staging leftovers.
    for e in std::fs::read_dir(&bb).unwrap() {
        let n = e.unwrap().file_name().to_string_lossy().into_owned();
        assert!(
            !n.ends_with(".tablegen-tmp") && !n.ends_with(".tablegen-bak"),
            "{n}"
        );
    }
    // A re-run over the existing output gives the same bytes.
    let before = files(&bb);
    let o = with_backbox(&bb, &feature_lab_src().join("backbox_layout.toml"));
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(files(&bb), before);
}

/// 4.3 fix round 1, N4 on the `--backbox` path: a hand-written base inside
/// the output folder is not overwritten.
#[test]
fn backbox_option_keeps_a_hand_written_base() {
    let dir = scratch("n4");
    let src = feature_lab_src();
    let base_text = std::fs::read_to_string(src.join("table.toml")).unwrap();
    std::fs::write(dir.join("table.toml"), &base_text).unwrap();
    let o = run([
        src.join("layout.toml").as_os_str(),
        "--table".as_ref(),
        dir.join("table.toml").as_os_str(),
        "--out".as_ref(),
        dir.as_os_str(),
        "--backbox".as_ref(),
        src.join("backbox_layout.toml").as_os_str(),
    ]);
    let err = assert_exit(&o, 1, "base in output");
    assert!(err.contains("outside the bundle folder"), "{err}");
    assert_eq!(
        std::fs::read_to_string(dir.join("table.toml")).unwrap(),
        base_text
    );
    assert!(!dir.join("backbox.glb").exists());
}

fn up_of(pitch_deg: f32) -> [f32; 3] {
    let u = BackboxFrame::new([0.0, 1150.0, 250.0], pitch_deg).up();
    [u.x, u.y, u.z]
}

proptest! {
    /// DECISIONS "6.4 — backbox VUK aimed through the entry transition":
    /// the VUK velocity is VUK_RISE_M_S up the backbox plus VUK_LATERAL_M_S
    /// across it, and lies in the backbox plane.
    #[test]
    fn vuk_eject_aims_up_the_backbox_plane(
        pitch in 0.0f32..=15.0,
        ox in -500.0f32..500.0,
        oy in 0.0f32..2000.0,
        oz in 0.0f32..600.0,
    ) {
        let frame = BackboxFrame::new([ox, oy, oz], pitch);
        let (eject, angle_deg, kick) = vuk_eject(&frame);
        let a = angle_deg.to_radians();
        let vel = [kick * libm::cosf(a), kick * libm::sinf(a), eject];
        let up = frame.up();
        let want = [
            VUK_LATERAL_M_S + VUK_RISE_M_S * up.x,
            VUK_RISE_M_S * up.y,
            VUK_RISE_M_S * up.z,
        ];
        for k in 0..3 {
            prop_assert!((vel[k] - want[k]).abs() < 1e-4, "{k}: {vel:?} vs {want:?}");
        }
        let n = frame.normal();
        let dot = vel[0] * n.x + vel[1] * n.y + vel[2] * n.z;
        prop_assert!(dot.abs() < 1e-4, "out of the backbox plane: {dot}");
        prop_assert!(kick > 0.0 && eject > 0.0);
    }
}

#[test]
fn vuk_eject_at_known_pitches() {
    // Pitch 0: up is table z, so the kick is purely lateral.
    let (eject, angle, kick) = vuk_eject(&BackboxFrame::new([0.0; 3], 0.0));
    assert!((eject - VUK_RISE_M_S).abs() < 1e-6);
    assert!(angle.abs() < 1e-4);
    assert!((kick - VUK_LATERAL_M_S).abs() < 1e-6);
    // The Feature Lab pitch, 6.5 degrees: the backbox's up axis is
    // (0, sin p, cos p) in the table frame, so the kick leans up the table.
    let up = up_of(6.5);
    let p = 6.5f32.to_radians();
    assert!((up[1] - libm::sinf(p)).abs() < 1e-6 && (up[2] - libm::cosf(p)).abs() < 1e-6);
    let (eject, angle, kick) = vuk_eject(&BackboxFrame::new([0.0; 3], 6.5));
    assert!(angle > 0.0, "{angle}");
    assert!((eject - VUK_RISE_M_S * libm::cosf(p)).abs() < 1e-6);
    let lat = f64::from(VUK_LATERAL_M_S);
    let rise = f64::from(VUK_RISE_M_S * libm::sinf(p));
    assert!((f64::from(kick) - libm::sqrt(lat * lat + rise * rise)).abs() < 1e-6);
}

/// Negative cases: usage errors exit 2, failures exit 1, never a panic.
#[test]
fn bad_backbox_arguments_and_files() {
    let out = scratch("neg");
    // --backbox without a value.
    let mut args = layout_args(&out);
    args.push("--backbox".into());
    let err = assert_exit(&run(args), 2, "--backbox without a value");
    assert!(
        err.contains("--backbox needs a value") && err.contains("usage:"),
        "{err}"
    );
    // --backbox twice.
    let bb = feature_lab_src().join("backbox_layout.toml");
    let mut args = layout_args(&out);
    for _ in 0..2 {
        args.push("--backbox".into());
        args.push(bb.clone().into());
    }
    let err = assert_exit(&run(args), 2, "--backbox twice");
    assert!(err.contains("--backbox given twice"), "{err}");
    // A missing file.
    let missing = out.join("nope.toml");
    let err = assert_exit(&with_backbox(&out, &missing), 1, "missing backbox file");
    assert!(err.contains("nope.toml"), "{err}");
    // Invalid TOML: the error names the file.
    let bad = out.join("bad_backbox.toml");
    std::fs::write(&bad, "schema_version = 1\n[backbox\n").unwrap();
    let err = assert_exit(&with_backbox(&out, &bad), 1, "invalid backbox TOML");
    assert!(err.contains("bad_backbox.toml"), "{err}");
    // A valid TOML file whose backbox does not fit: names the element.
    let text = std::fs::read_to_string(&bb)
        .unwrap()
        .replacen("depth = ", "depth = 1.0 #", 1);
    let small = out.join("thin_backbox.toml");
    std::fs::write(&small, text).unwrap();
    let err = assert_exit(&with_backbox(&out, &small), 1, "backbox too thin");
    assert!(
        err.contains("thin_backbox.toml") && err.contains("backbox"),
        "{err}"
    );
    // Nothing was written by any failed run.
    for f in ["playfield.glb", "table.toml", "backbox.glb"] {
        assert!(!out.join(f).exists(), "{f} written by a failed run");
    }
}

#[test]
fn bad_build_arguments_and_sources() {
    // Usage errors.
    for args in [
        &["build"][..],
        &["build", "src"],
        &["build", "--out", "o"],
        &["build", "src", "--out"],
        &["build", "-h"],
    ] {
        let err = assert_exit(&run(args), 2, &format!("{args:?}"));
        assert!(err.contains("pin_tablegen build"), "{err}");
    }
    // out == src, also through an unnormalised path; nothing is written.
    let src = scratch("src_copy");
    for e in std::fs::read_dir(feature_lab_src()).unwrap() {
        let p = e.unwrap().path();
        if p.is_file() {
            std::fs::copy(&p, src.join(p.file_name().unwrap())).unwrap();
        }
    }
    let alias = src.join("..").join(src.file_name().unwrap());
    let o = run([
        "build".as_ref(),
        src.as_os_str(),
        "--out".as_ref(),
        alias.as_os_str(),
    ]);
    let err = assert_exit(&o, 1, "out == src");
    let want = format!(
        "output directory {} is the source directory {}: the build would overwrite its own \
         inputs (table.toml); choose another output directory",
        alias.display(),
        src.display()
    );
    assert!(err.contains(&want), "{err}");
    assert!(!src.join("playfield.glb").exists());
    // A source folder without layout.toml.
    let empty = scratch("empty_src");
    let o = run([
        "build".as_ref(),
        empty.as_os_str(),
        "--out".as_ref(),
        scratch("empty_out").as_os_str(),
    ]);
    let err = assert_exit(&o, 1, "missing layout.toml");
    assert!(err.contains("layout.toml"), "{err}");
    // A missing source directory.
    let e = build_from(&empty.join("nope"), &scratch("nope_out")).unwrap_err();
    assert!(e.contains("nope"), "{e}");
    // Missing display folder: names it.
    let e = build_from(&src, &scratch("no_display")).unwrap_err();
    assert!(e.contains("display"), "{e}");
}

/// R11: only regular files with plain names in `display/` are copied; a
/// subdirectory or a symlink named `*.toml` is skipped, so nothing outside
/// the source folder is followed.
#[test]
fn build_copies_only_regular_display_files() {
    let src = scratch("r11_src");
    copy_tree(&feature_lab_src(), &src);
    std::fs::create_dir_all(src.join("display/sub.toml")).unwrap();
    std::fs::write(src.join("display/sub.toml/inner.toml"), "x = 1\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(src.join("layout.toml"), src.join("display/link.toml")).unwrap();
    let out = scratch("r11_out");
    build_from(&src, &out).unwrap();
    assert!(out.join("display/scene_score.toml").is_file());
    assert!(!out.join("display/sub.toml").exists());
    assert!(!out.join("display/link.toml").exists());
    // A COPIED source that is a symlink is refused.
    #[cfg(unix)]
    {
        std::fs::remove_file(src.join("glass.toml")).unwrap();
        std::os::unix::fs::symlink(feature_lab_src().join("glass.toml"), src.join("glass.toml"))
            .unwrap();
        let e = build_from(&src, &scratch("r11_link")).unwrap_err();
        assert!(
            e.contains("glass.toml") && e.contains("not a regular file"),
            "{e}"
        );
    }
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let p = e.unwrap().path();
        let dest = to.join(p.file_name().unwrap());
        if p.is_dir() {
            copy_tree(&p, &dest);
        } else {
            std::fs::copy(&p, &dest).unwrap();
        }
    }
}

/// A source the build reads that is too big or not UTF-8 fails naming it.
#[test]
fn build_source_reads_are_bounded() {
    let src = scratch("big_src");
    copy_tree(&feature_lab_src(), &src);
    std::fs::write(src.join("backbox_layout.toml"), vec![b'#'; (4 << 20) + 1]).unwrap();
    let e = build_from(&src, &scratch("big_out")).unwrap_err();
    assert!(
        e.starts_with("backbox_layout.toml:") && e.contains("max 4194304"),
        "{e}"
    );
    std::fs::write(src.join("backbox_layout.toml"), [0xff_u8, 0xfe]).unwrap();
    let e = build_from(&src, &scratch("utf8_out")).unwrap_err();
    assert!(
        e.contains("backbox_layout.toml") && e.contains("UTF-8"),
        "{e}"
    );
    // A bad palette colour is named.
    let t = std::fs::read_to_string(feature_lab_src().join("table.toml"))
        .unwrap()
        .replacen("\"#1b263b\"", "\"#1b263\"", 1);
    copy_tree(&feature_lab_src(), &src);
    std::fs::write(src.join("table.toml"), t).unwrap();
    let e = build_from(&src, &scratch("palette_out")).unwrap_err();
    assert!(e.contains("#1b263"), "{e}");
}

/// R11 / unit gate 5: random bytes as the `--backbox` file never panic the
/// command; an oversize file is refused naming the limit.
#[test]
fn random_backbox_bytes_never_panic() {
    let dir = scratch("fuzz");
    let valid = std::fs::read(feature_lab_src().join("backbox_layout.toml")).unwrap();
    // xorshift64*, fixed seed.
    let mut s: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = move || {
        s ^= s >> 12;
        s ^= s << 25;
        s ^= s >> 27;
        s.wrapping_mul(0x2545_f491_4f6c_dd1d)
    };
    let path = dir.join("fuzz_backbox.toml");
    let out = dir.join("out");
    for i in 0..80 {
        let bytes: Vec<u8> = if i % 4 == 3 {
            // A mutated valid file (reaches the generator, not only TOML).
            let mut b = valid.clone();
            for _ in 0..(1 + next() % 8) {
                let k = (next() % b.len() as u64) as usize;
                b[k] = b"0123456789.-=[]\n x"[(next() % 18) as usize];
            }
            b
        } else {
            let n = (next() % 4097) as usize;
            (0..n).map(|_| next() as u8).collect()
        };
        std::fs::write(&path, &bytes).unwrap();
        let o = with_backbox(&out, &path);
        let err = stderr(&o);
        assert!(
            o.status.code().is_some_and(|c| c == 0 || c == 1),
            "case {i}: status {:?}\n{err}",
            o.status
        );
        assert!(!err.contains("panicked"), "case {i}: {err}");
    }
    // Oversize (> 4 MiB): refused naming the limit.
    std::fs::write(&path, vec![b'#'; (4 << 20) + 1]).unwrap();
    let err = assert_exit(&with_backbox(&out, &path), 1, "oversize");
    assert!(err.contains("max 4194304"), "{err}");
}

#[test]
fn parse_hex_accepts_rrggbb_only() {
    assert_eq!(parse_hex("#1b263b"), Ok([0x1b, 0x26, 0x3b]));
    assert_eq!(parse_hex("FFb703"), Ok([0xff, 0xb7, 0x03]));
    for bad in ["#12345", "#1234567", "", "#", "#12345g", "zzzzzz", "#ééé"] {
        let e = parse_hex(bad).unwrap_err();
        assert!(e.contains("not #rrggbb"), "{bad:?}: {e}");
    }
}

/// `aim_backbox_vuk` sets exactly `vuk_eject`'s values on the generated VUK
/// and fails naming the mech when there is none; `backbox_table_toml`
/// starts with the generated header and round-trips.
#[test]
fn aim_backbox_vuk_sets_the_generated_vuk() {
    use pin_tablegen::backbox::cli::backbox_table_toml;
    use pin_tablegen::backbox::{aim_backbox_vuk, generate_backbox, parse_backbox_layout};
    use pin_tablegen::layout::parse_layout;
    let src = feature_lab_src();
    let read = |f: &str| std::fs::read_to_string(src.join(f)).unwrap();
    let base = TableManifest::from_toml_str(&read("table.toml")).unwrap();
    let playfield =
        pin_tablegen::generate(&parse_layout(&read("layout.toml")).unwrap(), &base).unwrap();
    let bb = parse_backbox_layout(&read("backbox_layout.toml")).unwrap();
    let g = generate_backbox(&bb.backbox, &playfield.table).unwrap();
    let mut t = g.table.clone();
    let vuk = |t: &TableManifest| {
        t.mechs
            .iter()
            .find(|m| m.id == "tablegen:backbox.vuk")
            .cloned()
            .unwrap()
    };
    assert_eq!(
        vuk(&t).eject_speed_m_s,
        None,
        "the generator leaves it unset"
    );
    aim_backbox_vuk(&mut t, &g.frame).unwrap();
    let (eject, angle, kick) = vuk_eject(&g.frame);
    let v = vuk(&t);
    assert_eq!(v.eject_speed_m_s, Some(eject));
    assert_eq!(v.angle_deg, Some(angle));
    assert_eq!(v.kick_speed_m_s, Some(kick));
    let text = backbox_table_toml(&t).unwrap();
    assert!(text.starts_with(pin_tablegen::layout::GENERATED_HEADER));
    assert_eq!(TableManifest::from_toml_str(&text).unwrap(), t);

    let mut none = base.clone();
    none.mechs.retain(|m| m.id != "tablegen:backbox.vuk");
    let e = aim_backbox_vuk(&mut none, &g.frame).unwrap_err();
    assert!(e.contains("tablegen:backbox.vuk"), "{e}");
}
