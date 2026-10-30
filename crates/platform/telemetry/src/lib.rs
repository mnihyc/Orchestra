//! Structured logging and metrics shared by components; what a system records stays inside it.
//!
//! A component writes its log where it runs and exposes its measurements
//! to whoever the operator points at them. Nothing is sent anywhere by
//! itself.

#![forbid(unsafe_code)]

pub mod database;
pub mod logging;
pub mod metrics;

pub use logging::{LogFormat, init as init_logging};
pub use metrics::{Readings, install as install_metrics};
