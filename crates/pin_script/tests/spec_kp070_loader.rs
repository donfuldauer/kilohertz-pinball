//! REQ-WEB-001, D10, SPEC §3.4: `load_backend` picks the backend by the
//! (case-insensitive) extension; a backend that is not built in reports
//! `UnsupportedBackend`.

use pin_core::ScriptError;
use pin_script::{NameTables, load_backend};

const LUA_SRC: &[u8] = b"function on_tick() pinball.set_lamp(1, 1) end";
const BAD_WASM: &[u8] = b"this is not a wasm module";

fn load(entry: &str, src: &[u8]) -> Result<(), ScriptError> {
    load_backend(entry, src, &NameTables::default()).map(|_| ())
}

fn assert_unsupported(entry: &str, src: &[u8]) {
    match load(entry, src) {
        Err(ScriptError::UnsupportedBackend(m)) => {
            assert!(!m.to_string().is_empty(), "{entry}: empty message")
        }
        other => panic!("{entry}: expected UnsupportedBackend, got {other:?}"),
    }
}

#[test]
fn req_web_001_unknown_and_missing_extensions_are_unsupported() {
    for entry in [
        "rules",
        "rules.txt",
        "rules.rs",
        "rules.",
        "",
        "lua",
        "wasm",
    ] {
        assert_unsupported(entry, LUA_SRC);
        assert_unsupported(entry, BAD_WASM);
    }
}

#[test]
fn req_web_001_lua_entries() {
    for entry in ["rules.lua", "rules.LUA"] {
        #[cfg(feature = "lua")]
        assert!(load(entry, LUA_SRC).is_ok(), "{entry}");
        #[cfg(not(feature = "lua"))]
        assert_unsupported(entry, LUA_SRC);
    }
}

#[test]
fn req_web_001_wasm_entries() {
    for entry in ["rules.wasm", "rules.WASM"] {
        #[cfg(feature = "wasmtime")]
        {
            let r = load(entry, BAD_WASM);
            assert!(r.is_err(), "{entry}: bad blob must be rejected");
            assert!(
                !matches!(r, Err(ScriptError::UnsupportedBackend(_))),
                "{entry}: backend is built in"
            );
        }
        #[cfg(not(feature = "wasmtime"))]
        assert_unsupported(entry, BAD_WASM);
    }
}
