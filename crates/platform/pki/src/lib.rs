//! Certificate authority and machine identity mechanisms.
//!
//! Every machine and every service of a system proves who it is with a
//! certificate that the system's own authority issued, and the system
//! trusts no other authority for them. This package is the authority and
//! everything that surrounds it: creating it and keeping its key, the keys
//! and requests a machine makes, the certificates the authority issues,
//! the verifiers that decide whom a connection belongs to, enrollment
//! tokens that carry the authority's pin, renewal, sealing a secret so that
//! only one machine opens it, and the signed tickets that allow a tunnel.
//!
//! Nothing here reads the clock by itself: whatever depends on the time is
//! given a [`Clock`](orch_types::Clock), so that expiry and renewal can be
//! tested without waiting. Nothing here opens a socket or knows a database:
//! what the system knows about a machine is handed in as plain values.

#![forbid(unsafe_code)]

pub mod ca;
pub mod certificates;
pub mod enrollment;
pub mod port_assertions;
pub mod renewal;
pub mod sealing;
pub mod service_renew;
pub mod tickets;
pub mod verification;

/// Why a key, a request or a certificate could not be made, read or used.
#[derive(Debug, thiserror::Error)]
pub enum PkiError {
    /// A key could not be made or read.
    #[error("key: {0}")]
    Key(String),
    /// A certificate could not be read, or is not one this system issues.
    #[error("certificate: {0}")]
    Certificate(String),
    /// A request for a certificate could not be read or does not hold.
    #[error("certificate request: {0}")]
    Request(String),
    /// A certificate could not be issued.
    #[error("issuing: {0}")]
    Issue(String),
    /// A file could not be read or written.
    #[error("file {path}: {source}")]
    File {
        /// The file.
        path: String,
        /// What went wrong.
        #[source]
        source: std::io::Error,
    },
    /// A secret could not be sealed or opened.
    #[error("sealing: {0}")]
    Seal(String),
    /// Something that was to be signed or checked could not be.
    #[error("signature: {0}")]
    Signature(String),
}

impl From<rcgen::Error> for PkiError {
    fn from(error: rcgen::Error) -> Self {
        Self::Issue(error.to_string())
    }
}
