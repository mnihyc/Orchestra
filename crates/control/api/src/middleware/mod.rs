//! Middleware components.
//!
//! What happens to every request around its handler: who is calling, what
//! they may do, and how a failure is answered.

pub(crate) mod admin_openings;
pub mod authentication;
pub mod authorization;
pub mod idempotency;
pub(crate) mod inspection;
pub mod requests;
pub mod transaction_guard;
