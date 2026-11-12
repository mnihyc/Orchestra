//! Middleware components.
//!
//! What happens to every request around its handler: who is calling, what
//! they may do, and how a failure is answered.

#[path = "admin_openings.rs"]
pub mod admin_openings;

#[path = "authentication.rs"]
pub mod authentication;

#[path = "authorization.rs"]
pub mod authorization;

#[path = "idempotency.rs"]
pub mod idempotency;

#[path = "inspection.rs"]
pub mod inspection;

#[path = "requests.rs"]
pub mod requests;

#[path = "transaction_guard.rs"]
pub mod transaction_guard;
