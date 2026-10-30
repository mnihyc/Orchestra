//! Wire-protocol and translator contracts for provider access.
//!
//! A provider's upstream speaks one or more model APIs. A [`protocol::WireProtocol`] knows the shape of one:
//! which requests are allowed below the upstream base, where the model name is, how to add the credential
//! and what an endpoint may configure about that ([`endpoint`]), how to shape an error, how to read usage
//! from a response as it streams ([`usage`]), how to discover models, and which credential forms can be
//! issued from a key ([`credentials`]). Orch itself is a plain relay. Serving a protocol the upstream does
//! not speak is the job of a [`translator::ProtocolTranslator`], an external implementation that converts
//! requests of one protocol into another and the response stream back.

#![forbid(unsafe_code)]

pub mod credentials;
pub mod endpoint;
pub mod protocol;
pub mod translator;
pub mod usage;
