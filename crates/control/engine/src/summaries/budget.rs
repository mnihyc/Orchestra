//! How much a call of the summary model is sent: the place's share of the model's window.
//!
//! The hand-off of a switch or a fork and the check of a goal both send the model a piece of the conversation. Each
//! stays within `summary.handoff_input_share` of the model's context window (half by default), so that the call fits
//! the model whatever the length of the thread, and so that the plan can say what it will cost before it is confirmed.

use crate::error::EngineError;
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::Id;

/// The window taken for a model whose entry does not say: what every model that does this work has.
pub(crate) const UNKNOWN_WINDOW_TOKENS: i64 = 32_000;

/// The share the setting ships with, for a place where it does not read.
pub(crate) const DEFAULT_SHARE: i64 = 50;

/// What a call of the summary model may be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InputBudget {
    /// The model's window in tokens, where its entry says.
    pub window: Option<i64>,
    /// The share of it, in percent.
    pub share: i64,
    /// The tokens that share comes to.
    pub tokens: i64,
    /// The bytes of text that is, at the carry rule's bytes to a token.
    pub bytes: usize,
}

/// The budget of a call at `thread`'s place to `model` of `provider`.
pub(crate) async fn input_budget(
    conn: &mut orch_db::Conn,
    thread: Id,
    provider: Id,
    model: &str,
) -> Result<InputBudget, EngineError> {
    let window = orch_db::queries::resources::thread_engine::model(conn, provider, model)
        .await?
        .and_then(|row| crate::fork_history::model_window(&row.parameters));
    let share =
        orch_settings::get::<i64>(conn, Place::Node(thread), keys::SUMMARY_HANDOFF_INPUT_SHARE)
            .await?
            .unwrap_or(DEFAULT_SHARE)
            .clamp(1, 100);
    Ok(budget_of(window, share))
}

/// The budget for a window and a share, with nothing read.
#[must_use]
pub(crate) fn budget_of(window: Option<i64>, share: i64) -> InputBudget {
    let tokens = window
        .unwrap_or(UNKNOWN_WINDOW_TOKENS)
        .saturating_mul(share)
        / 100;
    InputBudget {
        window,
        share,
        tokens,
        bytes: usize::try_from(tokens.saturating_mul(crate::fork_history::CARRY_BYTES_PER_TOKEN))
            .unwrap_or(usize::MAX),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_of_a_known_window_is_sent_at_the_carry_rules_bytes_to_a_token() {
        let budget = budget_of(Some(200_000), 50);
        assert_eq!(budget.tokens, 100_000);
        assert_eq!(budget.bytes, 300_000);
    }

    #[test]
    fn a_window_that_is_not_known_is_taken_as_the_one_every_such_model_has() {
        let budget = budget_of(None, 50);
        assert_eq!(budget.tokens, UNKNOWN_WINDOW_TOKENS / 2);
        assert_eq!(budget.window, None);
    }
}
