//! Q2: a virtual joystick can be attached, driven and detached with no display
//! (dummy video driver), producing ordinary SDL joystick events.

use sdl_virtual_joystick::{PLUNGER_AXIS, attach_cabinet, button_index, set_headless_hints};
use sdl3::EventPump;
use sdl3::event::Event;
use sdl3::gamepad::Button;

/// Drain the queue completely. Stopping a `poll_iter()` early leaves SDL3's
/// poll sentinel queued, which makes the *next* poll loop end immediately.
fn drain(pump: &mut EventPump) -> Vec<Event> {
    pump.poll_iter().collect()
}

fn main() {
    set_headless_hints();
    let sdl = sdl3::init().expect("SDL_Init");
    // Video is not needed for joysticks, but the engine will init it; prove the
    // dummy driver coexists headless.
    let video = sdl.video().expect("dummy video driver");
    println!("[sdl_vjoy] video driver: {}", video.current_video_driver());
    let joystick = sdl.joystick().expect("joystick subsystem");
    let mut pump = sdl.event_pump().expect("event pump");
    drain(&mut pump);

    let before = joystick.joysticks().expect("list").len();
    let conn = attach_cabinet(&joystick).expect("attach virtual joystick");
    let id = conn.id();
    assert!(joystick.is_virtual(id));
    let after = joystick.joysticks().expect("list").len();
    println!("[sdl_vjoy] attached {id:?}; joysticks {before} -> {after}");
    assert_eq!(after, before + 1);

    let added = drain(&mut pump)
        .iter()
        .any(|e| matches!(e, Event::JoyDeviceAdded { which, .. } if *which == id));
    assert!(added, "JoyDeviceAdded must be delivered headless");

    let js = joystick.open(id).expect("open virtual joystick");
    println!(
        "[sdl_vjoy] name={:?} buttons={} axes={}",
        js.name(),
        js.num_buttons(),
        js.num_axes()
    );

    // Button press -> event + polled state.
    let flipper = button_index(Button::RightShoulder);
    js.set_virtual_button(flipper, true).expect("press");
    joystick.update();
    let pressed: Vec<u8> = drain(&mut pump)
        .into_iter()
        .filter_map(|e| match e {
            Event::JoyButtonDown {
                which, button_idx, ..
            } if which == id => Some(button_idx),
            _ => None,
        })
        .collect();
    assert_eq!(pressed, [flipper as u8]);
    assert!(js.button(flipper).expect("button state"));

    js.set_virtual_button(flipper, false).expect("release");
    joystick.update();
    assert!(
        drain(&mut pump)
            .iter()
            .any(|e| matches!(e, Event::JoyButtonUp { which, .. } if *which == id))
    );
    assert!(!js.button(flipper).expect("button state"));

    // Analog plunger.
    let axis = PLUNGER_AXIS as u32;
    js.set_virtual_axis(axis, 20_000).expect("axis");
    joystick.update();
    // SDL emits the axis' initial value (0) once before the first real change,
    // so the latest event for the axis is the one that matters.
    let values: Vec<i16> = drain(&mut pump)
        .into_iter()
        .filter_map(|e| match e {
            Event::JoyAxisMotion {
                which,
                axis_idx,
                value,
                ..
            } if which == id && u32::from(axis_idx) == axis => Some(value),
            _ => None,
        })
        .collect();
    println!("[sdl_vjoy] plunger axis events: {values:?}");
    assert_eq!(values.last(), Some(&20_000));
    assert_eq!(js.axis(axis).expect("axis state"), 20_000);

    // Detach -> removal event.
    drop(js);
    drop(conn);
    assert!(
        drain(&mut pump)
            .iter()
            .any(|e| matches!(e, Event::JoyDeviceRemoved { which, .. } if *which == id))
    );
    println!("[sdl_vjoy] RESULT: virtual joystick attach/press/axis/detach works headless");
}
