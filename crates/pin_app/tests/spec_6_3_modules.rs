//! Subphase 6.3 spec tests (Independent Tester): the committed
//! `tables/<t>/rules.wasm` modules (brief requirement 6).
//!
//! Each exists beside `rules.lua`, is a structurally valid WebAssembly
//! binary (checked by a small parser here, and compiled and validated by
//! `WasmScriptBackend::prepare` with the engine limits), and imports only
//! from module `"pinball"`. When the `wasm32-unknown-unknown` target is
//! installed, `tables/<t>/rules-wasm/rebuild.sh` rebuilds each module into
//! a scratch directory byte-identical to the committed one; otherwise the
//! test prints a note and skips that part. The build scripts copy
//! `rules.wasm` into the bundle, and the CI workflow
//! `.github/workflows/rules-wasm.yml` exists and compares rebuilds.

mod spec_6_3_support;

use pin_script::wasm::{WasmConfig, WasmScriptBackend};
use proptest::prelude::*;
use spec_6_3_support::*;
use std::path::{Path, PathBuf};
use std::process::Command;

// ---------------------------------------------------------------------------
// A minimal WebAssembly binary reader (structure and imports only)
// ---------------------------------------------------------------------------

/// One import: module, field, kind byte.
#[derive(Debug, PartialEq, Eq)]
struct Import {
    module: String,
    field: String,
    kind: u8,
}

struct Reader<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Result<u8, String> {
        let v = *self.b.get(self.p).ok_or("unexpected end")?;
        self.p += 1;
        Ok(v)
    }

    fn u32(&mut self) -> Result<u32, String> {
        let mut v: u64 = 0;
        for i in 0..5 {
            let b = self.byte()?;
            v |= u64::from(b & 0x7f) << (7 * i);
            if b & 0x80 == 0 {
                return u32::try_from(v).map_err(|_| "LEB128 u32 overflow".into());
            }
        }
        Err("LEB128 u32 too long".into())
    }

    fn u64(&mut self) -> Result<u64, String> {
        let mut v: u64 = 0;
        for i in 0..10 {
            let b = self.byte()?;
            let bits = u64::from(b & 0x7f);
            if i == 9 && bits > 1 {
                return Err("LEB128 u64 overflow".into());
            }
            v |= bits << (7 * i);
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err("LEB128 u64 too long".into())
    }

    fn bytes(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.p.checked_add(n).ok_or("length overflow")?;
        let s = self.b.get(self.p..end).ok_or("unexpected end")?;
        self.p = end;
        Ok(s)
    }

    fn name(&mut self) -> Result<String, String> {
        let n = self.u32()? as usize;
        let s = self.bytes(n)?;
        String::from_utf8(s.to_vec()).map_err(|_| "name is not UTF-8".into())
    }

    fn limits(&mut self) -> Result<(), String> {
        let flags = self.byte()?;
        if flags > 0x07 {
            return Err(format!("bad limits flags {flags:#x}"));
        }
        let wide = flags & 0x04 != 0;
        let read = |r: &mut Self| {
            if wide {
                r.u64().map(|_| ())
            } else {
                r.u32().map(|_| ())
            }
        };
        read(self)?;
        if flags & 0x01 != 0 {
            read(self)?;
        }
        Ok(())
    }

    fn done(&self) -> bool {
        self.p == self.b.len()
    }
}

/// Walks the module's sections (magic, version 1, every section length
/// exactly consumed, known non-custom section ids at most once and in
/// order) and returns its imports.
fn parse_imports(bytes: &[u8]) -> Result<Vec<Import>, String> {
    let mut r = Reader { b: bytes, p: 0 };
    if r.bytes(4)? != b"\0asm" {
        return Err("bad magic".into());
    }
    if r.bytes(4)? != [1, 0, 0, 0] {
        return Err("not a version-1 core module".into());
    }
    // Section order of the core spec (data count 12 sits before code 10).
    const ORDER: [u8; 13] = [1, 2, 3, 4, 5, 13, 6, 7, 8, 9, 12, 10, 11];
    let mut last_rank: Option<usize> = None;
    let mut imports = Vec::new();
    let mut sections = 0;
    while !r.done() {
        let id = r.byte()?;
        let len = r.u32()? as usize;
        let body = r.bytes(len)?;
        sections += 1;
        if id == 0 {
            let mut s = Reader { b: body, p: 0 };
            s.name()?;
            continue;
        }
        let rank = ORDER
            .iter()
            .position(|&x| x == id)
            .ok_or(format!("unknown section id {id}"))?;
        if last_rank.is_some_and(|l| rank <= l) {
            return Err(format!("section {id} out of order or repeated"));
        }
        last_rank = Some(rank);
        if id == 2 {
            let mut s = Reader { b: body, p: 0 };
            let n = s.u32()?;
            for _ in 0..n {
                let module = s.name()?;
                let field = s.name()?;
                let kind = s.byte()?;
                match kind {
                    0x00 => {
                        s.u32()?;
                    }
                    0x01 => {
                        s.byte()?;
                        s.limits()?;
                    }
                    0x02 => s.limits()?,
                    0x03 => {
                        s.byte()?;
                        s.byte()?;
                    }
                    0x04 => {
                        s.byte()?;
                        s.u32()?;
                    }
                    k => return Err(format!("bad import kind {k:#x}")),
                }
                imports.push(Import {
                    module,
                    field,
                    kind,
                });
            }
            if !s.done() {
                return Err("import section has trailing bytes".into());
            }
        }
    }
    if sections == 0 {
        return Err("no sections".into());
    }
    Ok(imports)
}

// ---------------------------------------------------------------------------
// Committed modules
// ---------------------------------------------------------------------------

#[test]
fn committed_modules_exist_are_valid_and_import_only_pinball() {
    for t in Table::ALL {
        let p = committed_wasm_path(t);
        assert!(
            t.src().join("rules.lua").is_file(),
            "{t:?}: rules.lua stays the source"
        );
        let bytes = committed_wasm(t);
        assert!(!bytes.is_empty(), "{}: empty", p.display());
        let imports =
            parse_imports(&bytes).unwrap_or_else(|e| panic!("{}: not Wasm: {e}", p.display()));
        assert!(
            !imports.is_empty(),
            "{t:?}: a rules port calls the host API (imports from pinball)"
        );
        for i in &imports {
            assert_eq!(i.module, "pinball", "{t:?}: import {i:?} outside pinball");
            assert_eq!(i.kind, 0, "{t:?}: import {i:?} is not a function");
        }
        WasmScriptBackend::prepare(&bytes, WasmConfig::engine())
            .unwrap_or_else(|e| panic!("{t:?}: the engine rejects rules.wasm: {e}"));
    }
}

#[test]
fn build_scripts_copy_rules_wasm_and_ci_compares_rebuilds() {
    for t in Table::ALL {
        let build = std::fs::read_to_string(t.src().join("build.sh")).unwrap();
        assert!(
            build.contains("rules.wasm"),
            "{t:?}: build.sh must copy rules.wasm into the bundle when it exists"
        );
        assert!(
            t.src().join("rules-wasm").join("rebuild.sh").is_file(),
            "{t:?}: SPEC mismatch: tables/{}/rules-wasm/rebuild.sh missing",
            t.dir_name()
        );
    }
    let wf = repo_root().join(".github/workflows/rules-wasm.yml");
    let text = std::fs::read_to_string(&wf)
        .unwrap_or_else(|e| panic!("SPEC mismatch: {} missing ({e})", wf.display()));
    assert!(text.contains("cmp"), "rules-wasm.yml compares with cmp");
    assert!(
        text.contains("rebuild.sh"),
        "rules-wasm.yml runs rebuild.sh"
    );
}

fn wasm32_installed() -> bool {
    let out = Command::new(std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into()))
        .args(["--print", "sysroot"])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let root = String::from_utf8_lossy(&o.stdout).trim().to_owned();
            Path::new(&root)
                .join("lib/rustlib/wasm32-unknown-unknown")
                .is_dir()
        }
        _ => false,
    }
}

/// Where a rebuild put its module: `out` itself when it is a file, else
/// `out/rules.wasm` (see docs/decisions/2026-11-19-6.3-tester.md).
fn rebuilt_module(out: &Path) -> PathBuf {
    if out.is_dir() {
        out.join("rules.wasm")
    } else {
        out.to_path_buf()
    }
}

#[test]
fn rebuild_scripts_reproduce_the_committed_modules() {
    if !wasm32_installed() {
        println!(
            "note: wasm32-unknown-unknown is not installed; rebuild reproducibility not checked"
        );
        return;
    }
    let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("spec_6_3_rebuild_target");
    for t in Table::ALL {
        let committed = committed_wasm(t);
        let script = t.src().join("rules-wasm").join("rebuild.sh");
        let out = scratch(&format!("{}_rebuild", t.dir_name())).join("rules.wasm");
        let status = Command::new("sh")
            .arg(&script)
            .arg(&out)
            .current_dir(repo_root())
            // A target dir of its own: the test run may hold the lock of
            // the outer one.
            .env("CARGO_TARGET_DIR", &target)
            .env_remove("CARGO_PROFILE_DEV_DEBUG")
            .env_remove("CARGO_PROFILE_TEST_DEBUG")
            .status()
            .unwrap_or_else(|e| panic!("{t:?}: run rebuild.sh: {e}"));
        assert!(status.success(), "{t:?}: rebuild.sh failed: {status}");
        let rebuilt = std::fs::read(rebuilt_module(&out))
            .unwrap_or_else(|e| panic!("{t:?}: no rebuilt module at {}: {e}", out.display()));
        assert!(
            rebuilt == committed,
            "{t:?}: rebuilt rules.wasm ({} bytes) differs from the committed one ({} bytes)",
            rebuilt.len(),
            committed.len()
        );
        assert_eq!(
            committed_wasm(t),
            committed,
            "{t:?}: rebuild.sh must not touch the committed module"
        );
    }
}

// ---------------------------------------------------------------------------
// The reader itself
// ---------------------------------------------------------------------------

/// A hand-assembled module importing `pinball.tick` and `env.f`.
fn sample() -> Vec<u8> {
    let mut m = b"\0asm\x01\0\0\0".to_vec();
    // type section: one type () -> ()
    m.extend([1, 4, 1, 0x60, 0, 0]);
    let mut imp = vec![2];
    for (md, f) in [("pinball", "tick"), ("env", "f")] {
        imp.push(md.len() as u8);
        imp.extend(md.as_bytes());
        imp.push(f.len() as u8);
        imp.extend(f.as_bytes());
        imp.extend([0, 0]);
    }
    m.extend([2, imp.len() as u8]);
    m.extend(imp);
    m
}

#[test]
fn reader_finds_imports_and_rejects_garbage() {
    let imps = parse_imports(&sample()).unwrap();
    assert_eq!(imps.len(), 2);
    assert_eq!(imps[0].module, "pinball");
    assert_eq!(imps[1].module, "env");
    assert!(parse_imports(b"\0asm\x01\0\0\0").is_err(), "no sections");
    assert!(parse_imports(b"\0asm\x02\0\0\0\x01\x00").is_err());
    let mut bad = sample();
    bad.truncate(bad.len() - 1);
    assert!(parse_imports(&bad).is_err(), "truncated");
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, failure_persistence: None, ..ProptestConfig::default() })]

    /// Bounded random bytes never panic the reader.
    #[test]
    fn reader_survives_random_bytes(tail in proptest::collection::vec(any::<u8>(), 0..256),
                                    prefix in any::<bool>()) {
        let mut b = if prefix { b"\0asm\x01\0\0\0".to_vec() } else { Vec::new() };
        b.extend(tail);
        let _ = parse_imports(&b);
    }
}
