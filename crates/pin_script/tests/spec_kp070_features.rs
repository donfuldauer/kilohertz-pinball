//! REQ-WEB-001: the backend feature flags of pin_script (text checks of the manifest).

fn manifest() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap()
}

fn section<'a>(text: &'a str, name: &str) -> &'a str {
    let head = format!("[{name}]");
    let start = text.find(&head).unwrap_or_else(|| panic!("no {head}")) + head.len();
    let rest = &text[start..];
    // The section ends at the next table header at line start.
    let end = rest.find("\n[").unwrap_or(rest.len());
    &rest[..end]
}

/// The full text of feature `name` (its line up to the closing bracket).
fn feature<'a>(features: &'a str, name: &str) -> &'a str {
    for line in features.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix(name)
            && rest.trim_start().starts_with('=')
        {
            return line;
        }
    }
    panic!("feature {name} missing");
}

#[test]
fn req_web_001_feature_table() {
    let m = manifest();
    let f = section(&m, "features");
    let d = feature(f, "default");
    assert!(d.contains(r#"["lua", "wasmtime"]"#), "{d}");
    let lua = feature(f, "lua");
    assert!(
        lua.contains("dep:mlua") && lua.contains("dep:libm"),
        "{lua}"
    );
    let wt = feature(f, "wasmtime");
    for dep in ["dep:wasmtime", "dep:wat", "dep:notify"] {
        assert!(wt.contains(dep), "{wt}");
    }
    let wi = feature(f, "wasmi");
    assert!(wi.contains("dep:wasmi"), "{wi}");
}

#[test]
fn req_web_001_wasm_tooling_is_unconditional() {
    let m = manifest();
    let deps = section(&m, "dependencies");
    for name in ["wasmparser", "wasm-encoder"] {
        let line = deps
            .lines()
            .find(|l| l.trim_start().starts_with(name))
            .unwrap_or_else(|| panic!("{name} missing"));
        assert!(line.contains("workspace = true"), "{line}");
        assert!(!line.contains("optional"), "{line}");
    }
}

#[test]
fn req_web_001_wasm_smith_dev_dependency() {
    let m = manifest();
    let dev = section(&m, "dev-dependencies");
    assert!(
        dev.lines()
            .any(|l| l.trim().replace(' ', "") == "wasm-smith.workspace=true"),
        "{dev}"
    );
}
