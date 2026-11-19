//! 6.3: every committed `tables/<t>/rules.wasm` is exactly what its
//! `rules-wasm/rebuild.sh` builds from the Rust port (as CI's
//! `rules-wasm.yml` checks with `cmp`), and the table sources still select
//! the Lua rules (`script_entry = "rules.lua"`): the Wasm module is used
//! only by a bundle copy whose `table.toml` names it.
//!
//! The rebuild is skipped with a printed note where the
//! `wasm32-unknown-unknown` target is not installed; the CI workflow
//! installs it, so there the check always runs.

mod wasm_ports_support;

use wasm_ports_support::{check_rebuild, committed_wasm, table_src};

const TABLES: [&str; 3] = ["apex_circuit", "sky_deck", "feature_lab"];

#[test]
fn committed_rules_wasm_match_their_sources() {
    for t in TABLES {
        check_rebuild(t);
    }
}

#[test]
fn committed_modules_are_release_wasm_and_lua_stays_the_default() {
    for t in TABLES {
        let wasm = committed_wasm(t);
        assert_eq!(&wasm[..4], b"\0asm", "{t}: rules.wasm is a Wasm module");
        assert!(
            wasm.len() < 64 * 1024,
            "{t}: rules.wasm is {} bytes; a release build is far smaller",
            wasm.len()
        );
        let toml = std::fs::read_to_string(table_src(t).join("table.toml")).unwrap();
        assert!(
            toml.contains("script_entry = \"rules.lua\""),
            "{t}: the sources keep the Lua rules as the default backend"
        );
        assert!(table_src(t).join("rules.lua").is_file());
        assert!(table_src(t).join("rules-wasm/Cargo.toml").is_file());
    }
}
