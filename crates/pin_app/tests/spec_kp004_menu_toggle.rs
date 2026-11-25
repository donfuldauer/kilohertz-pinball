//! KP-004 (#84) black-box tests, REQ-VIEW-006 (SPEC section 7.1): Esc and the
//! controller Guide button toggle the menu; acceptance 1 (headless: `Esc`
//! toggles the menu state).

#[path = "spec_kp004_support/mod.rs"]
mod support;

use pin_core::{CabinetInput, InputEvent};
use sdl3::gamepad::Button;
use sdl3::keyboard::Scancode;
use support::{Rig, key, pad_button};

use pin_app::platform::Translated;

fn toggles_with(routed: bool) {
    let mut r = Rig::new("toggle", routed);
    assert!(!r.is_open());
    r.esc(100);
    assert!(r.is_open(), "Esc opens (routed={routed})");
    r.esc(200);
    assert!(!r.is_open(), "second Esc closes (routed={routed})");
    r.esc(300);
    assert!(r.is_open());
    assert!(r.drain().is_empty(), "Esc itself produces no cabinet event");
}

/// REQ-VIEW-006 acceptance 1: Esc toggles the menu (router set).
#[test]
fn req_view_006_esc_toggles_menu_with_router() {
    toggles_with(true);
}

/// REQ-VIEW-006 acceptance 1: Esc toggles the menu (unrouted keymap path).
#[test]
fn req_view_006_esc_toggles_menu_without_router() {
    toggles_with(false);
}

/// REQ-VIEW-006: the Esc event is swallowed (Translated::Ignored).
#[test]
fn req_view_006_esc_is_ignored_by_caller() {
    let mut r = Rig::new("ignored", true);
    let t = r.fwd.forward(&key(true, Scancode::Escape, 5, false));
    assert_eq!(t, Translated::Ignored);
}

/// REQ-VIEW-006: a repeat, a key-up and any other key do not toggle.
#[test]
fn req_view_006_repeat_keyup_and_other_keys_do_not_toggle() {
    let mut r = Rig::new("nontoggle", true);
    r.send(key(true, Scancode::Escape, 10, true));
    assert!(!r.is_open(), "repeat does not open");
    r.send(key(false, Scancode::Escape, 20, false));
    assert!(!r.is_open(), "key-up does not open");
    for sc in [
        Scancode::Q,
        Scancode::F1,
        Scancode::Tab,
        Scancode::Backspace,
    ] {
        r.send(key(true, sc, 30, false));
        assert!(!r.is_open(), "{sc:?} does not toggle");
    }
    r.esc(40);
    assert!(r.is_open());
    r.send(key(true, Scancode::Escape, 50, true));
    assert!(r.is_open(), "repeat does not close");
    r.send(key(false, Scancode::Escape, 60, false));
    assert!(r.is_open(), "key-up does not close");
    r.send(key(true, Scancode::Q, 70, false));
    assert!(r.is_open(), "other key does not close");
}

fn guide_toggles_with(routed: bool) {
    let mut r = Rig::new("guide", routed);
    r.send(pad_button(true, Button::Guide, 100));
    assert!(r.is_open(), "Guide opens");
    r.send(pad_button(false, Button::Guide, 150));
    assert!(r.is_open(), "Guide release does not toggle");
    r.send(pad_button(true, Button::Guide, 200));
    assert!(!r.is_open(), "Guide closes");
}

/// REQ-VIEW-006 acceptance 1: the controller Guide button toggles (router).
#[test]
fn req_view_006_guide_button_toggles_with_router() {
    guide_toggles_with(true);
}

/// REQ-VIEW-006 acceptance 1: the controller Guide button toggles (unrouted).
#[test]
fn req_view_006_guide_button_toggles_without_router() {
    guide_toggles_with(false);
}

/// REQ-VIEW-006: Start and Back (cabinet buttons) do not toggle the menu.
#[test]
fn req_view_006_start_and_back_do_not_toggle() {
    let mut r = Rig::new("startback", false);
    for b in [Button::Start, Button::Back, Button::South, Button::East] {
        r.send(pad_button(true, b, 100));
        r.send(pad_button(false, b, 110));
        assert!(!r.is_open(), "{b:?} must not toggle");
    }
    // Start (closed menu) still reaches the sim as a cabinet press.
    let evs = r.drain();
    assert!(
        evs.iter()
            .any(|e| support::is_button(e, CabinetInput::Start, true)),
        "Start reaches the sim while closed: {evs:?}"
    );
}

/// REQ-VIEW-006: without a menu the forwarder behaves as before (Esc is no
/// cabinet event and nothing toggles).
#[test]
fn req_view_006_no_menu_esc_is_inert() {
    use pin_core::spsc::channel;
    let (itx, mut irx) = channel::<InputEvent>(16);
    let (wtx, _wrx) = channel::<pin_render::topology::WindowEvent>(16);
    let mut f = pin_app::platform::EventForwarder::new(itx, wtx);
    assert!(f.menu().is_none());
    let t = f.forward(&key(true, Scancode::Escape, 1, false));
    assert_eq!(t, Translated::Ignored);
    assert!(irx.pop().is_none());
}

/// REQ-VIEW-006: take_menu returns the controller and clears the slot.
#[test]
fn req_view_006_take_menu_returns_controller() {
    let mut r = Rig::new("take", true);
    r.esc(10);
    let m = r.fwd.take_menu().expect("menu");
    assert!(m.is_open());
    assert!(r.fwd.menu().is_none());
    assert!(r.fwd.menu_mut().is_none());
}
