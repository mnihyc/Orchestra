//! One provider source binds every public model interface to its ordinary operation.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Provider,
}

const PROVIDER: Binding<Source> = Binding {
    source: Source::Provider,
    target: TargetResolver::PermissionObject,
    usage: FacetUsage::Required,
    cardinality: Cardinality::One,
};
macro_rules! call {
    ($name:ident,$operation:literal) => {
        pub const $name: Declaration<Source> = Declaration {
            operation: $operation,
            bindings: &[PROVIDER],
        };
    };
}
call!(RESPONSES, "ProviderRelayResponses");
call!(COMPACT, "ProviderRelayCompact");
call!(INPUT_TOKENS, "ProviderRelayInputTokens");
call!(MESSAGES, "ProviderRelayMessages");
call!(MESSAGE_COUNT, "ProviderRelayMessageCount");
call!(CHAT, "ProviderRelayChat");
call!(EMBEDDINGS, "ProviderRelayEmbeddings");
