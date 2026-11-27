//! Thin safe wrapper over the brief's C ABI (`pwa_*`) for the KP-086 spec
//! tests (REQ-WEB-022). Calls only the public `pub extern "C"` functions.
#![allow(dead_code)]

use pin_web_audio::ffi::{
    Engine, pwa_cmd_buffer, pwa_free, pwa_new, pwa_register, pwa_render, pwa_stage, pwa_stat,
    pwa_submit, pwa_trigger,
};

/// Owns one engine from `pwa_new`; frees it on drop.
pub struct Eng(pub *mut Engine);

impl Eng {
    /// `pwa_new`; `None` when it returns null.
    pub fn new(rate: u32, frames: u32, cap: u32, mib: u32) -> Option<Self> {
        let e = pwa_new(rate, frames, cap, mib);
        (!e.is_null()).then_some(Self(e))
    }

    /// `pwa_stage(n)` then copies `pcm` into it; false when null.
    pub fn stage(&mut self, n: u32, pcm: &[f32]) -> bool {
        // SAFETY: `self.0` came from `pwa_new` and is live.
        let p = unsafe { pwa_stage(self.0, n) };
        if p.is_null() {
            return false;
        }
        assert!(pcm.len() <= n as usize);
        // SAFETY: the brief guarantees at least `n` f32 at `p`.
        unsafe { std::ptr::copy_nonoverlapping(pcm.as_ptr(), p, pcm.len()) };
        true
    }

    /// `pwa_register`.
    pub fn register_raw(&mut self, id: u32, bus: u32, channels: u32, frames: u32) -> i32 {
        // SAFETY: live engine.
        unsafe { pwa_register(self.0, id, bus, channels, frames) }
    }

    /// Stages `pcm` and registers it (`frames = len / channels`).
    pub fn register(&mut self, id: u32, bus: u32, channels: u32, pcm: &[f32]) -> i32 {
        assert!(self.stage(pcm.len() as u32, pcm), "stage {}", pcm.len());
        self.register_raw(id, bus, channels, pcm.len() as u32 / channels)
    }

    /// `pwa_trigger`.
    pub fn trigger(&mut self, id: u32, gain: f32) -> u32 {
        // SAFETY: live engine.
        unsafe { pwa_trigger(self.0, id, gain) }
    }

    /// Copies `bytes` (at most 4096) into `pwa_cmd_buffer` and calls
    /// `pwa_submit(n_bytes)`.
    pub fn submit_with_len(&mut self, bytes: &[u8], n_bytes: u32) -> u32 {
        assert!(bytes.len() <= 4096);
        // SAFETY: live engine; the command buffer holds 4096 bytes.
        unsafe {
            let p = pwa_cmd_buffer(self.0);
            assert!(!p.is_null());
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
            pwa_submit(self.0, n_bytes)
        }
    }

    /// `submit_with_len(bytes, bytes.len())`.
    pub fn submit(&mut self, bytes: &[u8]) -> u32 {
        self.submit_with_len(bytes, bytes.len() as u32)
    }

    /// `pwa_render`; `None` for null, else the `2 * frames` interleaved values.
    pub fn render(&mut self, frames: u32) -> Option<&[f32]> {
        // SAFETY: live engine; a non-null result points at 2 × frames f32
        // valid until the next call, which `&mut self` enforces.
        unsafe {
            let p = pwa_render(self.0, frames);
            (!p.is_null()).then(|| std::slice::from_raw_parts(p, frames as usize * 2))
        }
    }

    /// `pwa_stat`.
    pub fn stat(&self, which: u32) -> f64 {
        // SAFETY: live engine.
        unsafe { pwa_stat(self.0, which) }
    }
}

impl Drop for Eng {
    fn drop(&mut self) {
        // SAFETY: from `pwa_new`, freed once.
        unsafe { pwa_free(self.0) }
    }
}
