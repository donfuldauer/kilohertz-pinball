//! The guest SDK for Wasm table rules lives in its own `no_std` crate,
//! `pin_guest_sdk` (`crates/pin_script/guest-sdk`), so it can be compiled to
//! `wasm32-unknown-unknown`. Its crate docs describe the guest ABI; the host
//! side is [`crate::wasm`].
