//! REQ-WEB-001: exact dependency pins of the web engine, mirrored in versions.toml.

fn read(rel: &str) -> String {
    std::fs::read_to_string(format!("{}/../../{rel}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// The root Cargo.toml lines after the `#164 KP-070 additions` comment.
fn additions(root: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut on = false;
    for l in root.lines() {
        if l.trim_start().starts_with('#') && l.contains("#164 KP-070 additions") {
            on = true;
            continue;
        }
        if on {
            let t = l.trim();
            if t.starts_with('[') {
                break;
            }
            if !t.is_empty() && !t.starts_with('#') {
                out.push(l);
            }
        }
    }
    assert!(
        !out.is_empty(),
        "no lines under the KP-070 additions comment"
    );
    out
}

fn line_for<'a>(lines: &[&'a str], name: &str) -> &'a str {
    lines
        .iter()
        .copied()
        .find(|l| {
            let t = l.trim_start();
            t.starts_with(name) && t[name.len()..].trim_start().starts_with('=')
        })
        .unwrap_or_else(|| panic!("{name} missing"))
}

/// The quoted `=x.y.z` string on `line` (first `"=..."`).
fn pin(line: &str) -> String {
    let i = line
        .find("\"=")
        .unwrap_or_else(|| panic!("no exact pin in {line}"));
    let rest = &line[i + 1..];
    rest[..rest.find('"').unwrap()].to_string()
}

const EXPECTED: &[(&str, &str)] = &[
    ("wasmi", "=2.0.2"),
    ("wasmparser", "=0.261.0"),
    ("wasm-encoder", "=0.261.0"),
    ("wasm-bindgen", "=0.2.130"),
    ("js-sys", "=0.3.107"),
    ("web-sys", "=0.3.107"),
    ("wasm-bindgen-futures", "=0.4.80"),
];

#[test]
fn req_web_001_root_pins_are_exact() {
    let root = read("Cargo.toml");
    let lines = additions(&root);
    for (name, want) in EXPECTED {
        assert_eq!(&pin(line_for(&lines, name)), want, "{name}");
    }
    let smith = pin(line_for(&lines, "wasm-smith"));
    let digits: Vec<&str> = smith[1..].split('.').collect();
    assert_eq!(digits.len(), 3, "{smith}");
    assert!(digits.iter().all(|d| d.parse::<u32>().is_ok()), "{smith}");
    for l in &lines {
        if l.contains('=') && l.contains("version") || l.contains("\"") {
            assert!(l.contains("\"="), "every pin starts with '=': {l}");
        }
    }
    let wasmi = line_for(&lines, "wasmi");
    assert!(wasmi.contains("default-features = false"), "{wasmi}");
    for f in ["std", "validate", "auto-dispatch", "deterministic"] {
        assert!(wasmi.contains(&format!("\"{f}\"")), "{wasmi}");
    }
}

#[test]
fn req_web_001_versions_toml_mirrors_the_pins() {
    let root = read("Cargo.toml");
    let lines = additions(&root);
    let v = read("versions.toml");
    let start = v.find("\n[crates]").expect("[crates]") + 1;
    let rest = &v[start + "[crates]".len()..];
    let section: Vec<&str> = rest
        .lines()
        .take_while(|l| !l.trim_start().starts_with('['))
        .collect();
    let mut names: Vec<&str> = EXPECTED.iter().map(|(n, _)| *n).collect();
    names.push("wasm-smith");
    for name in names {
        let want = pin(line_for(&lines, name));
        let l = line_for(&section, name);
        assert!(l.contains(&format!("\"{want}\"")), "{name}: {l} vs {want}");
    }
}
