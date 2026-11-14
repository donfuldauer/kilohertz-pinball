//! `versions.toml` must agree with what the spikes actually built and tested:
//! every [crates] pin equals the spikes workspace dependency (where declared)
//! and the version resolved in spikes/Cargo.lock.

use std::collections::BTreeMap;
use std::path::PathBuf;

fn read(rel: &str) -> toml::Table {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    text.parse()
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn strip_meta(v: &str) -> &str {
    v.split('+').next().unwrap_or(v)
}

#[test]
fn versions_toml_matches_workspace_and_lockfile() {
    let versions = read("../../versions.toml");
    let workspace = read("../Cargo.toml");
    let lock = read("../Cargo.lock");

    let toolchain = versions["toolchain"].as_table().expect("[toolchain]");
    assert_eq!(toolchain["edition"].as_str(), Some("2024"));
    assert!(toolchain["rust"].as_str().is_some());
    assert!(
        versions["notes"]
            .as_table()
            .is_some_and(|n| n.contains_key("sdl3_build"))
    );

    let mut locked: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for pkg in lock["package"].as_array().expect("lock packages") {
        let name = pkg["name"].as_str().unwrap();
        locked
            .entry(name)
            .or_default()
            .push(strip_meta(pkg["version"].as_str().unwrap()));
    }

    let ws_deps = workspace["workspace"]["dependencies"]
        .as_table()
        .expect("workspace deps");
    let crates = versions["crates"].as_table().expect("[crates]");
    let features = versions.get("features").and_then(|f| f.as_table());
    for (name, req) in crates {
        let req = req
            .as_str()
            .unwrap_or_else(|| panic!("{name}: pin must be a string"));
        let exact = req
            .strip_prefix('=')
            .unwrap_or_else(|| panic!("{name}: pin {req:?} must be exact (=x.y.z)"));
        assert_eq!(exact.split('.').count(), 3, "{name}: {req:?} is not x.y.z");
        let in_lock = locked
            .get(name.as_str())
            .unwrap_or_else(|| panic!("{name} not in spikes/Cargo.lock"));
        assert!(
            in_lock.contains(&exact),
            "{name}: pinned {exact}, lock has {in_lock:?}"
        );

        if let Some(dep) = ws_deps.get(name) {
            let ws_req = dep
                .as_str()
                .or_else(|| dep.get("version").and_then(|v| v.as_str()))
                .unwrap();
            assert_eq!(
                ws_req, req,
                "{name}: spikes/Cargo.toml differs from versions.toml"
            );
            let ws_feats: Vec<&str> = dep
                .get("features")
                .and_then(|f| f.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            let pinned_feats: Vec<&str> = features
                .and_then(|f| f.get(name))
                .and_then(|f| f.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            assert_eq!(
                ws_feats, pinned_feats,
                "{name}: features differ from versions.toml [features]"
            );
        }
    }
    // Every crate the spikes depend on directly is pinned.
    for name in ws_deps.keys() {
        assert!(
            crates.contains_key(name),
            "{name} used by spikes but not pinned in versions.toml"
        );
    }
}
