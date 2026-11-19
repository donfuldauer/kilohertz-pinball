//! 6.3: every committed `tables/<t>/rules.wasm` is exactly what its
//! `rules-wasm/rebuild.sh` builds from the Rust port (as CI's
//! `rules-wasm.yml` checks with `cmp`), and the table sources still select
//! the Lua rules (`script_entry = "rules.lua"`): the Wasm module is used
//! only by a bundle copy whose `table.toml` names it.
//!
//! The rebuild is skipped with a printed `SKIP:` note where the
//! `wasm32-unknown-unknown` target is not installed, unless
//! `PIN_REQUIRE_WASM32` is set (non-empty, not `0`): then a missing target
//! fails the test. The rules-wasm CI workflow installs the target and sets
//! the variable, so there the check always runs.

mod wasm_ports_support;

use wasm_ports_support::{
    Wasm32Gate, apply_wasm32_gate, check_rebuild, committed_wasm, table_src, wasm32_gate_decision,
};

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

#[test]
fn the_wasm32_gate_runs_skips_or_fails() {
    use Wasm32Gate::{Fail, Run, Skip};
    for require in [None, Some(""), Some("0"), Some("1"), Some("yes")] {
        assert_eq!(wasm32_gate_decision(true, require), Run, "{require:?}");
    }
    for require in [None, Some(""), Some("0")] {
        assert_eq!(wasm32_gate_decision(false, require), Skip, "{require:?}");
    }
    for require in [Some("1"), Some("true"), Some("00")] {
        assert_eq!(wasm32_gate_decision(false, require), Fail, "{require:?}");
    }
    assert!(apply_wasm32_gate(Run, "x"));
    assert!(!apply_wasm32_gate(Skip, "x"));
}

#[test]
#[should_panic(expected = "PIN_REQUIRE_WASM32 is set")]
fn a_required_but_missing_wasm32_target_fails() {
    apply_wasm32_gate(
        wasm32_gate_decision(false, Some("1")),
        "rebuild not checked",
    );
}
