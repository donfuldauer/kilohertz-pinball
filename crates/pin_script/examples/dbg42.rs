use pin_core::*;
use pin_script::lua::*;
fn main() {
    let src = std::env::args().nth(1).unwrap();
    let mut b = LuaScriptBackend::new(src.as_bytes(), &pin_script::NameTables::default(), LuaConfig{memory_limit_bytes: 1<<20, instruction_budget: 50_000}).unwrap();
    eprintln!("built");
    let view = HostView::default();
    let mut rng = Pcg32::seed_from_u64(1);
    for i in 0..5 {
        let mut cmd = HostCommandQueue::default();
        b.on_tick(&mut HostCtx { view: &view, rng: &mut rng, cmd: &mut cmd });
        eprintln!("tick {i} faulted={} err={:?}", b.is_faulted(), b.last_error());
    }
    drop(b);
    eprintln!("dropped");
}
