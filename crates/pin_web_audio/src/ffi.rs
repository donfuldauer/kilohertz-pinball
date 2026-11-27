//! The `pwa_*` C ABI (REQ-WEB-022) the AudioWorklet calls. Every function
//! delegates to [`WebMixer`]; all integers are little-endian (wasm32).
//!
//! Every function taking an `Engine` pointer accepts null and then does
//! nothing: it returns null, 0, NaN, or, for [`pwa_register`], `-6`.

use crate::protocol::CMD_BUFFER_BYTES;
use crate::web_mixer::WebMixer;

/// The opaque engine behind the C ABI.
pub type Engine = WebMixer;

/// `pwa_register` return code for a null engine pointer (not reachable with
/// a pointer from [`pwa_new`]).
pub const REGISTER_NULL_ENGINE: i32 = -6;

/// # Safety
/// `e` is null or a live pointer from [`pwa_new`] with no other reference
/// to it alive.
unsafe fn engine<'a>(e: *mut Engine) -> Option<&'a mut Engine> {
    // SAFETY: the caller guarantees `e` is null or valid and unaliased.
    unsafe { e.as_mut() }
}

/// Creates an engine (see [`WebMixer::new`]); null when an argument is out
/// of range. All per-render storage is allocated here.
#[unsafe(no_mangle)]
pub extern "C" fn pwa_new(
    sample_rate: u32,
    buffer_frames: u32,
    trigger_capacity: u32,
    budget_mib: u32,
) -> *mut Engine {
    match WebMixer::new(sample_rate, buffer_frames, trigger_capacity, budget_mib) {
        Some(m) => Box::into_raw(Box::new(m)),
        None => std::ptr::null_mut(),
    }
}

/// Frees an engine; null is a no-op.
///
/// # Safety
/// `e` is null or a pointer from [`pwa_new`] not yet freed; it is invalid
/// afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pwa_free(e: *mut Engine) {
    if !e.is_null() {
        // SAFETY: `e` came from `Box::into_raw` in `pwa_new` (caller contract).
        drop(unsafe { Box::from_raw(e) });
    }
}

/// The engine-owned PCM staging area of at least `n_values` `f32` (load time
/// only; may grow). Null when `n_values × 4` exceeds the budget limit. The
/// pointer is valid until the next `pwa_stage` or `pwa_free`.
///
/// # Safety
/// `e` is null or a live pointer from [`pwa_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pwa_stage(e: *mut Engine, n_values: u32) -> *mut f32 {
    // SAFETY: caller contract.
    match unsafe { engine(e) }.and_then(|m| m.stage(n_values)) {
        Some(s) => s.as_mut_ptr(),
        None => std::ptr::null_mut(),
    }
}

/// Registers `channels × frames` staged values as `sound_id` on `bus`
/// (0 music, 1 callout, 2 sfx). Returns 0, or -1 (`sound_id > 65535`),
/// -2 (`bus > 2`), -3 (`channels` not 1 or 2), -4 (`frames == 0` or more
/// than was staged), -5 (budget exhausted; nothing changes), -6 (null `e`).
///
/// # Safety
/// `e` is null or a live pointer from [`pwa_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pwa_register(
    e: *mut Engine,
    sound_id: u32,
    bus: u32,
    channels: u32,
    frames: u32,
) -> i32 {
    // SAFETY: caller contract.
    match unsafe { engine(e) } {
        Some(m) => match m.register(sound_id, bus, channels, frames) {
            Ok(()) => 0,
            Err(err) => err.code(),
        },
        None => REGISTER_NULL_ENGINE,
    }
}

/// One immediate ("next quantum") trigger; 1 accepted, 0 rejected or dropped.
///
/// # Safety
/// `e` is null or a live pointer from [`pwa_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pwa_trigger(e: *mut Engine, sound_id: u32, gain: f32) -> u32 {
    // SAFETY: caller contract.
    unsafe { engine(e) }.is_some_and(|m| m.trigger(sound_id, gain)) as u32
}

/// The command staging area (4096 bytes, 256 records), valid until
/// [`pwa_free`].
///
/// # Safety
/// `e` is null or a live pointer from [`pwa_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pwa_cmd_buffer(e: *mut Engine) -> *mut u8 {
    // SAFETY: caller contract.
    match unsafe { engine(e) } {
        Some(m) => m.cmd_buffer().as_mut_ptr(),
        None => std::ptr::null_mut(),
    }
}

/// Parses `n_bytes` (clamped to 4096) of the command buffer as 16-byte
/// records; returns how many entered the pending list.
///
/// # Safety
/// `e` is null or a live pointer from [`pwa_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pwa_submit(e: *mut Engine, n_bytes: u32) -> u32 {
    const _: () = assert!(CMD_BUFFER_BYTES == 4096);
    // SAFETY: caller contract.
    unsafe { engine(e) }.map_or(0, |m| m.submit(n_bytes))
}

/// Renders `frames` stereo frames; returns a pointer to `2 × frames`
/// interleaved values in engine memory, valid until the next call, or null
/// when `frames` is 0 or above `buffer_frames`.
///
/// # Safety
/// `e` is null or a live pointer from [`pwa_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pwa_render(e: *mut Engine, frames: u32) -> *const f32 {
    // SAFETY: caller contract.
    match unsafe { engine(e) }.and_then(|m| m.render(frames)) {
        Some(out) => out.as_ptr(),
        None => std::ptr::null(),
    }
}

/// One statistic (see [`WebMixer::stat`]); NaN for an unknown `which` or a
/// null `e`.
///
/// # Safety
/// `e` is null or a live pointer from [`pwa_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pwa_stat(e: *mut Engine, which: u32) -> f64 {
    // SAFETY: caller contract.
    unsafe { engine(e) }.map_or(f64::NAN, |m| m.stat(which))
}
