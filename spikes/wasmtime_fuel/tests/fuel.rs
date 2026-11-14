use wasmtime::Trap;
use wasmtime_fuel::{fuel_engine, instantiate};

const SPIN: &str = r#"
(module
  (func (export "spin") (loop $l (br $l))))
"#;

const SUM: &str = r#"
(module
  (func (export "sum") (param $n i32) (result i32)
    (local $acc i32)
    (block $done
      (loop $l
        (br_if $done (i32.eqz (local.get $n)))
        (local.set $acc (i32.add (local.get $acc) (local.get $n)))
        (local.set $n (i32.sub (local.get $n) (i32.const 1)))
        (br $l)))
    (local.get $acc)))
"#;

#[test]
fn infinite_loop_runs_out_of_fuel() {
    let engine = fuel_engine().unwrap();
    let (mut store, instance) = instantiate(&engine, SPIN, 100_000).unwrap();
    let spin = instance
        .get_typed_func::<(), ()>(&mut store, "spin")
        .unwrap();
    let err = spin.call(&mut store, ()).expect_err("must trap");
    let trap = err.downcast_ref::<Trap>().copied();
    println!(
        "[wasmtime] spin trap: {trap:?}; fuel left {}",
        store.get_fuel().unwrap()
    );
    assert_eq!(trap, Some(Trap::OutOfFuel));
    assert_eq!(store.get_fuel().unwrap(), 0);
}

#[test]
fn fuel_consumption_is_deterministic_and_refillable() {
    let engine = fuel_engine().unwrap();
    let budget = 1_000_000;
    let used = |n: i32| {
        let (mut store, instance) = instantiate(&engine, SUM, budget).unwrap();
        let sum = instance
            .get_typed_func::<i32, i32>(&mut store, "sum")
            .unwrap();
        let r = sum.call(&mut store, n).unwrap();
        (r, budget - store.get_fuel().unwrap())
    };
    let (r1, f1) = used(1000);
    let (r2, f2) = used(1000);
    let (_, f3) = used(2000);
    println!("[wasmtime] sum(1000)={r1} fuel={f1}; sum(2000) fuel={f3}");
    assert_eq!(r1, 500_500);
    assert_eq!((r1, f1), (r2, f2));
    assert!(f3 > f1);

    // Too little fuel traps; refilling the same store lets it finish.
    let (mut store, instance) = instantiate(&engine, SUM, f1 / 2).unwrap();
    let sum = instance
        .get_typed_func::<i32, i32>(&mut store, "sum")
        .unwrap();
    let err = sum.call(&mut store, 1000).unwrap_err();
    assert_eq!(err.downcast_ref::<Trap>().copied(), Some(Trap::OutOfFuel));
    store.set_fuel(f1).unwrap();
    assert_eq!(sum.call(&mut store, 1000).unwrap(), 500_500);
}
