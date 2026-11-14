//! Spike 0.1/Q1: does `sdl3::event::Event` expose SDL's nanosecond timestamp?
//!
//! Answer (sdl3 =0.22.1): yes. Every `Event` variant carries `timestamp: u64`
//! copied verbatim from `SDL_CommonEvent::timestamp`, which SDL3 fills from
//! `SDL_GetTicksNS()`; `Event::get_timestamp()` returns it for any variant.
//!
//! The §6 fallback (reading the raw `SDL_Event` through the `sdl3-sys`
//! re-export) is kept here as a thin wrapper so the test can prove both paths
//! agree, and so Subphase 1.x has it ready if a future crate version drops
//! the field.

use sdl3::event::Event;
use sdl3::sys;
use sdl3::sys::events::SDL_Event;

/// Nanosecond timestamp (SDL_GetTicksNS clock) of a high-level event.
#[inline]
pub fn timestamp_ns(event: &Event) -> u64 {
    event.get_timestamp()
}

/// Nanosecond timestamp of a raw `SDL_Event` (fallback path via `sdl3-sys`).
#[inline]
pub fn raw_timestamp_ns(event: &SDL_Event) -> u64 {
    // SAFETY: every SDL_Event variant starts with SDL_CommonEvent.
    unsafe { event.common.timestamp }
}

/// Raw event type id.
#[inline]
pub fn raw_type(event: &SDL_Event) -> u32 {
    // SAFETY: `type` is the first field of every variant.
    unsafe { event.r#type }
}

/// Current value of SDL's nanosecond tick clock.
#[inline]
pub fn ticks_ns() -> u64 {
    // SAFETY: plain FFI query, valid after SDL_Init.
    unsafe { sys::timer::SDL_GetTicksNS() }
}

/// Copy pending events (without removing them) into `buf`; returns the count.
pub fn peek_raw_events(buf: &mut [SDL_Event]) -> usize {
    let len = i32::try_from(buf.len()).unwrap_or(i32::MAX);
    // SAFETY: buf is valid for `len` writes; PEEK leaves the queue untouched.
    let n = unsafe {
        sys::events::SDL_PeepEvents(
            buf.as_mut_ptr(),
            len,
            sys::events::SDL_PEEKEVENT,
            sys::events::SDL_EVENT_FIRST.0,
            sys::events::SDL_EVENT_LAST.0,
        )
    };
    usize::try_from(n).unwrap_or(0)
}
