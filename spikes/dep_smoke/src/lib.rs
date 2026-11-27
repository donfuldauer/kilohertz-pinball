//! Spike 0.1/Q7 support: smoke checks for the utility crates pinned in
//! `versions.toml` that no other spike exercises.

use arrayvec::ArrayVec;
use rand_core::{Rng, SeedableRng};
use rand_pcg::Pcg64Mcg;
use serde::{Deserialize, Serialize};

/// A small config-like struct for the serde/toml round trip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub name: String,
    pub tilt_threshold: f32,
    pub balls: u8,
    pub switches: Vec<u16>,
}

/// First `N` outputs of a seeded PCG stream.
pub fn pcg_stream<const N: usize>(seed: u64) -> [u64; N] {
    let mut rng = Pcg64Mcg::seed_from_u64(seed);
    std::array::from_fn(|_| rng.next_u64())
}

/// Fixed-capacity collection without heap allocation.
pub fn first_four(xs: &[u32]) -> ArrayVec<u32, 4> {
    xs.iter().copied().take(4).collect()
}

/// #164 KP-070: a minimal module built with wasm-encoder, exporting
/// `add: (i32, i32) -> i32`.
pub fn add_module() -> Vec<u8> {
    use wasm_encoder::{
        CodeSection, ExportKind, ExportSection, Function, FunctionSection, Instruction, Module,
        TypeSection, ValType,
    };
    let mut types = TypeSection::new();
    types
        .ty()
        .function([ValType::I32, ValType::I32], [ValType::I32]);
    let mut funcs = FunctionSection::new();
    funcs.function(0);
    let mut exports = ExportSection::new();
    exports.export("add", ExportKind::Func, 0);
    let mut body = Function::new([]);
    body.instruction(&Instruction::LocalGet(0))
        .instruction(&Instruction::LocalGet(1))
        .instruction(&Instruction::I32Add)
        .instruction(&Instruction::End);
    let mut code = CodeSection::new();
    code.function(&body);
    let mut m = Module::new();
    m.section(&types)
        .section(&funcs)
        .section(&exports)
        .section(&code);
    m.finish()
}

/// #164 KP-070: validates `bytes` with wasmparser (default features) and
/// counts the functions it defines.
pub fn defined_functions(bytes: &[u8]) -> Result<u32, wasmparser::BinaryReaderError> {
    wasmparser::validate(bytes)?;
    let mut n = 0;
    for payload in wasmparser::Parser::new(0).parse_all(bytes) {
        if let wasmparser::Payload::FunctionSection(r) = payload? {
            n = r.count();
        }
    }
    Ok(n)
}

/// #164 KP-070: runs `add(a, b)` from [`add_module`] in wasmi (fuel
/// metering on) and returns the sum and the fuel it used.
pub fn wasmi_add(a: i32, b: i32) -> Result<(i32, u64), wasmi::Error> {
    let mut config = wasmi::Config::default();
    config.consume_fuel(true);
    let engine = wasmi::Engine::new(&config);
    let module = wasmi::Module::new(&engine, add_module())?;
    let mut store = wasmi::Store::new(&engine, ());
    store.set_fuel(1_000)?;
    let instance = wasmi::Linker::<()>::new(&engine).instantiate_and_start(&mut store, &module)?;
    let add = instance.get_typed_func::<(i32, i32), i32>(&store, "add")?;
    let sum = add.call(&mut store, (a, b))?;
    Ok((sum, 1_000 - store.get_fuel()?))
}

/// #164 KP-070: the wasm-bindgen family compiles for the host. Only type
/// layouts are touched: their JS calls exist only on wasm32.
pub fn web_type_sizes() -> [usize; 3] {
    let _ = web_sys::wasm_bindgen::JsValue::UNDEFINED;
    [
        std::mem::size_of::<wasm_bindgen::JsValue>(),
        std::mem::size_of::<js_sys::Array>(),
        std::mem::size_of::<wasm_bindgen_futures::JsFuture>(),
    ]
}
