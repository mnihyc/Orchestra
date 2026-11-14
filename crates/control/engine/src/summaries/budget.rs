//! How much a call of the summary model is sent: the place's share of the model's window.
//!
//! The hand-off of a switch or a fork and the check of a goal both send the model a piece of the conversation. Each
//! stays within `summary.handoff_input_share` of the model's context window (half by default), so that the call fits
//! the model whatever the length of the thread, and so that the plan can say what it will cost before it is confirmed.

use crate::error::EngineError;
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::Id;

/// One activity window serves every requested automatic field, including larger chosen title budgets.
pub(super) fn activity_field(
    config: &orch_summarizer_model::descriptor::Config,
    fields: &[orch_summarizer_contract::descriptor::SummaryField],
) -> orch_summarizer_contract::descriptor::SummaryField {
    use orch_summarizer_contract::descriptor::SummaryField;
    fields
        .iter()
        .copied()
        .max_by_key(|field| config.answer_tokens(*field))
        .unwrap_or(SummaryField::Title)
}

/// Current declared model context for ordinary summary activity, retaining its configured output.
pub(super) async fn context_tokens(
    conn: &mut orch_db::Conn,
    config: &orch_summarizer_model::descriptor::Config,
    field: orch_summarizer_contract::descriptor::SummaryField,
) -> Result<u32, EngineError> {
    let model = orch_db::queries::resources::thread_engine::model(
        conn,
        config.provider_id,
        &config.model_name,
    )
    .await?
    .ok_or_else(|| EngineError::unreadable("the summary model is unavailable"))?;
    let provider = orch_db::queries::resources::thread_engine::provider(conn, config.provider_id)
        .await?
        .ok_or_else(|| EngineError::unreadable("the summary provider is unavailable"))?;
    let protocol = super::resource::completion_protocol(&provider.protocols)?.ok_or_else(|| {
        EngineError::unreadable("the summary provider offers no generation interface")
    })?;
    let parameters = orch_provider_contract::models::composed_model_parameters(
        &model.parameters,
        &model.groups,
        &model.option_layers,
        Some(&protocol),
    )
    .map_err(EngineError::unreadable)?;
    orch_summarizer_model::context::fit_window(
        config.context_tokens,
        config.output_reserve(field, parameters.needs_reasoning_room()),
        parameters
            .context_open()
            .or(parameters.context_window.filter(|window| *window > 0)),
    )
    .map_err(|error| EngineError::unreadable(super::safe_reason(&error)))
}

/// The same output-room decision used by the actual request, independent of enabled tuning controls.
/// Unknown model facts reserve the configured room; they do not enable an effort selection.
pub(super) async fn reasons(
    conn: &mut orch_db::Conn,
    config: &orch_summarizer_model::descriptor::Config,
) -> Result<bool, EngineError> {
    let Some(model) = orch_db::queries::resources::thread_engine::model(
        conn,
        config.provider_id,
        &config.model_name,
    )
    .await?
    else {
        return Ok(true);
    };
    let Some(provider) =
        orch_db::queries::resources::thread_engine::provider(conn, config.provider_id).await?
    else {
        return Ok(true);
    };
    let Some(protocol) = super::resource::completion_protocol(&provider.protocols)? else {
        return Ok(true);
    };
    let parameters = orch_provider_contract::models::composed_model_parameters(
        &model.parameters,
        &model.groups,
        &model.option_layers,
        Some(&protocol),
    )
    .map_err(EngineError::unreadable)?;
    Ok(parameters.needs_reasoning_room())
}

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
    output_tokens: i64,
) -> Result<InputBudget, EngineError> {
    let window = orch_db::queries::resources::thread_engine::model(conn, provider, model)
        .await?
        .and_then(|row| crate::fork_history::model_window(&row.parameters));
    let share =
        orch_settings::get::<i64>(conn, Place::Node(thread), keys::SUMMARY_HANDOFF_INPUT_SHARE)
            .await?
            .unwrap_or(DEFAULT_SHARE)
            .clamp(1, 100);
    Ok(after_output(budget_of(window, share), output_tokens))
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

fn after_output(mut budget: InputBudget, output_tokens: i64) -> InputBudget {
    let remaining = budget
        .window
        .unwrap_or(UNKNOWN_WINDOW_TOKENS)
        .saturating_sub(output_tokens.max(0))
        .max(0);
    budget.tokens = budget.tokens.min(remaining);
    budget.bytes = usize::try_from(
        budget
            .tokens
            .saturating_mul(crate::fork_history::CARRY_BYTES_PER_TOKEN),
    )
    .unwrap_or(usize::MAX);
    budget
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shared_title_and_tags_input_fits_even_when_the_title_has_the_larger_chosen_answer() {
        use orch_summarizer_contract::descriptor::SummaryField;
        let config = orch_summarizer_model::descriptor::Config {
            provider_id: orch_types::new_id(),
            model_name: "synthetic-summary-model".into(),
            params: serde_json::json!({}),
            answer_max_output_tokens: 120,
            title_max_output_tokens: 4_096,
            goal_max_output_tokens: 512,
            reasoning_tokens: 2_000,
            context_tokens: 8_192,
            prompts: Default::default(),
        };
        for fields in [
            vec![SummaryField::Title, SummaryField::Tags],
            vec![SummaryField::Tags],
        ] {
            let field = activity_field(&config, &fields);
            let Ok(input) = orch_summarizer_model::context::fit_window(
                config.context_tokens,
                config.output_reserve(field, true),
                Some(8_192),
            ) else {
                panic!("the configured answer leaves a usable input window");
            };
            for field in &fields {
                assert!(i64::from(input) + config.output_reserve(*field, true) <= 8_192);
            }
            if fields == [SummaryField::Tags] {
                assert_eq!(
                    input,
                    8_192 - 2_000 - 120,
                    "a title not being written reserves no room"
                );
            }
        }
    }

    #[test]
    fn a_summarys_input_share_also_leaves_room_for_its_answer_and_reasoning() {
        let output = 3_024;
        let budget = after_output(budget_of(Some(4_096), 50), output);
        assert!(
            budget.tokens + output <= 4_096,
            "a percentage of the context is not additional space beyond the model's whole window"
        );
    }

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
