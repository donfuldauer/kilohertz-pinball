//! Subphase 6.3 spec tests (Independent Tester): the committed
//! `tables/<t>/rules.wasm` modules (brief requirement 6).
//!
//! Each exists beside `rules.lua`, is a structurally valid WebAssembly
//! binary (checked by a small parser here, and compiled and validated by
//! `WasmScriptBackend::prepare` with the engine limits), and imports only
//! from module `"pinball"`. When the `wasm32-unknown-unknown` target is
//! installed, `tables/<t>/rules-wasm/rebuild.sh` rebuilds each module into
//! a scratch directory byte-identical to the committed one; otherwise the
//! test prints `SKIP:` and skips that part, or fails when
//! `PIN_REQUIRE_WASM32` is set (the gate shared with `wasm_ports_rebuild`,
//! `wasm_ports_support::wasm32_gate`). Each table's real `build.sh` (Sky
//! Deck with `PIN_BUNDLE_RULES_WASM=1`) bundles a `rules.wasm` byte-equal to
//! the committed one, and the CI workflow `.github/workflows/rules-wasm.yml`
//! runs every `rebuild.sh` into a scratch file and compares it with the
//! committed module in a real (uncommented) `cmp` command.

mod spec_6_3_support;
mod wasm_ports_support;

use pin_script::wasm::{WasmConfig, WasmScriptBackend};
use proptest::prelude::*;
use spec_6_3_support::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use wasm_ports_support::TempDir;

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

#[cfg(unix)]
#[test]
fn build_scripts_bundle_the_committed_rules_wasm() {
    let tmp = TempDir::new("spec63_build_sh");
    for t in Table::ALL {
        let build = std::fs::read_to_string(t.src().join("build.sh")).unwrap();
        assert!(
            build.contains("rules.wasm"),
            "{t:?}: build.sh must copy rules.wasm into the bundle when it exists"
        );
        let out = tmp.path().join(t.dir_name());
        let mut cmd = Command::new("sh");
        cmd.arg(t.src().join("build.sh"))
            .arg(&out)
            .current_dir(repo_root());
        if t == Table::SkyDeck {
            // Sky Deck bundles the module only on request (its default
            // bundle must equal the in-process helper bundle).
            cmd.env("PIN_BUNDLE_RULES_WASM", "1");
        }
        let status = cmd
            .status()
            .unwrap_or_else(|e| panic!("{t:?}: run build.sh: {e}"));
        assert!(status.success(), "{t:?}: build.sh failed: {status}");
        let bundled = std::fs::read(out.join("rules.wasm"))
            .unwrap_or_else(|e| panic!("{t:?}: build.sh did not bundle rules.wasm: {e}"));
        let committed = committed_wasm(t);
        assert!(
            bundled == committed,
            "{t:?}: the bundled rules.wasm ({} bytes) differs from tables/{}/rules.wasm ({} bytes)",
            bundled.len(),
            t.dir_name(),
            committed.len()
        );
    }
}

#[test]
fn ci_rebuilds_and_compares_every_module() {
    for t in Table::ALL {
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
    for t in Table::ALL {
        if let Err(e) = compares_rebuild(&text, t.dir_name()) {
            panic!("rules-wasm.yml: {e}");
        }
    }
}

// ---------------------------------------------------------------------------
// A small reading of the workflow's shell commands
// ---------------------------------------------------------------------------

/// The lines of `text` that are not comments (first non-space char `#`).
fn code_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().filter(|l| !l.trim_start().starts_with('#'))
}

/// Splits a shell line into words: blanks separate words outside quotes
/// and `$( … )`; quotes are kept (removed by [`expand`]); a trailing `;`
/// is dropped from each word.
fn words(line: &str) -> Vec<String> {
    let (mut out, mut cur) = (Vec::new(), String::new());
    let (mut depth, mut dq, mut sq) = (0usize, false, false);
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' if !dq => sq = !sq,
            '"' if !sq => dq = !dq,
            '$' if !sq && chars.peek() == Some(&'(') => {
                cur.push(c);
                cur.push(chars.next().unwrap());
                depth += 1;
                continue;
            }
            ')' if !sq && depth > 0 => depth -= 1,
            c if c.is_whitespace() && !dq && !sq && depth == 0 => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    for w in &mut out {
        while w.ends_with(';') {
            w.pop();
        }
    }
    out.retain(|w| !w.is_empty());
    out
}

/// Expands a word: quotes removed, `$name` / `${name}` replaced by known
/// variables (unknown ones stay as written), and `$(basename …)` /
/// `$(dirname …)` evaluated; any other command substitution stays as
/// written.
fn expand(word: &str, vars: &BTreeMap<String, String>) -> String {
    let b: Vec<char> = word.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut sq = false;
    while i < b.len() {
        let c = b[i];
        if sq {
            if c == '\'' {
                sq = false;
            } else {
                out.push(c);
            }
            i += 1;
            continue;
        }
        match c {
            '\'' => sq = true,
            '"' => {}
            '$' if b.get(i + 1) == Some(&'(') => {
                let mut depth = 0;
                let mut j = i + 1;
                while j < b.len() {
                    match b[j] {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
                let inner: String = b[i + 2..j.min(b.len())].iter().collect();
                let w = words(&inner);
                let arg = w.get(1).map(|a| expand(a, vars));
                match (w.first().map(String::as_str), arg) {
                    (Some("basename"), Some(a)) => out.push_str(
                        Path::new(&a)
                            .file_name()
                            .map(|f| f.to_string_lossy().into_owned())
                            .unwrap_or_default()
                            .as_str(),
                    ),
                    (Some("dirname"), Some(a)) => out.push_str(
                        Path::new(&a)
                            .parent()
                            .map(|f| f.to_string_lossy().into_owned())
                            .unwrap_or_default()
                            .as_str(),
                    ),
                    _ => out.extend(&b[i..=j.min(b.len() - 1)]),
                }
                i = j + 1;
                continue;
            }
            '$' => {
                let braced = b.get(i + 1) == Some(&'{');
                let start = i + 1 + usize::from(braced);
                let mut j = start;
                while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == '_') {
                    j += 1;
                }
                let name: String = b[start..j].iter().collect();
                let end = if braced && b.get(j) == Some(&'}') {
                    j + 1
                } else {
                    j
                };
                match vars.get(&name) {
                    Some(v) if !name.is_empty() => out.push_str(v),
                    _ => out.extend(&b[i..end]),
                }
                i = end;
                continue;
            }
            _ => out.push(c),
        }
        i += 1;
    }
    out
}

/// A leading `name=value` word, split.
fn assignment(word: &str) -> Option<(&str, &str)> {
    let (name, value) = word.split_once('=')?;
    let ok = !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    ok.then_some((name, value))
}

/// Checks that the workflow, read as shell with table `table` substituted
/// for the `*` of every `for … in <glob>` loop, runs
/// `tables/<table>/rules-wasm/rebuild.sh <scratch>` and then a real `cmp`
/// of that same scratch file (outside `tables/`) with the committed
/// `tables/<table>/rules.wasm`. Comment lines are ignored, and `cmp`
/// counts only as a command word (not inside an `echo` string).
fn compares_rebuild(text: &str, table: &str) -> Result<(), String> {
    let script = format!("tables/{table}/rules-wasm/rebuild.sh");
    let committed = format!("tables/{table}/rules.wasm");
    let mut vars = BTreeMap::new();
    let mut rebuilt: Vec<String> = Vec::new();
    for line in code_lines(text) {
        let w = words(line.trim_start_matches(|c: char| c.is_whitespace() || c == '-'));
        if w.len() >= 4 && w[0] == "for" && w[2] == "in" {
            vars.insert(w[1].clone(), expand(&w[3], &vars).replace('*', table));
            continue;
        }
        let mut k = 0;
        while let Some((name, value)) = w.get(k).and_then(|x| assignment(x)) {
            let v = expand(value, &vars);
            vars.insert(name.to_owned(), v);
            k += 1;
        }
        for i in k..w.len() {
            let word = expand(&w[i], &vars);
            if (word == script || word.ends_with(&format!("/{script}"))) && i + 1 < w.len() {
                rebuilt.push(expand(&w[i + 1], &vars));
            }
            if w[i] == "cmp" {
                let ops: Vec<String> = w[i + 1..]
                    .iter()
                    .filter(|o| !o.starts_with('-'))
                    .take(2)
                    .map(|o| expand(o, &vars))
                    .collect();
                if ops.len() == 2 {
                    let pair = |a: &str, b: &str| {
                        b == committed
                            && !a.starts_with("tables/")
                            && rebuilt.iter().any(|r| r == a)
                    };
                    if pair(&ops[0], &ops[1]) || pair(&ops[1], &ops[0]) {
                        return Ok(());
                    }
                }
            }
        }
    }
    Err(format!(
        "no uncommented `cmp <rebuilt scratch file> {committed}` after a run of {script} \
         into that file (rebuild outputs seen: {rebuilt:?})"
    ))
}

#[test]
fn the_workflow_reader_needs_a_real_cmp_of_the_rebuilt_file() {
    let good = r#"
      - run: |
          for crate in tables/*/rules-wasm; do
            table=$(basename "$(dirname "$crate")")
            committed="tables/$table/rules.wasm"
            out="$RUNNER_TEMP/$table.wasm"
            CARGO_TARGET_DIR="$RUNNER_TEMP/t" sh "$crate/rebuild.sh" "$out"
            if ! cmp "$out" "$committed"; then exit 1; fi
          done
"#;
    for t in Table::ALL {
        assert_eq!(compares_rebuild(good, t.dir_name()), Ok(()), "{t:?}");
    }
    let literal = "sh tables/sky_deck/rules-wasm/rebuild.sh /tmp/a.wasm\ncmp /tmp/a.wasm tables/sky_deck/rules.wasm\n";
    assert_eq!(compares_rebuild(literal, "sky_deck"), Ok(()));
    assert!(
        compares_rebuild(literal, "apex_circuit").is_err(),
        "other table"
    );
    let commented = good.replace("if ! cmp", "# if ! cmp");
    assert!(
        compares_rebuild(&commented, "apex_circuit").is_err(),
        "comment"
    );
    let echoed = good.replace(
        "if ! cmp \"$out\" \"$committed\"; then exit 1; fi",
        "echo \"cmp $out $committed\"",
    );
    assert!(compares_rebuild(&echoed, "apex_circuit").is_err(), "echo");
    let wrong = good.replace("cmp \"$out\" \"$committed\"", "cmp \"$out\" \"$out\"");
    assert!(
        compares_rebuild(&wrong, "apex_circuit").is_err(),
        "self compare"
    );
    let unbuilt = good.replace("sh \"$crate/rebuild.sh\" \"$out\"", "true");
    assert!(
        compares_rebuild(&unbuilt, "apex_circuit").is_err(),
        "no rebuild"
    );
    let in_tree = good.replace(
        "out=\"$RUNNER_TEMP/$table.wasm\"",
        "out=\"tables/$table/rules.wasm\"",
    );
    assert!(
        compares_rebuild(&in_tree, "apex_circuit").is_err(),
        "in place"
    );
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
    if !wasm_ports_support::wasm32_gate("rebuild reproducibility not checked") {
        return;
    }
    // The guest build cache (a cargo target dir) is kept at a fixed path;
    // the rebuilt modules go to one temp dir removed at the end.
    let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("spec_6_3_rebuild_target");
    let tmp = TempDir::new("spec63_rebuild");
    for t in Table::ALL {
        let committed = committed_wasm(t);
        let script = t.src().join("rules-wasm").join("rebuild.sh");
        let out = tmp.path().join(format!("{}.wasm", t.dir_name()));
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
