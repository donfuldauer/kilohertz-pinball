//! External-force hooks: Magnus effect and magnetic remanence.
//!
//! Owner: Subphase 2.7. Stub created by Subphase 1.1.
//!
//! Hook API provided by Subphase 2.2: implement
//! [`crate::ball::ExternalForce`] (`force(handle, ball, profile, dt) -> Vec3`,
//! newtons, called once per ball sub-step in ball-handle order, after
//! gravity) and register it at setup time with
//! [`crate::ball::BallSolver::add_external_force`]. With no hook registered
//! the solver reproduces the core trajectory bit for bit.
