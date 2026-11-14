//! Versioned capability declarations and one deterministic per-thread route resolution.
//! Wire implementations, current authorization and request constraints remain their existing suppliers.

mod catalogue;
pub mod counters;
mod execute;
mod model;
pub mod pricing;
mod projection;
mod resolve;

pub use catalogue::{catalogue, protocol_interfaces};
pub use execute::{
    CallSource, CapabilityCall, CapabilityError, CapabilityExecutor, CapabilityItem,
    CapabilityResult, Executors,
};
pub use model::*;
pub use projection::{
    EndpointProjection, endpoint_facts, project_endpoint, project_result, render_auxiliary,
    render_request,
};
pub use resolve::{Registry, RegistryError, resolve};

/// The declaration contract read by this version. Later additive members remain opaque data.
pub const DECLARATION_CONTRACT: u32 = 1;
