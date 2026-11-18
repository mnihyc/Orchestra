//! One resolved work budget feeds the tool's declaration, validation and executor.
use super::{definition_of, setting};
use crate::error::EngineError;
use orch_settings::Snapshot;
use orch_settings_contract::registry::keys;

/// Adjustable per-thread work budgets. Byte/time windows and current source rights remain separate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchBudgets {
    pub read_bytes: i64,
    pub list_depth: u32,
    pub list_rows: i64,
    pub query_chars: usize,
    pub results: i64,
    pub context: i64,
}
impl Default for SearchBudgets {
    fn default() -> Self {
        Self {
            read_bytes: super::read::READ_BYTES_DEFAULT,
            list_depth: 1,
            list_rows: 50,
            query_chars: super::definitions::QUERY_MAX_CHARS,
            results: super::definitions::RESULTS_MAX,
            context: super::definitions::CONTEXT_MAX,
        }
    }
}
impl SearchBudgets {
    pub(crate) fn resolve(snapshot: &Snapshot) -> Result<Self, EngineError> {
        // Resolve only from the caller's existing settings snapshot, never reload inside the executor.
        let defaults = Self::default();
        let query: i64 = setting(snapshot, keys::ORCH_TOOLS_SEARCH_QUERY_CHARS)?
            .unwrap_or(i64::try_from(defaults.query_chars).map_err(EngineError::unreadable)?);
        let value = Self {
            read_bytes: setting(snapshot, keys::ORCH_TOOLS_READ_BYTES)?
                .unwrap_or(defaults.read_bytes),
            list_depth: setting(snapshot, keys::ORCH_TOOLS_LIST_DEPTH)?
                .unwrap_or(defaults.list_depth),
            list_rows: setting(snapshot, keys::ORCH_TOOLS_LIST_ROWS)?.unwrap_or(defaults.list_rows),
            query_chars: usize::try_from(query).map_err(EngineError::unreadable)?,
            results: setting(snapshot, keys::ORCH_TOOLS_SEARCH_RESULTS)?
                .unwrap_or(defaults.results),
            context: setting(snapshot, keys::ORCH_TOOLS_SEARCH_CONTEXT)?
                .unwrap_or(defaults.context),
        };
        if value.query_chars == 0
            || value.results <= 0
            || value.context < 0
            || value.read_bytes <= 0
            || value.list_depth == 0
            || value.list_rows <= 0
        {
            return Err(EngineError::unreadable(
                "shared search budgets must have a positive query/result count and nonnegative context",
            ));
        }
        Ok(value)
    }
    pub(crate) fn definitions()
    -> Result<Vec<&'static orch_settings_contract::registry::SettingDef>, EngineError> {
        [
            keys::ORCH_TOOLS_READ_BYTES,
            keys::ORCH_TOOLS_LIST_DEPTH,
            keys::ORCH_TOOLS_LIST_ROWS,
            keys::ORCH_TOOLS_SEARCH_QUERY_CHARS,
            keys::ORCH_TOOLS_SEARCH_RESULTS,
            keys::ORCH_TOOLS_SEARCH_CONTEXT,
        ]
        .into_iter()
        .map(definition_of)
        .collect()
    }
}
