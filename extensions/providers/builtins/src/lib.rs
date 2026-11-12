//! Provider extensions are linked here; core and executable consumers receive only their common declarations.
use orch_provider_contract::builtins::{BuiltinFault, BuiltinSource, BuiltinSources};

#[must_use]
pub fn entries() -> Vec<BuiltinSource> {
    vec![
        orch_provider_openrouter::builtin(),
        orch_provider_deepseek::builtin(),
        orch_provider_openai::builtin(),
        orch_provider_anthropic::builtin(),
    ]
}

pub fn sources() -> Result<BuiltinSources, BuiltinFault> {
    BuiltinSources::new(entries())
}

#[must_use]
pub fn names() -> Vec<&'static str> {
    entries().into_iter().map(|entry| entry.name).collect()
}
