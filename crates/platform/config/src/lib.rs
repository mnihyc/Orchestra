//! Configuration loading shared by service and machine entry points.
//!
//! A server component reads `orch.toml` ([`ServerConfig`]); a connector
//! reads its own `config.toml` ([`ConnectorConfig`]). Both hold what an
//! operator decides for a deployment and nobody changes while it runs:
//! what people change in the product is kept in the database. An absent
//! key takes its default, an unknown key stops the start, and every key
//! can be given by an environment variable instead.

#![forbid(unsafe_code)]

pub mod client_address;
pub mod connector;
pub mod loader;
pub mod secrets;
pub mod server;

pub use connector::ConnectorConfig;
pub use loader::{ConfigError, load, load_text};
pub use secrets::{DEFAULT_SECRETS_DIR, Secret, SecretError, read_secret, read_secret_hex};
pub use server::ServerConfig;
